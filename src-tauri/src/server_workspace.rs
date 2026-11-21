use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{server_host, storage, storage::Store};

type Properties = BTreeMap<String, String>;

fn property_defaults(key: &str) -> Option<&'static str> {
    Some(match key {
        "server-port" => "25565",
        "motd" => "A Minecraft Server",
        "max-players" => "20",
        "difficulty" => "easy",
        "gamemode" => "survival",
        "view-distance" | "simulation-distance" => "10",
        "spawn-protection" => "16",
        "pvp" | "white-list" => "true",
        "allow-flight" => "false",
        _ => return None,
    })
}

fn validate(key: &str, value: &str) -> bool {
    let number = |min, max| {
        value
            .parse::<u32>()
            .is_ok_and(|value| (min..=max).contains(&value))
    };
    match key {
        "server-port" => number(1, 65535),
        "motd" => value.chars().count() <= 160 && !value.chars().any(char::is_control),
        "max-players" => number(1, 1000),
        "difficulty" => matches!(value, "peaceful" | "easy" | "normal" | "hard"),
        "gamemode" => matches!(value, "survival" | "creative" | "adventure" | "spectator"),
        "view-distance" | "simulation-distance" => number(3, 32),
        "spawn-protection" => number(0, 1000),
        "pvp" | "allow-flight" | "white-list" => matches!(value, "true" | "false"),
        _ => false,
    }
}

fn checked_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    storage::regular_file(path)?;
    let mut data = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= limit,
        "The file is too large to read in Voltra."
    );
    Ok(data)
}

fn property(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start_matches([' ', '\t', '\u{000C}']);
    if line.is_empty() || line.starts_with(['#', '!']) {
        return None;
    }
    let end = line
        .find(['=', ':', ' ', '\t', '\u{000C}'])
        .unwrap_or(line.len());
    let value = line[end..].trim_start_matches([' ', '\t', '\u{000C}']);
    let value = value
        .strip_prefix(['=', ':'])
        .unwrap_or(value)
        .trim_start_matches([' ', '\t', '\u{000C}']);
    Some((&line[..end], value))
}

fn logical_lines(text: &str) -> impl Iterator<Item = (&str, String)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let original = rest;
        let mut value = String::new();
        let mut continued = false;
        loop {
            let end = rest.find('\n').map_or(rest.len(), |end| end + 1);
            let line = rest[..end].trim_end_matches(['\r', '\n']);
            let line = if continued {
                line.trim_start_matches([' ', '\t', '\u{000C}'])
            } else {
                line
            };
            rest = &rest[end..];
            value.push_str(line);
            let comment = value.trim_start().starts_with(['#', '!']);
            if comment
                || rest.is_empty()
                || line.bytes().rev().take_while(|&byte| byte == b'\\').count() % 2 == 0
            {
                break;
            }
            value.pop();
            continued = true;
        }
        Some((&original[..original.len() - rest.len()], value))
    })
}

fn unescape(value: &str) -> String {
    let mut chars = value.encode_utf16();
    let mut units = Vec::new();
    while let Some(unit) = chars.next() {
        if unit != u16::from(b'\\') {
            units.push(unit);
            continue;
        }
        let Some(unit) = chars.next() else { break };
        units.push(match unit {
            117 => {
                let digits: Vec<_> = chars.by_ref().take(4).collect();
                u16::from_str_radix(&String::from_utf16_lossy(&digits), 16).unwrap_or(0xfffd)
            }
            110 => 10, // n
            114 => 13, // r
            116 => 9,  // t
            102 => 12, // f
            other => other,
        });
    }
    String::from_utf16_lossy(&units)
}

fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for (index, unit) in value.encode_utf16().enumerate() {
        match unit {
            32 if index == 0 => escaped.push_str("\\ "),
            92 => escaped.push_str("\\\\"),
            32..=126 => escaped.push(char::from(unit as u8)),
            other => escaped.push_str(&format!("\\u{other:04x}")),
        }
    }
    escaped
}

fn read_property_text(path: &Path) -> Result<String> {
    String::from_utf8(checked_file(path, 128 * 1024)?).context("Reading server.properties as UTF-8")
}

pub fn port(path: &Path) -> Option<u16> {
    let text = read_property_text(&path.join("server.properties")).ok()?;
    logical_lines(&text)
        .filter_map(|(_, line)| {
            property(&line)
                .filter(|(key, _)| *key == "server-port")
                .map(|(_, value)| value.to_owned())
        })
        .last()?
        .parse::<u16>()
        .ok()
        .filter(|&port| port > 0)
}

