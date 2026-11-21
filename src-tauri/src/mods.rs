use anyhow::{bail, ensure, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use crate::{
    downloads::{self, Download, Job},
    mod_history::{self, Action, Change, Record, Revision},
    mod_sides::ModSide,
    providers::{self, Dependency, ModFile, Platform},
    storage::{
        atomic_json, check_directory, check_filename, file_added_at, is_mod, list_files, read_json,
        regular_file, Instance, Loader, Store, UpdateChannel,
    },
};

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedMod {
    pub platform: Platform,
    pub project: String,
    pub version: String,
    pub title: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub display_version: Option<String>,
    pub file: String,
    pub minecraft_version: String,
    pub loader: Loader,
    #[serde(default)]
    pub export_url: Option<String>,
    #[serde(default)]
    pub export_sha512: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub side: ModSide,
}

#[derive(Default, Deserialize, Serialize)]
struct ModState {
    mods: Vec<ManagedMod>,
    #[serde(default)]
    history: Vec<Record>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Registry {
    State(ModState),
    Legacy(Vec<ManagedMod>),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub file: String,
    pub enabled: bool,
    pub managed: Option<ManagedMod>,
    pub side: ModSide,
    side_override: Option<ModSide>,
    added_at: Option<u64>,
}

pub fn add_local(store: &Store, id: Uuid, path: &Path) -> Result<()> {
    let instance = store.instance(id)?;
    ensure!(
        instance.options.loader != Loader::Vanilla,
        "Choose a mod loader before adding mods."
    );
    regular_file(path)?;
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid mod filename.")?;
    check_filename(file)?;
    ensure!(
        file.to_ascii_lowercase().ends_with(".jar"),
        "Choose a mod JAR."
    );
    let source = fs::File::open(path)?;
    ensure!(
        source.metadata()?.len() <= 2 * 1024 * 1024 * 1024,
        "This mod is too large."
    );
    let mut source = zip::ZipArchive::new(source)
        .context("Reading the mod JAR")?
        .into_inner();
    std::io::Seek::rewind(&mut source)?;
    let folder = store.game_dir(id)?.join("mods");
    fs::create_dir_all(&folder)?;
    check_directory(&folder)?;
    let work = tempfile::tempdir_in(store.instance_dir(id))?;
    let path = work.path().join("import.jar");
    std::io::copy(&mut source, &mut fs::File::create(&path)?)?;
    let mut after = Revision::read(work.path(), "import.jar", None)?;
    after.file = file.into();
    let mut state = load_state(store, id)?;
    state
        .mods
        .retain(|entry| !entry.file.eq_ignore_ascii_case(file));
    state.history.push(Record::new(
        &instance,
        Action::Install,
        None,
        vec![Change::new(None, Some(after))],
    ));
    store.snapshot_content(id)?;
    apply_writes(store, id, state, vec![(path, folder.join(file))], work)
}

pub fn list(store: &Store, id: Uuid) -> Result<Vec<InstalledMod>> {
    let directory = store.game_dir(id)?.join("mods");
    let managed = installed(store, id)?;
    let overrides: HashMap<String, ModSide> =
        crate::storage::read_default(&override_path(store, id))?;
    Ok(list_files(&directory)?
        .into_iter()
        .filter(|name| is_mod(name))
        .map(|file| {
            let path = directory.join(&file);
            let entry = managed.iter().find(|item| item.file == file).cloned();
            let side_override = overrides.get(&side_key(&file, entry.as_ref())).copied();
            let side = side_override.unwrap_or_else(|| {
                let catalog = entry.as_ref().map_or(ModSide::Unknown, |item| item.side);
                if matches!(
                    catalog,
                    ModSide::Client | ModSide::Server | ModSide::Singleplayer
                ) {
                    return catalog;
                }
                match ModSide::from_jar(&path) {
                    ModSide::Unknown => catalog,
                    side => side,
                }
            });
            InstalledMod {
                added_at: fs::symlink_metadata(path)
                    .ok()
                    .as_ref()
                    .and_then(file_added_at),
                enabled: !disabled(&file),
                file,
                managed: entry,
                side,
                side_override,
            }
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterKind {
    Environment,
    Updates,
    OpenSource,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterData {
    matches: Vec<String>,
    environments: HashMap<String, ModSide>,
    unchecked: usize,
}

pub async fn filter_data(
    client: &Client,
    private_client: &Client,
    store: Store,
    id: Uuid,
    filter: FilterKind,
) -> Result<FilterData> {
    let include_local_licenses = matches!(filter, FilterKind::OpenSource);
    let (target, mods, local_licenses) = tokio::task::spawn_blocking(move || -> Result<_> {
        let options = store.instance(id)?.options;
        let managed = installed(&store, id)?;
        let mut local = Vec::new();
        if include_local_licenses {
            let directory = store.game_dir(id)?.join("mods");
            let catalog_files: HashSet<_> = managed
                .iter()
                .filter(|item| item.platform == Platform::Modrinth)
                .map(|item| item.file.as_str())
                .collect();
            for file in list_files(&directory)?
                .into_iter()
                .filter(|file| is_mod(file) && !catalog_files.contains(file.as_str()))
            {
                let Ok(Some(manifest)) = crate::mod_sides::fabric_manifest(&directory.join(&file))
                else {
                    continue;
                };
                let Some(license) = manifest.get("license") else {
                    continue;
                };
                let is_open = |value: &serde_json::Value| {
                    value.as_str().is_some_and(providers::open_source_license)
                };
                if is_open(license)
                    || license
                        .as_array()
                        .is_some_and(|licenses| licenses.iter().any(is_open))
                {
                    local.push(file);
                }
            }
        }
        Ok((options, managed, local))
    })
    .await??;

    let mut result = FilterData::default();
    let modrinth = mods
        .iter()
        .filter(|item| item.platform == Platform::Modrinth);
    match filter {
        FilterKind::Environment => {
            let version_ids = modrinth
                .clone()
                .map(|item| item.version.as_str())
                .collect::<Vec<_>>();
            let environments = providers::modrinth_environments(client, &version_ids).await?;
            result.environments = modrinth
                .filter_map(|item| {
                    environments
                        .get(&item.version)
                        .map(|side| (item.file.clone(), *side))
                })
                .collect();
        }
        FilterKind::OpenSource => {
            result.matches = local_licenses;
            let project_ids = modrinth
                .clone()
                .map(|item| item.project.as_str())
                .collect::<Vec<_>>();
            match providers::modrinth_open_source(client, &project_ids).await {
                Ok(licenses) => {
                    let licenses: HashSet<_> = licenses.iter().map(String::as_str).collect();
                    result.matches.extend(
                        modrinth
                            .filter(|item| licenses.contains(item.project.as_str()))
                            .map(|item| item.file.clone()),
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "Could not check Modrinth licenses");
                    result.unchecked = project_ids.len();
                }
            }
        }
        FilterKind::Updates if target.loader != Loader::Vanilla => {
            result = filter_updates(client, private_client, &target, &mods).await?;
        }
        FilterKind::Updates => {}
    }
    Ok(result)
}

async fn filter_updates(
    client: &Client,
    private_client: &Client,
    target: &crate::storage::InstanceOptions,
    mods: &[ManagedMod],
) -> Result<FilterData> {
    use futures_util::{stream, StreamExt};

    fn catalog_hash(item: &ManagedMod) -> Option<&str> {
        item.export_sha512.as_deref().filter(|hash| {
            item.platform == Platform::Modrinth
                && hash.len() == 128
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }

    let hashes = mods.iter().filter_map(catalog_hash).collect::<Vec<_>>();
    let latest = providers::modrinth_updates(client, &hashes, target).await?;
    let mut result = FilterData::default();
    let mut remaining = Vec::new();
    for item in mods {
        match catalog_hash(item).and_then(|hash| latest.get(hash)) {
            Some((project, version)) if project == &item.project => {
                if version != &item.version {
                    result.matches.push(item.file.clone());
                }
            }
            _ => remaining.push(item),
        }
    }

    for platform in [Platform::Modrinth, Platform::Curseforge] {
        let items = remaining
            .iter()
            .filter(|item| item.platform == platform)
            .map(|item| {
                (
                    item.project.clone(),
                    item.version.clone(),
                    item.file.clone(),
                )
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }
        let provider = match providers::provider(platform, client, private_client).await {
            Ok(provider) => provider,
            Err(_) => {
                result.unchecked += items.len();
                continue;
            }
        };
        let provider = provider.as_ref();
        let mut checks = stream::iter(items)
            .map(|(project, installed, file)| async move {
                let latest = provider
                    .file(Some(&project), None, target, Some(target.update_channel))
                    .await;
                (installed, file, latest)
            })
            .buffer_unordered(6);
        while let Some((installed, file, latest)) = checks.next().await {
            match latest {
                Ok(Some(version)) if version.version != installed => result.matches.push(file),
                Ok(_) => {}
                Err(_) => result.unchecked += 1,
            }
        }
    }
    Ok(result)
}

fn override_path(store: &Store, id: Uuid) -> PathBuf {
    store.instance_dir(id).join("mod-sides.json")
}

fn side_key(file: &str, entry: Option<&ManagedMod>) -> String {
    let name = if disabled(file) {
        &file[..file.len() - ".disabled".len()]
    } else {
        file
    };
    entry.map_or_else(
        || format!("file:{name}"),
        |item| format!("{}:{}", item.platform.label(), item.project),
    )
}

pub fn set_side(store: &Store, id: Uuid, file: &str, side: Option<ModSide>) -> Result<()> {
    check_filename(file)?;
    ensure!(is_mod(file), "Choose an installed mod.");
    let directory = store.game_dir(id)?.join("mods");
    check_directory(&directory)?;
    regular_file(&directory.join(file))?;
    let mods = installed(store, id)?;
    let key = side_key(file, mods.iter().find(|item| item.file == file));
    let path = override_path(store, id);
    let mut overrides: HashMap<String, ModSide> = crate::storage::read_default(&path)?;
    if let Some(side) = side {
        overrides.insert(key, side);
    } else {
        overrides.remove(&key);
    }
    atomic_json(&path, &overrides)
}

fn registry(store: &Store, id: Uuid) -> PathBuf {
    store.instance_dir(id).join("mods.json")
}

fn load_state(store: &Store, id: Uuid) -> Result<ModState> {
    let path = registry(store, id);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ModState::default())
        }
        Err(error) => return Err(error.into()),
        Ok(_) => regular_file(&path)?,
    }
    Ok(match read_json(&path)? {
        Registry::State(state) => state,
        Registry::Legacy(mods) => ModState {
            mods,
            history: Vec::new(),
        },
    })
}

pub fn registered(store: &Store, id: Uuid) -> Result<Vec<ManagedMod>> {
    Ok(load_state(store, id)?.mods)
}

pub fn stage_merged_registry(
    store: &Store,
    target: Uuid,
    source: Uuid,
    files: &HashSet<String>,
    path: &Path,
) -> Result<()> {
    let mut state = load_state(store, target)?;
    let current = installed(store, target)?;
    let incoming = installed(store, source)?;
    let selected = |file: &str| files.iter().any(|value| value.eq_ignore_ascii_case(file));
    let mut proposed: Vec<_> = current
        .iter()
        .filter(|item| !selected(&item.file))
        .cloned()
        .collect();
    for item in incoming.into_iter().filter(|item| selected(&item.file)) {
        ensure!(!proposed.iter().any(|old| old.platform == item.platform && old.project == item.project),
            "{} already exists as a different file. Preview an update or remove the existing version first.", item.title);
        proposed.push(item);
    }
    mod_history::verify_dependencies(&current, &proposed)?;
    state.mods = proposed;
    atomic_json(path, &state)
}

pub fn copy_registry(store: &Store, id: Uuid, target: &Path) -> Result<()> {
    let mods = registered(store, id)?;
    if !mods.is_empty() {
        atomic_json(&target.join("mods.json"), &mods)?;
    }
    let sides: HashMap<String, ModSide> = crate::storage::read_default(&override_path(store, id))?;
    if !sides.is_empty() {
        atomic_json(&target.join("mod-sides.json"), &sides)?;
    }
    Ok(())
}

fn installed(store: &Store, id: Uuid) -> Result<Vec<ManagedMod>> {
    let mut entries = registered(store, id)?;
    let files: HashSet<_> = list_files(&store.game_dir(id)?.join("mods"))?
        .into_iter()
        .collect();
    entries.retain(|entry| files.contains(&entry.file));
    Ok(entries)
}

pub fn installed_projects(store: &Store, id: Uuid) -> Result<Vec<(Platform, String)>> {
    Ok(installed(store, id)?
        .into_iter()
        .filter(|entry| is_mod(&entry.file))
        .map(|entry| (entry.platform, entry.project))
        .collect())
}

fn disabled(file: &str) -> bool {
    file.to_ascii_lowercase().ends_with(".disabled")
}

pub fn rename_local(store: &Store, id: Uuid, file: &str, name: &str) -> Result<()> {
    check_filename(file)?;
    check_filename(name)?;
    ensure!(
        is_mod(name),
        "Keep the .jar or .jar.disabled extension on mod files."
    );
    let folder = store.game_dir(id)?.join("mods");
    check_directory(&folder)?;
    let source = folder.join(file);
    let destination = folder.join(name);
    regular_file(&source)?;
    let mut state = load_state(store, id)?;
    store.snapshot_content(id)?;
    rename_absent(&source, &destination)?;
    if let Some(entry) = state.mods.iter_mut().find(|entry| entry.file == file) {
        entry.file = name.into();
        if let Err(error) = atomic_json(&registry(store, id), &state) {
            fs::rename(destination, source).context("Restoring the original mod filename")?;
            return Err(error);
        }
    }
    Ok(())
}

pub fn record_removed_files(store: &Store, id: Uuid, backup: Uuid, names: &[String]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let instance = store.instance(id)?;
    let mut state = load_state(store, id)?;
    record_removals(store, &instance, backup, names, &mut state)?;
    atomic_json(&registry(store, id), &state)
}

fn record_removals(
    store: &Store,
    instance: &Instance,
    backup: Uuid,
    names: &[String],
    state: &mut ModState,
) -> Result<()> {
    let source = store.backup_mods(backup, instance.id)?;
    let mut changes = Vec::new();
    for name in names {
        let managed = state
            .mods
            .iter()
            .position(|entry| name.eq_ignore_ascii_case(&entry.file))
            .map(|index| state.mods.remove(index));
        changes.push(Change::new(
            Some(Revision::read(&source, name, managed)?),
            None,
        ));
    }
    state
        .history
        .push(Record::new(instance, Action::Remove, Some(backup), changes));
    Ok(())
}

pub fn toggle(store: &Store, id: Uuid, file: &str, enabled: bool) -> Result<()> {
    check_filename(file)?;
    ensure!(is_mod(file), "Choose a mod JAR.");
    let dir = store.game_dir(id)?.join("mods");
    check_directory(&dir)?;
    regular_file(&dir.join(file))?;
    if disabled(file) != enabled {
        return Ok(());
    }
    let new = if enabled {
        file[..file.len() - ".disabled".len()].to_owned()
    } else {
        format!("{file}.disabled")
    };
    ensure!(
        !list_files(&dir)?
            .iter()
            .any(|file| file.eq_ignore_ascii_case(&new)),
        "A mod at the destination already exists."
    );
    let mut state = load_state(store, id)?;
    let source = dir.join(file);
    let destination = dir.join(&new);
    store.snapshot_content(id)?;
    rename_absent(&source, &destination)?;
    if let Some(item) = state.mods.iter_mut().find(|item| item.file == file) {
        item.file = new;
        if let Err(error) = atomic_json(&registry(store, id), &state) {
            fs::rename(destination, source)
                .context("Restoring original mod name after metadata update failed")?;
            return Err(error);
        }
    }
    Ok(())
}

pub fn remove(store: &Store, id: Uuid, file: &str) -> Result<()> {
    check_filename(file)?;
    ensure!(is_mod(file), "Choose a mod JAR.");
    let folder = store.game_dir(id)?.join("mods");
    check_directory(&folder)?;
    let path = folder.join(file);
    regular_file(&path)?;
    let instance = store.instance(id)?;
    let backup = store.snapshot_content(id)?;
    let mut state = load_state(store, id)?;
    record_removals(store, &instance, backup, &[file.into()], &mut state)?;
    let work = tempfile::tempdir_in(store.instance_dir(id))?;
    let previous = work.path().join("removed.jar");
    apply_writes(store, id, state, vec![(path, previous)], work)
}

pub fn enable_files(store: &Store, id: Uuid, files: &[String]) -> Result<()> {
    let folder = store.game_dir(id)?.join("mods");
    check_directory(&folder)?;
    let mut state = load_state(store, id)?;
    let names: HashSet<_> = list_files(&folder)?
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let mut writes = Vec::new();
    for file in files {
        check_filename(file)?;
        ensure!(is_mod(file) && disabled(file), "Choose a disabled mod.");
        let next = file[..file.len() - ".disabled".len()].to_owned();
        ensure!(
            !names.contains(&next.to_ascii_lowercase()),
            "{next} already exists."
        );
        regular_file(&folder.join(file))?;
        let item = state
            .mods
            .iter_mut()
            .find(|mod_file| mod_file.file == *file)
            .context("Required catalog mod changed.")?;
        item.file = next.clone();
        writes.push((folder.join(file), folder.join(next)));
    }
    store.snapshot_content(id)?;
    let work = tempfile::tempdir_in(store.instance_dir(id))?;
    apply_writes(store, id, state, writes, work)
}

#[derive(Clone)]
pub enum Operation {
    Install(Platform, String),
    Update(Option<String>),
}

type Plan = Vec<(Platform, ModFile)>;

#[derive(Clone)]
pub struct PreparedUpdate {
    pub instance_id: Uuid,
    old: Vec<ManagedMod>,
    plan: Plan,
    files: Vec<String>,
    revisions: Vec<Revision>,
    game: String,
    loader: Loader,
    loader_version: Option<String>,
    channel: UpdateChannel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePreview {
    pub token: Uuid,
    pub changes: Vec<UpdateChange>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChange {
    title: String,
    previous: Option<String>,
    next: String,
    file: String,
}

impl PreparedUpdate {
    pub fn view(&self, token: Uuid) -> UpdatePreview {
        UpdatePreview {
            token,
            changes: self
                .plan
                .iter()
                .map(|(platform, file)| {
                    let previous = self
                        .old
                        .iter()
                        .find(|entry| entry.platform == *platform && entry.project == file.project);
                    UpdateChange {
                        title: file.title.clone(),
                        previous: previous.map(|entry| {
                            entry
                                .display_version
                                .clone()
                                .unwrap_or_else(|| entry.version.clone())
                        }),
                        next: file
                            .display_version
                            .clone()
                            .unwrap_or_else(|| file.version.clone()),
                        file: target_filename(&file.filename, previous),
                    }
                })
                .collect(),
        }
    }

    pub fn check(&self, store: &Store, instance: &Instance) -> Result<()> {
        let directory = store.game_dir(instance.id)?.join("mods");
        ensure!(
            self.game == instance.options.minecraft_version
                && self.loader == instance.options.loader
                && self.loader_version == instance.options.loader_version
                && self.channel == instance.options.update_channel
                && self.old == installed(store, instance.id)?
                && self.files == list_files(&directory)?,
            "This instance changed. Preview the updates again."
        );
        for revision in &self.revisions {
            revision
                .check(&directory)
                .context("Preview the updates again")?;
        }
        Ok(())
    }
}

pub async fn preview_updates(
    client: &Client,
    private_client: &Client,
    store: &Store,
    instance: Instance,
    file: Option<String>,
) -> Result<PreparedUpdate> {
    ensure!(
        instance.options.loader != Loader::Vanilla,
        "Choose a modded instance first."
    );
    let old = installed(store, instance.id)?;
    if let Some(name) = &file {
        ensure!(
            old.iter().any(|item| &item.file == name),
            "This catalog mod is no longer installed."
        );
    }
    let plan = resolve(
        client,
        private_client,
        &instance,
        &old,
        Operation::Update(file),
    )
    .await?;
    let directory = store.game_dir(instance.id)?.join("mods");
    let files = list_files(&directory)?;
    let revisions = plan
        .iter()
        .filter_map(|(platform, file)| {
            old.iter()
                .find(|entry| entry.platform == *platform && entry.project == file.project)
        })
        .map(|entry| Revision::read(&directory, &entry.file, Some(entry.clone())))
        .collect::<Result<Vec<_>>>()?;
    Ok(PreparedUpdate {
        instance_id: instance.id,
        old,
        plan,
        files,
        revisions,
        game: instance.options.minecraft_version,
        loader: instance.options.loader,
        loader_version: instance.options.loader_version,
        channel: instance.options.update_channel,
    })
}

pub async fn apply(
    client: &Client,
    private_client: &Client,
    store: &Store,
    instance: &Instance,
    limit: usize,
    operation: Operation,
    job: &Job,
) -> Result<()> {
    ensure!(
        instance.options.loader != Loader::Vanilla,
        "Select a modded instance before installing mods."
    );
    let old = installed(store, instance.id)?;
    let action = if matches!(operation, Operation::Update(_)) {
        Action::Update
    } else {
        Action::Install
    };
    job.stage("Resolving compatible mods and dependencies", 0);
    let plan = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        plan = resolve(client, private_client, instance, &old, operation) => plan?,
    };
    install_plan(
        client,
        store,
        instance,
        limit,
        (old, plan, action),
        None,
        job,
    )
    .await
}

pub async fn apply_preview(
    client: &Client,
    store: &Store,
    instance: &Instance,
    limit: usize,
    prepared: PreparedUpdate,
    job: &Job,
) -> Result<()> {
    let install = (prepared.old.clone(), prepared.plan.clone(), Action::Update);
    install_plan(client, store, instance, limit, install, Some(prepared), job).await
}

async fn install_plan(
    client: &Client,
    store: &Store,
    instance: &Instance,
    limit: usize,
    (old, plan, action): (Vec<ManagedMod>, Plan, Action),
    preview: Option<PreparedUpdate>,
    job: &Job,
) -> Result<()> {
    if plan.is_empty() {
        job.stage("Mods are up to date", 0);
        return Ok(());
    }
    let id = instance.id;
    let work = tempfile::tempdir_in(store.instance_dir(id))?;
    let cache = store.root.join("cache/mod-downloads");
    fs::create_dir_all(&cache)?;
    check_directory(&cache)?;
    let mut files = Vec::new();
    let mut hashes = HashSet::new();
    let mut sources = Vec::new();
    for (_, file) in &plan {
        let hash = match &file.hash {
            downloads::Hash::Sha1(value)
            | downloads::Hash::Sha256(value)
            | downloads::Hash::Sha512(value) => value,
        };
        ensure!(
            !hash.is_empty() && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid mod checksum."
        );
        let destination = cache.join(format!("{}.jar", hash.to_ascii_lowercase()));
        sources.push(destination.clone());
        if hashes.insert(destination.clone()) {
            files.push(Download {
                url: file.url.clone(),
                destination,
                hash: file.hash.clone(),
                size: file.size,
            });
        }
    }
    downloads::batch(client, files, limit, job, "Checking shared mod downloads").await?;
    job.check()?;
    job.stage("Saving a backup and applying mods", 0);
    let store = store.clone();
    let instance = instance.clone();
    let cancel = job.cancel.clone();
    tokio::task::spawn_blocking(move || {
        for (index, path) in sources.into_iter().enumerate() {
            ensure!(!cancel.is_cancelled(), "Cancelled.");
            regular_file(&path)?;
            fs::copy(path, work.path().join(format!("{index}.jar")))?;
        }
        if let Some(preview) = &preview {
            preview.check(&store, &store.instance(id)?)?;
        }
        let backup = store.snapshot_content(id)?;
        ensure!(!cancel.is_cancelled(), "Cancelled.");
        commit(&store, &instance, old, plan, action, backup, work)
    })
    .await?
}

async fn resolve(
    client: &Client,
    private_client: &Client,
    instance: &Instance,
    old: &[ManagedMod],
    operation: Operation,
) -> Result<Plan> {
    let mut pending = VecDeque::new();
    let update =
        matches!(operation, Operation::Update(_)).then_some(instance.options.update_channel);
    match operation {
        Operation::Install(platform, project) => {
            pending.push_back((platform, Some(project), None, true))
        }
        Operation::Update(project) => {
            for entry in old
                .iter()
                .filter(|entry| project.as_ref().is_none_or(|file| file == &entry.file))
            {
                pending.push_back((entry.platform, Some(entry.project.clone()), None, true));
            }
        }
    }
    let mut providers = HashMap::new();
    let mut selected: HashMap<(Platform, String), String> = HashMap::new();
    let mut incompatible = Vec::new();
    let mut plan = Vec::new();
    while let Some((platform, project, version, direct)) = pending.pop_front() {
        ensure!(selected.len() < 500, "Mod dependency graph is too large.");
        ensure!(
            project.is_some() || version.is_some(),
            "A mod requires an unidentified dependency. Check the project page before installing."
        );
        if let Some(project) = &project {
            if let Some(chosen) = selected.get(&(platform, project.clone())) {
                ensure!(
                    version.as_ref().is_none_or(|version| version == chosen),
                    "Conflicting requirements for mod {project}."
                );
                continue;
            }
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = providers.entry(platform) {
            entry.insert(providers::provider(platform, client, private_client).await?);
        }
        let channel = if direct { update } else { None };
        let Some(file) = providers[&platform]
            .file(
                project.as_deref(),
                version.as_deref(),
                &instance.options,
                channel,
            )
            .await?
        else {
            continue;
        };
        let already = old
            .iter()
            .find(|entry| entry.platform == platform && entry.project == file.project);
        if let Some(already) = already.filter(|_| !direct) {
            ensure!(
                !disabled(&already.file),
                "{} is required but disabled. Enable it and try again.",
                already.title
            );
            ensure!(already.version == file.version || (version.is_none() && already.minecraft_version == instance.options.minecraft_version && already.loader == instance.options.loader),
                "{} requires a different version of an installed dependency. Update or remove {} first.", instance.options.name, already.title);
        }
        if let Some(chosen) =
            selected.insert((platform, file.project.clone()), file.version.clone())
        {
            ensure!(
                chosen == file.version,
                "Conflicting requirements for {}.",
                file.title
            );
            continue;
        }
        for dependency in &file.dependencies {
            if dependency.incompatible {
                incompatible.push((
                    platform,
                    dependency.project.clone(),
                    dependency.version.clone(),
                    file.title.clone(),
                ));
            } else {
                pending.push_back((
                    platform,
                    dependency.project.clone(),
                    dependency.version.clone(),
                    false,
                ));
            }
        }
        let needed = already
            .is_none_or(|entry| (direct || version.is_some()) && entry.version != file.version);
        if needed {
            plan.push((platform, file));
        }
    }
    for (platform, project, version, title) in incompatible {
        let planned = selected.iter().any(|((source, id), selected)| {
            *source == platform
                && project.as_ref().is_none_or(|project| project == id)
                && version.as_ref().is_none_or(|version| version == selected)
                && (project.is_some() || version.is_some())
        });
        let existing = old.iter().any(|entry| {
            entry.platform == platform
                && project.as_ref().is_none_or(|id| id == &entry.project)
                && version
                    .as_ref()
                    .is_none_or(|version| version == &entry.version)
                && !disabled(&entry.file)
                && (project.is_some() || version.is_some())
        });
        ensure!(
            !planned && !existing,
            "{title} declares an incompatibility with another selected or installed mod."
        );
    }
    Ok(plan)
}

fn commit(
    store: &Store,
    instance: &Instance,
    mut old: Vec<ManagedMod>,
    plan: Plan,
    action: Action,
    backup: Uuid,
    work: tempfile::TempDir,
) -> Result<()> {
    let id = instance.id;
    let directory = store.game_dir(id)?.join("mods");
    fs::create_dir_all(&directory)?;
    check_directory(&directory)?;
    let mut owned: HashMap<_, _> = list_files(&directory)?
        .into_iter()
        .map(|name| (name.to_ascii_lowercase(), name))
        .collect();
    let mut writes = Vec::new();
    let mut changes = Vec::new();
    let mut history = Vec::new();
    for (index, (platform, file)) in plan.into_iter().enumerate() {
        check_filename(&file.filename)?;
        ensure!(
            file.filename.to_ascii_lowercase().ends_with(".jar"),
            "The provider returned a non-JAR file."
        );
        let previous = old
            .iter()
            .position(|entry| entry.platform == platform && entry.project == file.project)
            .map(|index| old.swap_remove(index));
        let filename = target_filename(&file.filename, previous.as_ref());
        let before = if let Some(previous) = previous {
            owned.remove(&previous.file.to_ascii_lowercase());
            let source = store.backup_mods(backup, id)?;
            let revision = Revision::read(&source, &previous.file, Some(previous.clone()))?;
            revision.check(&directory)?;
            writes.push((
                directory.join(previous.file),
                work.path().join(format!("{index}.previous")),
            ));
            Some(revision)
        } else {
            None
        };
        ensure!(
            owned
                .insert(filename.to_ascii_lowercase(), filename.clone())
                .is_none(),
            "The mods folder already contains {filename}."
        );
        changes.push((
            work.path().join(format!("{index}.jar")),
            directory.join(&filename),
        ));
        let (export_url, export_sha512) = match (platform, file.hash) {
            (Platform::Modrinth, downloads::Hash::Sha512(hash)) => (Some(file.url), Some(hash)),
            _ => (None, None),
        };
        let managed = ManagedMod {
            platform,
            project: file.project,
            version: file.version,
            title: file.title,
            icon_url: file.icon_url,
            display_version: file.display_version,
            file: filename,
            minecraft_version: instance.options.minecraft_version.clone(),
            loader: instance.options.loader,
            export_url,
            export_sha512,
            dependencies: file.dependencies,
            side: file.side,
        };
        let mut after =
            Revision::read(work.path(), &format!("{index}.jar"), Some(managed.clone()))?;
        after.file = managed.file.clone();
        history.push(Change::new(before, Some(after)));
        old.push(managed);
    }
    writes.extend(changes);
    let mut state = load_state(store, id)?;
    state.mods = old;
    state
        .history
        .push(Record::new(instance, action, Some(backup), history));
    apply_writes(store, id, state, writes, work)
}

fn target_filename(name: &str, previous: Option<&ManagedMod>) -> String {
    if previous.is_some_and(|entry| disabled(&entry.file)) {
        format!("{name}.disabled")
    } else {
        name.into()
    }
}

fn apply_writes(
    store: &Store,
    id: Uuid,
    state: ModState,
    writes: Vec<(PathBuf, PathBuf)>,
    work: tempfile::TempDir,
) -> Result<()> {
    let mut applied = Vec::new();
    let result: Result<()> = (|| {
        for (from, to) in &writes {
            rename_absent(from, to)?;
            applied.push((from, to));
        }
        atomic_json(&registry(store, id), &state)
    })();
    if let Err(error) = result {
        for (from, to) in applied.into_iter().rev() {
            if let Err(rollback) = rename_absent(to, from) {
                let saved = work.keep();
                bail!("Mod change failed: {error}. Rollback stopped: {rollback}. Recover from Backups or {}.", saved.display());
            }
        }
        return Err(error);
    }
    Ok(())
}

pub fn history(store: &Store, id: Uuid) -> Result<Vec<mod_history::RecordView>> {
    let instance = store.instance(id)?;
    Ok(load_state(store, id)?
        .history
        .iter()
        .rev()
        .map(|entry| entry.view(&instance))
        .collect())
}

pub fn undo(store: &Store, id: Uuid, entry_id: Uuid, change: usize) -> Result<()> {
    let instance = store.instance(id)?;
    let mut state = load_state(store, id)?;
    let record = state
        .history
        .iter_mut()
        .find(|entry| entry.id == entry_id)
        .context("This mod history entry no longer exists.")?;
    ensure!(
        record.compatible(&instance),
        "This change was made for a different Minecraft version or loader."
    );
    let current = installed(store, id)?;
    let indexes = record.undo_group(change, &current)?;
    let directory = store.game_dir(id)?.join("mods");
    fs::create_dir_all(&directory)?;
    check_directory(&directory)?;
    let work = tempfile::tempdir_in(store.instance_dir(id))?;
    let mut proposed = current.clone();
    let mut removals = Vec::new();
    let mut restores = Vec::new();
    for index in indexes {
        let item = &mut record.changes[index];
        if let Some(after) = &item.after {
            let live = current.iter().find(|entry| entry.file == after.file);
            ensure!(
                live == after.managed.as_ref(),
                "{} was changed after this entry.",
                after.file
            );
            after.check(&directory)?;
            proposed.retain(|entry| entry.file != after.file);
            removals.push((
                directory.join(&after.file),
                work.path().join(format!("{index}.previous")),
            ));
        }
        if let Some(before) = &item.before {
            let backup = record
                .backup
                .context("The previous mod file has no backup.")?;
            let source = store.backup_mods(backup, id)?;
            before.check(&source)?;
            let staged = work.path().join(format!("{index}.restore"));
            fs::copy(source.join(&before.file), &staged)?;
            if let Some(managed) = &before.managed {
                ensure!(
                    !proposed
                        .iter()
                        .any(|entry| entry.platform == managed.platform
                            && entry.project == managed.project),
                    "{} is already installed at a different revision.",
                    managed.title
                );
                proposed.push(managed.clone());
            }
            restores.push((staged, directory.join(&before.file)));
        }
        item.undone = true;
    }
    mod_history::verify_dependencies(&current, &proposed)?;
    removals.extend(restores);
    state.mods = proposed;
    store.snapshot_content(id)?;
    apply_writes(store, id, state, removals, work)
}

fn rename_absent(from: &Path, to: &Path) -> Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => bail!("{} already exists.", to.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    fs::rename(from, to).with_context(|| format!("Moving {}", from.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store, Instance) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let options = serde_json::from_value(serde_json::json!({
            "name": "Pack", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096
        }))
        .unwrap();
        let instance = store.create(options).unwrap().instance;
        (root, store, instance)
    }

    fn file(project: &str, version: &str, dependencies: Vec<Dependency>) -> (Platform, ModFile) {
        (
            Platform::Modrinth,
            ModFile {
                side: ModSide::Unknown,
                project: project.into(),
                version: version.into(),
                title: project.into(),
                icon_url: None,
                display_version: Some(version.into()),
                filename: format!("{project}-{version}.jar"),
                url: String::new(),
                hash: downloads::Hash::Sha512(String::new()),
                size: None,
                dependencies,
            },
        )
    }

    fn apply_fixture(store: &Store, instance: &Instance, plan: Plan) {
        let work = tempfile::tempdir_in(store.instance_dir(instance.id)).unwrap();
        for (index, (_, file)) in plan.iter().enumerate() {
            fs::write(
                work.path().join(format!("{index}.jar")),
                file.filename.as_bytes(),
            )
            .unwrap();
        }
        let old = installed(store, instance.id).unwrap();
        let backup = store.snapshot(instance.id).unwrap();
        commit(store, instance, old, plan, Action::Update, backup, work).unwrap();
    }

    #[test]
    fn discover_index_uses_existing_managed_files_without_opening_jars() -> Result<()> {
        let (_root, store, instance) = fixture();
        apply_fixture(
            &store,
            &instance,
            vec![file("present", "1", vec![]), file("missing", "1", vec![])],
        );
        let directory = store.game_dir(instance.id)?.join("mods");
        let mut records = registered(&store, instance.id)?;
        let present = records.iter_mut().find(|entry| entry.project == "present").unwrap();
        let disabled = format!("{}.disabled", present.file);
        fs::rename(directory.join(&present.file), directory.join(&disabled))?;
        present.file = disabled;
        fs::remove_file(directory.join("missing-1.jar"))?;
        fs::write(directory.join("unmanaged.jar"), "Local file")?;
        atomic_json(&registry(&store, instance.id), &records)?;
        assert_eq!(installed_projects(&store, instance.id)?, vec![(Platform::Modrinth, "present".into())]);
        Ok(())
    }

    #[test]
    fn undoes_one_mod_and_its_added_dependency_without_rewinding_world() {
        let (_root, store, instance) = fixture();
        apply_fixture(
            &store,
            &instance,
            vec![file("a", "1", vec![]), file("b", "1", vec![])],
        );
        let previous = registered(&store, instance.id).unwrap();
        atomic_json(&registry(&store, instance.id), &previous).unwrap();
        assert_eq!(installed(&store, instance.id).unwrap().len(), 2);
        let game = store.game_dir(instance.id).unwrap();
        fs::create_dir(game.join("saves")).unwrap();
        fs::write(game.join("saves/world-progress"), "before update").unwrap();
        let dep = Dependency {
            project: Some("dependency".into()),
            version: Some("1".into()),
            incompatible: false,
        };
        apply_fixture(
            &store,
            &instance,
            vec![
                file("a", "2", vec![dep]),
                file("b", "2", vec![]),
                file("dependency", "1", vec![]),
            ],
        );
        fs::write(game.join("saves/world-progress"), "new progress").unwrap();
        let state = load_state(&store, instance.id).unwrap();
        let entry = state.history.last().unwrap().id;
        undo(&store, instance.id, entry, 0).unwrap();
        let current = list_files(&game.join("mods")).unwrap();
        assert_eq!(current, ["a-1.jar", "b-2.jar"]);
        assert_eq!(
            fs::read_to_string(game.join("saves/world-progress")).unwrap(),
            "new progress"
        );
        let state = load_state(&store, instance.id).unwrap();
        assert!(state.history[0].changes[0].undone);
        assert!(!state.history[0].changes[1].undone);
        assert!(state.history[0].changes[2].undone);
    }

    #[test]
    fn preserves_a_shared_dependency_and_rejects_changed_jar() {
        let (_root, store, instance) = fixture();
        let dep = Dependency {
            project: Some("shared".into()),
            version: None,
            incompatible: false,
        };
        apply_fixture(
            &store,
            &instance,
            vec![
                file("a", "1", vec![dep.clone()]),
                file("b", "1", vec![dep]),
                file("shared", "1", vec![]),
            ],
        );
        let entry = load_state(&store, instance.id).unwrap().history[0].id;
        undo(&store, instance.id, entry, 0).unwrap();
        let mods = store.game_dir(instance.id).unwrap().join("mods");
        assert_eq!(list_files(&mods).unwrap(), ["b-1.jar", "shared-1.jar"]);
        assert!(undo(&store, instance.id, entry, 2).is_err());
        fs::write(mods.join("b-1.jar"), "manually replaced").unwrap();
        assert!(undo(&store, instance.id, entry, 1).is_err());
        assert_eq!(
            fs::read_to_string(mods.join("b-1.jar")).unwrap(),
            "manually replaced"
        );
    }

    #[test]
    fn prepared_updates_match_a_copy_and_reject_new_source_contents() {
        let (_root, store, instance) = fixture();
        apply_fixture(&store, &instance, vec![file("a", "1", vec![])]);
        let old = installed(&store, instance.id).unwrap();
        let directory = store.game_dir(instance.id).unwrap().join("mods");
        let prepared = PreparedUpdate {
            instance_id: instance.id,
            revisions: vec![
                Revision::read(&directory, &old[0].file, Some(old[0].clone())).unwrap(),
            ],
            old,
            plan: vec![file("a", "2", vec![])],
            files: list_files(&directory).unwrap(),
            game: instance.options.minecraft_version.clone(),
            loader: instance.options.loader,
            loader_version: instance.options.loader_version.clone(),
            channel: instance.options.update_channel,
        };
        let copy = store
            .duplicate_with_suffix(instance.id, " (test)")
            .unwrap()
            .instance;
        prepared.check(&store, &copy).unwrap();
        assert!(history(&store, copy.id).unwrap().is_empty());
        fs::write(directory.join("a-1.jar"), "new contents").unwrap();
        assert!(prepared.check(&store, &instance).is_err());
    }
}
