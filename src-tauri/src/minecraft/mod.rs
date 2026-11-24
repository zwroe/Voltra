use anyhow::{bail, ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tempfile::TempDir;
use tokio::process::{Child, Command};

mod forge;
pub mod java;

use crate::{
    archive::{extract_zip, safe_relative},
    auth::Session,
    downloads::{self, Download, Hash, Job, MojangFile},
    quick_play,
    storage::{
        atomic_json, check_filename, check_id, read_json, Instance, Loader, Settings, Store,
    },
};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";
const ASSETS_URL: &str = "https://resources.download.minecraft.net";

#[derive(Deserialize, Serialize)]
pub struct Catalog {
    pub latest: Latest,
    pub versions: Vec<VersionEntry>,
}

#[derive(Deserialize, Serialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Deserialize, Serialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    url: String,
    sha1: String,
}

pub async fn catalog(client: &Client, store: &Store, refresh: bool) -> Result<Catalog> {
    let path = store.root.join("cache/versions.json");
    if !refresh
        && fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(3600))
    {
        if let Ok(cached) = read_json(&path) {
            return Ok(cached);
        }
    }
    let online: Result<Catalog> = async {
        Ok(client
            .get(MANIFEST_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    .await;
    match online {
        Ok(online) => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                atomic_json(&path, &online)?;
                Ok::<_, anyhow::Error>(online)
            })
            .await?
        }
        Err(error) => read_json(&path)
            .with_context(|| format!("Cannot obtain the Minecraft version list: {error}")),
    }
}

async fn entry(client: &Client, store: &Store, id: &str) -> Result<VersionEntry> {
    let cached: Result<Catalog> = read_json(&store.root.join("cache/versions.json"));
    if let Some(version) = cached
        .ok()
        .and_then(|list| list.versions.into_iter().find(|version| version.id == id))
    {
        return Ok(version);
    }
    catalog(client, store, true)
        .await?
        .versions
        .into_iter()
        .find(|version| version.id == id)
        .with_context(|| format!("Minecraft {id} was not found in Mojang's version manifest."))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    id: String,
    #[serde(rename = "type", default)]
    kind: String,
    main_class: String,
    #[serde(default)]
    libraries: Vec<Library>,
    arguments: Option<Arguments>,
    minecraft_arguments: Option<String>,
    downloads: Option<GameDownloads>,
    asset_index: Option<AssetIndexFile>,
    java_version: Option<RequiredJava>,
    logging: Option<Logging>,
    inherits_from: Option<String>,
}

#[derive(Deserialize)]
struct GameDownloads {
    client: MojangFile,
    server: Option<MojangFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequiredJava {
    major_version: u32,
}

#[derive(Deserialize)]
struct Logging {
    client: LogConfig,
}

#[derive(Deserialize)]
struct LogConfig {
    argument: String,
    file: LogFile,
}

#[derive(Deserialize)]
struct LogFile {
    id: String,
    #[serde(flatten)]
    source: MojangFile,
}

#[derive(Deserialize)]
struct AssetIndexFile {
    id: String,
    #[serde(flatten)]
    source: MojangFile,
}

#[derive(Deserialize)]
struct AssetIndex {
    objects: HashMap<String, Asset>,
    #[serde(rename = "virtual", default)]
    virtual_assets: bool,
    #[serde(default)]
    map_to_resources: bool,
}

#[derive(Deserialize)]
struct Asset {
    hash: String,
    size: u64,
}

#[derive(Deserialize)]
struct Library {
    name: String,
    downloads: Option<LibraryDownloads>,
    #[serde(default)]
    natives: HashMap<String, String>,
    extract: Option<ExtractRules>,
    url: Option<String>,
    rules: Option<Vec<Rule>>,
}

#[derive(Deserialize)]
struct LibraryDownloads {
    artifact: Option<MojangFile>,
    #[serde(default)]
    classifiers: HashMap<String, MojangFile>,
}

#[derive(Deserialize)]
struct ExtractRules {
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize)]
struct Rule {
    action: String,
    os: Option<OsRule>,
    features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize)]
struct OsRule {
    name: Option<String>,
    arch: Option<String>,
    version: Option<String>,
}

#[derive(Default, Deserialize)]
struct Arguments {
    #[serde(default)]
    game: Vec<Argument>,
    #[serde(default)]
    jvm: Vec<Argument>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Argument {
    Plain(String),
    Rule {
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArgumentValue {
    Plain(String),
    List(Vec<String>),
}

struct Platform {
    os: &'static str,
    arch: &'static str,
    version: String,
    features: HashSet<&'static str>,
}

impl Platform {
    fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "osx",
            name => name,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "x86" => "x86",
            "aarch64" => "aarch64",
            other => other,
        };
        Self {
            os,
            arch,
            version: os_info::get().version().to_string(),
            features: HashSet::new(),
        }
    }
}

fn allowed(rules: Option<&[Rule]>, platform: &Platform) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    rules
        .iter()
        .filter(|rule| {
            rule.os.as_ref().is_none_or(|os| {
                os.name.as_deref().is_none_or(|name| name == platform.os)
                    && os.arch.as_deref().is_none_or(|arch| {
                        arch == platform.arch
                            || matches!(
                                (arch, platform.arch),
                                ("amd64" | "x64", "x86_64") | ("arm64", "aarch64")
                            )
                    })
                    && os.version.as_deref().is_none_or(|pattern| {
                        Regex::new(pattern).is_ok_and(|re| re.is_match(&platform.version))
                    })
            }) && rule.features.as_ref().is_none_or(|features| {
                features
                    .iter()
                    .all(|(name, enabled)| platform.features.contains(name.as_str()) == *enabled)
            })
        })
        .next_back()
        .is_some_and(|rule| rule.action == "allow")
}

