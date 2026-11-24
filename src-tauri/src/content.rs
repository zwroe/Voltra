use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};
use uuid::Uuid;

use crate::storage::{
    check_directory, check_filename, file_added_at, list_files, regular_file, Store,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    ResourcePacks,
    Shaders,
}

impl Kind {
    fn folder(self) -> &'static str {
        match self {
            Self::ResourcePacks => "resourcepacks",
            Self::Shaders => "shaderpacks",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::ResourcePacks => "pack.mcmeta",
            Self::Shaders => "shaders/",
        }
    }

    fn directory(self, store: &Store, id: Uuid) -> Result<PathBuf> {
        Ok(store.game_dir(id)?.join(self.folder()))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    directory: bool,
    added_at: Option<u64>,
    icon_url: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct PackIcon {
    url: String,
    bytes: u64,
    modified: SystemTime,
}

fn stored_icons(path: &Path) -> HashMap<String, PackIcon> {
    regular_file(path).ok().and_then(|_| crate::storage::read_json(path).ok()).unwrap_or_default()
}

pub fn save_icon(store: &Store, id: Uuid, folder: &str, target: &Path, url: String) -> Result<()> {
    let name = target.file_name().and_then(|name| name.to_str()).context("Invalid pack filename.")?;
    let metadata = fs::symlink_metadata(target)?;
    let path = store.instance_dir(id).join(format!("{folder}-icons.json"));
    let mut icons = stored_icons(&path);
    icons.insert(name.to_owned(), PackIcon { bytes: metadata.len(), modified: metadata.modified()?, url });
    crate::storage::atomic_json(&path, &icons)
}

pub fn list(store: &Store, id: Uuid, kind: Kind) -> Result<Vec<Entry>> {
    let folder = kind.directory(store, id)?;
    let icons = stored_icons(&store.instance_dir(id).join(format!("{}-icons.json", kind.folder())));
    let mut entries: Vec<Entry> = list_files(&folder)?
        .into_iter()
        .filter(|name| is_zip(name))
        .map(|name| {
            let metadata = fs::symlink_metadata(folder.join(&name)).ok();
            let icon_url = icons.get(&name).and_then(|icon| {
                let metadata = metadata.as_ref()?;
                (icon.bytes == metadata.len() && metadata.modified().ok() == Some(icon.modified)).then(|| icon.url.clone())
            });
            Entry { added_at: metadata.as_ref().and_then(file_added_at), name, directory: false, icon_url }
        })
        .collect();
    if folder.exists() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && check_directory(&entry.path()).is_ok()
                && entry
                    .path()
                    .join(kind.marker().trim_end_matches('/'))
                    .exists()
            {
                if let Ok(name) = entry.file_name().into_string() {
                    entries.push(Entry {
                        name,
                        directory: true,
                        added_at: entry.metadata().ok().as_ref().and_then(file_added_at),
                        icon_url: None,
                    });
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
    Ok(entries)
}

pub fn add(store: &Store, id: Uuid, kind: Kind, source: &Path) -> Result<()> {
    regular_file(source)?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("Choose a ZIP file with a valid name.")?;
    check_filename(name)?;
    ensure!(is_zip(name), "Choose a ZIP file.");
    let input = fs::File::open(source)?;
    ensure!(
        input.metadata()?.len() <= 2 * 1024 * 1024 * 1024,
        "This pack is too large."
    );
    let archive = zip::ZipArchive::new(input).context("Reading the pack ZIP archive")?;
    let marker = kind.marker();
    ensure!(
        archive.file_names().any(|name| if marker.ends_with('/') {
            name.starts_with(marker)
        } else {
            name == marker
        }),
        "This ZIP is missing {marker} at its root. Check that you selected the right pack."
    );
    let folder = kind.directory(store, id)?;
    fs::create_dir_all(&folder)?;
    check_directory(&folder)?;
    let mut scratch = tempfile::NamedTempFile::new_in(&folder)?;
    let mut source = archive.into_inner();
    io::Seek::rewind(&mut source)?;
    io::copy(&mut source, scratch.as_file_mut())?;
    scratch.as_file().sync_all()?;
    scratch
        .persist_noclobber(folder.join(name))
        .map_err(|error| error.error)
        .context("Adding pack. A file with this name may already exist.")?;
    Ok(())
}

pub fn remove(store: &Store, id: Uuid, kind: Kind, name: &str) -> Result<()> {
    let packs = removal_paths(store, id, kind, &[name.to_owned()])?;
    crate::mods::remove_selected(store, id, &[], packs)
}

pub fn removal_paths(store: &Store, id: Uuid, kind: Kind, names: &[String]) -> Result<Vec<PathBuf>> {
    let mut names = names.to_owned();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() { return Ok(Vec::new()); }
    let folder = kind.directory(store, id)?;
    check_directory(&folder)?;
    for name in &names {
        check_filename(name)?;
        if !is_zip(name) || regular_file(&folder.join(name)).is_err() {
            bail!("Only ZIP packs can be removed here. Open the pack folder to manage folders.");
        }
    }
    Ok(names.into_iter().map(|name| folder.join(name)).collect())
}

fn is_zip(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}
