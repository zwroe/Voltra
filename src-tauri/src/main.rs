#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod auth;
mod content;
mod diagnostics;
mod discord;
mod downloads;
mod health;
mod instance_files;
mod media;
mod mclogs;
mod migration;
mod minecraft;
mod mod_artwork;
mod mod_history;
mod mod_sides;
mod mods;
mod packs;
mod peer_share;
mod providers;
mod quick_play;
mod server_export;
mod server_host;
mod server_workspace;
mod sessions;
mod snapshots;
mod storage;
mod test_launch;

use anyhow::{bail, ensure, Context, Result};
use auth::{Accounts, Auth, BrowserSignIn};
use downloads::{CatalogDownload, Job, JobView, Jobs};
use fs2::FileExt;
use providers::{Filters, Kind, Platform, Project, ProjectDetails};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use storage::{Backup, InstanceOptions, InstanceView, Settings, Store};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

type Reply<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct App(Arc<Services>);

type CatalogQueue = VecDeque<(Job, CatalogDownload)>;

struct Services {
    store: Store,
    auth: Auth,
    client: Client,
    private_client: Client,
    jobs: Jobs,
    catalog_queues: Mutex<HashMap<Option<Uuid>, CatalogQueue>>,
    mod_previews: Mutex<HashMap<Uuid, (Uuid, mods::PreparedUpdate)>>,
    instance_gates: Mutex<HashMap<Uuid, Weak<AsyncMutex<()>>>>,
    java_gate: AsyncMutex<()>,
    loader_gate: AsyncMutex<()>,
    running: Mutex<HashMap<Uuid, Running>>,
    servers: Mutex<HashMap<Uuid, server_host::RunningServer>>,
    monitor: Mutex<sysinfo::System>,
    discord: discord::Presence,
    _instance_lock: fs::File,
    _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Running {
    instance_id: Uuid,
    pid: u32,
    started_at: u64,
    playtime_seconds: u64,
    test: bool,
    #[serde(skip)]
    clock: Option<Instant>,
    #[serde(skip)]
    discord_game: Option<discord::Game>,
}

impl Running {
    fn snapshot(&self) -> Self {
        Self {
            playtime_seconds: self
                .playtime_seconds
                .saturating_add(self.clock.map_or(0, |clock| clock.elapsed().as_secs())),
            clock: None,
            ..self.clone()
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    settings: Settings,
    instances: Vec<InstanceView>,
    accounts: Accounts,
    data_dir: PathBuf,
    total_memory_mib: u64,
    microsoft_configured: bool,
    test_launch_available: bool,
}

impl App {
    fn new(root: PathBuf) -> Result<Self> {
        let store = Store::new(root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(store.root.join("config/voltra.lock"))?;
        FileExt::try_lock_exclusive(&lock)
            .context("Another Voltra process has this data directory open.")?;
        let file = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("voltra")
            .filename_suffix("log")
            .max_log_files(7)
            .build(store.root.join("logs"))?;
        let (writer, log_guard) = tracing_appender::non_blocking(file);
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info,reqwest=warn"))
            .with_writer(writer)
            .with_ansi(false)
            .try_init();
        let client = downloads::client(true)?;
        let private_client = downloads::client(false)?;
        Ok(Self(Arc::new(Services {
            auth: Auth::new(
                private_client.clone(),
                store.root.join("config/accounts.json"),
            ),
            store,
            client,
            private_client,
            jobs: Jobs::default(),
            catalog_queues: Mutex::default(),
            mod_previews: Mutex::default(),
            instance_gates: Mutex::default(),
            java_gate: AsyncMutex::new(()),
            loader_gate: AsyncMutex::new(()),
            running: Mutex::default(),
            servers: Mutex::default(),
            monitor: Mutex::new(sysinfo::System::new()),
            discord: discord::Presence::new(),
            _instance_lock: lock,
            _log_guard: log_guard,
        })))
    }

    fn instance_gate(&self, id: Uuid) -> Arc<AsyncMutex<()>> {
        let mut gates = self.0.instance_gates.lock().expect("instance lock");
        gates.retain(|_, lock| lock.strong_count() > 0);
        gates.get(&id).and_then(Weak::upgrade).unwrap_or_else(|| {
            let lock = Arc::new(AsyncMutex::new(()));
            gates.insert(id, Arc::downgrade(&lock));
            lock
        })
    }

    fn gate(&self, id: Uuid) -> Result<OwnedMutexGuard<()>> {
        self.instance_gate(id).try_lock_owned().map_err(|_| {
            if self.0.running.lock().expect("running lock").contains_key(&id) {
                anyhow::anyhow!("Close this instance's Minecraft session before making changes.")
            } else {
                anyhow::anyhow!("Voltra is finishing another operation on this instance. Try again when it completes.")
            }
        })
    }

    fn prevent_close(&self) -> Option<&'static str> {
        if !self.0.servers.lock().expect("server lock").is_empty() {
            return Some("Stop the Minecraft server before closing Voltra so it can save its world.");
        }
        if !self.0.running.lock().expect("running lock").is_empty() {
            return Some("Close Minecraft before quitting Voltra so the instance stays protected.");
        }
        if self.0.jobs.has_active()
            || self
                .0
                .instance_gates
                .lock()
                .expect("instance lock")
                .values()
                .any(|gate| gate.strong_count() > 0)
        {
            return Some("Wait for the current operation to finish, or cancel it from Downloads.");
        }
        None
    }

    fn refresh_discord(&self) {
        let running = self.0.running.lock().expect("running lock");
        let game = running.values()
            .filter_map(|running| running.discord_game.as_ref().map(|game| (running.pid, game)))
            .max_by_key(|(pid, game)| (game.started_at, *pid))
            .map(|(_, game)| game.clone());
        self.0.discord.set_game(game);
    }

    async fn local<T: Send + 'static>(
        &self,
        work: impl FnOnce(Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.0.store.clone();
        tokio::task::spawn_blocking(move || work(store)).await?
    }

    async fn change<T: Send + 'static>(
        &self,
        id: Uuid,
        work: impl FnOnce(Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let guard = self.gate(id)?;
        self.local(move |store| {
            let _guard = guard;
            work(store)
        })
        .await
    }

    fn prepare_instance(
        &self,
        id: Uuid,
        play: bool,
        target: Option<quick_play::Target>,
    ) -> Result<Uuid> {
        ensure!(
            play || target.is_none(),
            "Choose Play to open a destination."
        );
        let guard = self.gate(id)?;
        let app = self.clone();
        let job = self.0.jobs.start(
            Some(id),
            if play {
                "Launch Minecraft".into()
            } else {
                "Prepare Minecraft instance".into()
            },
        );
        let job_id = job.id;
        tauri::async_runtime::spawn(async move {
            let _guard = guard;
            let requested = storage::now();
            let clock = Instant::now();
            let result = app.prepare_and_launch(id, play, target, &job).await;
            if let Err(error) = &result {
                tracing::warn!(instance = %id, error = %error, "Minecraft preparation or launch failed");
                if play && !job.cancel.is_cancelled() {
                    let elapsed = clock.elapsed().as_secs();
                    let _ = app
                        .local(move |store| {
                            let mut record =
                                sessions::Session::new(Uuid::new_v4(), requested, elapsed, 0);
                            record.failed();
                            record.save(&store, id)
                        })
                        .await;
                }
            }
            job.finish(&result);
        });
        Ok(job_id)
    }

    async fn prepare_and_launch(
        &self,
        id: Uuid,
        play: bool,
        target: Option<quick_play::Target>,
        job: &Job,
    ) -> Result<()> {
        let preparing = Instant::now();
        let (instance, settings, game) = self
            .local({
                let target = target.clone();
                move |store| {
                    if let Some(target) = &target {
                        target.validate(&store, id)?;
                    }
                    Ok((store.instance(id)?, store.settings()?, store.game_dir(id)?))
                }
            })
            .await?;
        job.stage(
            if play {
                "Checking your Minecraft account"
            } else {
                "Resolving Minecraft version"
            },
            0,
        );
        if play {
            self.0.auth.has_profile().await?;
        }
        let prepared = minecraft::prepare(
            &self.0.client,
            &self.0.store,
            &instance,
            game,
            &settings,
            &self.0.java_gate,
            &self.0.loader_gate,
            job,
        )
        .await?;
        if !play {
            if prepared.loader_version != instance.options.loader_version {
                let mut updated = instance;
                updated.options.loader_version = prepared.loader_version.clone();
                self.local(move |store| store.pin_loader(&updated)).await?;
            }
            job.stage("Minecraft files are ready", 0);
            return Ok(());
        }
        if instance
            .options
            .launch
            .hooks
            .pre_launch
            .as_ref()
            .or(settings.hooks.pre_launch.as_ref())
            .is_some_and(|hook| !hook.trim().is_empty())
        {
            job.stage("Running pre-launch hook", 0);
            tokio::select! {
                _ = job.cancel.cancelled() => bail!("Cancelled."),
                result = prepared.run_hook(&self.0.store, &instance, &settings, true) => result.context("Pre-launch hook failed")?,
            };
        }
        job.stage("Refreshing Minecraft sign-in", 0);
        let session = tokio::select! {
            _ = job.cancel.cancelled() => bail!("Cancelled."),
            session = self.0.auth.session() => session?,
        };
        job.stage("Starting Minecraft", 0);
        let output = self
            .local(move |store| {
                Ok(fs::File::create(
                    store.root.join("logs").join(format!("{id}.log")),
                )?)
            })
            .await?;
        job.check()?;
        let token = session.token.clone();
        let session_id = Uuid::new_v4();
        let (prepared, instance, settings, mut child, mut visited) = tokio::task::spawn_blocking({
            let store = self.0.store.clone();
            move || {
                let visited = quick_play::existing_log_revision(&store, instance.id)?;
                let child = prepared.launch(
                    &store,
                    &instance,
                    &settings,
                    minecraft::LaunchIdentity::Microsoft(&session),
                    target.as_ref(),
                    Some(session_id),
                )?;
                Ok::<_, anyhow::Error>((prepared, instance, settings, child, visited))
            }
        })
        .await??;
        let consoles = capture_console(&mut child, output, Some(token))?;
        let pid = child.id().context("Minecraft process has no ID.")?;
        let started_at = storage::now();
        let clock = Instant::now();
        let previous_playtime = instance.playtime_seconds;
        let allocation = if instance.options.launch.inherit_memory {
            settings.ram_mib
        } else {
            instance.options.ram_mib
        };
        let mut record = sessions::Session::new(
            session_id,
            started_at,
            preparing.elapsed().as_secs(),
            allocation,
        );
        self.0.running.lock().expect("running lock").insert(
            id,
            Running {
                instance_id: id,
                pid,
                started_at,
                playtime_seconds: previous_playtime,
                test: false,
                clock: Some(clock),
                discord_game: Some(discord::Game {
                    version: instance.options.minecraft_version.clone(),
                    pack: instance.options.name.clone(),
                    started_at,
                    test: false,
                }),
            },
        );
        self.refresh_discord();
        let version = prepared.loader_version.clone();
        let played = instance.clone();
        let initial = record.clone();
        if let Err(error) = self
            .local(move |store| {
                store.mark_played(played, version, started_at)?;
                initial.save(&store, id)
            })
            .await
        {
            tracing::warn!(instance = %id, error = %error, "Minecraft started but last played could not be saved");
        }
        job.stage("Minecraft started", 0);
        job.finish(&Ok(()));
        let mut history_poll = tokio::time::interval(Duration::from_secs(2));
        history_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut next_checkpoint = 60;
        let mut next_sample = 6;
        let exit = loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = history_poll.tick() => {
                    let elapsed = clock.elapsed().as_secs();
                    if elapsed >= next_sample {
                        let services = self.0.clone();
                        let mut measured = record.clone();
                        if let Ok(value) = self.local(move |store| {
                            measured.update(&store, id, elapsed, sample_process(&services, pid));
                            Ok(measured)
                        }).await { record = value; }
                        next_sample = elapsed.saturating_add(6);
                    }
                    if elapsed >= next_checkpoint {
                        self.save_playtime(id, started_at, previous_playtime.saturating_add(elapsed)).await;
                        let checkpoint = record.clone();
                        let _ = self.local(move |store| checkpoint.save(&store, id)).await;
                        next_checkpoint = elapsed.saturating_add(60);
                    }
                    self.capture_play_history(id, &mut visited).await;
                },
            }
        };
        let total = previous_playtime.saturating_add(clock.elapsed().as_secs());
        self.0.running.lock().expect("running lock").remove(&id);
        self.refresh_discord();
        self.save_playtime(id, started_at, total).await;
        record.finish(total.saturating_sub(previous_playtime), &exit);
        self.capture_play_history(id, &mut visited).await;
        let drain_until = tokio::time::Instant::now() + Duration::from_secs(2);
        for mut logger in consoles {
            match tokio::time::timeout_at(drain_until, &mut logger).await {
                Ok(Ok(Err(error))) => tracing::warn!(instance = %id, %error, "Could not write Minecraft console output"),
                Err(_) => {
                    // A subprocess can inherit the console pipes and keep them open after Minecraft exits.
                    logger.abort();
                    let _ = logger.await;
                }
                _ => (),
            }
        }
        if let Err(error) = self
            .local(move |store| {
                if let Ok(signals) = diagnostics::session_signals(&store, id, started_at) {
                    record.add_signals(signals);
                }
                record.save(&store, id)
            })
            .await
        {
            tracing::warn!(instance = %id, %error, "Could not save the Minecraft session");
        }
        if exit.as_ref().is_ok_and(|status| status.success()) {
            let baseline = instance.clone();
            if let Err(error) = self
                .local(move |store| {
                    let worlds = quick_play::worlds_visited_since(&store, id, started_at)?;
                    snapshots::save_baseline(&store, &baseline, &worlds)
                })
                .await
            {
                tracing::warn!(instance = %id, %error, "Could not record the pack used for world compatibility checks");
            }
        }
        if let Err(error) = prepared
            .run_hook(&self.0.store, &instance, &settings, false)
            .await
        {
            tracing::warn!(instance = %id, %error, "Post-exit hook failed");
        }
        drop(prepared);
        match exit {
            Ok(status) if status.success() => {
                tracing::info!(instance = %id, %status, "Minecraft exited")
            }
            Ok(status) => {
                tracing::warn!(instance = %id, %status, "Minecraft exited with an error");
                self.0.jobs.start(Some(id), "Minecraft exited unexpectedly".into()).finish(&Err(anyhow::anyhow!("Minecraft exited with {status}. Review the instance Diagnostics and Performance tabs.")));
            }
            Err(error) => tracing::warn!(instance = %id, %error, "Could not wait for Minecraft"),
        }
        Ok(())
    }

