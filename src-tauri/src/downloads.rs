use anyhow::{bail, ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    providers::{Kind, Platform},
    storage::now,
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Cancelled,
    Failed,
}

impl JobStatus {
    fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDownload {
    pub kind: Kind,
    pub platform: Platform,
    pub project: String,
    pub world: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    id: Uuid,
    instance_id: Option<Uuid>,
    title: String,
    stage: String,
    status: JobStatus,
    completed: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<CatalogDownload>,
    bytes: u64,
    started_at: u64,
}

struct JobState {
    view: JobView,
    cancel: CancellationToken,
    created: Instant,
}

#[derive(Clone, Default)]
pub struct Jobs(Arc<Mutex<HashMap<Uuid, JobState>>>);

#[derive(Clone)]
pub struct Job {
    pub id: Uuid,
    pub cancel: CancellationToken,
    jobs: Jobs,
}

impl Jobs {
    pub fn has_active(&self) -> bool {
        self.0
            .lock()
            .expect("jobs lock")
            .values()
            .any(|job| job.view.status.active())
    }
    pub fn start(&self, instance_id: Option<Uuid>, title: String) -> Job {
        self.create(instance_id, title, None).0
    }

    pub fn enqueue(
        &self,
        instance_id: Option<Uuid>,
        title: String,
        catalog: CatalogDownload,
    ) -> (Job, bool) {
        self.create(instance_id, title, Some(catalog))
    }

    fn create(
        &self,
        instance_id: Option<Uuid>,
        title: String,
        catalog: Option<CatalogDownload>,
    ) -> (Job, bool) {
        let mut jobs = self.0.lock().expect("jobs lock");
        if let Some(existing) = jobs.values().find(|job| {
            catalog.is_some()
                && job.view.catalog == catalog
                && job.view.instance_id == instance_id
                && job.view.status.active()
                && !job.cancel.is_cancelled()
        }) {
            return (
                Job {
                    id: existing.view.id,
                    cancel: existing.cancel.clone(),
                    jobs: self.clone(),
                },
                false,
            );
        }
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        if jobs.len() >= 80 {
            if let Some(old) = jobs
                .values()
                .filter(|job| !job.view.status.active())
                .min_by_key(|job| job.created)
                .map(|job| job.view.id)
            {
                jobs.remove(&old);
            }
        }
        let queued = catalog.is_some();
        jobs.insert(
            id,
            JobState {
                view: JobView {
                    id,
                    instance_id,
                    title,
                    stage: if queued {
                        "Waiting to install"
                    } else {
                        "Preparing"
                    }
                    .into(),
                    status: if queued {
                        JobStatus::Queued
                    } else {
                        JobStatus::Running
                    },
                    completed: 0,
                    total: 0,
                    error: None,
                    catalog,
                    bytes: 0,
                    started_at: now(),
                },
                cancel: cancel.clone(),
                created: Instant::now(),
            },
        );
        (
            Job {
                id,
                cancel,
                jobs: self.clone(),
            },
            true,
        )
    }

    pub fn list(&self) -> Vec<JobView> {
        let mut jobs: Vec<_> = self
            .0
            .lock()
            .expect("jobs lock")
            .values()
            .map(|job| (job.created, job.view.clone()))
            .collect();
        jobs.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
        jobs.into_iter().map(|(_, view)| view).collect()
    }

    pub fn cancel(&self, id: Uuid) -> Result<()> {
        let mut jobs = self.0.lock().expect("jobs lock");
        let state = jobs
            .get_mut(&id)
            .context("This download is no longer available.")?;
        ensure!(state.view.status.active(), "This download has finished.");
        if matches!(state.view.status, JobStatus::Queued) {
            state.view.status = JobStatus::Cancelled;
            state.view.stage = "Download cancelled.".into();
        } else {
            state.view.stage = "Cancelling…".into();
        }
        state.cancel.cancel();
        Ok(())
    }
}

impl Job {
    pub fn begin(&self) -> Result<()> {
        self.update(|view| {
            if !self.cancel.is_cancelled() {
                view.status = JobStatus::Running;
                view.stage = "Preparing".into();
            }
        });
        self.check()
    }

    fn update(&self, update: impl FnOnce(&mut JobView)) {
        if let Some(job) = self.jobs.0.lock().expect("jobs lock").get_mut(&self.id) {
            update(&mut job.view);
        }
    }

    pub fn stage(&self, label: &str, total: usize) {
        self.update(|view| {
            view.stage = label.to_owned();
            view.completed = 0;
            view.total = total;
        });
    }

    pub(crate) fn completed(&self) {
        self.update(|view| view.completed += 1);
    }

    fn add_bytes(&self, bytes: usize) {
        self.update(|view| view.bytes += bytes as u64);
    }

    pub fn check(&self) -> Result<()> {
        ensure!(!self.cancel.is_cancelled(), "Cancelled.");
        Ok(())
    }

    pub fn finish(&self, result: &Result<()>) {
        self.update(|view| match result {
            Ok(()) => view.status = JobStatus::Done,
            Err(_) if self.cancel.is_cancelled() => view.status = JobStatus::Cancelled,
            Err(error) => {
                view.status = JobStatus::Failed;
                view.error = Some(format!("{error:#}"));
            }
        });
    }
}

#[derive(Clone, Deserialize)]
pub struct MojangFile {
    #[serde(default)]
    pub url: String,
    pub sha1: String,
    pub size: Option<u64>,
    pub path: Option<String>,
}

impl MojangFile {
    pub fn at(&self, destination: PathBuf) -> Download {
        Download {
            url: self.url.clone(),
            destination,
            hash: Hash::Sha1(self.sha1.clone()),
            size: self.size,
        }
    }
}

#[derive(Clone)]
pub enum Hash {
    Sha1(String),
    Sha256(String),
    Sha512(String),
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(Sha256),
    Sha512(sha2::Sha512),
}

impl Hash {
    fn start(&self) -> Hasher {
        match self {
            Self::Sha1(_) => Hasher::Sha1(sha1::Sha1::new()),
            Self::Sha256(_) => Hasher::Sha256(Sha256::new()),
            Self::Sha512(_) => Hasher::Sha512(sha2::Sha512::new()),
        }
    }

