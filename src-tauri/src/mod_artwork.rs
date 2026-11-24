use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use uuid::Uuid;

use crate::{
    providers::{self, ModrinthArtwork},
    storage::{self, Store},
};

const RETRY_UNMATCHED_SECS: u64 = 86_400;
static CACHE_WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    Mods,
    ResourcePacks,
    Shaders,
}

impl Group {
    fn folder(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::ResourcePacks => "resourcepacks",
            Self::Shaders => "shaderpacks",
        }
    }

    fn accepts(self, file: &str) -> bool {
        match self {
            Self::Mods => storage::is_mod(file),
            _ => file.to_ascii_lowercase().ends_with(".zip"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Stamp {
    size: u64,
    modified: SystemTime,
}

#[derive(Deserialize, Serialize)]
struct Match {
    stamp: Stamp,
    checked_at: u64,
    artwork: Option<ModrinthArtwork>,
}

struct Candidate {
    file: String,
    stamp: Stamp,
    hash: String,
}

struct Scan {
    path: PathBuf,
    matches: HashMap<String, Match>,
    pending: Vec<Candidate>,
}

pub async fn list(
    client: &Client,
    store: Store,
    id: Uuid,
    group: Group,
    files: Vec<String>,
) -> Result<HashMap<String, ModrinthArtwork>> {
    let mut scan = tokio::task::spawn_blocking(move || scan(&store, id, group, &files)).await??;
    if !scan.pending.is_empty() {
        let hashes: HashSet<_> = scan.pending.iter().map(|item| item.hash.as_str()).collect();
        let hashes: Vec<_> = hashes.into_iter().collect();
        match providers::modrinth_artwork(client, &hashes).await {
            Ok(found) => {
                let checked_at = storage::now();
                let fresh: HashMap<_, _> = scan.pending.into_iter().map(|item| (
                        item.file, Match {
                            stamp: item.stamp,
                            checked_at,
                            artwork: found.get(&item.hash).cloned(),
                        },
                    )).collect();
                for (file, entry) in &fresh {
                    if let Some(artwork) = &entry.artwork {
                        scan.matches.insert(file.clone(), Match { stamp: entry.stamp.clone(), checked_at, artwork: Some(artwork.clone()) });
                    }
                }
                let path = scan.path;
                let result = tokio::task::spawn_blocking(move || {
                    let _guard = CACHE_WRITES.lock().expect("artwork cache lock");
                    let mut cached = read_cache(&path);
                    cached.extend(fresh);
                    storage::atomic_json(&path, &cached)
                }).await?;
                if let Err(error) = result {
                    tracing::debug!(%error, "Could not save the artwork cache");
                }
            }
            Err(error) => tracing::debug!(%error, "Could not match local mod artwork on Modrinth"),
        }
    }
    Ok(scan
        .matches
        .into_iter()
        .filter_map(|(file, entry)| Some((file, entry.artwork?)))
        .collect())
}

fn stamp(path: &Path) -> Result<Stamp> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !storage::linked(&metadata),
        "Invalid mod file."
    );
    ensure!(
        metadata.len() <= 2 * 1024 * 1024 * 1024,
        "Mod file is too large."
    );
    Ok(Stamp {
        size: metadata.len(),
        modified: metadata.modified()?,
    })
}

fn read_cache(path: &Path) -> HashMap<String, Match> {
    if storage::regular_file(path).is_ok() {
        storage::read_json(path).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

fn scan(store: &Store, id: Uuid, group: Group, files: &[String]) -> Result<Scan> {
    let folder = store.game_dir(id)?.join(group.folder());
    let path = store.instance_dir(id).join(if matches!(group, Group::Mods) {
        "modrinth-artwork.json".to_owned()
    } else {
        format!("{}-artwork.json", group.folder())
    });
    let mut cached = read_cache(&path);
    let now = storage::now();
    let mut matches = HashMap::new();
    let mut pending = Vec::new();
    storage::check_directory(&folder)?;
    for file in files.iter().take(64) {
        if !group.accepts(file) || storage::check_filename(file).is_err() {
            continue;
        }
        let jar = folder.join(file);
        let Ok(current) = stamp(&jar) else { continue };
        if let Some(entry) = cached.remove(file) {
            if entry.stamp == current
                && entry.artwork.as_ref().map_or_else(
                    || now.saturating_sub(entry.checked_at) < RETRY_UNMATCHED_SECS,
                    |artwork| !artwork.version.is_empty(),
                )
            {
                matches.insert(file.clone(), entry);
                continue;
            }
        }
        let mut digest = Sha512::new();
        if fs::File::open(&jar)
            .and_then(|mut jar| io::copy(&mut jar, &mut digest))
            .is_ok_and(|size| size == current.size)
            && stamp(&jar).is_ok_and(|checked| checked == current)
        {
            pending.push(Candidate {
                file: file.clone(),
                stamp: current,
                hash: hex::encode(digest.finalize()),
            });
        }
    }
    Ok(Scan {
        path,
        matches,
        pending,
    })
}

pub fn file_hash(path: &Path) -> Result<String> {
    let start = stamp(path)?;
    let mut digest = Sha512::new();
    let size = io::copy(&mut fs::File::open(path)?, &mut digest)?;
    ensure!(size == start.size && stamp(path)? == start, "The mod changed during lookup. Try again.");
    Ok(hex::encode(digest.finalize()))
}

pub fn embedded_icons(store: &Store, id: Uuid, group: Group, files: Vec<String>) -> Result<HashMap<String, String>> {
    let folder = store.game_dir(id)?.join(group.folder());
    storage::check_directory(&folder)?;
    Ok(files.into_iter().take(64).filter_map(|name| {
        storage::check_filename(&name).ok()?;
        let path = folder.join(&name);
        let mut bytes = Vec::new();
        if !matches!(group, Group::Mods) && storage::check_directory(&path).is_ok() {
            let icon = path.join("pack.png");
            storage::regular_file(&icon).ok()?;
            fs::File::open(icon).ok()?.take(262_145).read_to_end(&mut bytes).ok()?;
        } else {
            if !group.accepts(&name) { return None; }
            stamp(&path).ok()?;
            let mut zip = zip::ZipArchive::new(fs::File::open(path).ok()?).ok()?;
            let choices = match group {
                Group::Mods => {
                    let mut options = Vec::new();
                    for metadata in ["fabric.mod.json", "quilt.mod.json"] {
                        if let Ok(mut file) = zip.by_name(metadata) {
                            if file.size() > 256 * 1024 { continue; }
                            let Ok(json) = serde_json::from_reader::<_, serde_json::Value>(&mut file) else { continue };
                            let icon = if metadata == "fabric.mod.json" { &json["icon"] } else { &json["quilt_loader"]["metadata"]["icon"] };
                            if let Some(path) = icon.as_str().or_else(|| icon.as_object()?.values().find_map(|value| value.as_str())) {
                                options.push(path.to_owned());
                            }
                        }
                    }
                    for metadata in ["META-INF/mods.toml", "META-INF/neoforge.mods.toml"] {
                        if let Ok(file) = zip.by_name(metadata) {
                            let mut text = String::new();
                            if file.take(128 * 1024).read_to_string(&mut text).is_ok() {
                                options.extend(text.lines().filter_map(|line| {
                                    let (key, value) = line.split_once('=')?;
                                    (key.trim() == "logoFile").then(|| value.trim().trim_matches(['"', '\'']).to_owned())
                                }));
                            }
                        }
                    }
                    options.extend(["icon.png".to_owned(), "logo.png".to_owned()]);
                    options
                }
                _ => ["pack.png", "icon.png", "preview.png", "shaders/pack.png", "shaders/icon.png"].map(str::to_owned).to_vec(),
            };
            for choice in choices {
                if let Ok(file) = zip.by_name(&choice) {
                    if file.is_file() && file.size() <= 256 * 1024 {
                        bytes.clear();
                        if file.take(262_145).read_to_end(&mut bytes).is_ok() && valid_icon(&bytes) { break; }
                    }
                }
            }
        }
        valid_icon(&bytes).then(|| (name, format!("data:image/png;base64,{}", STANDARD.encode(bytes))))
    }).collect())
}

fn valid_icon(bytes: &[u8]) -> bool {
    bytes.len() <= 256 * 1024
        && png::Decoder::new(io::Cursor::new(bytes)).read_info().is_ok_and(|reader| {
            let info = reader.info();
            info.width <= 2048 && info.height <= 2048
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_embedded_mod_and_pack_icons_without_a_catalog() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let id = store.create(serde_json::from_value(serde_json::json!({
            "name": "Local artwork", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096,
        }))?)?.instance.id;
        let image = include_bytes!("../../public/skins/rift.png");
        for (group, filename, icon) in [
            (Group::Mods, "my-mod.jar", "assets/example/icon.png"),
            (Group::ResourcePacks, "my-resource.zip", "pack.png"),
            (Group::Shaders, "my-shader.zip", "preview.png"),
        ] {
            let folder = store.game_dir(id)?.join(group.folder());
            fs::create_dir_all(&folder)?;
            let mut zip = zip::ZipWriter::new(fs::File::create(folder.join(filename))?);
            let options = zip::write::SimpleFileOptions::default();
            if matches!(group, Group::Mods) {
                zip.start_file("fabric.mod.json", options)?;
                zip.write_all(br#"{"icon":"assets/example/icon.png"}"#)?;
            }
            zip.start_file(icon, options)?;
            zip.write_all(image)?;
            zip.finish()?;
            let icons = embedded_icons(&store, id, group, vec![filename.to_owned()])?;
            assert_eq!(icons[filename], format!("data:image/png;base64,{}", STANDARD.encode(image)));
        }
        Ok(())
    }

    #[test]
    fn matches_existing_mod_files_by_contents_and_invalidates_a_changed_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let id = store.create(serde_json::from_value(serde_json::json!({
            "name": "Imported pack", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096,
        }))?)?.instance.id;
        let folder = store.game_dir(id)?.join("mods");
        fs::create_dir_all(&folder)?;
        let file = "renamed.jar.disabled";
        fs::write(folder.join(file), b"original mod contents")?;
        fs::write(folder.join("notes.txt"), b"ignore this")?;

        let files = vec![file.to_owned(), "notes.txt".into()];
        let initial = scan(&store, id, Group::Mods, &files)?;
        assert_eq!(initial.pending.len(), 1);
        let item = initial.pending.into_iter().next().unwrap();
        assert_eq!(item.file, file);
        assert_eq!(
            item.hash,
            hex::encode(Sha512::digest(b"original mod contents"))
        );
        let cached = HashMap::from([(
            file,
            Match {
                stamp: item.stamp,
                checked_at: storage::now(),
                artwork: Some(ModrinthArtwork {
                    project: "Fixture1".into(),
                    version: "fixture-version".into(),
                    title: "Fixture mod".into(),
                    icon_url: None,
                    display_version: Some("1.0".into()),
                }),
            },
        )]);
        storage::atomic_json(&initial.path, &cached)?;
        let reopened = scan(&store, id, Group::Mods, &files)?;
        assert!(reopened.pending.is_empty());
        assert_eq!(
            reopened.matches[file].artwork.as_ref().unwrap().title,
            "Fixture mod"
        );

        fs::write(folder.join(file), b"a replacement")?;
        let changed = scan(&store, id, Group::Mods, &files)?;
        assert!(changed.matches.is_empty());
        assert_eq!(
            changed.pending[0].hash,
            hex::encode(Sha512::digest(b"a replacement"))
        );
        assert!(crate::mods::registered(&store, id)?.is_empty());
        Ok(())
    }
}