fn args(arguments: &[Argument], platform: &Platform) -> Vec<String> {
    arguments
        .iter()
        .flat_map(|argument| match argument {
            Argument::Plain(arg) => vec![arg.clone()],
            Argument::Rule { rules, value } if allowed(Some(rules), platform) => match value {
                ArgumentValue::Plain(arg) => vec![arg.clone()],
                ArgumentValue::List(args) => args.clone(),
            },
            _ => vec![],
        })
        .collect()
}

#[derive(Deserialize)]
struct LoaderEntry {
    loader: LoaderRelease,
}

#[derive(Deserialize, Serialize)]
struct LoaderRelease {
    version: String,
    #[serde(default)]
    stable: bool,
}

pub async fn loader_versions(client: &Client, loader: Loader, game: &str) -> Result<Vec<String>> {
    check_id(game)?;
    match loader {
        Loader::Vanilla => Ok(vec![]),
        Loader::Forge | Loader::Neoforge => forge::versions(client, loader, game).await,
        Loader::Fabric | Loader::Quilt => {
            let root = loader_api(loader).expect("Fabric or Quilt");
            let response = client
                .get(format!("{root}/versions/loader/{game}"))
                .send()
                .await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(vec![]);
            }
            let entries: Vec<LoaderEntry> = response.error_for_status()?.json().await?;
            Ok(entries
                .into_iter()
                .map(|entry| entry.loader.version)
                .collect())
        }
    }
}

pub async fn validate_loader(client: &Client, loader: Loader, game: &str, version: Option<&str>) -> Result<()> {
    if loader == Loader::Vanilla {
        return Ok(());
    }
    let builds = loader_versions(client, loader, game).await
        .context("Couldn't verify loader compatibility. Try again.")?;
    ensure!(!builds.is_empty(), "{} has no published builds for Minecraft {game}.", loader.as_str());
    if let Some(version) = version {
        ensure!(builds.iter().any(|build| build == version
            || (matches!(loader, Loader::Forge | Loader::Neoforge) && forge::is_build(loader, game, version, build))),
            "{} {version} is unavailable for Minecraft {game}.", loader.as_str());
    }
    Ok(())
}

fn loader_api(loader: Loader) -> Option<&'static str> {
    match loader {
        Loader::Fabric => Some("https://meta.fabricmc.net/v2"),
        Loader::Quilt => Some("https://meta.quiltmc.org/v3"),
        _ => None,
    }
}

async fn loader_profile(
    client: &Client,
    store: &Store,
    instance: &Instance,
    job: &Job,
) -> Result<(Option<Version>, Option<String>)> {
    let game = &instance.options.minecraft_version;
    let api = match instance.options.loader {
        Loader::Vanilla => return Ok((None, None)),
        Loader::Fabric | Loader::Quilt => {
            loader_api(instance.options.loader).expect("Fabric or Quilt")
        }
        Loader::Forge | Loader::Neoforge => bail!("This loader requires the client installer."),
    };
    let root = format!("{api}/versions/loader/{game}");
    job.stage("Resolving mod loader", 0);
    let version = if let Some(version) = &instance.options.loader_version {
        version.clone()
    } else {
        let choices: Vec<LoaderEntry> = client
            .get(&root)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        choices
            .iter()
            .find(|item| item.loader.stable)
            .or_else(|| {
                choices
                    .iter()
                    .find(|item| !item.loader.version.contains('-'))
            })
            .or_else(|| choices.first())
            .map(|item| item.loader.version.clone())
            .with_context(|| {
                format!(
                    "No {} version is available for Minecraft {game}.",
                    instance.options.loader.as_str()
                )
            })?
    };
    check_id(&version)?;
    let dir = store
        .root
        .join("cache/profiles")
        .join(instance.options.loader.as_str())
        .join(game);
    let path = dir.join(format!("{version}.json"));
    let profile: Version = if let Ok(profile) = read_json(&path) {
        profile
    } else {
        let response = client
            .get(format!("{root}/{version}/profile/json"))
            .send()
            .await?;
        let status = response.status();
        ensure!(
            status.is_success(),
            "Cannot load the {} {version} profile for Minecraft {game} ({status}).",
            instance.options.loader.as_str()
        );
        let value: serde_json::Value = response.json().await?;
        let profile = serde_json::from_value(value.clone())?;
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(dir)?;
            atomic_json(&path, &value)
        })
        .await??;
        profile
    };
    ensure!(
        profile.inherits_from.as_deref() == Some(game),
        "The loader profile targets a different Minecraft version."
    );
    Ok((Some(profile), Some(version)))
}

