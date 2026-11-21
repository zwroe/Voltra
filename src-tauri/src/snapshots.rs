use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::storage::{self, Instance, Store};

pub const GAME_DIRS: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "resourcepacks",
    "shaderpacks",
    "datapacks",
    "kubejs",
    "scripts",
    "global_packs",
    "openloader",
    "patchouli_books",
];
pub const GAME_FILES: &[&str] = &["options.txt", "optionsof.txt", "optionsshaders.txt"];
pub const REGISTRY_FILES: &[&str] = &["mods.json", "mod-sides.json"];

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    bytes: u64,
    sha256: String,
}

pub type Manifest = BTreeMap<String, FileInfo>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Difference {
    pub path: String,
    pub before: Option<FileInfo>,
    pub after: Option<FileInfo>,
}

pub fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn copy_content(source: &Path, destination: &Path) -> Result<()> {
    storage::check_directory(source)?;
    for name in GAME_DIRS {
        let path = source.join(name);
        if present(&path)? {
            storage::copy_tree(&path, &destination.join(name))?;
        }
    }
    for name in GAME_FILES {
        let path = source.join(name);
        if present(&path)? {
            storage::regular_file(&path)?;
            fs::copy(path, destination.join(name))?;
        }
    }
    Ok(())
}

fn fingerprint(path: &Path) -> Result<FileInfo> {
    storage::regular_file(path)?;
    let mut input = fs::File::open(path)?;
    let metadata = input.metadata()?;
    let mut digest = Sha256::new();
    io::copy(&mut input, &mut digest)?;
    Ok(FileInfo {
        bytes: metadata.len(),
        sha256: hex::encode(digest.finalize()),
    })
}

fn walk(path: &Path, prefix: &str, files: &mut Manifest) -> Result<()> {
    storage::check_directory(path)?;
    for child in fs::read_dir(path)? {
        let child = child?;
        let name = child
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("A pack file has an unreadable name."))?;
        storage::check_filename(&name)?;
        let key = format!("{prefix}/{name}");
        if child.file_type()?.is_dir() {
            walk(&child.path(), &key, files)?;
        } else {
            files.insert(key, fingerprint(&child.path())?);
        }
    }
    Ok(())
}

pub fn scan(root: &Path, include_registration: bool) -> Result<Manifest> {
    let game = root.join(".minecraft");
    storage::check_directory(&game)?;
    let mut files = Manifest::new();
    for name in GAME_DIRS {
        let folder = game.join(name);
        if present(&folder)? {
            walk(&folder, name, &mut files)?;
        }
    }
    for name in GAME_FILES {
        let path = game.join(name);
        if present(&path)? {
            files.insert((*name).into(), fingerprint(&path)?);
        }
    }
    if include_registration {
        for name in REGISTRY_FILES.iter().chain([&"instance.json"]) {
            let path = root.join(name);
            if present(&path)? {
                files.insert(format!("metadata/{name}"), fingerprint(&path)?);
            }
        }
    }
    Ok(files)
}

pub fn diff(before: &Manifest, after: &Manifest) -> Vec<Difference> {
    before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let (old, new) = (before.get(key), after.get(key));
            (old != new).then(|| Difference {
                path: key.clone(),
                before: old.cloned(),
                after: new.cloned(),
            })
        })
        .collect()
}