    async fn save_playtime(&self, id: Uuid, started_at: u64, total: u64) {
        if let Err(error) = self
            .local(move |store| store.save_playtime(id, started_at, total))
            .await
        {
            tracing::warn!(instance = %id, %error, "Could not save instance playtime");
        }
    }

    async fn capture_play_history(&self, id: Uuid, visited: &mut Option<std::time::SystemTime>) {
        let mut seen = *visited;
        match self
            .local(move |store| {
                quick_play::capture(&store, id, &mut seen)?;
                Ok(seen)
            })
            .await
        {
            Ok(seen) => *visited = seen,
            Err(error) => {
                tracing::debug!(instance = %id, %error, "Could not capture Quick Play history")
            }
        }
    }

    async fn apply_mod_change(
        &self,
        id: Uuid,
        operation: mods::Operation,
        job: &Job,
    ) -> Result<()> {
        let (instance, settings) = self
            .local(move |store| Ok((store.instance(id)?, store.settings()?)))
            .await?;
        mods::apply(
            &self.0.client,
            &self.0.private_client,
            &self.0.store,
            &instance,
            settings.download_limit,
            operation,
            job,
        )
        .await
    }

    fn update_mods(&self, id: Uuid, file: Option<String>) -> Result<Uuid> {
        let guard = self.gate(id)?;
        let app = self.clone();
        let job = self.0.jobs.start(Some(id), "Check for mod updates".into());
        let job_id = job.id;
        tauri::async_runtime::spawn(async move {
            let _guard = guard;
            let result = app
                .apply_mod_change(id, mods::Operation::Update(file), &job)
                .await;
            if let Err(error) = &result {
                tracing::warn!(instance = %id, error = %error, "Mod change failed");
            } else if !job.cancel.is_cancelled() {
                job.stage("Instance mods are up to date", 0);
            }
            job.finish(&result);
        });
        Ok(job_id)
    }

    fn queue_catalog(
        &self,
        id: Option<Uuid>,
        catalog: CatalogDownload,
        title: Option<String>,
    ) -> Uuid {
        let title = title
            .as_deref()
            .map(|title| {
                title
                    .trim()
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(128)
                    .collect::<String>()
            })
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("Install from {}", catalog.platform.label()));
        let mut queues = self.0.catalog_queues.lock().expect("catalog queues lock");
        let (job, added) = self.0.jobs.enqueue(id, title, catalog.clone());
        let job_id = job.id;
        if added {
            let start_worker = !queues.contains_key(&id);
            queues.entry(id).or_default().push_back((job, catalog));
            drop(queues);
            if start_worker {
                let app = self.clone();
                tauri::async_runtime::spawn(async move {
                    app.drain_catalog(id).await;
                });
            }
        }
        job_id
    }

