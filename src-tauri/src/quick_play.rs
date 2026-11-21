use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::{
    media,
    storage::{atomic_json, check_filename, read_default, regular_file, Store},
};

pub const GAME_LOG: &str = "voltra-quick-play.json";
const MAX_LOG_BYTES: u64 = 128 * 1024;
const MAX_ENTRIES_PER_INSTANCE: usize = 40;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Target {
    World(String),
    Server(String),
}

impl Target {
    pub fn validate(&self, store: &Store, instance: Uuid) -> Result<()> {
        match self {
            Self::World(name) => {
                media::world_dir(store, instance, name)
                    .context("This world is no longer available.")?;
            }
            Self::Server(address) => {
                ensure!(
                    valid_server(address),
                    "The saved server address is invalid."
                );
                ensure!(
                    history(store, instance)?
                        .iter()
                        .any(|visit| &visit.target == self),
                    "This server is no longer in the instance's play history."
                );
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct GameLog {
    #[serde(rename = "type")]
    kind: String,
    #[serde(alias = "identifier")]
    id: String,
    #[serde(default, deserialize_with = "deserialize_port")]
    port: Option<u16>,
    name: String,
    #[serde(rename = "gamemode")]
    mode: Option<String>,
}

fn deserialize_port<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<u16>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Port {
        Number(u16),
        Text(String),
    }
    Ok(
        Option::<Port>::deserialize(value)?.and_then(|port| match port {
            Port::Number(number) => Some(number),
            Port::Text(text) => text.parse().ok(),
        }),
    )
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Visit {
    target: Target,
    name: String,
    played_at: u64,
    mode: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Destination {
    pub instance_id: Uuid,
    #[serde(flatten)]
    visit: Visit,
}

fn history_path(store: &Store, id: Uuid) -> PathBuf {
    store.instance_dir(id).join("quick-play-history.json")
}

fn history(store: &Store, id: Uuid) -> Result<Vec<Visit>> {
    let file = history_path(store, id);
    if file.exists() {
        regular_file(&file)?;
    }
    read_default(&file)
}

pub fn worlds_visited_since(store: &Store, id: Uuid, since: u64) -> Result<Vec<String>> {
    Ok(history(store, id)?
        .into_iter()
        .filter_map(|visit| match visit.target {
            Target::World(world) if visit.played_at >= since => Some(world),
            _ => None,
        })
        .collect())
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn check_log_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => regular_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn existing_log_revision(store: &Store, id: Uuid) -> Result<Option<SystemTime>> {
    let path = store.game_dir(id)?.join(GAME_LOG);
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata.modified()?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn capture(store: &Store, id: Uuid, seen: &mut Option<SystemTime>) -> Result<()> {
    let path = store.game_dir(id)?.join(GAME_LOG);
    check_log_file(&path)?;
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let changed = file.metadata()?.modified()?;
    if *seen == Some(changed) {
        return Ok(());
    }
    let Ok(entries) = serde_json::from_reader::<_, Vec<GameLog>>(file.take(MAX_LOG_BYTES)) else {
        return Ok(()); // Minecraft may still be writing the log.
    };

    let mut visits = history(store, id)?;
    let mut updated = false;
    for entry in entries.into_iter().filter_map(|entry| entry.visit(changed)) {
        if visits
            .iter()
            .any(|old| old.target == entry.target && old.played_at >= entry.played_at)
        {
            continue;
        }
        visits.retain(|old| old.target != entry.target);
        visits.push(entry);
        updated = true;
    }
    if updated {
        sort_visits(&mut visits);
        visits.truncate(MAX_ENTRIES_PER_INSTANCE);
        atomic_json(&history_path(store, id), &visits)?;
    }
    *seen = Some(changed);
    Ok(())
}

impl GameLog {
    fn visit(self, time: SystemTime) -> Option<Visit> {
        let target = match self.kind.as_str() {
            "singleplayer" => {
                check_filename(&self.id).ok()?;
                Target::World(self.id.clone())
            }
            "multiplayer" => {
                let address = match self.port {
                    Some(port) if self.id.parse::<std::net::Ipv6Addr>().is_ok() => {
                        format!("[{}]:{port}", self.id)
                    }
                    Some(port) if !self.id.contains(':') => format!("{}:{port}", self.id),
                    _ => self.id.clone(),
                };
                if !valid_server(&address) {
                    return None;
                }
                Target::Server(address)
            }
            _ => return None,
        };
        let name: String = self
            .name
            .trim()
            .chars()
            .filter(|ch| !ch.is_control())
            .take(128)
            .collect();
        Some(Visit {
            target,
            name: if name.is_empty() { self.id } else { name },
            played_at: seconds(time),
            mode: self.mode.filter(|value| {
                matches!(
                    value.as_str(),
                    "survival" | "creative" | "adventure" | "spectator"
                )
            }),
        })
    }
}

fn valid_server(address: &str) -> bool {
    if address.is_empty()
        || address.len() > 320
        || address.starts_with('-')
        || address.ends_with(':')
        || address.chars().any(|ch| {
            ch.is_whitespace()
                || ch.is_control()
                || matches!(ch, '/' | '\\' | '?' | '#' | '@' | '%')
        })
    {
        return false;
    }
    address.parse::<std::net::Ipv6Addr>().is_ok()
        || reqwest::Url::parse(&format!("http://{address}"))
            .is_ok_and(|url| url.has_host() && url.port() != Some(0) && url.path() == "/")
}

fn sort_visits(visits: &mut [Visit]) {
    visits.sort_by_key(|visit| std::cmp::Reverse(visit.played_at));
}

pub fn destinations(store: &Store) -> Result<Vec<Destination>> {
    let mut result = Vec::new();
    for id in store.instance_ids()? {
        let recent = match history(store, id) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(instance = %id, %error, "Could not read Quick Play history");
                Vec::new()
            }
        };
        let mut worlds = HashMap::new();
        let names = match media::worlds(store, id) {
            Ok(names) => names,
            Err(error) => {
                tracing::warn!(instance = %id, %error, "Could not read worlds");
                Vec::new()
            }
        };
        for name in names {
            let played_at = media::world_dir(store, id, &name)
                .and_then(|dir| Ok(fs::metadata(dir.join("level.dat"))?.modified()?))
                .map(seconds)
                .unwrap_or_default();
            worlds.insert(
                name.clone(),
                Visit {
                    target: Target::World(name.clone()),
                    name,
                    played_at,
                    mode: None,
                },
            );
        }
        for mut visit in recent {
            match &visit.target {
                Target::Server(address) if valid_server(address) => result.push(Destination {
                    instance_id: id,
                    visit,
                }),
                Target::World(folder) => {
                    if let Some(world) = worlds.get_mut(folder) {
                        visit.played_at = visit.played_at.max(world.played_at);
                        *world = visit;
                    }
                }
                _ => (),
            }
        }
        result.extend(worlds.into_values().map(|visit| Destination {
            instance_id: id,
            visit,
        }));
    }
    result.sort_by(|left, right| {
        right
            .visit
            .played_at
            .cmp(&left.visit.played_at)
            .then(left.visit.name.cmp(&right.visit.name))
            .then(left.instance_id.cmp(&right.instance_id))
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InstanceOptions, Loader};

    #[test]
    fn captures_only_played_servers_for_their_original_instance_and_lists_worlds() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_owned()).unwrap();
        let options = InstanceOptions {
            name: "Pack A".into(),
            minecraft_version: "1.21.1".into(),
            loader: Loader::Fabric,
            loader_version: None,
            ram_mib: 4096,
            jvm_args: vec![],
            icon: None,
            update_channel: Default::default(),
            launch: Default::default(),
        };
        let first = store.create(options.clone()).unwrap().instance.id;
        let second = store.create(options).unwrap().instance.id;
        let game = store.game_dir(first).unwrap();
        let world = game.join("saves/Local World");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), b"test metadata").unwrap();
        fs::write(
            game.join(GAME_LOG),
            r#"[{"type":"multiplayer","id":"play.example.com:25565","name":"Friends","lastPlayedTime":"2026-11-18T13:00:00Z","gamemode":"survival"}]"#,
        ).unwrap();
        let mut seen = None;
        capture(&store, first, &mut seen).unwrap();
        capture(&store, first, &mut seen).unwrap();
        let server = Target::Server("play.example.com:25565".into());
        assert!(server.validate(&store, first).is_ok());
        assert!(server.validate(&store, second).is_err());
        let duplicate = store.duplicate(first).unwrap().instance.id;
        let mut baseline = existing_log_revision(&store, duplicate).unwrap();
        capture(&store, duplicate, &mut baseline).unwrap();
        assert!(server.validate(&store, duplicate).is_err());
        let items = destinations(&store).unwrap();
        assert_eq!(items.len(), 3);
        assert!(items
            .iter()
            .any(|item| item.instance_id == first && item.visit.target == server));
        assert!(items
            .iter()
            .any(|item| item.visit.target == Target::World("Local World".into())));
    }

    #[test]
    fn accepts_standard_server_addresses_and_rejects_other_url_components() {
        for address in ["server.example", "localhost:25565", "[::1]:25565", "::1"] {
            assert!(valid_server(address), "{address}");
        }
        for address in [
            "",
            "https://server.example",
            "user@server.example",
            "--option",
            "host/path",
            "host:0",
            "host:99999",
        ] {
            assert!(!valid_server(address), "{address}");
        }
    }
}
