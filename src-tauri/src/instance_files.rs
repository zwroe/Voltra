use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, Metadata},
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use uuid::Uuid;

use crate::storage::{check_directory, check_filename, copy_tree, linked, regular_file, Store};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    directory: bool,
    size: u64,
    created_at: Option<u64>,
    modified_at: Option<u64>,
}

fn directory(store: &Store, id: Uuid, segments: &[String]) -> Result<PathBuf> {
    ensure!(segments.len() <= 64, "This path is too deep.");
    let mut path = store.game_dir(id)?;
    for segment in segments {
        check_filename(segment)?;
        path.push(segment);
        check_directory(&path)?;
    }
    Ok(path)
}

fn item(path: &Path) -> Result<Metadata> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("Opening {}", path.display()))?;
    ensure!(
        !linked(&metadata) && (metadata.is_dir() || metadata.is_file()),
        "Choose a regular file or folder within the instance."
    );
    Ok(metadata)
}

fn available(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => anyhow::bail!("A file or folder with that name already exists."),
    }
}

pub fn list(store: &Store, id: Uuid, segments: &[String]) -> Result<Vec<Entry>> {
    let mut rows = Vec::new();
    for entry in fs::read_dir(directory(store, id, segments)?)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if check_filename(&name).is_err() {
            continue;
        }
        let Ok(metadata) = item(&entry.path()) else {
            continue;
        };
        let size = if metadata.is_dir() {
            fs::read_dir(entry.path()).map_or(0, |items| {
                items.filter_map(|entry| entry.ok()).count() as u64
            })
        } else {
            metadata.len()
        };
        let seconds = |time: io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
        };
        rows.push(Entry {
            name,
            directory: metadata.is_dir(),
            size,
            created_at: seconds(metadata.created()),
            modified_at: seconds(metadata.modified()),
        });
    }
    Ok(rows)
}

pub fn open_directory(store: &Store, id: Uuid, segments: &[String]) -> Result<()> {
    open::that(directory(store, id, segments)?).context("Opening the instance folder")
}

pub fn reveal(store: &Store, id: Uuid, segments: &[String], name: &str) -> Result<()> {
    check_filename(name)?;
    let parent = directory(store, id, segments)?;
    let path = parent.join(name);
    item(&path)?;
    #[cfg(windows)]
    {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path);
        std::process::Command::new("explorer.exe")
            .arg(argument)
            .spawn()?;
    }
    #[cfg(not(windows))]
    open::that(parent)?;
    Ok(())
}

pub fn create_folder(store: &Store, id: Uuid, segments: &[String], name: &str) -> Result<()> {
    check_filename(name)?;
    let path = directory(store, id, segments)?.join(name);
    fs::create_dir(path).context("Creating the folder")
}

pub fn rename(store: &Store, id: Uuid, segments: &[String], name: &str, next: &str) -> Result<()> {
    check_filename(name)?;
    check_filename(next)?;
    let parent = directory(store, id, segments)?;
    let from = parent.join(name);
    let to = parent.join(next);
    item(&from)?;
    if name == next {
        return Ok(());
    }
    available(&to)?;
    if segments == ["mods"] && crate::storage::is_mod(name) {
        return crate::mods::rename_local(store, id, name, next);
    }
    fs::rename(from, to).context("Renaming the selected item")
}

pub fn import_file(store: &Store, id: Uuid, segments: &[String], source: &Path) -> Result<()> {
    let parent = directory(store, id, segments)?;
    regular_file(source)?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid filename.")?;
    check_filename(name)?;
    if segments == ["mods"] && name.to_ascii_lowercase().ends_with(".jar") {
        return crate::mods::add_local(store, id, source);
    }
    let mut staging = tempfile::NamedTempFile::new_in(&parent)?;
    io::copy(&mut fs::File::open(source)?, staging.as_file_mut())?;
    staging
        .persist_noclobber(parent.join(name))
        .map_err(|error| error.error)
        .context("A file or folder with that name may already exist.")?;
    Ok(())
}