    async fn drain_catalog(&self, id: Option<Uuid>) {
        loop {
            let (job, catalog) = {
                let mut queues = self.0.catalog_queues.lock().expect("catalog queues lock");
                let Some(request) = queues.get_mut(&id).and_then(VecDeque::pop_front) else {
                    queues.remove(&id);
                    return;
                };
                request
            };
            let result = async {
                let _guard = if let Some(id) = id {
                    tokio::select! {
                        _ = job.cancel.cancelled() => bail!("Installation cancelled."),
                        guard = self.instance_gate(id).lock_owned() => Some(guard),
                    }
                } else {
                    None
                };
                job.begin()?;
                self.install_catalog(id, catalog, &job).await
            }
            .await;
            if let Err(error) = &result {
                if !job.cancel.is_cancelled() {
                    tracing::warn!(instance = ?id, error = %error, "Catalog installation failed");
                }
            }
            job.finish(&result);
        }
    }

    async fn install_catalog(
        &self,
        id: Option<Uuid>,
        catalog: CatalogDownload,
        job: &Job,
    ) -> Result<()> {
        let CatalogDownload {
            kind,
            platform,
            project,
            world,
        } = catalog;
        if kind == Kind::Mod {
            self.apply_mod_change(
                id.context("Select an instance first.")?,
                mods::Operation::Install(platform, project),
                job,
            )
            .await?;
            job.stage("Mod installed", 0);
            return Ok(());
        }

        let instance = id.map(|id| self.0.store.instance(id)).transpose()?;
        let game = instance
            .as_ref()
            .map(|instance| instance.options.minecraft_version.as_str());
        job.stage("Finding a compatible release", 0);
        let provider =
            providers::provider(platform, &self.0.client, &self.0.private_client).await?;
        let file = tokio::select! {
            _ = job.cancel.cancelled() => bail!("Installation cancelled."),
            file = provider.content_file(&project, kind, game) => file?,
        };
        storage::check_filename(&file.filename)?;
        let stage = tempfile::tempdir_in(self.0.store.root.join("cache"))?;
        let archive = stage.path().join(file.filename);
        let icon_url = file.icon_url;
        downloads::batch(
            &self.0.client,
            vec![downloads::Download {
                url: file.url,
                destination: archive.clone(),
                hash: file.hash,
                size: file.size,
            }],
            1,
            job,
            "Downloading project",
        )
        .await?;
        job.check()?;
        if kind == Kind::Modpack {
            packs::import(&self.0.client, &self.0.store, archive, job).await?;
        } else {
            let id = id.context("Select an instance first.")?;
            let cancel = job.cancel.clone();
            self.local(move |store| {
                let folder = if kind == Kind::Datapack {
                    media::world_dir(
                        &store,
                        id,
                        &world.context("Select a world for the data pack.")?,
                    )?
                } else {
                    store.game_dir(id)?
                }
                .join(
                    kind.folder()
                        .context("This content type has no installation folder.")?,
                );
                fs::create_dir_all(&folder)?;
                storage::check_directory(&folder)?;
                let file = archive.file_name().context("Invalid project filename.")?;
                let target = folder.join(file);
                let mut temp = tempfile::NamedTempFile::new_in(&folder)?;
                let mut data = fs::File::open(&archive)?;
                std::io::copy(&mut data, temp.as_file_mut())?;
                ensure!(!cancel.is_cancelled(), "Installation cancelled.");
                temp.persist_noclobber(&target)
                    .map_err(|error| error.error)
                    .context("A file with this name may already be installed.")?;
                if matches!(kind, Kind::Resourcepack | Kind::Shader) {
                    if let Some(url) = icon_url {
                        let label = kind.folder().expect("resource or shader pack folder");
                        if let Err(error) = content::save_icon(&store, id, label, &target, url) {
                            tracing::warn!(%error, "Could not save the pack icon");
                        }
                    }
                }
                Ok(())
            })
            .await?;
        }
        job.stage("Content installed", 0);
        Ok(())
    }
}

fn capture_console(
    child: &mut tokio::process::Child,
    file: fs::File,
    token: Option<String>,
) -> Result<[tokio::task::JoinHandle<Result<()>>; 2]> {
    let stdout = child
        .stdout
        .take()
        .context("Minecraft console output is unavailable.")?;
    let stderr = child
        .stderr
        .take()
        .context("Minecraft error output is unavailable.")?;
    let output = Arc::new(AsyncMutex::new(tokio::fs::File::from_std(file)));
    Ok([
        tokio::spawn(write_console(stdout, output.clone(), token.clone())),
        tokio::spawn(write_console(stderr, output, token)),
    ])
}

async fn write_console(
    stream: impl AsyncRead + Unpin,
    output: Arc<AsyncMutex<tokio::fs::File>>,
    token: Option<String>,
) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = Vec::new();
    while stream.read_until(b'\n', &mut line).await? != 0 {
        let line_text = String::from_utf8_lossy(&line);
        let sanitized = match token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => std::borrow::Cow::Owned(line_text.replace(token, "[REDACTED]")),
            None => line_text,
        };
        if let Err(error) = output.lock().await.write_all(sanitized.as_bytes()).await {
            tracing::warn!(%error, "Minecraft console output could not be saved");
            tokio::io::copy(&mut stream, &mut tokio::io::sink()).await?;
            return Ok(());
        }
        line.clear();
    }
    output.lock().await.flush().await?;
    Ok(())
}

fn reply<T>(result: Result<T>) -> Reply<T> {
    result.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn start_local_server(app: State<'_, App>, id: Uuid, port: u16, ram: u32, eula: bool) -> Reply<Uuid> {
    reply(server_host::start(&app, id, port, ram, eula))
}

#[tauri::command]
async fn local_server_status(app: State<'_, App>, id: Uuid, metrics: Option<bool>) -> Reply<server_host::Status> {
    let services = app.0.clone();
    reply(app.local(move |store| {
        let mut status = server_host::status(&store, id)?;
        let pid = if let Some(server) = services.servers.lock().expect("server lock").get(&id) {
            let pid = server.pid.load(std::sync::atomic::Ordering::Acquire);
            status.job_id = Some(server.job.id);
            status.process_started = pid > 0;
            status.stopping = server.job.cancel.is_cancelled();
            pid
        } else {
            0
        };
        if metrics == Some(true) && pid > 0 {
            status.metrics = sample_process(&services, pid).map(|(cpu_percent, memory_bytes)| Performance { cpu_percent, memory_bytes });
        }
        Ok(status)
    }).await)
}

#[tauri::command]
fn stop_local_server(app: State<'_, App>, id: Uuid) -> Reply<()> {
    reply((|| {
        let servers = app.0.servers.lock().expect("server lock");
        let server = servers.get(&id).context("This server is not running.")?;
        server.job.cancel.cancel();
        Ok(())
    })())
}

#[tauri::command]
fn local_server_command(app: State<'_, App>, id: Uuid, command: String) -> Reply<()> {
    reply((|| {
        let command = command.trim().trim_start_matches('/');
        ensure!(!command.is_empty() && command.len() <= 512 && !command.chars().any(char::is_control), "Enter a single Minecraft console command.");
        let servers = app.0.servers.lock().expect("server lock");
        let server = servers.get(&id).context("This server is not running.")?;
        ensure!(
            server.pid.load(std::sync::atomic::Ordering::Acquire) > 0 && !server.job.cancel.is_cancelled(),
            "Wait for the server to finish starting."
        );
        server.commands.try_send(command.into()).context("The server console is busy.")?;
        Ok(())
    })())
}

async fn change_server<T: Send + 'static>(
    app: &App,
    id: Uuid,
    work: impl FnOnce(Store) -> Result<T> + Send + 'static,
) -> Reply<T> {
    let services = app.clone();
    reply(app.change(id, move |store| {
        ensure!(!services.0.servers.lock().expect("server lock").contains_key(&id), "Stop this server before changing its files.");
        work(store)
    }).await)
}

#[tauri::command]
async fn local_server_properties(app: State<'_, App>, id: Uuid) -> Reply<std::collections::BTreeMap<String, String>> {
    reply(app.local(move |store| server_workspace::properties(&store, id)).await)
}

#[tauri::command]
async fn save_local_server_properties(app: State<'_, App>, id: Uuid, values: std::collections::BTreeMap<String, String>) -> Reply<()> {
    change_server(&app, id, move |store| server_workspace::save_properties(&store, id, values)).await
}

#[tauri::command]
async fn save_local_server_memory(app: State<'_, App>, id: Uuid, ram: u32) -> Reply<()> {
    change_server(&app, id, move |store| server_host::save_memory(&store, id, ram)).await
}

#[tauri::command]
async fn local_server_addons(app: State<'_, App>, id: Uuid) -> Reply<Vec<server_workspace::Addon>> {
    reply(app.local(move |store| server_workspace::addons(&store, id)).await)
}

#[tauri::command]
async fn import_local_server_addon(app: State<'_, App>, window: tauri::WebviewWindow, id: Uuid) -> Reply<bool> {
    let picker = window.dialog().file().add_filter("Minecraft mod", &["jar"]);
    let Some(path) = reply(tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await.map_err(Into::into))? else {
        return Ok(false);
    };
    let path = reply(path.into_path().map_err(Into::into))?;
    change_server(&app, id, move |store| server_workspace::import_addon(&store, id, &path)).await?;
    Ok(true)
}