struct Maven {
    name: String,
    path: String,
}

fn coordinate(input: &str, override_classifier: Option<&str>) -> Result<Maven> {
    let (name, extension) = input.split_once('@').unwrap_or((input, "jar"));
    let parts: Vec<_> = name.split(':').collect();
    ensure!(
        matches!(parts.len(), 3 | 4),
        "Invalid library coordinate: {input}"
    );
    let [group, artifact, version] = [parts[0], parts[1], parts[2]];
    for part in group.split('.').chain([artifact, version, extension]) {
        check_id(part)?;
    }
    let classifier = override_classifier.or_else(|| parts.get(3).copied());
    let suffix = if let Some(classifier) = classifier {
        check_id(classifier)?;
        format!("-{classifier}")
    } else {
        String::new()
    };
    Ok(Maven {
        name: format!("{group}:{artifact}:{suffix}@{extension}"),
        path: format!(
            "{}/{artifact}/{version}/{artifact}-{version}{suffix}.{extension}",
            group.replace('.', "/")
        ),
    })
}

struct LibraryFile {
    path: PathBuf,
    direct: Option<Download>,
    maven_url: Option<String>,
}

async fn resolve_file(client: &Client, file: LibraryFile) -> Result<Download> {
    if let Some(download) = file.direct {
        return Ok(download);
    }
    let url = file.maven_url.context("No URL for a required library.")?;
    let sidecar = file.path.with_file_name(format!(
        "{}.sha1",
        file.path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let cached: Result<String> = read_json(&sidecar);
    let sha1 = match cached {
        Ok(value) => value,
        Err(_) => {
            let response = client
                .get(downloads::https_url(&format!("{url}.sha1"))?)
                .send()
                .await?
                .error_for_status()
                .with_context(|| format!("No checksum was provided for {}", file.path.display()))?;
            let checksum = response
                .text()
                .await?
                .split_whitespace()
                .next()
                .context("Missing Maven SHA-1 checksum.")?
                .to_owned();
            ensure!(
                checksum.len() == 40 && checksum.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid Maven checksum."
            );
            let folder = sidecar
                .parent()
                .context("Library cache directory not available.")?
                .to_owned();
            let saved = checksum.clone();
            tokio::task::spawn_blocking(move || {
                fs::create_dir_all(folder)?;
                atomic_json(&sidecar, &saved)
            })
            .await??;
            checksum
        }
    };
    Ok(Download {
        url,
        destination: file.path,
        hash: Hash::Sha1(sha1),
        size: None,
    })
}

fn library_file(
    root: &Path,
    library: &Library,
    native: Option<&str>,
) -> Result<Option<(String, LibraryFile)>> {
    let maven = coordinate(&library.name, native)?;
    if let Some(files) = &library.downloads {
        let source = match native {
            Some(classifier) => files
                .classifiers
                .get(classifier)
                .with_context(|| format!("Missing {classifier} for {}", library.name))?,
            None => match &files.artifact {
                Some(artifact) => artifact,
                None => return Ok(None),
            },
        };
        let path = root.join(safe_relative(
            source.path.as_deref().unwrap_or(&maven.path),
        )?);
        return Ok(Some((
            maven.name,
            LibraryFile {
                direct: Some(source.at(path.clone())),
                path,
                maven_url: None,
            },
        )));
    }
    let path = root.join(safe_relative(&maven.path)?);
    let base = library
        .url
        .as_deref()
        .unwrap_or(LIBRARIES_URL)
        .trim_end_matches('/');
    Ok(Some((
        maven.name,
        LibraryFile {
            direct: None,
            path,
            maven_url: Some(format!("{base}/{}", maven.path)),
        },
    )))
}

pub struct Prepared {
    base: Version,
    profile: Option<Version>,
    game: PathBuf,
    pub loader_version: Option<String>,
    java: PathBuf,
    java_major: u32,
    classpath: Vec<PathBuf>,
    library_dir: PathBuf,
    asset_dir: PathBuf,
    asset_index: String,
    natives: TempDir,
    logging: Option<(String, PathBuf)>,
}

async fn version_metadata(client: &Client, store: &Store, id: &str) -> Result<Version> {
    check_id(id)?;
    let descriptor = store
        .root
        .join("cache/versions")
        .join(id)
        .join("version.json");
    let version: Version = match read_json(&descriptor) {
        Ok(version) => version,
        Err(_) => {
            let entry = entry(client, store, id).await?;
            let url = downloads::https_url(&entry.url)?;
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        }
    };
    ensure!(
        version.id == id,
        "Minecraft metadata does not match the selected version."
    );
    Ok(version)
}

pub async fn required_java(client: &Client, store: &Store, id: &str) -> Result<u32> {
    Ok(version_metadata(client, store, id).await?
        .java_version
        .as_ref()
        .map_or(8, |java| java.major_version))
}

pub async fn prepare_server(client: &Client, store: &Store, options: &crate::storage::InstanceOptions, directory: &Path, job: &Job) -> Result<PathBuf> {
    let game = &options.minecraft_version;
    check_id(game)?;
    let destination = directory.join("voltra-server.jar");
    if options.loader == Loader::Vanilla {
        let version = version_metadata(client, store, game).await?;
        let file = version.downloads.and_then(|files| files.server).context("Mojang does not publish a server for this version.")?;
        downloads::file(client, &file.at(destination.clone()), job).await?;
        return Ok(destination);
    }
    ensure!(options.loader == Loader::Fabric, "Local hosting currently supports Vanilla and Fabric.");
    if destination.exists() {
        crate::storage::regular_file(&destination)?;
        return Ok(destination);
    }
    let loader = options.loader_version.as_deref().context("The Fabric loader version was not resolved.")?;
    check_id(loader)?;
    let api = "https://meta.fabricmc.net/v2";
    let installers: Vec<LoaderRelease> = client.get(format!("{api}/versions/installer")).send().await?.error_for_status()?.json().await?;
    let installer = installers.into_iter().find(|release| release.stable).context("Fabric did not return a stable server installer.")?.version;
    check_id(&installer)?;
    let mut chunks = client.get(format!("{api}/versions/loader/{game}/{loader}/{installer}/server/jar")).send().await?.error_for_status()?.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        job.check()?;
        bytes.extend_from_slice(&chunk?);
        ensure!(bytes.len() <= 16 * 1024 * 1024, "The Fabric server launcher is too large.");
    }
    ensure!(zip::ZipArchive::new(std::io::Cursor::new(&bytes)).is_ok(), "Fabric did not return a valid server JAR.");
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(&bytes)?;
    temp.persist_noclobber(&destination).map_err(|error| error.error)?;
    Ok(destination)
}

pub async fn prepare(
    client: &Client,
    store: &Store,
    instance: &Instance,
    game: PathBuf,
    settings: &Settings,
    java_gate: &tokio::sync::Mutex<()>,
    loader_gate: &tokio::sync::Mutex<()>,
    job: &Job,
) -> Result<Prepared> {
    let id = &instance.options.minecraft_version;
    let version = entry(client, store, id).await?;
    let cache = store.root.join("cache");
    let version_dir = cache.join("versions").join(id);
    let descriptor = version_dir.join("version.json");
    job.stage("Checking Minecraft version", 1);
    downloads::file(
        client,
        &Download {
            url: version.url,
            destination: descriptor.clone(),
            hash: Hash::Sha1(version.sha1),
            size: None,
        },
        job,
    )
    .await?;
    let base: Version = read_json(&descriptor)?;
    ensure!(
        base.id == *id,
        "Minecraft metadata does not match the selected version."
    );
    let client_path = version_dir.join("client.jar");
    let client_download = base
        .downloads
        .as_ref()
        .context("Minecraft version has no client download.")?
        .client
        .at(client_path.clone());
    let required = base
        .java_version
        .as_ref()
        .map_or(8, |java| java.major_version);
    let java_path = instance
        .options
        .launch
        .java_path
        .as_deref()
        .unwrap_or(&settings.java_path);
    let installer = matches!(instance.options.loader, Loader::Forge | Loader::Neoforge);
    let mut java = None;
    let mut launch_client = client_path.clone();
    let shared_libraries = cache.join("libraries");
    let (profile, loader_version, library_dir) = if installer {
        downloads::batch(
            client,
            vec![client_download.clone()],
            1,
            job,
            "Checking Minecraft client",
        )
        .await?;
        let _loader_guard = tokio::select! {
            _ = job.cancel.cancelled() => bail!("Cancelled."),
            lock = loader_gate.lock() => lock,
        };
        if instance.options.loader == Loader::Forge && forge::uses_client_patch(id) {
            let (version, patched) = forge::prepare_patched_client(client, &cache, id,
                instance.options.loader_version.as_deref(), &client_path, &game, job).await?;
            launch_client = patched;
            (None, Some(version), shared_libraries.clone())
        } else {
            let java_path = resolve_java(client, store, java_path, required, java_gate, job).await?;
            let (profile, version, libraries) = forge::prepare(
                client,
                &cache,
                instance.options.loader,
                id,
                instance.options.loader_version.as_deref(),
                &descriptor,
                &client_path,
                &java_path,
                job,
            )
            .await?;
            java = Some(java_path);
            (Some(profile), Some(version), libraries)
        }
    } else {
        let (profile, version) = loader_profile(client, store, instance, job).await?;
        (profile, version, shared_libraries.clone())
    };
    let platform = Platform::current();
    let sources = profile
        .iter()
        .flat_map(|version| version.libraries.iter())
        .map(|library| (library, &library_dir))
        .chain(
            base.libraries
                .iter()
                .map(|library| (library, &shared_libraries)),
        )
        .filter(|(library, _)| allowed(library.rules.as_deref(), &platform));
    let mut classpath = Vec::new();
    let mut native_archives = Vec::new();
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for (library, root) in sources {
        if let Some((key, file)) = library_file(root, library, None)? {
            if seen.insert(key) {
                classpath.push(file.path.clone());
                files.push(file);
            }
        }
        if let Some(classifier) = library.natives.get(platform.os) {
            let width = if platform.arch == "x86" { "32" } else { "64" };
            let classifier = classifier.replace("${arch}", width);
            if let Some((key, file)) = library_file(root, library, Some(&classifier))? {
                if seen.insert(key) {
                    let mut prefixes = library
                        .extract
                        .as_ref()
                        .map(|rules| rules.exclude.clone())
                        .unwrap_or_default();
                    prefixes.push("META-INF/".to_owned());
                    prefixes.push("META-INF".to_owned());
                    native_archives.push((file.path.clone(), prefixes));
                    files.push(file);
                }
            }
        }
    }
    classpath.push(launch_client);
    job.stage("Checking library metadata", 0);
    let dependencies = stream::iter(files)
        .map(|file| resolve_file(client, file))
        .buffer_unordered(settings.download_limit)
        .try_collect::<Vec<_>>();
    let mut transfers = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        result = dependencies => result?,
    };
    if !installer {
        transfers.push(client_download);
    }
    let assets = cache.join("assets");
    let asset_config = base
        .asset_index
        .as_ref()
        .context("Minecraft version has no asset index.")?;
    check_id(&asset_config.id)?;
    let index_path = assets
        .join("indexes")
        .join(format!("{}.json", asset_config.id));
    transfers.push(asset_config.source.at(index_path.clone()));
    let logging = if let Some(logging) = &base.logging {
        check_filename(&logging.client.file.id)?;
        let path = assets.join("log_configs").join(&logging.client.file.id);
        transfers.push(logging.client.file.source.at(path.clone()));
        Some((logging.client.argument.clone(), path))
    } else {
        None
    };
    downloads::batch(
        client,
        transfers,
        settings.download_limit,
        job,
        "Checking game files and libraries",
    )
    .await?;
    let index: AssetIndex = read_json(&index_path)?;
    let mut assets_to_download = Vec::new();
    let mut hashes = HashSet::new();
    for asset in index.objects.values() {
        ensure!(
            asset.hash.len() == 40 && asset.hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid Minecraft asset hash."
        );
        if hashes.insert(&asset.hash) {
            let prefix = &asset.hash[..2];
            assets_to_download.push(Download {
                destination: assets.join("objects").join(prefix).join(&asset.hash),
                url: format!("{ASSETS_URL}/{prefix}/{}", asset.hash),
                size: Some(asset.size),
                hash: Hash::Sha1(asset.hash.clone()),
            });
        }
    }
    downloads::batch(
        client,
        assets_to_download,
        settings.download_limit,
        job,
        "Checking Minecraft assets",
    )
    .await?;
    let assets_id = asset_config.id.clone();
    let game_assets = if index.virtual_assets {
        assets.join("virtual").join(&assets_id)
    } else {
        assets.clone()
    };
    let cancel = job.cancel.clone();
    let virtual_path = game_assets.clone();
    let resources = game.join("resources");
    let natives = tokio::task::spawn_blocking(move || {
        if index.virtual_assets || index.map_to_resources {
            for (name, asset) in index.objects {
                ensure!(!cancel.is_cancelled(), "Cancelled.");
                let name = safe_relative(&name)?;
                let source = assets
                    .join("objects")
                    .join(&asset.hash[..2])
                    .join(&asset.hash);
                if index.virtual_assets {
                    copy_asset(&source, &virtual_path.join(&name))?;
                }
                if index.map_to_resources {
                    copy_asset(&source, &resources.join(&name))?;
                }
            }
        }
        let natives = tempfile::Builder::new()
            .prefix("natives-")
            .tempdir_in(cache.join("versions"))?;
        for (jar, prefixes) in native_archives {
            extract_zip(&jar, natives.path(), &prefixes, &cancel)?;
        }
        Ok::<_, anyhow::Error>(natives)
    })
    .await??;
    let java = match java {
        Some(java) => java,
        None => resolve_java(client, store, java_path, required, java_gate, job).await?,
    };
    job.check()?;
    Ok(Prepared {
        base,
        profile,
        game,
        loader_version,
        java,
        java_major: required,
        classpath,
        library_dir,
        asset_dir: game_assets,
        asset_index: assets_id,
        natives,
        logging,
    })
}