    fn expected(&self) -> &str {
        match self {
            Self::Sha1(value) | Self::Sha256(value) | Self::Sha512(value) => value,
        }
    }
}

impl Hasher {
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha1(digest) => digest.update(bytes),
            Self::Sha256(digest) => digest.update(bytes),
            Self::Sha512(digest) => digest.update(bytes),
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Sha1(digest) => hex::encode(digest.finalize()),
            Self::Sha256(digest) => hex::encode(digest.finalize()),
            Self::Sha512(digest) => hex::encode(digest.finalize()),
        }
    }
}

#[derive(Clone)]
pub struct Download {
    pub url: String,
    pub destination: PathBuf,
    pub hash: Hash,
    pub size: Option<u64>,
}

pub async fn batch(
    client: &Client,
    entries: Vec<Download>,
    limit: usize,
    job: &Job,
    label: &str,
) -> Result<()> {
    job.stage(label, entries.len());
    stream::iter(entries)
        .map(|download| async move { file(client, &download, job).await })
        .buffer_unordered(limit.clamp(1, 16))
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}

pub async fn file(client: &Client, download: &Download, job: &Job) -> Result<()> {
    let expected = download.hash.expected();
    let width = match &download.hash {
        Hash::Sha1(_) => 40,
        Hash::Sha256(_) => 64,
        Hash::Sha512(_) => 128,
    };
    ensure!(
        expected.len() == width && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid download checksum."
    );
    job.check()?;
    if verified(&download.destination, &download.hash, download.size, job).await? {
        job.completed();
        return Ok(());
    }
    ensure!(!download.url.is_empty(), "A loader-generated library is missing or corrupt: {}. Close Voltra, remove this loader’s cache directory, and prepare it again.", download.destination.display());
    let url = https_url(&download.url)?;
    let mut latest_error = None;
    for attempt in 0..3 {
        job.check()?;
        let result = tokio::select! {
            _ = job.cancel.cancelled() => bail!("Cancelled."),
            result = transfer(client, &url, download, job) => result,
        };
        match result {
            Ok(()) => {
                job.completed();
                return Ok(());
            }
            Err(error) => latest_error = Some(error),
        }
        if attempt < 2 {
            tokio::select! {
                _ = job.cancel.cancelled() => bail!("Cancelled."),
                _ = tokio::time::sleep(Duration::from_millis(300 * (1 << attempt))) => (),
            }
        }
    }
    Err(latest_error.expect("at least one download attempt")).with_context(|| {
        format!(
            "Downloading {}",
            download
                .destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        )
    })
}

async fn transfer(client: &Client, url: &Url, download: &Download, job: &Job) -> Result<()> {
    let response = client.get(url.clone()).send().await?.error_for_status()?;
    let folder = download
        .destination
        .parent()
        .context("Download has no parent folder.")?;
    tokio::fs::create_dir_all(folder).await?;
    let folder = folder.to_owned();
    let temp =
        tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(folder)).await??;
    let mut output = tokio::fs::File::from_std(temp.reopen()?);
    let mut digest = download.hash.start();
    let mut bytes = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes += chunk.len() as u64;
        ensure!(
            bytes <= download.size.unwrap_or(2 * 1024 * 1024 * 1024),
            "Download exceeds its expected size."
        );
        digest.update(&chunk);
        output.write_all(&chunk).await?;
        job.add_bytes(chunk.len());
    }
    ensure!(
        download.size.is_none_or(|size| bytes == size),
        "Download size does not match its manifest."
    );
    ensure!(
        digest
            .finish()
            .eq_ignore_ascii_case(download.hash.expected()),
        "Download checksum did not match."
    );
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    job.check()?;
    let destination = download.destination.clone();
    tokio::task::spawn_blocking(move || {
        temp.persist(destination)
            .map(|_| ())
            .map_err(|error| error.error)
    })
    .await??;
    Ok(())
}

async fn verified(path: &Path, hash: &Hash, size: Option<u64>, job: &Job) -> Result<bool> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.is_symlink(),
        "Download target is not a regular file."
    );
    if size.is_some_and(|size| metadata.len() != size) {
        return Ok(false);
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut digest = hash.start();
    loop {
        job.check()?;
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finish().eq_ignore_ascii_case(hash.expected()))
}

pub fn https_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).context("Invalid download URL.")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "Only HTTPS URLs without embedded credentials are supported."
    );
    Ok(url)
}

pub fn client(redirects: bool) -> Result<Client> {
    let redirect = if redirects {
        reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 || attempt.url().scheme() != "https" {
                attempt.error("Rejected an insecure or excessive redirect")
            } else {
                attempt.follow()
            }
        })
    } else {
        reqwest::redirect::Policy::none()
    };
    Ok(Client::builder()
        .user_agent(concat!("Voltra/", env!("CARGO_PKG_VERSION")))
        .redirect(redirect)
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(Duration::from_secs(25))
        .build()?)
}