#[tauri::command]
async fn change_local_server_addon(app: State<'_, App>, id: Uuid, file: String, action: server_workspace::AddonAction) -> Reply<()> {
    change_server(&app, id, move |store| server_workspace::change_addon(&store, id, &file, action)).await
}

#[tauri::command]
async fn local_server_players(app: State<'_, App>, id: Uuid) -> Reply<server_workspace::Players> {
    reply(app.local(move |store| server_workspace::players(&store, id)).await)
}

#[tauri::command]
async fn local_server_log(app: State<'_, App>, id: Uuid, crash: bool) -> Reply<(String, bool)> {
    reply(app.local(move |store| server_workspace::log(&store, id, crash)).await)
}

#[tauri::command]
async fn prepare_log_share(content: String) -> Reply<String> {
    reply(async { tokio::task::spawn_blocking(move || mclogs::redact(&content)).await? }.await)
}

#[tauri::command]
async fn upload_log_share(content: String) -> Reply<String> {
    reply(mclogs::upload(content).await)
}

#[tauri::command]
async fn open_log_share(url: String) -> Reply<()> {
    reply(async { tokio::task::spawn_blocking(move || mclogs::open(&url)).await? }.await)
}

#[tauri::command]
async fn open_local_server_folder(app: State<'_, App>, id: Uuid) -> Reply<()> {
    reply(app.local(move |store| {
        let folder = server_host::directory(&store, id);
        storage::check_directory(&folder)?;
        open::that(folder).context("Opening the server folder")
    }).await)
}

#[tauri::command]
async fn open_minecraft_eula() -> Reply<()> {
    reply(async {
        tokio::task::spawn_blocking(|| open::that("https://www.minecraft.net/en-us/eula")).await??;
        Ok(())
    }.await)
}

#[tauri::command]
fn local_share_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareLink { code: String, job_id: Uuid }

#[tauri::command]
async fn share_instance(app: State<'_, App>, id: Uuid, address: String) -> Reply<ShareLink> {
    reply(async {
        let address = address.trim().parse().context("Enter an IP and port, such as 192.168.1.25:24842.")?;
        let guard = app.gate(id)?;
        let (invite, listener) = peer_share::Invite::listen(address).await?;
        let code = invite.code()?;
        let job = app.0.jobs.start(None, "Share instance directly".into());
        let job_id = job.id;
        let app = app.inner().clone();
        tauri::async_runtime::spawn(async move {
            let result: Result<()> = async {
                job.stage("Packing the instance", 0);
                let work = app.local(move |store| {
                    let _guard = guard;
                    let work = tempfile::tempdir_in(store.root.join("cache"))?;
                    packs::export_peer(&store, id, &work.path().join("instance.zip"))?;
                    Ok(work)
                }).await?;
                job.check()?;
                let archive = work.path().join("instance.zip");
                tokio::select! {
                    _ = job.cancel.cancelled() => bail!("Share cancelled."),
                    result = tokio::time::timeout(Duration::from_secs(2700), peer_share::serve(invite, listener, &archive, &job)) => result.context("Share timed out.")??,
                }
                job.stage("Instance sent", 0);
                Ok(())
            }.await;
            job.finish(&result);
        });
        Ok(ShareLink { code, job_id })
    }.await)
}

#[tauri::command]
fn receive_instance(app: State<'_, App>, code: String) -> Reply<Uuid> {
    reply((|| {
        let invite = peer_share::Invite::parse(&code)?;
        let job = app.0.jobs.start(None, "Receive shared instance".into());
        let job_id = job.id;
        let app = app.inner().clone();
        tauri::async_runtime::spawn(async move {
            let result: Result<()> = async {
                let work = tempfile::tempdir_in(app.0.store.root.join("cache"))?;
                let archive = work.path().join("instance.zip");
                tokio::select! {
                    _ = job.cancel.cancelled() => bail!("Transfer cancelled."),
                    result = tokio::time::timeout(Duration::from_secs(1800), peer_share::receive(invite, &archive, &job)) => result.context("Transfer timed out.")??,
                }
                job.check()?;
                packs::import(&app.0.client, &app.0.store, archive, &job).await
            }.await;
            job.finish(&result);
        });
        Ok(job_id)
    })())
}

#[tauri::command]
async fn overview(app: State<'_, App>) -> Reply<Overview> {
    reply(
        async {
            let (settings, instances, data_dir) = app
                .local(|store| Ok((store.settings()?, store.instances()?, store.root)))
                .await?;
            Ok(Overview {
                microsoft_configured: auth::launcher_client_id().is_ok(),
                test_launch_available: cfg!(feature = "dev-test-launch"),
                total_memory_mib: tokio::task::spawn_blocking(|| {
                    let mut system = sysinfo::System::new();
                    system.refresh_memory();
                    system.total_memory() / (1024 * 1024)
                })
                .await?,
                settings,
                instances,
                data_dir,
                accounts: app.inner().0.auth.accounts().await?,
            })
        }
        .await,
    )
}

#[tauri::command]
async fn list_instances(app: State<'_, App>) -> Reply<Vec<InstanceView>> {
    reply(app.local(|store| store.instances()).await)
}

#[tauri::command]
async fn save_settings(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    settings: Settings,
) -> Reply<Settings> {
    reply(
        async {
            let settings = app
                .local(move |store| {
                    store.save_settings(&settings)?;
                    Ok(settings)
                })
                .await?;
            apply_ui_scale(&handle, settings.ui_scale);
            app.0.discord.configure(settings.discord_presence, settings.discord_show_pack);
            Ok(settings)
        }
        .await,
    )
}

fn apply_ui_scale(handle: &tauri::AppHandle, scale: u16) {
    if let Some(webview) = handle.get_webview_window("main") {
        if let Err(error) = webview.set_zoom(f64::from(scale.clamp(90, 150)) / 100.0) {
            tracing::warn!(%error, "Could not apply the UI scale");
        }
    }
}

#[tauri::command]
fn discord_presence_status(app: State<'_, App>) -> discord::Status {
    app.0.discord.status()
}

#[tauri::command]
async fn minecraft_catalog(app: State<'_, App>, refresh: bool) -> Reply<minecraft::Catalog> {
    reply(minecraft::catalog(&app.inner().0.client, &app.inner().0.store, refresh).await)
}

#[tauri::command]
async fn detect_java(app: State<'_, App>) -> Reply<Vec<minecraft::java::JavaVersion>> {
    reply(minecraft::java::detect(&app.inner().0.store).await)
}

#[tauri::command]
async fn inspect_java(path: PathBuf) -> Reply<minecraft::java::JavaVersion> {
    reply(minecraft::java::inspect(path).await)
}

#[tauri::command]
async fn required_java(app: State<'_, App>, game: String) -> Reply<u32> {
    let service = &app.inner().0;
    reply(minecraft::required_java(&service.client, &service.store, &game).await)
}

#[tauri::command]
async fn loader_versions(
    app: State<'_, App>,
    loader: storage::Loader,
    game: String,
) -> Reply<Vec<String>> {
    reply(minecraft::loader_versions(&app.inner().0.client, loader, game.trim()).await)
}

#[tauri::command]
async fn available_java(app: State<'_, App>) -> Reply<Vec<u32>> {
    reply(minecraft::java::available(&app.inner().0.client).await)
}

#[tauri::command]
async fn install_java(app: State<'_, App>, major: u32) -> Reply<Uuid> {
    let app = app.inner().clone();
    let job = app.0.jobs.start(None, format!("Install Java {major}"));
    let id = job.id;
    tauri::async_runtime::spawn(async move {
        let result = async {
            let _gate = tokio::select! {
                _ = job.cancel.cancelled() => bail!("Java installation cancelled."),
                gate = app.0.java_gate.lock() => gate,
            };
            minecraft::java::install(&app.0.client, &app.0.store, major, &job).await?;
            job.stage(&format!("Java {major} is ready"), 0);
            Ok(())
        }
        .await;
        job.finish(&result);
    });
    Ok(id)
}

#[tauri::command]
async fn create_instance(app: State<'_, App>, options: InstanceOptions) -> Reply<InstanceView> {
    reply(async {
        minecraft::validate_loader(&app.inner().0.client, options.loader, &options.minecraft_version,
            options.loader_version.as_deref()).await?;
        app.local(move |store| store.create(options)).await
    }.await)
}

#[tauri::command]
async fn update_instance(
    app: State<'_, App>,
    id: Uuid,
    options: InstanceOptions,
) -> Reply<InstanceView> {
    reply(async {
        let guard = app.gate(id)?;
        let current = app.local(move |store| store.instance(id)).await?.options;
        if current.loader != options.loader || current.minecraft_version != options.minecraft_version
            || current.loader_version != options.loader_version {
            minecraft::validate_loader(&app.inner().0.client, options.loader, &options.minecraft_version,
                options.loader_version.as_deref()).await?;
        }
        app.local(move |store| {
            let _guard = guard;
            store.update(id, options)
        }).await
    }.await)
}