async fn resolve_java(
    client: &Client,
    store: &Store,
    configured: &str,
    required: u32,
    java_gate: &tokio::sync::Mutex<()>,
    job: &Job,
) -> Result<PathBuf> {
    let _java_guard = tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        lock = java_gate.lock() => lock,
    };
    tokio::select! {
        _ = job.cancel.cancelled() => bail!("Cancelled."),
        java = java::resolve(client, store, configured, required, job) => java,
    }
}

fn copy_asset(source: &Path, destination: &Path) -> Result<()> {
    if destination.is_file() && fs::metadata(destination)?.len() == fs::metadata(source)?.len() {
        return Ok(());
    }
    let parent = destination.parent().context("Asset path has no parent.")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut fs::File::open(source)?, temp.as_file_mut())?;
    temp.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

fn substitute(argument: &str, values: &HashMap<&str, String>) -> String {
    let regex = Regex::new(r"\$\{([a-zA-Z0-9_]+)\}").expect("constant argument pattern");
    regex
        .replace_all(argument, |captures: &regex::Captures<'_>| {
            values
                .get(&captures[1])
                .cloned()
                .unwrap_or_else(|| captures[0].to_owned())
        })
        .into_owned()
}

impl Prepared {
    pub async fn run_hook(
        &self,
        store: &Store,
        instance: &Instance,
        settings: &Settings,
        before_launch: bool,
    ) -> Result<()> {
        let hooks = instance.options.launch.hooks.with_defaults(&settings.hooks);
        let hook = if before_launch {
            hooks.pre_launch
        } else {
            hooks.post_exit
        };
        let Some(hook) = hook
            .as_deref()
            .map(str::trim)
            .filter(|hook| !hook.is_empty())
        else {
            return Ok(());
        };
        let game = store.game_dir(instance.id)?;
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            let mut shell =
                std::process::Command::new(PathBuf::from(root).join("System32/cmd.exe"));
            shell
                .args(["/D", "/S", "/C"])
                .raw_arg(format!("\"{hook}\""))
                .creation_flags(0x08000000);
            Command::from(shell)
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut shell = Command::new("/bin/sh");
            shell.args(["-c", hook]);
            shell
        };
        command
            .current_dir(&game)
            .envs(launch_environment(instance, settings))
            .env("INST_NAME", &instance.options.name)
            .env("INST_ID", instance.id.to_string())
            .env("INST_DIR", &game)
            .env("INST_MC_DIR", &game)
            .env("INST_JAVA", &self.java)
            .env(
                "INST_JAVA_ARGS",
                extra_jvm_args(instance, settings).join(" "),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = tokio::time::timeout(std::time::Duration::from_secs(120), command.status())
            .await
            .context("Launch hook timed out.")??;
        ensure!(status.success(), "Launch hook exited with {status}.");
        Ok(())
    }

