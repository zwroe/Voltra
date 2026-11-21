use anyhow::{bail, ensure, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

use super::Version;
use crate::{
    downloads::{self, Download, Hash, Job},
    storage::{atomic_json, check_id, read_json, Loader},
};

const FORGE: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

#[derive(Deserialize)]
struct Metadata {
    versioning: Versioning,
}
#[derive(Deserialize)]
struct Versioning {
    versions: Versions,
}
#[derive(Deserialize)]
struct Versions {
    version: Vec<String>,
}

fn maven_version(loader: Loader, game: &str, version: &str) -> Result<String> {
    let parts: Vec<_> = game
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<_, _>>()
        .context("Forge and NeoForge require a compatible Minecraft release.")?;
    check_id(version)?;
    match loader {
        Loader::Forge => {
            ensure!(
                matches!(parts.as_slice(), [1, minor, ..] if *minor >= 13)
                    || matches!(parts.as_slice(), [year, ..] if *year >= 26),
                "Forge client installers before Minecraft 1.13 are unsupported."
            );
            Ok(if version.starts_with(&format!("{game}-")) {
                version.to_owned()
            } else {
                format!("{game}-{version}")
            })
        }
        Loader::Neoforge => {
            let series = match parts.as_slice() {
                [1, minor, rest @ ..] if *minor >= 20 => {
                    format!("{minor}.{}.", rest.first().copied().unwrap_or(0))
                }
                [year, season, rest @ ..] if *year >= 26 => {
                    format!("{year}.{season}.{}.", rest.first().copied().unwrap_or(0))
                }
                _ => bail!("NeoForge is unavailable for Minecraft {game}."),
            };
            ensure!(
                version.starts_with(&series),
                "NeoForge {version} does not match Minecraft {game}."
            );
            Ok(version.to_owned())
        }
        _ => bail!("This loader does not use a Forge installer."),
    }
}

async fn choose(
    client: &Client,
    loader: Loader,
    game: &str,
    configured: Option<&str>,
) -> Result<String> {
    if let Some(version) = configured {
        return maven_version(loader, game, version);
    }
    if loader == Loader::Forge {
        let data: serde_json::Value = client
            .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let promos = &data["promos"];
        let version = promos[format!("{game}-recommended")].as_str()
            .or_else(|| promos[format!("{game}-latest")].as_str())
            .with_context(|| format!("Forge has no recommended or latest build for Minecraft {game}. Enter an exact loader version."))?;
        return maven_version(loader, game, version);
    }
    let compatible = versions(client, loader, game).await?;
    compatible
        .iter()
        .find(|version| !version.contains('-'))
        .cloned()
        .or_else(|| compatible.into_iter().next())
        .with_context(|| format!("NeoForge has no build for Minecraft {game}."))
}

pub async fn versions(client: &Client, loader: Loader, game: &str) -> Result<Vec<String>> {
    let base = match loader {
        Loader::Forge => FORGE,
        Loader::Neoforge => NEOFORGE,
        _ => bail!("Choose Forge or NeoForge."),
    };
    let response = client
        .get(format!("{base}/maven-metadata.xml"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data: Metadata = quick_xml::de::from_str(&response)?;
    let prefix = format!("{game}-");
    let compatible = data
        .versioning
        .versions
        .version
        .into_iter()
        .rev()
        .filter_map(|version| {
            if loader == Loader::Forge && !version.starts_with(&prefix) {
                return None;
            }
            maven_version(loader, game, &version).ok()?;
            Some(display_version(loader, game, &version))
        })
        .collect();
    Ok(compatible)
}

pub async fn prepare(
    client: &Client,
    cache: &Path,
    loader: Loader,
    game: &str,
    configured: Option<&str>,
    descriptor: &Path,
    client_jar: &Path,
    java: &Path,
    job: &Job,
) -> Result<(Version, String, PathBuf)> {
    job.stage("Resolving mod loader", 0);
    let version = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        version = choose(client, loader, game, configured) => version?,
    };
    let (base, name) = if loader == Loader::Forge {
        (FORGE, "forge")
    } else {
        (NEOFORGE, "neoforge")
    };
    let parent = cache.join("loader-installs").join(name).join(game);
    tokio::fs::create_dir_all(&parent).await?;
    let directory = parent.join(&version);
    if let Some(profile) = profile(&directory, game)? {
        return Ok((
            profile,
            display_version(loader, game, &version),
            directory.join("libraries"),
        ));
    }
    let work = tempfile::tempdir_in(&parent)?;
    let root = work.path().join("install");
    let vanilla = root.join("versions").join(game);
    tokio::fs::create_dir_all(&vanilla).await?;
    tokio::fs::copy(client_jar, vanilla.join(format!("{game}.jar"))).await?;
    tokio::fs::copy(descriptor, vanilla.join(format!("{game}.json"))).await?;
    atomic_json(
        &root.join("launcher_profiles.json"),
        &json!({"profiles": {}, "settings": {}}),
    )?;
    let url = format!("{base}/{version}/{name}-{version}-installer.jar");
    let checksum = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        response = client.get(format!("{url}.sha1")).send() => response?,
    }
    .error_for_status()
    .context("Fetching the official loader installer checksum")?
    .text()
    .await?;
    let checksum = checksum
        .split_whitespace()
        .next()
        .context("No installer checksum was provided.")?;
    let installer = work.path().join("installer.jar");
    downloads::batch(
        client,
        vec![Download {
            url,
            destination: installer.clone(),
            hash: Hash::Sha1(checksum.to_owned()),
            size: None,
        }],
        1,
        job,
        "Downloading loader installer",
    )
    .await?;
    job.stage("Running the official loader installer", 0);
    let output = work.path().join("installer-output.log");
    let log = fs::File::create(&output)?;
    let mut cmd = Command::new(java);
    cmd.current_dir(work.path())
        .kill_on_drop(true)
        .args(["-Djava.awt.headless=true", "-jar"])
        .arg(&installer)
        .arg(if loader == Loader::Forge {
            "--installClient"
        } else {
            "--install-client"
        })
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut process = cmd
        .spawn()
        .context("Starting the official loader installer")?;
    let result = tokio::select! {
        _ = job.cancel.cancelled() => {
            process.kill().await?;
            bail!("Cancelled.")
        },
        status = tokio::time::timeout(Duration::from_secs(600), process.wait()) => status,
    };
    let status = match result {
        Ok(status) => status?,
        Err(_) => {
            process.kill().await?;
            bail!("The loader installer timed out. Check your internet connection and try again.");
        }
    };
    let installed = profile(&root, game);
    let installed = match (status.success(), installed) {
        (true, Ok(Some(profile))) => profile,
        _ => {
            let contents = fs::read_to_string(&output).unwrap_or_default();
            let excerpt = contents
                .lines()
                .rev()
                .filter(|line| !line.trim().is_empty())
                .take(5)
                .map(|line| line.chars().take(180).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("The official {name} client installer did not complete ({status}).\n{excerpt}");
        }
    };
    job.check()?;
    tokio::fs::rename(root, &directory).await?;
    Ok((
        installed,
        display_version(loader, game, &version),
        directory.join("libraries"),
    ))
}

fn display_version(loader: Loader, game: &str, version: &str) -> String {
    if loader == Loader::Forge {
        version
            .strip_prefix(&format!("{game}-"))
            .unwrap_or(version)
            .to_owned()
    } else {
        version.to_owned()
    }
}

fn profile(root: &Path, game: &str) -> Result<Option<Version>> {
    let versions = root.join("versions");
    if !versions.is_dir() {
        return Ok(None);
    }
    for candidate in fs::read_dir(versions)? {
        let candidate = candidate?;
        let name = candidate.file_name().to_string_lossy().into_owned();
        if name == game || !candidate.file_type()?.is_dir() {
            continue;
        }
        check_id(&name)?;
        let loaded: Version = read_json(&candidate.path().join(format!("{name}.json")))?;
        if loaded.id == name && loaded.inherits_from.as_deref() == Some(game) {
            return Ok(Some(loaded));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_loader_coordinates_for_old_and_current_versions() {
        assert_eq!(
            maven_version(Loader::Forge, "1.21.1", "52.0.6").unwrap(),
            "1.21.1-52.0.6"
        );
        assert!(maven_version(Loader::Forge, "1.12.2", "14.23.5.2847").is_err());
        assert!(maven_version(Loader::Neoforge, "1.21.1", "21.1.172").is_ok());
        assert!(maven_version(Loader::Neoforge, "26.1", "26.1.0.10-beta").is_ok());
        assert!(maven_version(Loader::Neoforge, "26.1.2", "26.1.0.10-beta").is_err());
    }
}
