use anyhow::{bail, ensure, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

use super::{coordinate, Version};
use crate::{
    downloads::{self, Download, Hash, Job},
    storage::{atomic_json, check_id, read_json, Loader},
};

const FORGE: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
const NEOFORGE_1201: &str = "https://maven.neoforged.net/releases/net/neoforged/forge";

fn artifact(loader: Loader, game: &str) -> Result<(&'static str, &'static str)> {
    match loader {
        Loader::Forge => Ok((FORGE, "forge")),
        Loader::Neoforge if game == "1.20.1" => Ok((NEOFORGE_1201, "forge")),
        Loader::Neoforge => Ok((NEOFORGE, "neoforge")),
        _ => bail!("Choose Forge or NeoForge."),
    }
}

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
    check_id(game)?;
    check_id(version)?;
    if loader == Loader::Forge || (loader == Loader::Neoforge && game == "1.20.1") {
        let prefix = forge_prefix(game);
        return Ok(if version.starts_with(&prefix) {
            version.to_owned()
        } else {
            format!("{prefix}{version}")
        });
    }
    ensure!(
        loader == Loader::Neoforge,
        "This loader does not use a Forge installer."
    );
    let parts: Vec<_> = game
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<_, _>>()
        .context("Forge and NeoForge require a compatible Minecraft release.")?;
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

async fn choose(
    client: &Client,
    loader: Loader,
    game: &str,
    configured: Option<&str>,
) -> Result<String> {
    if let Some(version) = configured {
        return maven_version(loader, game, version);
    }
    let compatible = versions(client, loader, game).await?;
    if loader == Loader::Forge {
        let promos = async {
            client
            .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
            .send()
            .await?
            .error_for_status()?
            .json::<Value>().await
        }
        .await;
        if let Ok(data) = promos {
            for channel in ["recommended", "latest"] {
                if let Some(promoted) = data["promos"][format!("{game}-{channel}")].as_str() {
                    if let Some(version) = compatible.iter().find(|version| {
                        version.as_str() == promoted
                            || version
                                .strip_prefix(promoted)
                                .is_some_and(|suffix| suffix.starts_with('-'))
                    }) {
                        return maven_version(loader, game, version);
                    }
                }
            }
        }
    }
    let version = compatible
        .iter()
        .find(|version| !version.contains('-'))
        .or_else(|| compatible.first())
        .with_context(|| {
            format!(
                "{} has no published build for Minecraft {game}.",
                if loader == Loader::Forge {
                    "Forge"
                } else {
                    "NeoForge"
                }
            )
        })?;
    maven_version(loader, game, version)
}

pub async fn versions(client: &Client, loader: Loader, game: &str) -> Result<Vec<String>> {
    let (base, name) = artifact(loader, game)?;
    let response = client
        .get(format!("{base}/maven-metadata.xml"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data: Metadata = quick_xml::de::from_str(&response)?;
    let prefix = forge_prefix(game);
    let compatible = data
        .versioning
        .versions
        .version
        .into_iter()
        .rev()
        .filter_map(|version| {
            if name == "forge" && !version.starts_with(&prefix) {
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
    let (base, name) = artifact(loader, game)?;
    let cache_name = if loader == Loader::Forge {
        "forge"
    } else {
        "neoforge"
    };
    let parent = cache.join("loader-installs").join(cache_name).join(game);
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
    let installer = work.path().join("installer.jar");
    let download = checked_download(client, url, installer.clone(), job)
        .await?
        .context("No download was published for this loader installer.")?;
    downloads::batch(
        client,
        vec![download],
        1,
        job,
        "Downloading loader installer",
    )
    .await?;
    let legacy_root = root.clone();
    let legacy_installer = installer.clone();
    let legacy_game = game.to_owned();
    if tokio::task::spawn_blocking(move || {
        extract_legacy_profile(&legacy_installer, &legacy_root, &legacy_game)
    })
    .await??
    {
        let installed = profile(&root, game)?
            .context("The legacy Forge installer has no compatible profile.")?;
        job.check()?;
        tokio::fs::rename(root, &directory).await?;
        return Ok((
            installed,
            display_version(loader, game, &version),
            directory.join("libraries"),
        ));
    }
    job.stage("Running the official loader installer", 0);
    let output = work.path().join("installer-output.log");
    let log = fs::File::create(&output)?;
    let mut cmd = Command::new(java);
    cmd.current_dir(work.path())
        .kill_on_drop(true)
        .args(["-Djava.awt.headless=true", "-jar"])
        .arg(&installer)
        .arg(if name == "forge" {
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
    if loader == Loader::Forge || (loader == Loader::Neoforge && game == "1.20.1") {
        version
            .strip_prefix(&forge_prefix(game))
            .unwrap_or(version)
            .to_owned()
    } else {
        version.to_owned()
    }
}

pub(super) fn is_build(loader: Loader, game: &str, version: &str, build: &str) -> bool {
    match (
        maven_version(loader, game, version),
        maven_version(loader, game, build),
    ) {
        (Ok(version), Ok(build)) => version == build,
        _ => false,
    }
}

fn forge_prefix(game: &str) -> String {
    if game == "1.7.10-pre4" {
        "1.7.10_pre4-".into()
    } else {
        format!("{game}-")
    }
}

async fn checked_download(
    client: &Client,
    url: String,
    destination: PathBuf,
    job: &Job,
) -> Result<Option<Download>> {
    let response = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        response = client.get(format!("{url}.sha1")).send() => response?,
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let checksum = response.error_for_status()?.text().await?;
    let checksum = checksum
        .split_whitespace()
        .next()
        .context("The loader checksum is empty.")?;
    ensure!(
        checksum.len() == 40 && checksum.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid loader checksum."
    );
    Ok(Some(Download {
        url,
        destination,
        hash: Hash::Sha1(checksum.to_owned()),
        size: None,
    }))
}

fn extract_legacy_profile(installer: &Path, root: &Path, game: &str) -> Result<bool> {
    let mut zip = zip::ZipArchive::new(fs::File::open(installer)?)?;
    let manifest: Value = {
        let mut file = zip.by_name("install_profile.json")?;
        ensure!(
            file.size() <= 2 * 1024 * 1024,
            "The Forge installer profile is too large."
        );
        serde_json::from_reader(&mut file)?
    };
    let Some(mut profile) = manifest.get("versionInfo").cloned() else {
        return Ok(false);
    };
    let install = &manifest["install"];
    ensure!(
        install["minecraft"].as_str() == Some(game),
        "The Forge installer targets a different Minecraft version."
    );
    let id = profile["id"]
        .as_str()
        .context("The Forge profile has no version ID.")?
        .to_owned();
    check_id(&id)?;
    ensure!(
        id != game,
        "The Forge profile must have its own version ID."
    );

    profile["inheritsFrom"] = json!(game);
    let coordinate_name = install["path"]
        .as_str()
        .context("The Forge installer is missing the library coordinate.")?;
    let library = coordinate(coordinate_name, None)?;
    let bundled_name = install["filePath"]
        .as_str()
        .context("The Forge installer is missing the bundled library name.")?;
    let destination = root.join("libraries").join(&library.path);
    fs::create_dir_all(
        destination
            .parent()
            .context("Missing Forge library directory.")?,
    )?;
    let mut bundled = zip.by_name(bundled_name.trim_start_matches('/'))?;
    ensure!(
        bundled.size() <= 128 * 1024 * 1024,
        "The bundled Forge library is too large."
    );
    let mut bytes = Vec::new();
    bundled.read_to_end(&mut bytes)?;
    fs::write(&destination, &bytes)?;
    let checksum = hex::encode(Sha1::digest(&bytes));
    let checksum_file = destination.with_file_name(format!(
        "{}.sha1",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    atomic_json(&checksum_file, &checksum)?;

    if let Some(libraries) = profile["libraries"].as_array_mut() {
        libraries.retain(|library| library["clientreq"].as_bool() != Some(false));
        for library in libraries {
            if let Some(url) = library["url"].as_str() {
                if url.starts_with("http://files.minecraftforge.net/maven")
                    || url.starts_with("https://files.minecraftforge.net/maven")
                {
                    library["url"] = json!("https://maven.minecraftforge.net/");
                } else if let Some(host) = url.strip_prefix("http://") {
                    library["url"] = json!(format!("https://{host}"));
                }
            }
        }
    }
    let directory = root.join("versions").join(&id);
    fs::create_dir_all(&directory)?;
    atomic_json(&directory.join(format!("{id}.json")), &profile)?;
    Ok(true)
}

pub fn uses_client_patch(game: &str) -> bool {
    game.split('.')
        .nth(1)
        .and_then(|part| part.parse::<u32>().ok())
        .is_some_and(|minor| game.starts_with("1.") && minor < 6)
}

pub async fn prepare_patched_client(
    client: &Client,
    cache: &Path,
    game: &str,
    configured: Option<&str>,
    client_jar: &Path,
    game_dir: &Path,
    job: &Job,
) -> Result<(String, PathBuf)> {
    job.stage("Resolving legacy Forge", 0);
    let version = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        version = choose(client, Loader::Forge, game, configured) => version?,
    };
    let parent = cache.join("loader-installs").join("forge").join(game);
    tokio::fs::create_dir_all(&parent).await?;
    let directory = parent.join(&version);
    let patched = directory.join("client.jar");
    if !patched.is_file() {
        let work = tempfile::tempdir_in(&parent)?;
        let patch = work.path().join("forge.zip");
        let mut download = None;
        for artifact in ["universal.zip", "client.zip", "universal.jar", "client.jar"] {
            let url = format!("{FORGE}/{version}/forge-{version}-{artifact}");
            download = checked_download(client, url, patch.clone(), job).await?;
            if download.is_some() {
                break;
            }
        }
        let download =
            download.context("This Forge build has no published legacy client patch.")?;
        downloads::batch(client, vec![download], 1, job, "Downloading legacy Forge").await?;
        let original = client_jar.to_owned();
        job.stage("Patching the legacy Minecraft client", 0);
        let work = tokio::task::spawn_blocking(move || -> Result<_> {
            merge_client(&original, &patch, &work.path().join("client.jar"))?;
            fs::remove_file(&patch)?;
            Ok(work)
        })
        .await??;
        job.check()?;
        tokio::fs::rename(work.path(), &directory).await?;
    }

    let libs = legacy_fml_files(game, game_dir);
    if !libs.is_empty() {
        downloads::batch(client, libs, 3, job, "Checking legacy Forge libraries").await?;
    }
    Ok((display_version(Loader::Forge, game, &version), patched))
}

fn merge_client(original: &Path, patch: &Path, output: &Path) -> Result<()> {
    let mut writer = zip::ZipWriter::new(fs::File::create(output)?);
    let mut names = HashSet::new();
    for source in [patch, original] {
        let mut input = zip::ZipArchive::new(fs::File::open(source)?)?;
        for index in 0..input.len() {
            let entry = input.by_index(index)?;
            if entry.is_file()
                && entry.enclosed_name().is_some()
                && !entry.name().to_ascii_uppercase().starts_with("META-INF/")
                && names.insert(entry.name().to_owned())
            {
                writer.raw_copy_file(entry)?;
            }
        }
    }
    writer.finish()?;
    Ok(())
}

fn legacy_fml_files(game: &str, game_dir: &Path) -> Vec<Download> {
    const EARLY: &[(&str, &str)] = &[
        ("argo-2.25.jar", "bb672829fde76cb163004752b86b0484bd0a7f4b"),
        (
            "guava-12.0.1.jar",
            "b8e78b9af7bf45900e14c6f958486b6ca682195f",
        ),
        (
            "asm-all-4.0.jar",
            "98308890597acb64047f7e896638e0d98753ae82",
        ),
    ];
    const V15: &[(&str, &str)] = &[
        (
            "argo-small-3.2.jar",
            "58912ea2858d168c50781f956fa5b59f0f7c6b51",
        ),
        (
            "guava-14.0-rc3.jar",
            "931ae21fa8014c3ce686aaa621eae565fefb1a6a",
        ),
        (
            "asm-all-4.1.jar",
            "054986e962b88d8660ae4566475658469595ef58",
        ),
        (
            "bcprov-jdk15on-148.jar",
            "960dea7c9181ba0b17e8bab0c06a43f0a5f04e65",
        ),
        (
            "scala-library.jar",
            "458d046151ad179c85429ed7420ffb1eaf6ddf85",
        ),
    ];
    let mut files = Vec::new();
    if game.starts_with("1.3") || game.starts_with("1.4") {
        files.extend_from_slice(EARLY);
        if game.starts_with("1.4") {
            files.push((
                "bcprov-jdk15on-147.jar",
                "b6f5d9926b0afbde9f4dbe3db88c5247be7794bb",
            ));
        }
    }
    let mappings = match game {
        "1.5" => Some((
            "deobfuscation_data_1.5.zip",
            "5f7c142d53776f16304c0bbe10542014abad6af8",
        )),
        "1.5.1" => Some((
            "deobfuscation_data_1.5.1.zip",
            "22e221a0d89516c1f721d6cab056a7e37471d0a6",
        )),
        "1.5.2" => Some((
            "deobfuscation_data_1.5.2.zip",
            "446e55cd986582c70fcf12cb27bc00114c5adfd9",
        )),
        _ => None,
    };
    if let Some(mappings) = mappings {
        files.extend_from_slice(V15);
        files.push(mappings);
    }
    files
        .into_iter()
        .map(|(name, hash)| Download {
            url: format!("https://files.prismlauncher.org/fmllibs/{name}"),
            destination: game_dir.join("lib").join(name),
            hash: Hash::Sha1(hash.to_owned()),
            size: None,
        })
        .collect()
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
    use std::io::Write;

    fn archive(path: &Path, files: &[(&str, &[u8])]) -> Result<()> {
        let mut zip = zip::ZipWriter::new(fs::File::create(path)?);
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
        Ok(())
    }

    #[test]
    fn resolves_loader_coordinates_for_old_and_current_versions() {
        assert_eq!(
            maven_version(Loader::Forge, "1.21.1", "52.0.6").unwrap(),
            "1.21.1-52.0.6"
        );
        assert_eq!(
            maven_version(Loader::Forge, "1.12.2", "14.23.5.2847").unwrap(),
            "1.12.2-14.23.5.2847"
        );
        assert_eq!(
            maven_version(Loader::Neoforge, "1.20.1", "47.1.106").unwrap(),
            "1.20.1-47.1.106"
        );
        assert_eq!(
            artifact(Loader::Neoforge, "1.20.1").unwrap(),
            (NEOFORGE_1201, "forge")
        );
        assert!(maven_version(Loader::Neoforge, "1.21.1", "21.1.172").is_ok());
        assert!(maven_version(Loader::Neoforge, "26.1", "26.1.0.10-beta").is_ok());
        assert!(maven_version(Loader::Neoforge, "26.1.2", "26.1.0.10-beta").is_err());
    }

    #[test]
    fn extracts_a_legacy_installer_without_running_its_ui() -> Result<()> {
        let work = tempfile::tempdir()?;
        let installer = work.path().join("installer.jar");
        let manifest = json!({
            "install": {"minecraft": "1.12.2", "path": "net.minecraftforge:forge:1.12.2-14.23.5.2846", "filePath": "universal.jar"},
            "versionInfo": {
                "id": "1.12.2-forge", "mainClass": "net.minecraft.launchwrapper.Launch",
                "minecraftArguments": "--tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker",
                "libraries": [
                    {"name":"net.minecraftforge:forge:1.12.2-14.23.5.2846", "url":"http://files.minecraftforge.net/maven/"},
                    {"name":"example:server-only:1", "clientreq": false}
                ]
            }
        });
        archive(
            &installer,
            &[
                ("install_profile.json", &serde_json::to_vec(&manifest)?),
                ("universal.jar", b"forge"),
            ],
        )?;
        let root = work.path().join("root");
        assert!(extract_legacy_profile(&installer, &root, "1.12.2")?);
        let installed = profile(&root, "1.12.2")?.unwrap();
        assert!(installed
            .minecraft_arguments
            .unwrap()
            .contains("FMLTweaker"));
        assert_eq!(installed.libraries.len(), 1);
        let library = root.join(
            "libraries/net/minecraftforge/forge/1.12.2-14.23.5.2846/forge-1.12.2-14.23.5.2846.jar",
        );
        assert_eq!(fs::read(library)?, b"forge");
        Ok(())
    }

    #[test]
    fn patches_a_legacy_client_without_replacing_the_vanilla_jar() -> Result<()> {
        let work = tempfile::tempdir()?;
        let vanilla = work.path().join("vanilla.jar");
        let patch = work.path().join("forge.zip");
        let client = work.path().join("client.jar");
        archive(
            &vanilla,
            &[
                ("Replace.class", b"old"),
                ("Keep.class", b"vanilla"),
                ("META-INF/SIGNATURE", b"signature"),
            ],
        )?;
        archive(
            &patch,
            &[("Replace.class", b"new"), ("Forge.class", b"loader")],
        )?;
        merge_client(&vanilla, &patch, &client)?;
        let mut zipped = zip::ZipArchive::new(fs::File::open(client)?)?;
        let mut patched_class = String::new();
        zipped
            .by_name("Replace.class")?
            .read_to_string(&mut patched_class)?;
        assert_eq!(patched_class, "new");
        assert!(zipped.by_name("Keep.class").is_ok());
        assert!(zipped.by_name("Forge.class").is_ok());
        assert!(zipped.by_name("META-INF/SIGNATURE").is_err());
        Ok(())
    }
}