    pub fn launch(
        &self,
        store: &Store,
        instance: &Instance,
        settings: &Settings,
        identity: LaunchIdentity<'_>,
        target: Option<&quick_play::Target>,
        telemetry_session: Option<uuid::Uuid>,
    ) -> Result<Child> {
        let game = &self.game;
        let window = instance
            .options
            .launch
            .window
            .with_defaults(&settings.window);
        let resolution = window.width.is_some() || window.height.is_some();
        let width = window.width.unwrap_or(854).to_string();
        let height = window.height.unwrap_or(480).to_string();
        let mut platform = Platform::current();
        if identity.is_local_test() {
            ensure!(
                target.is_none(),
                "Choose a single-player test from the title screen."
            );
            platform.features.insert("is_demo_user");
        } else {
            platform.features.insert("has_quick_plays_support");
        }
        if resolution {
            platform.features.insert("has_custom_resolution");
        }
        let connection = match target {
            Some(quick_play::Target::World(name)) => {
                platform.features.insert("is_quick_play_singleplayer");
                Some(("--quickPlaySingleplayer", "quickPlaySingleplayer", name))
            }
            Some(quick_play::Target::Server(address)) => {
                platform.features.insert("is_quick_play_multiplayer");
                Some(("--quickPlayMultiplayer", "quickPlayMultiplayer", address))
            }
            None => None,
        };
        let version = self.profile.as_ref().unwrap_or(&self.base);
        let classpath = std::env::join_paths(&self.classpath)?
            .to_string_lossy()
            .into_owned();
        let separator = if cfg!(windows) { ";" } else { ":" };
        let natives = self.natives.path().display().to_string();
        let mut values = HashMap::from([
            ("version_name", version.id.clone()),
            (
                "version_type",
                if self.base.kind.is_empty() {
                    "release".into()
                } else {
                    self.base.kind.clone()
                },
            ),
            ("game_directory", game.display().to_string()),
            (
                "assets_root",
                store.root.join("cache/assets").display().to_string(),
            ),
            ("assets_index_name", self.asset_index.clone()),
            ("game_assets", self.asset_dir.display().to_string()),
            ("user_properties", "{}".into()),
            ("user_property_map", "{}".into()),
            ("launcher_name", "Voltra".into()),
            ("launcher_version", env!("CARGO_PKG_VERSION").into()),
            ("classpath", classpath.clone()),
            ("classpath_separator", separator.into()),
            ("library_directory", self.library_dir.display().to_string()),
            ("natives_directory", natives.clone()),
            ("quickPlayPath", quick_play::GAME_LOG.to_owned()),
            ("resolution_width", width.clone()),
            ("resolution_height", height.clone()),
        ]);
        values.extend(identity.arguments());
        let mut jvm = vec![
            format!(
                "-Xmx{}M",
                if instance.options.launch.inherit_memory {
                    settings.ram_mib
                } else {
                    instance.options.ram_mib
                }
            ),
            "-Dlog4j2.formatMsgNoLookups=true".into(),
        ];
        if let Some(arguments) = &self.base.arguments {
            jvm.extend(args(&arguments.jvm, &platform));
        } else {
            jvm.extend([
                format!("-Djava.library.path={natives}"),
                "-cp".into(),
                classpath,
            ]);
        }
        if let Some(arguments) = self
            .profile
            .as_ref()
            .and_then(|profile| profile.arguments.as_ref())
        {
            jvm.extend(args(&arguments.jvm, &platform));
        }
        if let Some((argument, file)) = &self.logging {
            values.insert("path", file.display().to_string());
            jvm.push(argument.clone());
        }
        let legacy_loader_args = self.profile.as_ref().and_then(|profile| profile.minecraft_arguments.as_deref());
        let mut game_args = if let Some(arguments) = legacy_loader_args {
            arguments.split_whitespace().map(str::to_owned).collect()
        } else if let Some(arguments) = &self.base.arguments {
            args(&arguments.game, &platform)
        } else {
            self.base
                .minecraft_arguments
                .as_deref()
                .context("Minecraft version has no launch arguments.")?
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        };
        if let Some(arguments) = self
            .profile
            .as_ref()
            .and_then(|profile| profile.arguments.as_ref())
        {
            game_args.extend(args(&arguments.game, &platform));
        }
        if identity.is_local_test() {
            for flag in ["--demo", "--disableMultiplayer", "--disableChat"] {
                if !game_args.iter().any(|argument| argument == flag) {
                    game_args.push(flag.into());
                }
            }
        }
        if let Some((flag, key, name)) = connection {
            ensure!(
                game_args.iter().any(|argument| argument == flag),
                "This Minecraft version does not support Quick Play. Launch the instance and select the destination in Minecraft."
            );
            values.insert(key, name.clone());
        }
        if game_args
            .iter()
            .any(|argument| argument == "--quickPlayPath")
        {
            quick_play::check_log_file(&game.join(quick_play::GAME_LOG))?;
        }
        if resolution {
            for (flag, value) in [("--width", width), ("--height", height)] {
                if !game_args.iter().any(|arg| arg == flag) {
                    game_args.extend([flag.into(), value]);
                }
            }
        }
        let mut java_args = extra_jvm_args(instance, settings).to_vec();
        java_args.extend(jvm.iter().map(|argument| substitute(argument, &values)));
        java_args.push(version.main_class.clone());
        let hooks = instance.options.launch.hooks.with_defaults(&settings.hooks);
        let wrapper = hooks
            .wrapper
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut command = if let Some(wrapper) = wrapper {
            let path = Path::new(wrapper);
            ensure!(
                path.is_absolute() && path.is_file(),
                "Choose an existing wrapper executable using its full path."
            );
            let mut command = Command::new(path);
            command
                .args(hooks.wrapper_args.iter().flatten())
                .arg(&self.java);
            command
        } else {
            Command::new(&self.java)
        };
        if cfg!(windows) && self.java_major >= 9 {
            let argfile = self.natives.path().join("launcher-arguments.txt");
            let mut file = fs::File::create(&argfile)?;
            for argument in java_args {
                writeln!(file, "{}", java_argfile_argument(&argument))?;
            }
            drop(file);
            command.arg(format!("@{}", argfile.display()));
        } else {
            command.args(java_args);
        }
        command
            .current_dir(game)
            .envs(launch_environment(instance, settings))
            .args(
                game_args
                    .iter()
                    .map(|argument| substitute(argument, &values)),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(telemetry_session.is_none());
        if let Some(session) = telemetry_session {
            command.env("VOLTRA_TELEMETRY_SESSION", session.to_string());
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        if let Some(fullscreen) = window.fullscreen {
            write_fullscreen(&game.join("options.txt"), fullscreen)?;
        }
        command.spawn().context("Starting Minecraft with Java")
    }
}

pub enum LaunchIdentity<'a> {
    Microsoft(&'a Session),
    #[cfg(feature = "dev-test-launch")]
    LocalTest,
}

impl LaunchIdentity<'_> {
    fn is_local_test(&self) -> bool {
        match self {
            Self::Microsoft(_) => false,
            #[cfg(feature = "dev-test-launch")]
            Self::LocalTest => true,
        }
    }

    fn arguments(&self) -> [(&'static str, String); 7] {
        match self {
            Self::Microsoft(session) => [
                ("auth_player_name", session.account.name.clone()),
                ("auth_access_token", session.token.clone()),
                (
                    "auth_session",
                    format!("token:{}:{}", session.token, session.account.id.simple()),
                ),
                ("auth_uuid", session.account.id.simple().to_string()),
                ("auth_xuid", session.xuid.clone()),
                ("clientid", session.account.client_id().to_owned()),
                ("user_type", "msa".into()),
            ],
            #[cfg(feature = "dev-test-launch")]
            Self::LocalTest => [
                ("auth_player_name", "VoltraTest".into()),
                ("auth_access_token", "0".into()),
                ("auth_session", "0".into()),
                ("auth_uuid", uuid::Uuid::new_v4().simple().to_string()),
                ("auth_xuid", String::new()),
                ("clientid", String::new()),
                ("user_type", "legacy".into()),
            ],
        }
    }
}

fn extra_jvm_args<'a>(instance: &'a Instance, settings: &'a Settings) -> &'a [String] {
    if instance.options.jvm_args.is_empty() {
        &settings.jvm_args
    } else {
        &instance.options.jvm_args
    }
}

fn launch_environment<'a>(
    instance: &'a Instance,
    settings: &'a Settings,
) -> impl Iterator<Item = (&'a str, &'a str)> {
    instance
        .options
        .launch
        .environment
        .as_deref()
        .unwrap_or(&settings.environment)
        .iter()
        .map(|item| (item.name.as_str(), item.value.as_str()))
}

fn write_fullscreen(path: &Path, fullscreen: bool) -> Result<()> {
    use std::io::Read;
    let mut previous = String::new();
    if let Err(error) = crate::storage::regular_file(path) {
        if !matches!(fs::symlink_metadata(path), Err(cause) if cause.kind() == std::io::ErrorKind::NotFound)
        {
            return Err(error);
        }
    } else {
        fs::File::open(path)?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut previous)?;
        ensure!(
            previous.len() <= 1024 * 1024,
            "Minecraft options.txt is too large to edit."
        );
    }
    let line_end = if previous.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let desired = format!("fullscreen:{fullscreen}");
    let mut result = String::with_capacity(previous.len() + desired.len() + 2);
    let mut replaced = false;
    for line in previous.split_inclusive('\n') {
        if line.starts_with("fullscreen:") {
            if !replaced {
                result.push_str(&desired);
                result.push_str(line_end);
                replaced = true;
            }
        } else {
            result.push_str(line);
        }
    }
    if !replaced {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(line_end);
        }
        result.push_str(&desired);
        result.push_str(line_end);
    }
    if result != previous {
        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().context("Missing game directory.")?)?;
        temp.write_all(result.as_bytes())?;
        temp.persist(path).map_err(|error| error.error)?;
    }
    Ok(())
}