fn workspace(store: &Store, id: Uuid) -> Result<PathBuf> {
    store.instance(id)?;
    let path = server_host::directory(store, id);
    storage::check_directory(&store.root.join("servers"))?;
    storage::check_directory(&path).context("Start the server once to create its workspace.")?;
    Ok(path)
}

pub fn properties(store: &Store, id: Uuid) -> Result<Properties> {
    let path = workspace(store, id)?.join("server.properties");
    let text = read_property_text(&path)?;
    let mut properties = Properties::new();
    for key in [
        "server-port",
        "motd",
        "max-players",
        "difficulty",
        "gamemode",
        "view-distance",
        "simulation-distance",
        "spawn-protection",
        "pvp",
        "white-list",
        "allow-flight",
    ] {
        properties.insert(key.into(), property_defaults(key).unwrap().into());
    }
    for (_, line) in logical_lines(&text) {
        if let Some((key, value)) = property(&line) {
            if let Some(current) = properties.get_mut(key) {
                *current = unescape(value);
            }
        }
    }
    Ok(properties)
}

fn update_properties(text: &str, values: Properties) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut updated = String::new();
    for (original, line) in logical_lines(text) {
        let key = property(&line).map(|(key, _)| key);
        if key.is_some_and(|key| values.contains_key(key)) {
            continue;
        }
        updated.push_str(original);
    }
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push_str(newline);
    }
    for (key, value) in values {
        updated.push_str(&format!("{key}={}{}", escape(&value), newline));
    }
    updated
}

