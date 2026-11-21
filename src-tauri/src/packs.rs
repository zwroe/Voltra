use anyhow::{ensure, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

use crate::{
    archive::safe_relative,
    downloads::{self, Download, Hash, Job},
    mods,
    providers::Platform,
    storage::{check_directory, check_filename, InstanceOptions, Loader, Store},
};

const MAX_EXTRACTED: u64 = 2 * 1024 * 1024 * 1024;
const ALLOWED: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "resourcepacks",
    "shaderpacks",
    "kubejs",
    "scripts",
    "datapacks",
    "global_packs",
    "openloader",
    "patchouli_books",
];
const MODRINTH_INDEX: &str = "modrinth.index.json";
const VOLTRA_INDEX: &str = "voltra-pack.json";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoltraIndex {
    format_version: u32,
    instance: InstanceOptions,
    #[serde(default)]
    modrinth_files: Vec<MrFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MrIndex {
    format_version: u32,
    game: String,
    name: String,
    dependencies: HashMap<String, String>,
    files: Vec<MrFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MrFile {
    path: String,
    hashes: HashMap<String, String>,
    downloads: Vec<String>,
    file_size: u64,
    #[serde(default)]
    env: HashMap<String, String>,
}

enum Index {
    Voltra(VoltraIndex),
    Modrinth(MrIndex),
}

pub async fn import(client: &Client, store: &Store, path: PathBuf, job: &Job) -> Result<()> {
    let work = tempfile::tempdir_in(store.root.join("cache"))?;
    let target = work.path().join("contents");
    let cancel = job.cancel.clone();
    let extracted = target.clone();
    let source_is_mrpack = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mrpack"));
    job.stage("Reading the modpack", 0);
    let index = tokio::task::spawn_blocking(move || {
        let mut zip = ZipArchive::new(fs::File::open(path)?)?;
        let index = if source_is_mrpack {
            Index::Modrinth(read_index(&mut zip, MODRINTH_INDEX)?)
        } else {
            Index::Voltra(read_index(&mut zip, VOLTRA_INDEX)?)
        };
        fs::create_dir(&extracted)?;
        let mut total = 0u64;
        for number in 0..zip.len() {
            ensure!(!cancel.is_cancelled(), "Cancelled.");
            let mut file = zip.by_index(number)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name();
            let relative = if source_is_mrpack {
                name.strip_prefix("overrides/")
                    .or_else(|| name.strip_prefix("client-overrides/"))
            } else {
                name.strip_prefix("files/")
            };
            let Some(relative) = relative else {
                continue;
            };
            let Some(relative) = allowed_path(relative)? else {
                continue;
            };
            ensure!(
                !file
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000),
                "Modpack contains a symbolic link."
            );
            total = total.saturating_add(file.size());
            ensure!(
                total <= MAX_EXTRACTED,
                "Modpack exceeds the extraction limit."
            );
            let path = extracted.join(relative);
            fs::create_dir_all(path.parent().context("Missing override folder.")?)?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .context("Modpack contains duplicate override paths.")?;
            io::copy(&mut file, &mut output)?;
        }
        Ok::<_, anyhow::Error>(index)
    })
    .await??;
    let settings = store.settings()?;
    let mut options = match index {
        Index::Voltra(index) => {
            ensure!(
                index.format_version == 1,
                "This Voltra pack format is unsupported."
            );
            download_files(
                client,
                &target,
                index.modrinth_files,
                settings.download_limit,
                job,
            )
            .await?;
            let mut options = index.instance;
            // Launch commands and environment overrides belong to the local launcher.
            options.jvm_args.clear();
            options.launch = Default::default();
            options
        }
        Index::Modrinth(index) => {
            ensure!(
                index.format_version == 1 && index.game == "minecraft",
                "This Modrinth pack format is unsupported."
            );
            download_files(client, &target, index.files, settings.download_limit, job).await?;
            let mut loaders = [
                ("fabric-loader", Loader::Fabric),
                ("quilt-loader", Loader::Quilt),
                ("forge", Loader::Forge),
                ("neoforge", Loader::Neoforge),
            ]
            .into_iter()
            .filter_map(|(key, loader)| index.dependencies.get(key).cloned().map(|v| (loader, v)));
            let (loader, version) = match loaders.next() {
                Some((loader, version)) => (loader, Some(version)),
                None => (Loader::Vanilla, None),
            };
            ensure!(
                loaders.next().is_none(),
                "The modpack declares multiple mod loaders."
            );
            InstanceOptions {
                name: index.name,
                minecraft_version: index
                    .dependencies
                    .get("minecraft")
                    .cloned()
                    .context("Modpack omits its Minecraft version.")?,
                loader,
                loader_version: version,
                ram_mib: settings.ram_mib,
                jvm_args: vec![],
                launch: Default::default(),
                update_channel: Default::default(),
                icon: None,
            }
        }
    };
    options.validate()?;
    job.check()?;
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let _work = work;
        store.create_from(&mut options, Some(&target))?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    job.stage("Modpack imported to Library", 0);
    Ok(())
}

async fn download_files(
    client: &Client,
    target: &Path,
    files: Vec<MrFile>,
    limit: usize,
    job: &Job,
) -> Result<()> {
    let mut downloads = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for file in files {
        if matches!(
            file.env.get("client").map(String::as_str),
            Some("unsupported" | "optional")
        ) {
            continue;
        }
        let Some(path) = allowed_path(&file.path)? else {
            continue;
        };
        ensure!(
            seen.insert(file.path.to_ascii_lowercase()),
            "The modpack has duplicate download paths."
        );
        let destination = target.join(path);
        ensure!(
            !destination.exists(),
            "A pack download conflicts with an override: {}",
            file.path
        );
        total = total.saturating_add(file.file_size);
        ensure!(
            total <= MAX_EXTRACTED,
            "Modpack downloads exceed the size limit."
        );
        let url = file
            .downloads
            .into_iter()
            .find(|url| {
                downloads::https_url(url)
                    .is_ok_and(|url| url.host_str() == Some("cdn.modrinth.com"))
            })
            .with_context(|| {
                format!(
                    "{} has no Modrinth CDN download. External pack downloads are unsupported.",
                    file.path
                )
            })?;
        let hash = file
            .hashes
            .get("sha512")
            .cloned()
            .context("Modrinth file has no SHA-512 checksum.")?;
        downloads.push(Download {
            url,
            destination,
            hash: Hash::Sha512(hash),
            size: Some(file.file_size),
        });
    }
    downloads::batch(client, downloads, limit, job, "Downloading modpack files").await
}

fn read_index<T: serde::de::DeserializeOwned>(
    zip: &mut ZipArchive<fs::File>,
    name: &str,
) -> Result<T> {
    let file = zip
        .by_name(name)
        .with_context(|| format!("This archive is missing {name}."))?;
    ensure!(
        file.size() <= 8 * 1024 * 1024,
        "Modpack index is too large."
    );
    Ok(serde_json::from_reader(file.take(8 * 1024 * 1024))?)
}

fn allowed_path(path: &str) -> Result<Option<PathBuf>> {
    let relative = safe_relative(path)?;
    Ok(path
        .split_once('/')
        .filter(|(folder, _)| ALLOWED.contains(folder))
        .map(|_| relative))
}

pub fn export(store: &Store, id: Uuid, path: &Path) -> Result<()> {
    export_pack(store, id, path, false)
}

pub fn export_peer(store: &Store, id: Uuid, path: &Path) -> Result<()> {
    export_pack(store, id, path, true)
}

fn export_pack(store: &Store, id: Uuid, path: &Path, include_mods: bool) -> Result<()> {
    let instance = store.instance(id)?;
    let game = store.game_dir(id)?;
    let mut options = instance.options;
    options.jvm_args.clear();
    options.launch = Default::default();
    let installed = if include_mods { Vec::new() } else { mods::registered(store, id)? };
    let mut modrinth_files = Vec::new();
    for entry in installed {
        check_filename(&entry.file)?;
        let jar = game.join("mods").join(&entry.file);
        if entry.platform != Platform::Modrinth || !jar.is_file() {
            continue;
        }
        let (Some(url), Some(hash)) = (entry.export_url, entry.export_sha512) else {
            continue;
        };
        if !downloads::https_url(&url).is_ok_and(|url| url.host_str() == Some("cdn.modrinth.com")) {
            continue;
        }
        crate::storage::regular_file(&jar)?;
        modrinth_files.push(MrFile {
            path: format!("mods/{}", entry.file),
            hashes: HashMap::from([("sha512".into(), hash)]),
            downloads: vec![url],
            file_size: fs::metadata(jar)?.len(),
            env: HashMap::new(),
        });
    }
    let index = VoltraIndex {
        format_version: 1,
        instance: options,
        modrinth_files,
    };
    let parent = path
        .parent()
        .context("Choose a destination folder for the modpack.")?;
    let mut work = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut zip = ZipWriter::new(work.as_file_mut());
        let flags =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(VOLTRA_INDEX, flags)?;
        zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
        let mut total = 0;
        for folder in ALLOWED {
            if *folder == "mods" && !include_mods {
                continue;
            }
            let local = game.join(folder);
            if local.exists() {
                write_tree(
                    &mut zip,
                    &local,
                    &format!("files/{folder}"),
                    flags,
                    &mut total,
                )?;
            }
        }
        zip.finish()?;
    }
    work.as_file().sync_all()?;
    work.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn write_tree(
    zip: &mut ZipWriter<&mut fs::File>,
    source: &Path,
    prefix: &str,
    flags: SimpleFileOptions,
    total: &mut u64,
) -> Result<()> {
    check_directory(source)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("A modpack filename is not Unicode."))?;
        check_filename(&name)?;
        let path = entry.path();
        let name = format!("{prefix}/{name}");
        if entry.file_type()?.is_dir() {
            write_tree(zip, &path, &name, flags, total)?;
        } else {
            crate::storage::regular_file(&path)?;
            let mut file = fs::File::open(path)?;
            *total = total.saturating_add(file.metadata()?.len());
            ensure!(*total <= MAX_EXTRACTED, "Pack exceeds the size limit.");
            zip.start_file(name, flags)?;
            io::copy(&mut file, zip)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn peer_archive_restores_mods_and_configs_without_saves() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = Store::new(temp.path().join("source"))?;
        let destination = Store::new(temp.path().join("destination"))?;
        let instance = source.create(serde_json::from_value(serde_json::json!({
            "name": "Shared pack", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096,
            "jvmArgs": ["-Dhost.path=private"]
        }))?)?.instance;
        let game = source.game_dir(instance.id)?;
        for folder in ["mods", "config", "saves/private"] { fs::create_dir_all(game.join(folder))?; }
        fs::write(game.join("mods/sample.jar.disabled"), "mod")?;
        fs::write(game.join("config/settings.json"), "{}")?;
        fs::write(game.join("saves/private/level.dat"), "private")?;
        let archive = temp.path().join("shared.zip");
        export_peer(&source, instance.id, &archive)?;
        let job = crate::downloads::Jobs::default().start(None, "Import".into());
        import(&Client::new(), &destination, archive, &job).await?;
        let imported = destination.instances()?.pop().unwrap().instance;
        let game = destination.game_dir(imported.id)?;
        assert!(game.join("mods/sample.jar.disabled").is_file());
        assert!(game.join("config/settings.json").is_file());
        assert!(!game.join("saves").exists());
        assert!(imported.options.jvm_args.is_empty());
        Ok(())
    }

    #[test]
    fn world_and_authentication_paths_are_excluded() {
        assert!(allowed_path("saves/world/level.dat").unwrap().is_none());
        assert!(allowed_path("config/../../accounts.json").is_err());
        assert!(allowed_path("mods/sodium.jar").unwrap().is_some());
    }
}
