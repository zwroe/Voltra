use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{self, File, Metadata},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    Neoforge,
}

impl Loader {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::Neoforge => "neoforge",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub ram_mib: u32,
    pub java_path: String,
    pub download_limit: usize,
    pub theme: Theme,
    pub animations: bool,
    pub ui_scale: u16,
    pub snapshot_keep: usize,
    pub snapshot_limit_mib: u64,
    pub discord_presence: bool,
    pub discord_show_pack: bool,
    pub window: GameWindow,
    pub hooks: LaunchHooks,
    pub jvm_args: Vec<String>,
    pub environment: Vec<EnvironmentVariable>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ram_mib: 4096,
            java_path: String::new(),
            download_limit: 8,
            theme: Theme::Dark,
            animations: true,
            ui_scale: 115,
            snapshot_keep: 12,
            snapshot_limit_mib: 8192,
            discord_presence: false,
            discord_show_pack: false,
            window: GameWindow::default(),
            hooks: LaunchHooks::default(),
            jvm_args: Vec::new(),
            environment: Vec::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        check_ram(self.ram_mib)?;
        ensure!(
            (1..=16).contains(&self.download_limit),
            "Choose 1–16 concurrent downloads."
        );
        ensure!(
            (90..=150).contains(&self.ui_scale),
            "Choose a UI scale from 90% to 150%."
        );
        self.window.validate()?;
        ensure!(
            (1..=500).contains(&self.snapshot_keep),
            "Keep between 1 and 500 modpack snapshots."
        );
        ensure!(
            (256..=1_048_576).contains(&self.snapshot_limit_mib),
            "Choose a snapshot budget between 256 MiB and 1 TiB."
        );
        self.hooks.validate()?;
        check_arguments(&self.jvm_args)?;
        check_environment(&self.environment)?;
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceOptions {
    pub name: String,
    pub minecraft_version: String,
    pub loader: Loader,
    #[serde(default)]
    pub loader_version: Option<String>,
    pub ram_mib: u32,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub update_channel: UpdateChannel,
    #[serde(default)]
    pub launch: LaunchOverrides,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Release,
    Beta,
    Alpha,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchOverrides {
    pub inherit_memory: bool,
    pub java_path: Option<String>,
    pub environment: Option<Vec<EnvironmentVariable>>,
    pub window: GameWindow,
    pub hooks: LaunchHooks,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameWindow {
    pub fullscreen: Option<bool>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl GameWindow {
    fn validate(&self) -> Result<()> {
        ensure!(
            [self.width, self.height]
                .into_iter()
                .flatten()
                .all(|size| (100..=16384).contains(&size)),
            "Window dimensions must be between 100 and 16384 pixels."
        );
        Ok(())
    }

    pub fn with_defaults(&self, defaults: &Self) -> Self {
        Self {
            fullscreen: self.fullscreen.or(defaults.fullscreen),
            width: self.width.or(defaults.width),
            height: self.height.or(defaults.height),
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchHooks {
    pub pre_launch: Option<String>,
    pub post_exit: Option<String>,
    pub wrapper: Option<String>,
    pub wrapper_args: Option<Vec<String>>,
}

impl LaunchHooks {
    fn validate(&self) -> Result<()> {
        ensure!(
            [&self.pre_launch, &self.post_exit, &self.wrapper]
                .into_iter()
                .flatten()
                .all(|value| value.len() <= 4096 && !value.contains(['\0', '\r', '\n'])),
            "Use a single line of up to 4096 characters for each launch hook."
        );
        if let Some(args) = &self.wrapper_args {
            check_arguments(args)?;
        }
        Ok(())
    }

    pub fn with_defaults(&self, defaults: &Self) -> Self {
        Self {
            pre_launch: self
                .pre_launch
                .clone()
                .or_else(|| defaults.pre_launch.clone()),
            post_exit: self
                .post_exit
                .clone()
                .or_else(|| defaults.post_exit.clone()),
            wrapper: self.wrapper.clone().or_else(|| defaults.wrapper.clone()),
            wrapper_args: self
                .wrapper_args
                .clone()
                .or_else(|| defaults.wrapper_args.clone()),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

fn check_arguments(args: &[String]) -> Result<()> {
    ensure!(
        args.len() <= 64
            && args
                .iter()
                .all(|arg| !arg.is_empty() && arg.len() <= 4096 && !arg.contains('\0')),
        "Use up to 64 nonempty arguments."
    );
    Ok(())
}

fn check_environment(items: &[EnvironmentVariable]) -> Result<()> {
    ensure!(items.len() <= 64, "Use up to 64 environment variables.");
    let mut names = std::collections::HashSet::new();
    for item in items {
        ensure!(
            !item.name.is_empty()
                && item.name.len() <= 128
                && item
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                && !item.name.as_bytes()[0].is_ascii_digit()
                && item.value.len() <= 4096
                && !item.value.contains(['\0', '\r', '\n'])
                && names.insert(item.name.to_ascii_uppercase()),
            "Use unique environment variable names and a single-line value for each."
        );
    }
    Ok(())
}

fn validate_icon(data: &str) -> Result<()> {
    ensure!(data.len() <= 256 * 1024, "The instance icon is too large.");
    let bytes = STANDARD
        .decode(data)
        .context("The instance icon is not valid PNG data.")?;
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .context("The instance icon is not a PNG.")?;
    let info = reader.info();
    ensure!(
        (1..=192).contains(&info.width) && (1..=192).contains(&info.height),
        "The instance icon must fit within 192 by 192 pixels."
    );
    let mut pixels = vec![
        0;
        reader
            .output_buffer_size()
            .context("Invalid icon dimensions.")?
    ];
    reader
        .next_frame(&mut pixels)
        .context("Decoding the instance icon")?;
    Ok(())
}

impl InstanceOptions {
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_owned();
        ensure!(
            !self.name.is_empty()
                && self.name.chars().count() <= 64
                && !self.name.chars().any(char::is_control),
            "Enter an instance name between 1 and 64 characters."
        );
        check_id(&self.minecraft_version)?;
        if let Some(version) = &self.loader_version {
            check_id(version)?;
        }
        check_ram(self.ram_mib)?;
        check_arguments(&self.jvm_args)?;
        self.launch.window.validate()?;
        self.launch.hooks.validate()?;
        if let Some(path) = &self.launch.java_path {
            ensure!(
                path.len() <= 4096 && !path.contains('\0'),
                "Invalid Java executable path."
            );
        }
        if let Some(environment) = &self.launch.environment {
            check_environment(environment)?;
        }
        if let Some(icon) = &self.icon {
            validate_icon(icon)?;
        }
        if self.loader == Loader::Vanilla {
            self.loader_version = None;
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub id: Uuid,
    #[serde(flatten)]
    pub options: InstanceOptions,
    pub created_at: u64,
    pub last_played_at: Option<u64>,
    #[serde(default)]
    pub playtime_seconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceView {
    #[serde(flatten)]
    pub instance: Instance,
    pub mod_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub id: Uuid,
    pub instance_id: Uuid,
    pub name: String,
    pub minecraft_version: String,
    pub created_at: u64,
    pub archived: bool,
    pub content_only: bool,
    pub bytes: u64,
}

#[derive(Deserialize, Serialize)]
struct BackupInfo {
    created_at: u64,
    archived: bool,
    #[serde(default)]
    content_only: bool,
    #[serde(default)]
    bytes: u64,
}

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        for folder in ["config", "instances", "cache", "logs", "backups", "servers"] {
            fs::create_dir_all(root.join(folder)).with_context(|| format!("Creating {folder}"))?;
        }
        Ok(Self { root })
    }

    pub fn settings(&self) -> Result<Settings> {
        read_default(&self.root.join("config/settings.json"))
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        atomic_json(&self.root.join("config/settings.json"), settings)
    }

    pub fn instance_dir(&self, id: Uuid) -> PathBuf {
        self.root.join("instances").join(id.to_string())
    }

    pub fn game_dir(&self, id: Uuid) -> Result<PathBuf> {
        let root = self.instance_dir(id);
        check_directory(&root)?;
        let game = root.join(".minecraft");
        check_directory(&game)?;
        Ok(game)
    }

    pub fn instance(&self, id: Uuid) -> Result<Instance> {
        read_instance(&self.instance_dir(id), id)
    }

    pub fn instance_ids(&self) -> Result<Vec<Uuid>> {
        Ok(uuid_directories(&self.root.join("instances"))?
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    pub fn instances(&self) -> Result<Vec<InstanceView>> {
        let mut instances = Vec::new();
        for (id, path) in uuid_directories(&self.root.join("instances"))? {
            instances.push(self.view(read_instance(&path, id)?)?);
        }
        instances.sort_by_key(|item| {
            std::cmp::Reverse(
                item.instance
                    .last_played_at
                    .unwrap_or(item.instance.created_at),
            )
        });
        Ok(instances)
    }

    fn view(&self, instance: Instance) -> Result<InstanceView> {
        let mods = self.game_dir(instance.id)?.join("mods");
        let mod_count = list_files(&mods)?
            .iter()
            .filter(|name| is_mod(name))
            .count();
        Ok(InstanceView {
            instance,
            mod_count,
        })
    }

    pub fn create(&self, mut options: InstanceOptions) -> Result<InstanceView> {
        self.create_from(&mut options, None)
    }

    pub fn create_from(
        &self,
        options: &mut InstanceOptions,
        contents: Option<&Path>,
    ) -> Result<InstanceView> {
        options.validate()?;
        let instance = Instance {
            id: Uuid::new_v4(),
            options: options.clone(),
            created_at: now(),
            last_played_at: None,
            playtime_seconds: 0,
        };
        let staging = tempfile::tempdir_in(self.root.join("instances"))?;
        let game = staging.path().join(".minecraft");
        if let Some(contents) = contents {
            copy_tree(contents, &game)?;
        } else {
            fs::create_dir(&game)?;
        }
        atomic_json(&staging.path().join("instance.json"), &instance)?;
        fs::rename(staging.path(), self.instance_dir(instance.id))?;
        self.view(instance)
    }

    pub fn update(&self, id: Uuid, mut options: InstanceOptions) -> Result<InstanceView> {
        options.validate()?;
        let mut instance = self.instance(id)?;
        if instance.options.minecraft_version != options.minecraft_version
            || instance.options.loader != options.loader
            || instance.options.loader_version != options.loader_version
        {
            self.snapshot_content(id)?;
        }
        instance.options = options;
        self.write_instance(&instance)?;
        self.view(instance)
    }

    pub fn mark_played(
        &self,
        mut instance: Instance,
        loader_version: Option<String>,
        started_at: u64,
    ) -> Result<()> {
        instance.last_played_at = Some(started_at);
        instance.options.loader_version = loader_version;
        self.write_instance(&instance)
    }

    pub fn save_playtime(&self, id: Uuid, started_at: u64, total: u64) -> Result<()> {
        let mut instance = self.instance(id)?;
        if instance.last_played_at == Some(started_at) && instance.playtime_seconds >= total {
            return Ok(());
        }
        instance.last_played_at = Some(started_at);
        instance.playtime_seconds = instance.playtime_seconds.max(total);
        self.write_instance(&instance)
    }

    pub fn pin_loader(&self, instance: &Instance) -> Result<()> {
        self.write_instance(instance)
    }

    fn write_instance(&self, instance: &Instance) -> Result<()> {
        atomic_json(
            &self.instance_dir(instance.id).join("instance.json"),
            instance,
        )
    }

    pub fn duplicate(&self, id: Uuid) -> Result<InstanceView> {
        self.duplicate_with_suffix(id, " (copy)")
    }

    pub fn duplicate_with_suffix(&self, id: Uuid, suffix: &str) -> Result<InstanceView> {
        let mut instance = self.instance(id)?;
        let staging = tempfile::tempdir_in(self.root.join("instances"))?;
        copy_tree(&self.game_dir(id)?, &staging.path().join(".minecraft"))?;
        crate::mods::copy_registry(self, id, staging.path())?;
        instance.id = Uuid::new_v4();
        let prefix: String = instance
            .options
            .name
            .chars()
            .take(64usize.saturating_sub(suffix.chars().count()))
            .collect();
        instance.options.name = format!("{prefix}{suffix}");
        instance.created_at = now();
        instance.last_played_at = None;
        instance.playtime_seconds = 0;
        atomic_json(&staging.path().join("instance.json"), &instance)?;
        fs::rename(staging.path(), self.instance_dir(instance.id))?;
        self.view(instance)
    }

    pub fn snapshot(&self, id: Uuid) -> Result<Uuid> {
        let instance = self.instance(id)?;
        let (backup_id, staging) = self.prepare_backup(false)?;
        let target = staging.path().join("instance");
        fs::create_dir(&target)?;
        copy_tree(&self.game_dir(id)?, &target.join(".minecraft"))?;
        self.copy_mod_registry(id, &target)?;
        atomic_json(&target.join("instance.json"), &instance)?;
        fs::rename(staging.path(), self.backup_dir(backup_id))?;
        Ok(backup_id)
    }

    pub fn snapshot_content(&self, id: Uuid) -> Result<Uuid> {
        let instance = self.instance(id)?;
        let (backup_id, staging) = self.prepare_backup(false)?;
        let target = staging.path().join("instance");
        let game = target.join(".minecraft");
        fs::create_dir_all(&game)?;
        crate::snapshots::copy_content(&self.game_dir(id)?, &game)?;
        self.copy_mod_registry(id, &target)?;
        atomic_json(&target.join("instance.json"), &instance)?;
        let manifest = crate::snapshots::scan(&target, true)?;
        atomic_json(&staging.path().join("manifest.json"), &manifest)?;
        atomic_json(
            &staging.path().join("backup.json"),
            &BackupInfo {
                created_at: now(),
                archived: false,
                content_only: true,
                bytes: crate::snapshots::capacity(&manifest),
            },
        )?;
        fs::rename(staging.path(), self.backup_dir(backup_id))?;
        Ok(backup_id)
    }

    pub fn content_snapshots(&self, id: Uuid) -> Result<Vec<Backup>> {
        self.instance(id)?;
        Ok(self
            .backups()?
            .into_iter()
            .filter(|item| item.content_only && item.instance_id == id)
            .collect())
    }

    pub fn snapshot_difference(
        &self,
        id: Uuid,
        snapshot: Uuid,
        other: Option<Uuid>,
    ) -> Result<Vec<crate::snapshots::Difference>> {
        let manifest = |snapshot| -> Result<crate::snapshots::Manifest> {
            let backup = self.backup(snapshot)?;
            ensure!(
                backup.content_only && backup.instance_id == id,
                "Choose a snapshot of this modpack."
            );
            read_json(&self.backup_dir(snapshot).join("manifest.json"))
        };
        let before = manifest(snapshot)?;
        let after = match other {
            Some(other) => manifest(other)?,
            None => crate::snapshots::scan(&self.instance_dir(id), true)?,
        };
        Ok(crate::snapshots::diff(&before, &after))
    }

    pub fn snapshot_cleanup(&self, id: Uuid) -> Result<Vec<Backup>> {
        let settings = self.settings()?;
        let mut kept_bytes = 0u64;
        Ok(self
            .content_snapshots(id)?
            .into_iter()
            .enumerate()
            .filter_map(|(index, snapshot)| {
                kept_bytes = kept_bytes.saturating_add(snapshot.bytes);
                (index > 0
                    && (index >= settings.snapshot_keep
                        || kept_bytes > settings.snapshot_limit_mib * 1024 * 1024))
                    .then_some(snapshot)
            })
            .collect())
    }

    pub fn prune_snapshots(&self, id: Uuid, selections: &[Uuid]) -> Result<()> {
        let candidates = self.snapshot_cleanup(id)?;
        ensure!(
            !selections.is_empty()
                && selections
                    .iter()
                    .all(|id| candidates.iter().any(|item| item.id == *id)),
            "The retention preview changed. Review the current list first."
        );
        let selections: std::collections::HashSet<_> = selections.iter().copied().collect();
        for id in selections {
            let target = self.backup_dir(id);
            check_directory(&target)?;
            fs::remove_dir_all(&target).with_context(|| format!("Removing snapshot {id}"))?;
        }
        Ok(())
    }

    pub fn archive(&self, id: Uuid) -> Result<Uuid> {
        self.instance(id)?;
        let (backup_id, staging) = self.prepare_backup(true)?;
        let final_dir = self.backup_dir(backup_id);
        fs::rename(staging.path(), &final_dir)?;
        // Publish metadata before moving the only copy of the game files.
        fs::rename(self.instance_dir(id), final_dir.join("instance"))
            .context("Moving the instance to Backups. Close Minecraft and any open game files.")?;
        Ok(backup_id)
    }

    pub fn backup_instance(&self, backup_id: Uuid) -> Result<Uuid> {
        Ok(self.backup(backup_id)?.instance_id)
    }

    pub fn backup_mods(&self, backup_id: Uuid, id: Uuid) -> Result<PathBuf> {
        ensure!(
            self.backup_instance(backup_id)? == id,
            "This backup belongs to another instance."
        );
        let game = self.backup_dir(backup_id).join("instance/.minecraft");
        check_directory(&game)?;
        let mods = game.join("mods");
        check_directory(&mods)?;
        Ok(mods)
    }

    pub fn restore(&self, backup_id: Uuid) -> Result<InstanceView> {
        let backup = self.backup(backup_id)?;
        let source = self.backup_dir(backup_id).join("instance");
        let active = self.instance_dir(backup.instance_id);
        if backup.content_only {
            let mut instance = self.instance(backup.instance_id)?;
            let saved = read_instance(&source, instance.id)?;
            let live_game = self.game_dir(instance.id)?;
            let work = tempfile::tempdir_in(&active)?;
            let staged = work.path().join("new");
            let previous = work.path().join("previous");
            fs::create_dir(&staged)?;
            fs::create_dir(&previous)?;
            let game = staged.join(".minecraft");
            let old_game = previous.join(".minecraft");
            fs::create_dir(&game)?;
            fs::create_dir(&old_game)?;
            crate::snapshots::copy_content(&source.join(".minecraft"), &game)?;
            for name in crate::snapshots::REGISTRY_FILES {
                let path = source.join(name);
                if crate::snapshots::present(&path)? {
                    regular_file(&path)?;
                    fs::copy(path, staged.join(name))?;
                }
            }
            instance.options = saved.options;
            atomic_json(&staged.join("instance.json"), &instance)?;
            let mut moves = Vec::new();
            let mut stage_moves =
                |live: &Path, next: &Path, old: &Path, names: &[&str]| -> Result<()> {
                    for name in names {
                        let installed = live.join(name);
                        let replacement = next.join(name);
                        if crate::snapshots::present(&installed)? {
                            moves.push((installed.clone(), old.join(name)));
                        }
                        if crate::snapshots::present(&replacement)? {
                            moves.push((replacement, installed));
                        }
                    }
                    Ok(())
                };
            stage_moves(&live_game, &game, &old_game, crate::snapshots::GAME_DIRS)?;
            stage_moves(&live_game, &game, &old_game, crate::snapshots::GAME_FILES)?;
            stage_moves(
                &active,
                &staged,
                &previous,
                crate::snapshots::REGISTRY_FILES,
            )?;
            stage_moves(&active, &staged, &previous, &["instance.json"])?;
            self.snapshot_content(instance.id)?;
            crate::snapshots::move_all(&moves, work)?;
            return self.view(instance);
        }
        if backup.archived && !active.exists() {
            fs::rename(source, active)?;
            return self.view(self.instance(backup.instance_id)?);
        }
        let staging = tempfile::tempdir_in(self.root.join("instances"))?;
        let replacement = staging.path().join("replacement");
        copy_tree(&source, &replacement)?;
        let safety = if active.exists() {
            let current = self.instance(backup.instance_id)?;
            let mut restored = read_instance(&replacement, backup.instance_id)?;
            restored.last_played_at = restored.last_played_at.max(current.last_played_at);
            restored.playtime_seconds = restored.playtime_seconds.max(current.playtime_seconds);
            atomic_json(&replacement.join("instance.json"), &restored)?;
            let sessions = active.join("sessions");
            if crate::snapshots::present(&sessions)? {
                let destination = replacement.join("sessions");
                fs::create_dir_all(&destination)?;
                check_directory(&destination)?;
                for file in list_files(&sessions)? {
                    if !crate::snapshots::present(&destination.join(&file))? {
                        fs::copy(sessions.join(&file), destination.join(&file))?;
                    }
                }
            }
            Some(self.archive(backup.instance_id)?)
        } else {
            None
        };
        if let Err(error) = fs::rename(&replacement, &active) {
            if let Some(id) = safety {
                if let Err(recovery) = fs::rename(self.backup_dir(id).join("instance"), &active) {
                    bail!("Restore failed: {error}. The current instance is safe in archive {id}; moving it back failed: {recovery}.");
                }
            }
            return Err(error).context("Restoring instance");
        }
        self.view(self.instance(backup.instance_id)?)
    }

    pub fn backups(&self) -> Result<Vec<Backup>> {
        let mut backups = Vec::new();
        for (id, path) in uuid_directories(&self.root.join("backups"))? {
            if path.join("instance").exists() {
                backups.push(self.backup(id)?);
            }
        }
        backups.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        Ok(backups)
    }

    fn backup(&self, id: Uuid) -> Result<Backup> {
        let path = self.backup_dir(id);
        check_directory(&path)?;
        check_directory(&path.join("instance"))?;
        let info: BackupInfo = read_json(&path.join("backup.json"))?;
        let instance: Instance = read_json(&path.join("instance/instance.json"))?;
        Ok(Backup {
            id,
            instance_id: instance.id,
            name: instance.options.name,
            minecraft_version: instance.options.minecraft_version,
            created_at: info.created_at,
            archived: info.archived,
            content_only: info.content_only,
            bytes: info.bytes,
        })
    }

    fn prepare_backup(&self, archived: bool) -> Result<(Uuid, tempfile::TempDir)> {
        let staging = tempfile::tempdir_in(self.root.join("backups"))?;
        atomic_json(
            &staging.path().join("backup.json"),
            &BackupInfo {
                created_at: now(),
                archived,
                content_only: false,
                bytes: 0,
            },
        )?;
        Ok((Uuid::new_v4(), staging))
    }

    fn copy_mod_registry(&self, id: Uuid, target: &Path) -> Result<()> {
        for name in ["mods.json", "mod-sides.json"] {
            let source = self.instance_dir(id).join(name);
            match fs::symlink_metadata(&source) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    regular_file(&source)?;
                    fs::copy(source, target.join(name))?;
                }
            }
        }
        Ok(())
    }

    fn backup_dir(&self, id: Uuid) -> PathBuf {
        self.root.join("backups").join(id.to_string())
    }
}

fn read_instance(path: &Path, id: Uuid) -> Result<Instance> {
    check_directory(path).with_context(|| format!("Opening instance {id}"))?;
    let instance: Instance = read_json(&path.join("instance.json"))?;
    ensure!(
        instance.id == id,
        "Instance metadata does not match its directory."
    );
    Ok(instance)
}

fn uuid_directories(root: &Path) -> Result<Vec<(Uuid, PathBuf)>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if let Ok(id) = Uuid::parse_str(&entry.file_name().to_string_lossy()) {
            let path = entry.path();
            check_directory(&path)?;
            result.push((id, path));
        }
    }
    Ok(result)
}

pub fn check_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("Opening {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !linked(&metadata),
        "{} is not a regular directory.",
        path.display()
    );
    Ok(())
}

pub fn regular_file(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("Opening {}", path.display()))?;
    ensure!(
        metadata.is_file() && !linked(&metadata),
        "{} is not a regular file.",
        path.display()
    );
    Ok(())
}

pub(crate) fn linked(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.is_symlink()
}

pub fn list_files(path: &Path) -> Result<Vec<String>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
        Ok(metadata) => ensure!(
            metadata.is_dir() && !linked(&metadata),
            "{} is not a regular directory.",
            path.display()
        ),
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file() && !linked(&metadata) {
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

pub fn is_mod(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".jar") || lower.ends_with(".jar.disabled")
}

pub(crate) fn file_added_at(metadata: &Metadata) -> Option<u64> {
    metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

pub fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    check_directory(source)?;
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !linked(&metadata),
            "Cannot copy a linked file or directory: {}",
            path.display()
        );
        if metadata.is_dir() {
            copy_tree(&path, &destination)?;
        } else if metadata.is_file() {
            fs::copy(&path, destination)?;
        } else {
            bail!("Cannot copy special file: {}", path.display());
        }
    }
    Ok(())
}

pub fn check_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 96
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            && !value.ends_with('.'),
        "Invalid version or product ID: {value}"
    );
    check_filename(value)
}

pub fn check_filename(value: &str) -> Result<()> {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(*prefix).is_some_and(|digit| {
                matches!(
                    digit,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    ensure!(
        !value.is_empty()
            && value != "."
            && value != ".."
            && !reserved
            && !value.ends_with(['.', ' '])
            && !value
                .chars()
                .any(|c| c.is_control() || r#"/\:<>"|?*"#.contains(c)),
        "Unsafe filename: {value}"
    );
    Ok(())
}

pub fn check_ram(value: u32) -> Result<()> {
    ensure!(
        (512..=65536).contains(&value),
        "RAM must be between 512 and 65536 MiB."
    );
    Ok(())
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    serde_json::from_reader(io::BufReader::new(file))
        .with_context(|| format!("Reading {}", path.display()))
}

pub fn read_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(io::BufReader::new(file))
            .with_context(|| format!("Reading {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error).with_context(|| format!("Opening {}", path.display())),
    }
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let folder = path.parent().context("Missing JSON parent directory")?;
    let mut temp = NamedTempFile::new_in(folder)?;
    {
        let mut writer = BufWriter::new(&mut temp);
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(folder)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> InstanceOptions {
        InstanceOptions {
            name: "Survival".into(),
            minecraft_version: "1.21.1".into(),
            loader: Loader::Fabric,
            loader_version: None,
            ram_mib: 4096,
            jvm_args: vec![],
            icon: None,
            update_channel: UpdateChannel::default(),
            launch: LaunchOverrides::default(),
        }
    }

    #[test]
    fn playtime_survives_repeated_checkpoints_and_backup_restores() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_path_buf())?;
        let instance = store.create(options())?.instance;
        let id = instance.id;
        let mut legacy = serde_json::to_value(&instance)?;
        legacy.as_object_mut().unwrap().remove("playtimeSeconds");
        legacy["lastPlayedAt"] = 100.into();
        atomic_json(&store.instance_dir(id).join("instance.json"), &legacy)?;
        assert_eq!(store.instance(id)?.last_played_at, Some(100));
        assert_eq!(store.instance(id)?.playtime_seconds, 0);

        let backup = store.snapshot(id)?;
        store.mark_played(store.instance(id)?, None, 200)?;
        store.save_playtime(id, 200, 60)?;
        store.save_playtime(id, 200, 125)?;
        store.save_playtime(id, 200, 60)?;
        assert_eq!(store.instance(id)?.playtime_seconds, 125);

        let second_session = store.instance(id)?;
        let first_total = second_session.playtime_seconds;
        store.mark_played(second_session, None, 400)?;
        store.save_playtime(id, 400, first_total + 70)?;
        let restored = store.restore(backup)?.instance;
        assert_eq!(restored.playtime_seconds, 195);
        assert_eq!(restored.last_played_at, Some(400));

        let copy = store.duplicate(id)?.instance;
        assert_eq!(copy.playtime_seconds, 0);
        assert_eq!(copy.last_played_at, None);
        Ok(())
    }

    #[test]
    fn backups_preserve_worlds_across_duplicate_archive_and_restore() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let id = store.create(options())?.instance.id;
        let saves = store.game_dir(id)?.join("saves");
        fs::create_dir(&saves)?;
        fs::write(saves.join("world.dat"), b"original")?;
        let copy = store.duplicate(id)?.instance.id;
        let snapshot = store.snapshot(id)?;
        let mut changed = options();
        changed.minecraft_version = "1.21.4".into();
        store.update(id, changed)?;
        assert_eq!(store.backups()?.len(), 2);
        fs::write(saves.join("world.dat"), b"changed")?;
        store.restore(snapshot)?;
        assert_eq!(fs::read(saves.join("world.dat"))?, b"original");
        assert_eq!(
            fs::read(store.game_dir(copy)?.join("saves/world.dat"))?,
            b"original"
        );
        assert_eq!(store.instance(id)?.options.minecraft_version, "1.21.1");
        let archive = store.archive(id)?;
        assert!(store.instance(id).is_err());
        store.restore(archive)?;
        assert_eq!(fs::read(saves.join("world.dat"))?, b"original");
        Ok(())
    }

    #[test]
    fn pack_snapshot_restore_reverts_files_without_rewinding_worlds() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let id = store.create(options())?.instance.id;
        let game = store.game_dir(id)?;
        fs::create_dir_all(game.join("saves/world"))?;
        fs::create_dir_all(game.join("config"))?;
        fs::create_dir_all(game.join("mods"))?;
        fs::write(game.join("config/pack.toml"), b"old")?;
        fs::write(game.join("mods/worldgen.jar"), b"fixture")?;
        fs::write(game.join("saves/world/level.dat"), b"original")?;
        crate::snapshots::save_baseline(&store, &store.instance(id)?, &["world".into()])?;
        let snapshot = store.snapshot_content(id)?;
        fs::write(game.join("config/pack.toml"), b"new")?;
        fs::rename(
            game.join("mods/worldgen.jar"),
            game.join("mods/worldgen.jar.disabled"),
        )?;
        fs::write(game.join("saves/world/level.dat"), b"played since snapshot")?;
        assert_eq!(
            crate::snapshots::world_risk(&store, id, Some("world"))?.removed,
            ["worldgen.jar"]
        );
        assert!(store
            .snapshot_difference(id, snapshot, None)?
            .iter()
            .any(|change| change.path == "config/pack.toml"));
        store.restore(snapshot)?;
        assert_eq!(fs::read(game.join("config/pack.toml"))?, b"old");
        assert_eq!(
            fs::read(game.join("saves/world/level.dat"))?,
            b"played since snapshot"
        );
        assert!(crate::snapshots::world_risk(&store, id, Some("world"))?
            .removed
            .is_empty());
        Ok(())
    }

    #[test]
    fn rejects_traversal_and_windows_device_paths() {
        for name in [
            "../x", "..\\x", "C:out", "CON.txt", "LPT1.jar", "foo. ", "a/b",
        ] {
            assert!(check_filename(name).is_err(), "{name}");
        }
        assert!(check_filename("sodium-fabric.jar").is_ok());
    }

    #[test]
    fn settings_replace_and_corrupt_json_is_reported() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        store.save_settings(&Settings::default())?;
        store.save_settings(&Settings {
            ram_mib: 8192,
            ..Settings::default()
        })?;
        assert_eq!(store.settings()?.ram_mib, 8192);
        fs::write(temp.path().join("config/settings.json"), "{")?;
        assert!(store.settings().is_err());
        Ok(())
    }
}
