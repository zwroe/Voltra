use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{ensure, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use uuid::Uuid;

use crate::{
    mods,
    providers::{self, ModrinthArtwork},
    storage::{self, Store},
};

const RETRY_UNMATCHED_SECS: u64 = 86_400;

#[derive(PartialEq, Eq, Deserialize, Serialize)]
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
    changed: bool,
}

pub async fn list(
    client: &Client,
    store: Store,
    id: Uuid,
) -> Result<HashMap<String, ModrinthArtwork>> {
    let mut scan = tokio::task::spawn_blocking(move || scan(&store, id)).await??;
    if !scan.pending.is_empty() {
        let hashes: HashSet<_> = scan.pending.iter().map(|item| item.hash.as_str()).collect();
        let hashes: Vec<_> = hashes.into_iter().collect();
        match providers::modrinth_artwork(client, &hashes).await {
            Ok(found) => {
                let checked_at = storage::now();
                for item in scan.pending {
                    scan.matches.insert(
                        item.file,
                        Match {
                            stamp: item.stamp,
                            checked_at,
                            artwork: found.get(&item.hash).cloned(),
                        },
                    );
                }
                scan.changed = true;
            }
            Err(error) => tracing::debug!(%error, "Could not match local mod artwork on Modrinth"),
        }
    }
    let artwork = scan
        .matches
        .iter()
        .filter_map(|(file, entry)| Some((file.clone(), entry.artwork.clone()?)))
        .collect();
    if scan.changed {
        let result =
            tokio::task::spawn_blocking(move || storage::atomic_json(&scan.path, &scan.matches))
                .await?;
        if let Err(error) = result {
            tracing::debug!(%error, "Could not save the mod artwork cache");
        }
    }
    Ok(artwork)
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

fn scan(store: &Store, id: Uuid) -> Result<Scan> {
    let folder = store.game_dir(id)?.join("mods");
    let path = store.instance_dir(id).join("modrinth-artwork.json");
    let mut cached: HashMap<String, Match> = if storage::regular_file(&path).is_ok() {
        storage::read_json(&path).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let managed: HashSet<_> = mods::registered(store, id)?
        .into_iter()
        .map(|entry| entry.file)
        .collect();
    let now = storage::now();
    let mut matches = HashMap::new();
    let mut pending = Vec::new();
    for file in storage::list_files(&folder)? {
        if !storage::is_mod(&file) || managed.contains(&file) {
            continue;
        }
        let jar = folder.join(&file);
        let Ok(current) = stamp(&jar) else { continue };
        if let Some(entry) = cached.remove(&file) {
            if entry.stamp == current
                && (entry.artwork.is_some()
                    || now.saturating_sub(entry.checked_at) < RETRY_UNMATCHED_SECS)
            {
                matches.insert(file, entry);
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
                file,
                stamp: current,
                hash: hex::encode(digest.finalize()),
            });
        }
    }
    Ok(Scan {
        path,
        matches,
        pending,
        changed: !cached.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let initial = scan(&store, id)?;
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
                    title: "Fixture mod".into(),
                    icon_url: None,
                    display_version: Some("1.0".into()),
                }),
            },
        )]);
        storage::atomic_json(&initial.path, &cached)?;
        let reopened = scan(&store, id)?;
        assert!(reopened.pending.is_empty());
        assert_eq!(
            reopened.matches[file].artwork.as_ref().unwrap().title,
            "Fixture mod"
        );

        fs::write(folder.join(file), b"a replacement")?;
        let changed = scan(&store, id)?;
        assert!(changed.matches.is_empty());
        assert_eq!(
            changed.pending[0].hash,
            hex::encode(Sha512::digest(b"a replacement"))
        );
        assert!(mods::registered(&store, id)?.is_empty());
        Ok(())
    }
}