pub fn capacity(entries: &Manifest) -> u64 {
    entries.values().map(|entry| entry.bytes).sum()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeFile {
    path: String,
    before: Option<FileInfo>,
    after: FileInfo,
}

pub fn compare_instances(store: &Store, id: Uuid, source: Uuid) -> Result<Vec<Difference>> {
    ensure!(id != source, "Choose a different source instance.");
    store.instance(id)?;
    store.instance(source)?;
    Ok(diff(
        &scan(&store.instance_dir(id), false)?,
        &scan(&store.instance_dir(source), false)?,
    ))
}

pub fn merge(store: &Store, id: Uuid, source: Uuid, entries: &[MergeFile]) -> Result<()> {
    ensure!(
        id != source && !entries.is_empty() && entries.len() <= 4000,
        "Choose files to copy from another pack."
    );
    let target = store.instance(id)?;
    let incoming = store.instance(source)?;
    let game = store.game_dir(id)?;
    let source_game = store.game_dir(source)?;
    let instance_dir = store.instance_dir(id);
    let work = tempfile::tempdir_in(&instance_dir)?;
    let staged = work.path().join("incoming");
    let previous = work.path().join("previous");
    fs::create_dir(&staged)?;
    fs::create_dir(&previous)?;
    let mut moves = Vec::new();
    let mut paths = std::collections::HashSet::new();
    let mut mod_files = std::collections::HashSet::new();
    let mut create_dirs = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        ensure!(
            paths.insert(entry.path.to_lowercase()),
            "The selection includes a duplicate path."
        );
        let segments: Vec<_> = entry.path.split('/').collect();
        for segment in &segments {
            storage::check_filename(segment)?;
        }
        let root = *segments.first().context("Missing file path")?;
        ensure!(
            (segments.len() == 1 && GAME_FILES.contains(&root))
                || (segments.len() > 1 && GAME_DIRS.contains(&root)),
            "Choose files from the pack comparison."
        );
        if root == "mods" {
            ensure!(
                segments.len() == 2 && storage::is_mod(segments[1]),
                "Choose a mod JAR."
            );
            ensure!(target.options.minecraft_version == incoming.options.minecraft_version && target.options.loader == incoming.options.loader,
                "The source and destination need the same Minecraft version and loader to merge mods.");
            mod_files.insert(segments[1].to_owned());
        }
        let mut from = source_game.clone();
        let mut to = game.clone();
        for (segment_index, segment) in segments.iter().enumerate() {
            from.push(segment);
            to.push(segment);
            if segment_index + 1 < segments.len() {
                storage::check_directory(&from)?;
                if present(&to)? {
                    storage::check_directory(&to)?;
                } else {
                    create_dirs.push(to.clone());
                }
            }
        }
        ensure!(
            fingerprint(&from)? == entry.after,
            "{} changed in the source. Preview the merge again.",
            entry.path
        );
        let before = present(&to)?.then(|| fingerprint(&to)).transpose()?;
        ensure!(
            before == entry.before,
            "{} changed in the destination. Preview the merge again.",
            entry.path
        );
        let staged_file = staged.join(i.to_string());
        fs::copy(&from, &staged_file)?;
        ensure!(
            fingerprint(&staged_file)? == entry.after,
            "{} changed while copying.",
            entry.path
        );
        if before.is_some() {
            moves.push((to.clone(), previous.join(i.to_string())));
        }
        moves.push((staged_file, to));
    }
    if !mod_files.is_empty() {
        let registry = instance_dir.join("mods.json");
        let stage_registry = staged.join("mods.json");
        crate::mods::stage_merged_registry(store, id, source, &mod_files, &stage_registry)?;
        if present(&registry)? {
            storage::regular_file(&registry)?;
            moves.push((registry.clone(), previous.join("mods.json")));
        }
        moves.push((stage_registry, registry));
    }
    store.snapshot_content(id)?;
    for directory in create_dirs {
        fs::create_dir_all(directory)?;
    }
    move_all(&moves, work)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldRisk {
    pub has_worlds: bool,
    pub baseline_available: bool,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct WorldBaseline {
    minecraft: String,
    loader: String,
    mods: Manifest,
}

fn baseline(store: &Store, instance: &Instance) -> Result<WorldBaseline> {
    let directory = store.game_dir(instance.id)?.join("mods");
    let mut mods = Manifest::new();
    if present(&directory)? {
        for name in storage::list_files(&directory)? {
            if name.to_ascii_lowercase().ends_with(".jar") {
                mods.insert(name.clone(), fingerprint(&directory.join(&name))?);
            }
        }
    }
    Ok(WorldBaseline {
        minecraft: instance.options.minecraft_version.clone(),
        loader: instance.options.loader.as_str().into(),
        mods,
    })
}

pub fn world_risk(store: &Store, id: Uuid, world: Option<&str>) -> Result<WorldRisk> {
    let game = store.game_dir(id)?;
    let saves = game.join("saves");
    let mut risk = WorldRisk {
        has_worlds: false,
        baseline_available: false,
        removed: Vec::new(),
        changed: Vec::new(),
    };
    if !present(&saves)? {
        return Ok(risk);
    }
    storage::check_directory(&saves)?;
    let worlds: Vec<_> = fs::read_dir(saves)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.path().join("level.dat").is_file()
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| world.is_none_or(|world| world == name))
        .collect();
    risk.has_worlds = !worlds.is_empty();
    let path = store.instance_dir(id).join("world-mod-baselines.json");
    if !risk.has_worlds || !present(&path)? {
        return Ok(risk);
    }
    storage::regular_file(&path)?;
    let baselines: BTreeMap<String, WorldBaseline> = storage::read_json(&path)?;
    risk.baseline_available = worlds.iter().all(|name| baselines.contains_key(name));
    if !worlds.iter().any(|name| baselines.contains_key(name)) {
        return Ok(risk);
    }
    let current = baseline(store, &store.instance(id)?)?;
    for previous in worlds.iter().filter_map(|name| baselines.get(name)) {
        risk.removed.extend(
            previous
                .mods
                .keys()
                .filter(|name| !current.mods.contains_key(*name))
                .cloned(),
        );
        risk.changed.extend(
            previous
                .mods
                .iter()
                .filter(|(name, entry)| {
                    current.mods.get(*name).is_some_and(|value| value != *entry)
                })
                .map(|(name, _)| name.clone()),
        );
        if previous.minecraft != current.minecraft {
            risk.changed.push(format!(
                "Minecraft {} to {}",
                previous.minecraft, current.minecraft
            ));
        }
        if previous.loader != current.loader {
            risk.changed
                .push(format!("Loader {} to {}", previous.loader, current.loader));
        }
    }
    risk.removed.sort_unstable();
    risk.removed.dedup();
    risk.changed.sort_unstable();
    risk.changed.dedup();
    Ok(risk)
}

pub fn save_baseline(store: &Store, instance: &Instance, worlds: &[String]) -> Result<()> {
    if worlds.is_empty() {
        return Ok(());
    }
    let path = store
        .instance_dir(instance.id)
        .join("world-mod-baselines.json");
    if present(&path)? {
        storage::regular_file(&path)?;
    }
    let mut baselines: BTreeMap<String, WorldBaseline> = storage::read_default(&path)?;
    let files = baseline(store, instance)?;
    for world in worlds {
        storage::check_filename(world)?;
        baselines.insert(world.clone(), files.clone());
    }
    storage::atomic_json(&path, &baselines)
}

pub fn move_all(moves: &[(PathBuf, PathBuf)], work: tempfile::TempDir) -> Result<()> {
    for (index, (from, to)) in moves.iter().enumerate() {
        let move_file = || -> Result<()> {
            ensure!(
                !present(to)?,
                "The destination {} already exists.",
                to.display()
            );
            fs::rename(from, to).with_context(|| format!("Moving {}", from.display()))
        };
        if let Err(error) = move_file() {
            for (source, target) in moves[..index].iter().rev() {
                if let Err(recovery) = fs::rename(target, source) {
                    anyhow::bail!(
                        "Change failed: {error}. Rollback stopped: {recovery}. Recovery files: {}",
                        work.keep().display()
                    );
                }
            }
            return Err(error);
        }
    }
    Ok(())
}
