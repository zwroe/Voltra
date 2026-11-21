use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
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
}

pub fn list(store: &Store, id: Uuid, kind: Kind) -> Result<Vec<Entry>> {
    let folder = kind.directory(store, id)?;
    let mut entries: Vec<Entry> = list_files(&folder)?
        .into_iter()
        .filter(|name| is_zip(name))
        .map(|name| Entry {
            added_at: fs::symlink_metadata(folder.join(&name))
                .ok()
                .as_ref()
                .and_then(file_added_at),
            name,
            directory: false,
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
    check_filename(name)?;
    let folder = kind.directory(store, id)?;
    check_directory(&folder)?;
    let file = folder.join(name);
    if !is_zip(name) || regular_file(&file).is_err() {
        bail!("Only ZIP packs can be removed here. Open the pack folder to manage folders.");
    }
    store.snapshot_content(id)?;
    fs::remove_file(file).context("Removing the pack")?;
    Ok(())
}

fn is_zip(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}