#[tauri::command]
async fn duplicate_instance(app: State<'_, App>, id: Uuid) -> Reply<InstanceView> {
    reply(app.change(id, move |store| store.duplicate(id)).await)
}

#[tauri::command]
async fn archive_instance(app: State<'_, App>, id: Uuid) -> Reply<Uuid> {
    change_server(&app, id, move |store| store.archive(id)).await
}

#[tauri::command]
async fn backup_instance(app: State<'_, App>, id: Uuid) -> Reply<Uuid> {
    reply(app.change(id, move |store| store.snapshot(id)).await)
}

#[tauri::command]
async fn list_backups(app: State<'_, App>) -> Reply<Vec<Backup>> {
    reply(
        app.local(|store| {
            Ok(store
                .backups()?
                .into_iter()
                .filter(|backup| !backup.content_only)
                .collect())
        })
        .await,
    )
}

#[tauri::command]
async fn list_pack_snapshots(app: State<'_, App>, id: Uuid) -> Reply<Vec<Backup>> {
    reply(app.local(move |store| store.content_snapshots(id)).await)
}

#[tauri::command]
async fn save_pack_snapshot(app: State<'_, App>, id: Uuid) -> Reply<Uuid> {
    reply(
        app.change(id, move |store| store.snapshot_content(id))
            .await,
    )
}

#[tauri::command]
async fn compare_pack_snapshot(
    app: State<'_, App>,
    id: Uuid,
    snapshot: Uuid,
    other: Option<Uuid>,
) -> Reply<Vec<snapshots::Difference>> {
    reply(
        app.local(move |store| store.snapshot_difference(id, snapshot, other))
            .await,
    )
}

#[tauri::command]
async fn preview_snapshot_cleanup(app: State<'_, App>, id: Uuid) -> Reply<Vec<Backup>> {
    reply(app.local(move |store| store.snapshot_cleanup(id)).await)
}

#[tauri::command]
async fn cleanup_snapshots(app: State<'_, App>, id: Uuid, snapshots: Vec<Uuid>) -> Reply<()> {
    reply(
        app.change(id, move |store| store.prune_snapshots(id, &snapshots))
            .await,
    )
}

#[tauri::command]
async fn compare_instances(
    app: State<'_, App>,
    id: Uuid,
    source: Uuid,
) -> Reply<Vec<snapshots::Difference>> {
    reply(
        app.local(move |store| snapshots::compare_instances(&store, id, source))
            .await,
    )
}

#[tauri::command]
async fn merge_instances(
    app: State<'_, App>,
    id: Uuid,
    source: Uuid,
    files: Vec<snapshots::MergeFile>,
) -> Reply<()> {
    reply(
        async {
            ensure!(id != source, "Choose another source instance.");
            let _source_guard = app.gate(source)?;
            app.change(id, move |store| {
                snapshots::merge(&store, id, source, &files)
            })
            .await
        }
        .await,
    )
}

#[tauri::command]
async fn inspect_world_risk(
    app: State<'_, App>,
    id: Uuid,
    world: Option<String>,
) -> Reply<snapshots::WorldRisk> {
    reply(
        app.local(move |store| snapshots::world_risk(&store, id, world.as_deref()))
            .await,
    )
}

#[tauri::command]
async fn inspect_modpack_health(app: State<'_, App>, id: Uuid) -> Reply<health::Health> {
    reply(app.local(move |store| health::inspect(&store, id)).await)
}

#[tauri::command]
async fn preview_pack_migration(
    app: State<'_, App>,
    id: Uuid,
    game: String,
) -> Reply<Vec<migration::Entry>> {
    reply(
        migration::preview(
            &app.0.client,
            &app.0.private_client,
            app.0.store.clone(),
            id,
            game,
        )
        .await,
    )
}

#[tauri::command]
async fn find_migration_candidates(
    app: State<'_, App>,
    id: Uuid,
    game: String,
    file: String,
) -> Reply<Vec<Project>> {
    reply(
        migration::candidates(
            &app.0.client,
            &app.0.private_client,
            app.0.store.clone(),
            id,
            game,
            file,
        )
        .await,
    )
}

#[tauri::command]
async fn preview_health_fixes(app: State<'_, App>, id: Uuid) -> Reply<Vec<String>> {
    reply(
        app.local(move |store| health::enable_plan(&store, id))
            .await,
    )
}

#[tauri::command]
async fn apply_health_fixes(app: State<'_, App>, id: Uuid, files: Vec<String>) -> Reply<()> {
    reply(
        app.change(id, move |store| {
            health::apply_enable_plan(&store, id, &files)
        })
        .await,
    )
}

#[tauri::command]
async fn isolation_preview(
    app: State<'_, App>,
    id: Uuid,
    files: Vec<String>,
) -> Reply<std::collections::BTreeSet<String>> {
    reply(
        app.local(move |store| health::isolation_files(&store, id, &files))
            .await,
    )
}

#[tauri::command]
async fn launch_isolation(app: State<'_, App>, id: Uuid, files: Vec<String>) -> Reply<Uuid> {
    reply(test_launch::start_isolation(&app, id, files))
}

