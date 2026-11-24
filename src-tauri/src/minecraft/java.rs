use anyhow::{bail, ensure, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

use crate::{
    downloads::{self, Download, Hash, Job},
    storage::{check_directory, Store},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub path: PathBuf,
    pub major: u32,
}

pub async fn inspect(path: PathBuf) -> Result<JavaVersion> {
    ensure!(
        path.is_file(),
        "Java executable was not found at {}.",
        path.display()
    );
    let mut command = Command::new(&path);
    command.arg("-version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .context("Java did not respond to -version.")??;
    ensure!(
        output.status.success(),
        "Java could not report its version."
    );
    let version = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let re = regex::Regex::new(r#"(?m)^(?:openjdk|java) version "(?:1\.)?(\d+)"#)?;
    let major = re
        .captures(&version)
        .context("Java did not report a recognizable version.")?[1]
        .parse()?;
    ensure!(
        !version.contains("32-Bit"),
        "Choose a 64-bit Java installation."
    );
    let path = tokio::fs::canonicalize(path).await?;
    #[cfg(windows)]
    let path = {
        let display = path.to_string_lossy();
        display
            .strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.clone())
    };
    Ok(JavaVersion { path, major })
}

pub async fn detect(store: &Store) -> Result<Vec<JavaVersion>> {
    detect_matching(store, None).await
}

async fn detect_matching(store: &Store, required: Option<u32>) -> Result<Vec<JavaVersion>> {
    let managed = store.root.join("cache/java");
    let candidates = tokio::task::spawn_blocking(move || java_candidates(&managed)).await?;
    let mut seen = std::collections::HashSet::new();
    let results =
        futures_util::future::join_all(candidates.into_iter().take(60).map(inspect)).await;
    Ok(results
        .into_iter()
        .flatten()
        .filter(|java| {
            required.is_none_or(|major| major == java.major) && seen.insert(java.path.clone())
        })
        .collect())
}

pub async fn resolve(
    client: &Client,
    store: &Store,
    configured: &str,
    required: u32,
    job: &Job,
) -> Result<PathBuf> {
    job.stage("Checking Java", 0);
    if !configured.trim().is_empty() {
        let java = inspect(PathBuf::from(configured.trim())).await?;
        ensure!(java.major == required, "This instance requires Java {required}. Your override uses Java {}. Clear the override to install the matching version automatically.", java.major);
        return Ok(java.path);
    }
    if let Some(java) = detect_matching(store, Some(required))
        .await?
        .into_iter()
        .next()
    {
        return Ok(java.path);
    }
    install(client, store, required, job).await
}

pub async fn available(client: &Client) -> Result<Vec<u32>> {
    #[derive(Deserialize)]
    struct Releases {
        available_releases: Vec<u32>,
    }
    let releases: Releases = client
        .get("https://api.adoptium.net/v3/info/available_releases")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(releases.available_releases)
}

pub async fn install(client: &Client, store: &Store, major: u32, job: &Job) -> Result<PathBuf> {
    ensure!(
        (8..=99).contains(&major),
        "Choose an available Java release."
    );
    let (os, suffix) = match env::consts::OS {
        "windows" => ("windows", ".zip"),
        "macos" => ("mac", ".tar.gz"),
        "linux" => ("linux", ".tar.gz"),
        _ => bail!("Install Java {major} locally and select it in Settings."),
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => bail!("Automatic Java installation does not support this architecture."),
    };
    let root = store.root.join("cache/java");
    let target = root.join(format!("java-{major}-{arch}"));
    if let Some(path) = find_binary(&target) {
        if let Ok(java) = inspect(path).await {
            if java.major == major {
                return Ok(java.path);
            }
        }
    }
    job.stage("Finding a Java runtime", 0);
    let mut selected = None;
    for kind in ["jre", "jdk"] {
        let endpoint = format!("https://api.adoptium.net/v3/assets/latest/{major}/hotspot?architecture={arch}&image_type={kind}&os={os}&vendor=eclipse");
        let response = client.get(endpoint).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        let assets: Vec<Value> = response.error_for_status()?.json().await?;
        if let Some(package) =
            assets
                .iter()
                .map(|asset| &asset["binary"]["package"])
                .find(|package| {
                    package["name"]
                        .as_str()
                        .is_some_and(|name| name.ends_with(suffix))
                })
        {
            selected = Some(package.clone());
            break;
        }
    }
    let package = selected.with_context(|| format!("Adoptium has no Java {major} runtime for {os} ({arch}). Install Java locally and select it in Settings."))?;
    tokio::fs::create_dir_all(&root).await?;
    let work = tempfile::tempdir_in(&root)?;
    let archive = work.path().join(format!("runtime{suffix}"));
    let download = Download {
        url: package["link"]
            .as_str()
            .context("Java package has no URL.")?
            .into(),
        destination: archive.clone(),
        hash: Hash::Sha256(
            package["checksum"]
                .as_str()
                .context("Java package has no checksum.")?
                .into(),
        ),
        size: package["size"].as_u64(),
    };
    downloads::batch(client, vec![download], 1, job, "Downloading Java").await?;
    job.stage("Extracting Java", 0);
    let staged = work.path().join("runtime");
    if os == "windows" {
        let directory = staged.clone();
        let cancel = job.cancel.clone();
        tokio::task::spawn_blocking(move || {
            crate::archive::extract_zip(&archive, &directory, &[], &cancel)
        })
        .await??;
    } else {
        extract_runtime_tar(&archive, &staged, &job.cancel).await?;
    }
    let path = find_binary(&staged).context("Downloaded Java archive contains no executable.")?;
    let java = inspect(path.clone()).await?;
    ensure!(
        java.major == major,
        "Downloaded Java package returned the wrong version."
    );
    job.check()?;
    let relative = path.strip_prefix(&staged)?.to_owned();
    if tokio::fs::try_exists(&target).await? {
        check_directory(&target)?;
        tokio::fs::rename(&target, work.path().join("previous")).await?;
    }
    if let Err(error) = tokio::fs::rename(staged, &target).await {
        let previous = work.path().join("previous");
        if previous.exists() {
            if let Err(recovery) = tokio::fs::rename(&previous, &target).await {
                let saved = work.keep();
                bail!(
                    "Cannot install Java: {error}. The previous runtime is in {}: {recovery}",
                    saved.display()
                );
            }
        }
        return Err(error).context("Installing the Java runtime");
    }
    Ok(target.join(relative))
}

async fn extract_runtime_tar(
    archive: &Path,
    directory: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    tokio::fs::create_dir(directory).await?;
    let mut tar = Command::new("tar");
    tar.arg("--no-same-owner")
        .arg("--no-same-permissions")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .env_remove("TAR_OPTIONS")
        .kill_on_drop(true);
    let output = tokio::select! {
        _ = cancel.cancelled() => bail!("Cancelled."),
        result = tokio::time::timeout(Duration::from_secs(120), tar.output()) => result
            .context("Extracting the Java runtime timed out.")?
            .context("Could not start tar to extract the Java runtime.")?,
    };
    let error = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "Could not extract the Java runtime: {}",
        error.trim()
    );
    Ok(())
}

fn executable() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn find_binary(directory: &Path) -> Option<PathBuf> {
    fn at_root(root: &Path) -> Option<PathBuf> {
        [
            root.join("bin").join(executable()),
            root.join("Contents/Home/bin").join(executable()),
        ]
        .into_iter()
        .find(|path| path.is_file())
    }
    if let Some(direct) = at_root(directory) {
        return Some(direct);
    }
    std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .find_map(|entry| at_root(&entry.path()))
}

fn java_candidates(managed: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut push_roots = |root: &Path| {
        if let Ok(entries) = std::fs::read_dir(root) {
            candidates.extend(
                entries
                    .flatten()
                    .filter_map(|entry| find_binary(&entry.path())),
            );
        }
    };
    push_roots(managed);
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(key).map(PathBuf::from) {
            for vendor in [
                "Java",
                "Eclipse Adoptium",
                "Microsoft",
                "Amazon Corretto",
                "Zulu",
                "BellSoft",
            ] {
                push_roots(&root.join(vendor));
            }
        }
    }
    push_roots(Path::new("/usr/lib/jvm"));
    #[cfg(target_os = "macos")]
    {
        push_roots(Path::new("/Library/Java/JavaVirtualMachines"));
        if let Some(home) = env::var_os("HOME") {
            push_roots(&PathBuf::from(home).join("Library/Java/JavaVirtualMachines"));
        }
    }
    if let Some(home) = env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(executable()));
    }
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(
            env::split_paths(&path)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(executable())),
        );
    }
    candidates.retain(|path| path.is_file());
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_linux_runtime_and_macos_bundle_binaries() -> Result<()> {
        for layout in ["runtime/bin", "runtime/Contents/Home/bin"] {
            let archive = tempfile::tempdir()?;
            let bin = archive.path().join(layout);
            std::fs::create_dir_all(&bin)?;
            let java = bin.join(executable());
            std::fs::write(&java, [])?;
            assert_eq!(find_binary(archive.path()), Some(java));
        }
        Ok(())
    }

    #[test]
    fn reports_java_version_for_legacy_and_modern_output() {
        let regex = regex::Regex::new(r#"(?m)^(?:openjdk|java) version "(?:1\.)?(\d+)"#).unwrap();
        for (line, major) in [
            (r#"java version "1.8.0_402""#, "8"),
            (r#"openjdk version "21.0.7" 2025-04-15"#, "21"),
        ] {
            assert_eq!(&regex.captures(line).unwrap()[1], major);
        }
    }
}