pub fn remove(store: &Store, id: Uuid, segments: &[String], names: &[String]) -> Result<()> {
    let parent = directory(store, id, segments)?;
    ensure!(
        !names.is_empty() && names.len() <= 1000,
        "Select between 1 and 1000 items."
    );
    let mut unique = HashSet::new();
    for name in names {
        check_filename(name)?;
        ensure!(unique.insert(name), "Select each item only once.");
        item(&parent.join(name))?;
    }
    let removed_mods = if matches!(segments, [folder] if folder.eq_ignore_ascii_case("mods")) {
        names
            .iter()
            .filter(|name| crate::storage::is_mod(name) && parent.join(name).is_file())
            .cloned()
            .collect()
    } else if segments.is_empty() {
        match names.iter().find(|name| name.eq_ignore_ascii_case("mods")) {
            Some(folder) if parent.join(folder).is_dir() => {
                crate::storage::list_files(&parent.join(folder))?
                    .into_iter()
                    .filter(|name| crate::storage::is_mod(name))
                    .collect()
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let pack_only = if let Some(root) = segments.first() {
        crate::snapshots::GAME_DIRS.contains(&root.as_str())
    } else {
        names.iter().all(|name| {
            crate::snapshots::GAME_DIRS
                .iter()
                .chain(crate::snapshots::GAME_FILES)
                .any(|entry| name == entry)
        })
    };
    let backup = if pack_only {
        store.snapshot_content(id)?
    } else {
        store.snapshot(id)?
    };
    let staging = tempfile::tempdir_in(store.instance_dir(id))?;
    move_items(&parent, staging.path(), names)
        .with_context(|| format!("Moving selected items. The complete backup is {backup}."))?;
    let mod_registry = crate::mods::record_removed_files(store, id, backup, &removed_mods);
    if let Err(error) = mod_registry {
        move_items(staging.path(), &parent, names).with_context(|| {
            format!("Restoring selected items. The complete backup is {backup}.")
        })?;
        return Err(error);
    }
    Ok(())
}

fn move_items(from: &Path, to: &Path, names: &[String]) -> Result<()> {
    for (index, name) in names.iter().enumerate() {
        if let Err(error) = fs::rename(from.join(name), to.join(name)) {
            for previous in names[..index].iter().rev() {
                fs::rename(to.join(previous), from.join(previous))
                    .context("Rolling back the previous file move")?;
            }
            return Err(error.into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SyncItem {
    Options,
    Servers,
    ResourcePacks,
    CommandHistory,
    Hotbars,
}

impl SyncItem {
    fn name(self) -> &'static str {
        match self {
            Self::Options => "options.txt",
            Self::Servers => "servers.dat",
            Self::ResourcePacks => "resourcepacks",
            Self::CommandHistory => "command_history.txt",
            Self::Hotbars => "hotbar.nbt",
        }
    }
}

pub fn sync(store: &Store, id: Uuid, source: Uuid, items: &[SyncItem]) -> Result<usize> {
    ensure!(id != source, "Choose a different source instance.");
    let target = store.game_dir(id)?;
    let source = store.game_dir(source)?;
    let staging = tempfile::tempdir_in(store.instance_dir(id))?;
    let incoming = staging.path().join("incoming");
    let outgoing = staging.path().join("outgoing");
    fs::create_dir(&incoming)?;
    fs::create_dir(&outgoing)?;
    let mut existing = Vec::new();
    let mut copies = Vec::new();
    let selected: HashSet<_> = items.iter().copied().collect();
    for entry in selected {
        let name = entry.name();
        let path = source.join(name);
        if available(&path).is_ok() {
            continue;
        }
        let is_directory = entry == SyncItem::ResourcePacks;
        if is_directory {
            copy_tree(&path, &incoming.join(name))?;
        } else {
            regular_file(&path)?;
            fs::copy(&path, incoming.join(name))?;
        }
        let current = target.join(name);
        if available(&current).is_err() {
            ensure!(
                item(&current)?.is_dir() == is_directory,
                "Unexpected file type for {name}."
            );
            existing.push(name.to_string());
        }
        copies.push(name.to_string());
    }
    ensure!(
        !copies.is_empty(),
        "The source instance has none of the selected items."
    );
    let backup = store.snapshot(id)?;
    move_items(&target, &outgoing, &existing)
        .with_context(|| format!("Moving current files. The complete backup is {backup}."))?;
    if let Err(error) = move_items(&incoming, &target, &copies) {
        move_items(&outgoing, &target, &existing).with_context(|| {
            format!("Restoring the previous files. The complete backup is {backup}.")
        })?;
        return Err(error).context("Copying the selected instance data");
    }
    Ok(copies.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(store: &Store, name: &str) -> Uuid {
        let options = serde_json::from_value(serde_json::json!({
            "name": name, "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096
        }))
        .unwrap();
        store.create(options).unwrap().instance.id
    }

    #[test]
    fn file_operations_stay_in_the_game_directory() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf()).unwrap();
        let id = create(&store, "Test");
        let instance = store.instance(id).unwrap();
        assert!(instance.options.icon.is_none());
        assert!(!instance.options.launch.inherit_memory);
        assert!(list(&store, id, &["..".into()]).is_err());
        assert!(create_folder(&store, id, &[], "a/b").is_err());
        create_folder(&store, id, &[], "test folder").unwrap();
        rename(&store, id, &[], "test folder", "config").unwrap();
        assert_eq!(list(&store, id, &[]).unwrap()[0].name, "config");
        remove(&store, id, &[], &["config".into()]).unwrap();
        assert!(list(&store, id, &[]).unwrap().is_empty());
        let backup = store.backups().unwrap().remove(0);
        store.restore(backup.id).unwrap();
        assert!(store.game_dir(id).unwrap().join("config").is_dir());
    }

    #[test]
    fn sync_copies_existing_items_and_keeps_a_backup() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().to_path_buf()).unwrap();
        let id = create(&store, "Destination");
        let source = create(&store, "Source");
        let target_file = store.game_dir(id).unwrap().join("options.txt");
        let source_file = store.game_dir(source).unwrap().join("options.txt");
        fs::write(&target_file, "old options").unwrap();
        fs::write(&source_file, "new options").unwrap();
        assert_eq!(
            sync(&store, id, source, &[SyncItem::Servers, SyncItem::Options]).unwrap(),
            1
        );
        assert_eq!(fs::read_to_string(&target_file).unwrap(), "new options");
        let backup = store.backups().unwrap().remove(0);
        store.restore(backup.id).unwrap();
        assert_eq!(fs::read_to_string(target_file).unwrap(), "old options");
    }
}