#[tauri::command]
async fn restore_backup(app: State<'_, App>, backup_id: Uuid) -> Reply<InstanceView> {
    reply(
        async {
            let id = app
                .local(move |store| store.backup_instance(backup_id))
                .await?;
            app.change(id, move |store| store.restore(backup_id)).await
        }
        .await,
    )
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Folder {
    Data,
    Game,
    Mods,
    ResourcePacks,
    ShaderPacks,
    Screenshots,
    Worlds,
}

#[tauri::command]
async fn open_folder(app: State<'_, App>, id: Option<Uuid>, folder: Folder) -> Reply<()> {
    reply(
        app.local(move |store| {
            if matches!(folder, Folder::Data) {
                return Ok(open::that(&store.root)?);
            }
            let mut path = store.game_dir(id.context("Select an instance first.")?)?;
            match folder {
                Folder::Mods => path.push("mods"),
                Folder::ResourcePacks => path.push("resourcepacks"),
                Folder::ShaderPacks => path.push("shaderpacks"),
                Folder::Screenshots => path.push("screenshots"),
                Folder::Worlds => path.push("saves"),
                Folder::Game | Folder::Data => (),
            };
            fs::create_dir_all(&path)?;
            storage::check_directory(&path)?;
            open::that(path).context("Opening the game folder")
        })
        .await,
    )
}

#[tauri::command]
async fn list_instance_files(
    app: State<'_, App>,
    id: Uuid,
    path: Vec<String>,
) -> Reply<Vec<instance_files::Entry>> {
    reply(
        app.local(move |store| instance_files::list(&store, id, &path))
            .await,
    )
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum FileAction {
    Open,
    Reveal { name: String },
    NewFolder { name: String },
    Rename { name: String, next: String },
    Remove { names: Vec<String> },
}

#[tauri::command]
async fn instance_file_action(
    app: State<'_, App>,
    id: Uuid,
    path: Vec<String>,
    action: FileAction,
) -> Reply<()> {
    reply(match action {
        FileAction::Open => {
            app.local(move |store| instance_files::open_directory(&store, id, &path))
                .await
        }
        FileAction::Reveal { name } => {
            app.local(move |store| instance_files::reveal(&store, id, &path, &name))
                .await
        }
        action => {
            app.change(id, move |store| match action {
                FileAction::NewFolder { name } => {
                    instance_files::create_folder(&store, id, &path, &name)
                }
                FileAction::Rename { name, next } => {
                    instance_files::rename(&store, id, &path, &name, &next)
                }
                FileAction::Remove { names } => instance_files::remove(&store, id, &path, &names),
                _ => unreachable!(),
            })
            .await
        }
    })
}

#[tauri::command]
async fn import_instance_file(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    id: Uuid,
    path: Vec<String>,
) -> Reply<bool> {
    reply(
        async {
            let picker = handle.dialog().file();
            let Some(source) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await?
            else {
                return Ok(false);
            };
            let source = source.into_path()?;
            app.change(id, move |store| {
                instance_files::import_file(&store, id, &path, &source)
            })
            .await?;
            Ok(true)
        }
        .await,
    )
}

#[tauri::command]
async fn sync_instance_files(
    app: State<'_, App>,
    id: Uuid,
    source: Uuid,
    items: Vec<instance_files::SyncItem>,
) -> Reply<usize> {
    reply(
        async {
            ensure!(id != source, "Choose a different source instance.");
            let _source = app.gate(source)?;
            app.change(id, move |store| {
                instance_files::sync(&store, id, source, &items)
            })
            .await
        }
        .await,
    )
}

#[tauri::command]
async fn start_game(
    app: State<'_, App>,
    id: Uuid,
    play: bool,
    target: Option<quick_play::Target>,
) -> Reply<Uuid> {
    reply(app.prepare_instance(id, play, target))
}

#[tauri::command]
async fn start_test_game(app: State<'_, App>, id: Uuid) -> Reply<Uuid> {
    #[cfg(feature = "dev-test-launch")]
    return reply(test_launch::start(&app, id));
    #[cfg(not(feature = "dev-test-launch"))]
    {
        let _ = (app, id);
        Err("This build has Test Play disabled.".into())
    }
}

#[tauri::command]
async fn quick_destinations(app: State<'_, App>) -> Reply<Vec<quick_play::Destination>> {
    reply(app.local(|store| quick_play::destinations(&store)).await)
}

#[tauri::command]
async fn running_games(app: State<'_, App>) -> Reply<Vec<Running>> {
    Ok(app
        .inner()
        .0
        .running
        .lock()
        .expect("running lock")
        .values()
        .map(Running::snapshot)
        .collect())
}

#[tauri::command]
async fn jobs(app: State<'_, App>) -> Reply<Vec<JobView>> {
    Ok(app.inner().0.jobs.list())
}

#[tauri::command]
async fn cancel_job(app: State<'_, App>, id: Uuid) -> Reply<()> {
    reply(app.inner().0.jobs.cancel(id))
}

#[tauri::command]
async fn begin_sign_in(app: State<'_, App>) -> Reply<BrowserSignIn> {
    reply(
        async {
            let client_id = auth::launcher_client_id()?;
            app.inner().0.auth.begin(client_id).await
        }
        .await,
    )
}

#[tauri::command]
async fn open_sign_in(app: State<'_, App>, id: Uuid) -> Reply<()> {
    reply(app.inner().0.auth.open_browser(id).await)
}

#[tauri::command]
async fn complete_sign_in(app: State<'_, App>, id: Uuid) -> Reply<Accounts> {
    reply(app.inner().0.auth.finish(id).await)
}

#[tauri::command]
async fn cancel_sign_in(app: State<'_, App>, id: Uuid) -> Reply<()> {
    app.inner().0.auth.cancel(id).await;
    Ok(())
}

#[tauri::command]
async fn select_account(app: State<'_, App>, id: Uuid) -> Reply<Accounts> {
    reply(app.inner().0.auth.select(id).await)
}

#[tauri::command]
async fn remove_account(app: State<'_, App>, id: Uuid) -> Reply<Accounts> {
    reply(app.inner().0.auth.remove(id).await)
}

#[tauri::command]
async fn curseforge_status() -> Reply<bool> {
    reply(providers::key_status().await)
}

#[tauri::command]
async fn save_curseforge_key(value: String) -> Reply<bool> {
    reply(providers::set_key(value).await)
}

#[tauri::command]
async fn search_mods(
    app: State<'_, App>,
    platform: Platform,
    query: String,
    filters: Filters,
    offset: u32,
) -> Reply<Vec<Project>> {
    reply(
        async {
            ensure!(
                query.chars().count() <= 140 && offset <= 1960 && (1..=40).contains(&filters.limit),
                "Search query or page is out of range."
            );
            if let Some(game) = &filters.game {
                storage::check_id(game)?;
            }
            if let Some(category) = &filters.category {
                storage::check_id(category)?;
            }
            providers::provider(
                platform,
                &app.inner().0.client,
                &app.inner().0.private_client,
            )
            .await?
            .search(query.trim(), &filters, offset)
            .await
        }
        .await,
    )
}

#[tauri::command]
async fn project_details(
    app: State<'_, App>,
    platform: Platform,
    project: String,
) -> Reply<ProjectDetails> {
    reply(
        async {
            providers::provider(platform, &app.0.client, &app.0.private_client)
                .await?
                .details(&project)
                .await
        }
        .await,
    )
}

#[tauri::command]
async fn installed_mods(app: State<'_, App>, id: Uuid) -> Reply<Vec<mods::InstalledMod>> {
    reply(app.local(move |store| mods::list(&store, id)).await)
}

#[tauri::command]
async fn installed_mod_projects(app: State<'_, App>, id: Uuid) -> Reply<Vec<(Platform, String)>> {
    reply(app.local(move |store| mods::installed_projects(&store, id)).await)
}

#[tauri::command]
async fn installed_mod_artwork(
    app: State<'_, App>,
    id: Uuid,
    group: mod_artwork::Group,
    files: Vec<String>,
) -> Reply<HashMap<String, providers::ModrinthArtwork>> {
    reply(mod_artwork::list(&app.0.client, app.0.store.clone(), id, group, files).await)
}

#[tauri::command]
async fn installed_content_icons(
    app: State<'_, App>, id: Uuid, group: mod_artwork::Group, files: Vec<String>,
) -> Reply<HashMap<String, String>> {
    reply(app.local(move |store| mod_artwork::embedded_icons(&store, id, group, files)).await)
}

#[tauri::command]
async fn set_mod_side(
    app: State<'_, App>,
    id: Uuid,
    file: String,
    side: Option<mod_sides::ModSide>,
) -> Reply<()> {
    reply(
        app.change(id, move |store| mods::set_side(&store, id, &file, side))
            .await,
    )
}

#[tauri::command]
async fn export_mod_folders(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    id: Uuid,
) -> Reply<Option<server_export::Export>> {
    reply(
        async {
            let picker = handle
                .dialog()
                .file()
                .set_title("Choose an export location");
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_folder()).await?
            else {
                return Ok(None);
            };
            let destination = path.into_path()?;
            let result = app
                .change(id, move |store| {
                    server_export::copy(&store, id, &destination)
                })
                .await?;
            if let Err(error) = open::that(&result.path) {
                tracing::warn!(%error, "Could not open the server export folder");
            }
            Ok(Some(result))
        }
        .await,
    )
}

#[tauri::command]
async fn install_mod(
    app: State<'_, App>,
    id: Uuid,
    platform: Platform,
    project: String,
) -> Reply<Uuid> {
    Ok(app.queue_catalog(
        Some(id),
        CatalogDownload {
            kind: Kind::Mod,
            platform,
            project,
            world: None,
        },
        None,
    ))
}

#[tauri::command]
async fn update_mods(app: State<'_, App>, id: Uuid, file: Option<String>) -> Reply<Uuid> {
    reply(app.update_mods(id, file))
}

#[tauri::command]
async fn mod_filter_data(
    app: State<'_, App>,
    id: Uuid,
    filter: mods::FilterKind,
) -> Reply<mods::FilterData> {
    reply(
        mods::filter_data(
            &app.0.client,
            &app.0.private_client,
            app.0.store.clone(),
            id,
            filter,
        )
        .await,
    )
}

#[tauri::command]
async fn preview_mod_updates(
    app: State<'_, App>,
    id: Uuid,
    file: Option<String>,
    version: Option<String>,
) -> Reply<mods::UpdatePreview> {
    reply(
        async {
            let instance = app.local(move |store| store.instance(id)).await?;
            let result = mods::preview_updates(
                &app.0.client,
                &app.0.private_client,
                &app.0.store,
                instance,
                file,
                version,
            )
            .await?;
            let token = Uuid::new_v4();
            let view = result.view(token);
            app.0
                .mod_previews
                .lock()
                .expect("mod previews lock")
                .insert(id, (token, result));
            Ok(view)
        }
        .await,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModVersions {
    current: String,
    project: String,
    platform: Platform,
    versions: Vec<providers::ModVersionChoice>,
}

#[tauri::command]
async fn list_mod_update_versions(app: State<'_, App>, id: Uuid, file: String) -> Reply<ModVersions> {
    reply(async {
        let instance = app.local(move |store| store.instance(id)).await?;
        let target = mods::update_target(&app.0.client, &app.0.store, &instance, &file).await?;
        let provider = providers::provider(target.platform, &app.0.client, &app.0.private_client).await?;
        let versions = provider.mod_versions(&target.project, &instance.options).await?;
        Ok(ModVersions { current: target.version, project: target.project, platform: target.platform, versions })
    }.await)
}

#[tauri::command]
async fn mod_version_changelog(app: State<'_, App>, project: String, version: String) -> Reply<String> {
    reply(providers::curseforge_changelog(&app.0.private_client, &project, &version).await)
}

#[tauri::command]
async fn apply_mod_preview(
    app: State<'_, App>,
    id: Uuid,
    token: Uuid,
    test_copy: bool,
) -> Reply<Uuid> {
    reply(
        async {
            let source_guard = app.gate(id)?;
            let preview = {
                let previews = app.0.mod_previews.lock().expect("mod previews lock");
                let (saved_token, plan) = previews
                    .get(&id)
                    .context("Preview these mod updates first.")?;
                ensure!(
                    *saved_token == token && plan.instance_id == id,
                    "This update preview expired. Check again."
                );
                plan.clone()
            };
            ensure!(
                !preview.view(token).changes.is_empty(),
                "There are no updates to apply."
            );
            let check = preview.clone();
            let target = app
                .local(move |store| {
                    check.check(&store, &store.instance(id)?)?;
                    if test_copy {
                        return Ok(store.duplicate_with_suffix(id, " (test)")?.instance.id);
                    }
                    Ok(id)
                })
                .await?;
            let guard = if test_copy {
                let copy_guard = app.gate(target)?;
                drop(source_guard);
                copy_guard
            } else {
                app.0
                    .mod_previews
                    .lock()
                    .expect("mod previews lock")
                    .remove(&id);
                source_guard
            };
            let job = app.0.jobs.start(
                Some(target),
                if test_copy {
                    "Test mod updates".into()
                } else {
                    "Apply mod updates".into()
                },
            );
            let app = app.inner().clone();
            tauri::async_runtime::spawn(async move {
                let _guard = guard;
                let result = async {
                    let (instance, settings) = app
                        .local(move |store| Ok((store.instance(target)?, store.settings()?)))
                        .await?;
                    mods::apply_preview(
                        &app.0.client,
                        &app.0.store,
                        &instance,
                        settings.download_limit,
                        preview,
                        &job,
                    )
                    .await?;
                    job.stage(
                        if test_copy {
                            "Test copy ready"
                        } else {
                            "Mod updates applied"
                        },
                        0,
                    );
                    Ok(())
                }
                .await;
                job.finish(&result);
            });
            Ok(target)
        }
        .await,
    )
}

#[tauri::command]
async fn mod_history(app: State<'_, App>, id: Uuid) -> Reply<Vec<mod_history::RecordView>> {
    reply(app.local(move |store| mods::history(&store, id)).await)
}

#[tauri::command]
async fn undo_mod_change(app: State<'_, App>, id: Uuid, entry: Uuid, change: usize) -> Reply<()> {
    reply(
        app.change(id, move |store| mods::undo(&store, id, entry, change))
            .await,
    )
}

#[tauri::command]
async fn toggle_mod(app: State<'_, App>, id: Uuid, file: String, enabled: bool) -> Reply<Vec<mods::ModToggle>> {
    reply(
        app.change(id, move |store| mods::toggle(&store, id, &file, enabled))
            .await,
    )
}

#[tauri::command]
async fn remove_mod(app: State<'_, App>, id: Uuid, file: String) -> Reply<()> {
    reply(
        app.change(id, move |store| mods::remove(&store, id, &file))
            .await,
    )
}

#[tauri::command]
async fn toggle_selected_mods(app: State<'_, App>, id: Uuid, files: Vec<String>, enabled: bool) -> Reply<Vec<mods::ModToggle>> {
    reply(app.change(id, move |store| mods::toggle_many(&store, id, &files, enabled)).await)
}

#[tauri::command]
async fn remove_selected_content(
    app: State<'_, App>, id: Uuid, mods: Vec<String>, resource_packs: Vec<String>, shaders: Vec<String>,
) -> Reply<()> {
    reply(app.change(id, move |store| {
        let mut packs = content::removal_paths(&store, id, content::Kind::ResourcePacks, &resource_packs)?;
        packs.extend(content::removal_paths(&store, id, content::Kind::Shaders, &shaders)?);
        mods::remove_selected(&store, id, &mods, packs)
    }).await)
}

#[tauri::command]
async fn read_diagnostics(
    app: State<'_, App>,
    id: Uuid,
    kind: String,
) -> Reply<Option<diagnostics::Log>> {
    reply(
        app.local(move |store| {
            let kind = match kind.as_str() {
                "console" => diagnostics::LogKind::Console,
                "game" => diagnostics::LogKind::Game,
                "crash" => diagnostics::LogKind::Crash,
                "test" => diagnostics::LogKind::Test,
                _ => bail!("Choose a diagnostic log."),
            };
            diagnostics::read(&store, id, kind)
        })
        .await,
    )
}

#[tauri::command]
async fn diagnostic_share_summary(app: State<'_, App>, id: Uuid) -> Reply<String> {
    reply(
        app.local(move |store| diagnostics::share_summary(&store, id))
            .await,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Performance {
    cpu_percent: f32,
    memory_bytes: u64,
}

fn sample_process(services: &Services, pid: u32) -> Option<(f32, u64)> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = services.monitor.lock().expect("performance lock");
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory(),
    );
    system
        .process(pid)
        .map(|process| (process.cpu_usage(), process.memory()))
}

#[tauri::command]
async fn session_history(app: State<'_, App>, id: Uuid) -> Reply<Vec<sessions::Summary>> {
    reply(app.local(move |store| sessions::list(&store, id)).await)
}

#[tauri::command]
async fn session_details(app: State<'_, App>, id: Uuid, session: Uuid) -> Reply<sessions::Session> {
    reply(
        app.local(move |store| sessions::detail(&store, id, session))
            .await,
    )
}

#[tauri::command]
async fn game_performance(app: State<'_, App>, id: Uuid) -> Reply<Option<Performance>> {
    let Some(pid) = app
        .inner()
        .0
        .running
        .lock()
        .expect("running lock")
        .get(&id)
        .map(|game| game.pid)
    else {
        return Ok(None);
    };
    let services = app.inner().0.clone();
    reply(
        async {
            Ok(tokio::task::spawn_blocking(move || {
                sample_process(&services, pid).map(|(cpu_percent, memory_bytes)| Performance {
                    cpu_percent,
                    memory_bytes,
                })
            })
            .await?)
        }
        .await,
    )
}

#[tauri::command]
async fn import_pack(handle: tauri::AppHandle, app: State<'_, App>) -> Reply<Option<Uuid>> {
    let picker = handle
        .dialog()
        .file()
        .add_filter("Minecraft modpacks", &["mrpack", "zip"]);
    reply(
        async {
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await?
            else {
                return Ok(None);
            };
            let path = path.into_path()?;
            let app = app.inner().clone();
            let job = app.0.jobs.start(None, "Import modpack".into());
            let id = job.id;
            tauri::async_runtime::spawn(async move {
                let result = packs::import(&app.0.client, &app.0.store, path, &job).await;
                job.finish(&result);
            });
            Ok(Some(id))
        }
        .await,
    )
}

#[tauri::command]
async fn export_pack(handle: tauri::AppHandle, app: State<'_, App>, id: Uuid) -> Reply<bool> {
    let picker = handle
        .dialog()
        .file()
        .add_filter("Voltra modpack", &["zip"])
        .set_file_name("voltra-pack.zip");
    reply(
        async {
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_save_file()).await?
            else {
                return Ok(false);
            };
            let path = path.into_path()?;
            app.change(id, move |store| packs::export(&store, id, &path))
                .await?;
            Ok(true)
        }
        .await,
    )
}

#[tauri::command]
async fn list_content(
    app: State<'_, App>,
    id: Uuid,
    kind: content::Kind,
) -> Reply<Vec<content::Entry>> {
    reply(
        app.local(move |store| content::list(&store, id, kind))
            .await,
    )
}

#[tauri::command]
async fn add_content(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    id: Uuid,
    kind: content::Kind,
) -> Reply<bool> {
    let picker = handle.dialog().file().add_filter("Pack archives", &["zip"]);
    reply(
        async {
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await?
            else {
                return Ok(false);
            };
            let path = path.into_path()?;
            app.change(id, move |store| content::add(&store, id, kind, &path))
                .await?;
            Ok(true)
        }
        .await,
    )
}

#[tauri::command]
async fn remove_content(
    app: State<'_, App>,
    id: Uuid,
    kind: content::Kind,
    name: String,
) -> Reply<()> {
    reply(
        app.change(id, move |store| content::remove(&store, id, kind, &name))
            .await,
    )
}

#[tauri::command]
async fn open_project_url(url: String) -> Reply<()> {
    reply(
        async {
            let url = downloads::https_url(&url)?;
            ensure!(
                matches!(
                    url.host_str(),
                    Some(
                        "modrinth.com"
                            | "www.modrinth.com"
                            | "curseforge.com"
                            | "www.curseforge.com"
                    )
                ),
                "This link is outside the project catalogs."
            );
            tokio::task::spawn_blocking(move || open::that(url.as_str())).await??;
            Ok(())
        }
        .await,
    )
}

#[tauri::command]
async fn install_project(
    app: State<'_, App>,
    id: Option<Uuid>,
    world: Option<String>,
    platform: Platform,
    project: String,
    kind: Kind,
    title: Option<String>,
) -> Reply<Uuid> {
    reply((|| {
        let id = if kind == Kind::Modpack {
            None
        } else {
            Some(id.context("Select an instance first.")?)
        };
        let world = if kind == Kind::Datapack {
            let name = world.context("Select a world for the data pack.")?;
            storage::check_filename(&name)?;
            Some(name)
        } else {
            None
        };
        Ok(app.queue_catalog(
            id,
            CatalogDownload {
                kind,
                platform,
                project,
                world,
            },
            title,
        ))
    })())
}

#[tauri::command]
async fn list_screenshots(app: State<'_, App>, id: Option<Uuid>) -> Reply<Vec<media::Screenshot>> {
    reply(app.local(move |store| media::screenshots(&store, id)).await)
}

#[tauri::command]
async fn read_screenshot(
    app: State<'_, App>,
    id: Uuid,
    name: String,
) -> Reply<tauri::ipc::Response> {
    reply(
        app.local(move |store| {
            media::screenshot_bytes(&store, id, &name).map(tauri::ipc::Response::new)
        })
        .await,
    )
}

#[tauri::command]
async fn open_screenshot(app: State<'_, App>, id: Uuid, name: String) -> Reply<()> {
    reply(
        app.local(move |store| Ok(open::that(media::screenshot_path(&store, id, &name)?)?))
            .await,
    )
}

#[tauri::command]
async fn list_skins(app: State<'_, App>) -> Reply<Vec<media::Skin>> {
    reply(app.local(|store| media::skins(&store)).await)
}

#[tauri::command]
async fn current_account_skin(app: State<'_, App>, account_id: Uuid) -> Reply<Option<auth::AccountSkin>> {
    reply(app.0.auth.current_skin(account_id).await)
}

#[tauri::command]
async fn set_skin_variant(app: State<'_, App>, id: String, variant: media::Variant) -> Reply<media::Skin> {
    reply(app.local(move |store| media::set_skin_variant(&store, &id, variant)).await)
}

#[tauri::command]
async fn read_skin(app: State<'_, App>, id: String) -> Reply<tauri::ipc::Response> {
    reply(
        app.local(move |store| {
            media::skin_bytes(&store, &id).map(|(_, data)| tauri::ipc::Response::new(data))
        })
        .await,
    )
}

#[tauri::command]
async fn import_skin(
    handle: tauri::AppHandle,
    app: State<'_, App>,
    variant: media::Variant,
) -> Reply<Option<media::Skin>> {
    let picker = handle
        .dialog()
        .file()
        .add_filter("Minecraft skin", &["png"]);
    reply(
        async {
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await?
            else {
                return Ok(None);
            };
            let path = path.into_path()?;
            app.local(move |store| media::import_skin(&store, &path, variant))
                .await
                .map(Some)
        }
        .await,
    )
}

#[tauri::command]
async fn apply_skin(app: State<'_, App>, id: String) -> Reply<Accounts> {
    reply(
        async {
            let (skin, data) = app
                .local(move |store| media::skin_bytes(&store, &id))
                .await?;
            app.inner().0.auth.apply_skin(skin, data).await
        }
        .await,
    )
}

#[tauri::command]
async fn list_capes(app: State<'_, App>, account_id: Uuid) -> Reply<Vec<auth::Cape>> {
    reply(app.inner().0.auth.capes(account_id).await)
}

#[tauri::command]
async fn set_cape(
    app: State<'_, App>,
    account_id: Uuid,
    cape_id: Option<Uuid>,
) -> Reply<Vec<auth::Cape>> {
    reply(app.inner().0.auth.set_cape(account_id, cape_id).await)
}

#[tauri::command]
async fn list_worlds(app: State<'_, App>, id: Uuid) -> Reply<Vec<String>> {
    reply(app.local(move |store| media::worlds(&store, id)).await)
}

#[tauri::command]
async fn open_world(app: State<'_, App>, id: Uuid, name: String) -> Reply<()> {
    reply(
        app.local(move |store| Ok(open::that(media::world_dir(&store, id, &name)?)?))
            .await,
    )
}

#[tauri::command]
async fn add_local_mod(handle: tauri::AppHandle, app: State<'_, App>, id: Uuid) -> Reply<bool> {
    let picker = handle.dialog().file().add_filter("Minecraft mod", &["jar"]);
    reply(
        async {
            let Some(path) =
                tokio::task::spawn_blocking(move || picker.blocking_pick_file()).await?
            else {
                return Ok(false);
            };
            let path = path.into_path()?;
            app.change(id, move |store| mods::add_local(&store, id, &path))
                .await?;
            Ok(true)
        }
        .await,
    )
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|handle| {
            let root = handle.path().app_local_data_dir()?;
            let app =
                App::new(root).map_err(|error| std::io::Error::other(format!("{error:#}")))?;
            let settings = app.0.store.settings().unwrap_or_default();
            apply_ui_scale(handle.handle(), settings.ui_scale);
            app.0.discord.configure(settings.discord_presence, settings.discord_show_pack);
            handle.manage(app);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<App>();
                if let Some(message) = state.prevent_close() {
                    api.prevent_close();
                    window
                        .dialog()
                        .message(message)
                        .title("Voltra is busy")
                        .show(|_| {});
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            list_instances,
            save_settings,
            discord_presence_status,
            minecraft_catalog,
            detect_java,
            inspect_java,
            required_java,
            loader_versions,
            available_java,
            install_java,
            create_instance,
            update_instance,
            duplicate_instance,
            archive_instance,
            backup_instance,
            list_backups,
            list_pack_snapshots,
            save_pack_snapshot,
            compare_pack_snapshot,
            preview_snapshot_cleanup,
            cleanup_snapshots,
            compare_instances,
            merge_instances,
            inspect_world_risk,
            inspect_modpack_health,
            preview_pack_migration,
            find_migration_candidates,
            preview_health_fixes,
            apply_health_fixes,
            isolation_preview,
            launch_isolation,
            restore_backup,
            open_folder,
            list_instance_files,
            instance_file_action,
            import_instance_file,
            sync_instance_files,
            start_game,
            start_test_game,
            quick_destinations,
            running_games,
            jobs,
            cancel_job,
            begin_sign_in,
            open_sign_in,
            complete_sign_in,
            cancel_sign_in,
            select_account,
            remove_account,
            curseforge_status,
            save_curseforge_key,
            search_mods,
            project_details,
            open_project_url,
            install_project,
            installed_mods,
            installed_mod_projects,
            installed_mod_artwork,
            installed_content_icons,
            set_mod_side,
            export_mod_folders,
            install_mod,
            update_mods,
            preview_mod_updates,
            list_mod_update_versions,
            mod_version_changelog,
            mod_filter_data,
            apply_mod_preview,
            mod_history,
            undo_mod_change,
            toggle_mod,
            remove_mod,
            toggle_selected_mods,
            remove_selected_content,
            read_diagnostics,
            diagnostic_share_summary,
            game_performance,
            session_history,
            session_details,
            import_pack,
            export_pack,
            start_local_server,
            local_server_status,
            stop_local_server,
            local_server_command,
            local_server_properties,
            save_local_server_properties,
            save_local_server_memory,
            local_server_addons,
            import_local_server_addon,
            change_local_server_addon,
            local_server_players,
            local_server_log,
            prepare_log_share,
            upload_log_share,
            open_log_share,
            open_local_server_folder,
            open_minecraft_eula,
            local_share_address,
            share_instance,
            receive_instance,
            list_content,
            add_content,
            remove_content,
            list_screenshots,
            read_screenshot,
            open_screenshot,
            list_skins,
            current_account_skin,
            set_skin_variant,
            read_skin,
            import_skin,
            apply_skin,
            list_capes,
            set_cape,
            list_worlds,
            open_world,
            add_local_mod,
        ])
        .run(tauri::generate_context!())
        .expect("Voltra could not start");
}

#[cfg(test)]
mod catalog_queue_tests {
    use super::*;

    #[tokio::test]
    async fn waits_for_instance_and_cancels_queued_projects() -> Result<()> {
        let root = tempfile::tempdir()?;
        let app = App::new(root.path().to_path_buf())?;
        let instance_id = Uuid::new_v4();
        let guard = app.gate(instance_id)?;
        let queue = |project: &str| {
            app.queue_catalog(
                Some(instance_id),
                CatalogDownload {
                    kind: Kind::Mod,
                    platform: Platform::Modrinth,
                    project: project.into(),
                    world: None,
                },
                Some(project.into()),
            )
        };
        let first = queue("first");
        let second = queue("second");
        assert_eq!(queue("first"), first);
        assert_ne!(second, first);
        let status = |id: Uuid| -> Result<String> {
            let jobs = serde_json::to_value(app.0.jobs.list())?;
            jobs.as_array()
                .and_then(|jobs| jobs.iter().find(|job| job["id"] == id.to_string()))
                .and_then(|job| job["status"].as_str())
                .map(str::to_owned)
                .context("Missing download status")
        };
        assert_eq!(status(first)?, "queued");
        assert_eq!(status(second)?, "queued");
        assert!(app.0.jobs.has_active());
        app.0.jobs.cancel(first)?;
        assert_eq!(status(first)?, "cancelled");
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while app.0.jobs.has_active() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await?;
        assert_eq!(status(first)?, "cancelled");
        // The second request reached the installer, which found no instance at this ID.
        assert_eq!(status(second)?, "failed");
        Ok(())
    }
}