fn java_argfile_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    for character in argument.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_maven_classifiers_and_rejects_path_components() {
        let item = coordinate("org.lwjgl:lwjgl:3.3.3:natives-windows", None).unwrap();
        assert_eq!(
            item.path,
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );
        assert!(coordinate("evil/..:name:1.0", None).is_err());
    }

    #[test]
    fn escapes_windows_paths_for_java_argument_files() {
        assert_eq!(
            java_argfile_argument(r#"C:\Test User\a"b"#),
            r#""C:\\Test User\\a\"b""#
        );
    }

    #[test]
    fn mojang_rules_use_last_match_and_feature_defaults() {
        let rules: Vec<Rule> = serde_json::from_str(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"windows","arch":"x86"}},{"action":"disallow","features":{"is_demo_user":true}}]"#).unwrap();
        let windows = Platform {
            os: "windows",
            arch: "x86_64",
            version: "10.0".into(),
            features: HashSet::new(),
        };
        let x86 = Platform {
            arch: "x86",
            ..windows
        };
        assert!(!allowed(Some(&rules), &x86));
        let x64 = Platform {
            arch: "x86_64",
            ..x86
        };
        assert!(allowed(Some(&rules), &x64));
    }

    #[test]
    fn quick_play_includes_the_log_and_only_the_selected_destination_flag() {
        let manifest: Vec<Argument> = serde_json::from_str(r#"[
          {"rules":[{"action":"allow","features":{"has_quick_plays_support":true}}],"value":["--quickPlayPath","${quickPlayPath}"]},
          {"rules":[{"action":"allow","features":{"is_quick_play_singleplayer":true}}],"value":["--quickPlaySingleplayer","${quickPlaySingleplayer}"]},
          {"rules":[{"action":"allow","features":{"is_quick_play_multiplayer":true}}],"value":["--quickPlayMultiplayer","${quickPlayMultiplayer}"]}
        ]"#).unwrap();
        let mut platform = Platform::current();
        platform.features.insert("has_quick_plays_support");
        assert_eq!(
            args(&manifest, &platform),
            ["--quickPlayPath", "${quickPlayPath}"]
        );
        platform.features.insert("is_quick_play_multiplayer");
        assert_eq!(
            args(&manifest, &platform),
            [
                "--quickPlayPath",
                "${quickPlayPath}",
                "--quickPlayMultiplayer",
                "${quickPlayMultiplayer}"
            ]
        );
    }

    #[test]
    fn treats_paths_with_spaces_as_one_argument() {
        let values = HashMap::from([("game_directory", "C:\\Users\\Test User\\Games".into())]);
        assert_eq!(
            substitute("${game_directory}", &values),
            "C:\\Users\\Test User\\Games"
        );
    }
}