pub fn save_properties(store: &Store, id: Uuid, values: Properties) -> Result<()> {
    for (key, value) in &values {
        ensure!(validate(key, value), "Invalid value for {key}.");
    }
    let folder = workspace(store, id)?;
    let path = folder.join("server.properties");
    let original = read_property_text(&path)?;
    let updated = update_properties(&original, values);
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(updated.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[derive(Serialize)]
pub struct Addon {
    file: String,
    enabled: bool,
}

fn addon_file(name: &str) -> Option<bool> {
    let lowercase = name.to_ascii_lowercase();
    if lowercase.ends_with(".jar") {
        Some(true)
    } else if lowercase.ends_with(".jar.disabled") {
        Some(false)
    } else {
        None
    }
}

pub fn addons(store: &Store, id: Uuid) -> Result<Vec<Addon>> {
    let folder = workspace(store, id)?.join("mods");
    Ok(storage::list_files(&folder)?
        .into_iter()
        .filter_map(|file| {
            let enabled = addon_file(&file)?;
            Some(Addon { file, enabled })
        })
        .collect())
}

pub fn import_addon(store: &Store, id: Uuid, path: &Path) -> Result<()> {
    let folder = workspace(store, id)?.join("mods");
    storage::check_directory(&folder)?;
    storage::regular_file(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid add-on filename.")?;
    storage::check_filename(name)?;
    ensure!(addon_file(name) == Some(true), "Choose a mod JAR.");
    let source = fs::File::open(path)?;
    ensure!(
        source.metadata()?.len() <= 2 * 1024 * 1024 * 1024,
        "This add-on is too large."
    );
    let mut source = zip::ZipArchive::new(source)
        .context("Reading the add-on JAR")?
        .into_inner();
    source.rewind()?;
    ensure!(
        !folder.join(format!("{name}.disabled")).try_exists()?,
        "An add-on with this filename is already disabled."
    );
    let mut file = tempfile::NamedTempFile::new_in(&folder)?;
    std::io::copy(&mut source, &mut file)?;
    file.persist_noclobber(folder.join(name))
        .context("Saving the add-on without replacing any existing file")?;
    Ok(())
}

pub fn change_addon(store: &Store, id: Uuid, file: &str, action: AddonAction) -> Result<()> {
    storage::check_filename(file)?;
    let enabled = addon_file(file).context("Choose a server add-on.")?;
    let workspace = workspace(store, id)?;
    let folder = workspace.join("mods");
    storage::check_directory(&folder)?;
    let source = folder.join(file);
    storage::regular_file(&source)?;
    let destination = match action {
        AddonAction::Toggle if enabled => folder.join(format!("{file}.disabled")),
        AddonAction::Toggle => folder.join(&file[..file.len() - ".disabled".len()]),
        AddonAction::Remove => {
            let removed = workspace.join("removed-addons");
            if !removed.try_exists()? {
                fs::create_dir(&removed)?;
            }
            storage::check_directory(&removed)?;
            removed.join(format!("{}-{file}", Uuid::new_v4()))
        }
    };
    ensure!(
        !destination.try_exists()?,
        "A file with this name already exists in the destination."
    );
    fs::rename(source, destination).context("Moving the server add-on")
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddonAction {
    Toggle,
    Remove,
}

#[derive(Deserialize)]
struct Profile {
    name: String,
}

#[derive(Serialize)]
pub struct Players {
    whitelist: Vec<String>,
    operators: Vec<String>,
}

fn profile_names(path: &Path) -> Result<Vec<String>> {
    if !path.try_exists()? {
        return Ok(Vec::new());
    }
    let profiles: Vec<Profile> = serde_json::from_slice(&checked_file(path, 1024 * 1024)?)?;
    let mut names: Vec<_> = profiles.into_iter().map(|profile| profile.name).collect();
    names.sort_by_cached_key(|name| name.to_ascii_lowercase());
    names.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(names)
}

pub fn players(store: &Store, id: Uuid) -> Result<Players> {
    let folder = workspace(store, id)?;
    Ok(Players {
        whitelist: profile_names(&folder.join("whitelist.json"))?,
        operators: profile_names(&folder.join("ops.json"))?,
    })
}

pub fn log(store: &Store, id: Uuid, crash: bool) -> Result<(String, bool)> {
    let folder = workspace(store, id)?;
    let path = if crash {
        let reports = folder.join("crash-reports");
        let file = storage::list_files(&reports)?
            .into_iter()
            .filter(|name| name.starts_with("crash-") && name.ends_with(".txt"))
            .max()
            .context("This server has no crash report.")?;
        reports.join(file)
    } else {
        let path = folder.join("voltra-console.log");
        ensure!(path.try_exists()?, "This server has no console output yet.");
        path
    };
    crate::diagnostics::tail(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updating_properties_preserves_unrelated_lines_and_handles_java_escapes() {
        let before = "# keep\r\nmotd=first\\\r\n  line\r\nunrelated=a\\b\r\nmax-players: 12\r\nmotd=older\r\n";
        let after = update_properties(
            before,
            Properties::from([("motd".into(), " Voltra ☁ 🟪".into())]),
        );
        assert_eq!(after, "# keep\r\nunrelated=a\\b\r\nmax-players: 12\r\nmotd=\\ Voltra \\u2601 \\ud83d\\udfea\r\n");
        let (_, value) = property(after.lines().last().unwrap()).unwrap();
        assert_eq!(unescape(value), " Voltra ☁ 🟪");
        assert!(!validate("motd", "bad\nline"));
        assert!(!validate("online-mode", "false"));
        assert!(!validate("server-port", "65536"));
    }

    #[test]
    fn server_actions_apply_only_to_the_dedicated_workspace() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let id = store.create(serde_json::from_value(serde_json::json!({
            "name": "Workspace fixture", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096
        }))?)?.instance.id;
        let game = store.game_dir(id)?;
        let folder = server_host::directory(&store, id);
        fs::create_dir_all(folder.join("mods"))?;
        fs::create_dir_all(game.join("mods"))?;
        let local = game.join("mods").join("test.jar");
        let mut jar = zip::ZipWriter::new(fs::File::create(&local)?);
        jar.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())?;
        jar.write_all(br#"{"environment":"*"}"#)?;
        jar.finish()?;

        import_addon(&store, id, &local)?;
        assert!(import_addon(&store, id, &local).is_err());
        assert!(addons(&store, id)?[0].enabled);
        change_addon(&store, id, "test.jar", AddonAction::Toggle)?;
        assert!(!addons(&store, id)?[0].enabled);
        change_addon(&store, id, "test.jar.disabled", AddonAction::Remove)?;
        assert!(addons(&store, id)?.is_empty());
        assert_eq!(
            storage::list_files(&folder.join("removed-addons"))?.len(),
            1
        );
        assert!(local.is_file());

        fs::write(
            folder.join("server.properties"),
            "online-mode=true\nserver-port=25566\n",
        )?;
        save_properties(
            &store,
            id,
            Properties::from([("server-port".into(), "25568".into())]),
        )?;
        assert_eq!(port(&folder), Some(25568));
        assert_eq!(properties(&store, id)?["server-port"], "25568");
        assert!(fs::read_to_string(folder.join("server.properties"))?.contains("online-mode=true"));
        assert!(!game.join("server.properties").exists());
        fs::write(
            folder.join("whitelist.json"),
            br#"[{"name":"VoltraPlayer","uuid":"unused"}]"#,
        )?;
        assert_eq!(players(&store, id)?.whitelist, ["VoltraPlayer"]);
        Ok(())
    }
}
