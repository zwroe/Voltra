use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{mpsc, OwnedMutexGuard},
    time::timeout,
};
use uuid::Uuid;

use crate::{
    capture_console,
    downloads::Job,
    minecraft,
    mod_sides::ModSide,
    mods, server_workspace,
    storage::{self, InstanceOptions, Loader, Store},
    App,
};

pub struct RunningServer {
    pub job: Job,
    pub commands: mpsc::Sender<String>,
    pub pid: Arc<AtomicU32>,
}

#[derive(Serialize, Deserialize)]
struct ServerConfig {
    options: InstanceOptions,
    port: u16,
    #[serde(default)]
    ram_mib: Option<u32>,
    skipped: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    initialized: bool,
    port: Option<u16>,
    ram_mib: Option<u32>,
    minecraft_version: Option<String>,
    loader: Option<Loader>,
    skipped: Vec<String>,
    log: String,
    pub job_id: Option<Uuid>,
    pub process_started: bool,
    pub stopping: bool,
    pub metrics: Option<crate::Performance>,
}

pub fn directory(store: &Store, id: Uuid) -> PathBuf {
    store.root.join("servers").join(id.to_string())
}

fn config_at(path: &Path) -> Result<Option<ServerConfig>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    storage::check_directory(path)?;
    let file = path.join("voltra-server.json");
    storage::regular_file(&file)?;
    Ok(Some(storage::read_json(&file)?))
}

pub fn status(store: &Store, id: Uuid) -> Result<Status> {
    store.instance(id)?;
    let directory = directory(store, id);
    let config = config_at(&directory)?;
    let log = directory.join("voltra-console.log");
    let log = if storage::regular_file(&log).is_ok() {
        let mut file = fs::File::open(log)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(16_000)))?;
        let mut buf = Vec::new();
        file.take(16_000).read_to_end(&mut buf)?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    };
    Ok(Status {
        initialized: config.is_some(),
        port: config
            .as_ref()
            .map(|item| server_workspace::port(&directory).unwrap_or(item.port)),
        ram_mib: config.as_ref().and_then(|item| item.ram_mib),
        minecraft_version: config
            .as_ref()
            .map(|item| item.options.minecraft_version.clone()),
        loader: config.as_ref().map(|item| item.options.loader),
        skipped: config.map_or_else(Vec::new, |item| item.skipped),
        log,
        job_id: None,
        process_started: false,
        stopping: false,
        metrics: None,
    })
}

pub fn save_memory(store: &Store, id: Uuid, ram_mib: u32) -> Result<()> {
    ensure!(
        (512..=65536).contains(&ram_mib),
        "Enter 512–65536 MiB of memory."
    );
    store.instance(id)?;
    let path = directory(store, id);
    let mut config = config_at(&path)?.context("Start the server once to create its workspace.")?;
    config.ram_mib = Some(ram_mib);
    storage::atomic_json(&path.join("voltra-server.json"), &config)
}

fn initialize(
    store: &Store,
    id: Uuid,
    options: InstanceOptions,
    port: u16,
    ram: u32,
    job: &Job,
) -> Result<ServerConfig> {
    let destination = directory(store, id);
    if let Some(config) = config_at(&destination)? {
        return Ok(config);
    }
    let parent = store.root.join("servers");
    storage::check_directory(&parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let path = staging.path();
    fs::create_dir(path.join("mods"))?;
    let game = store.game_dir(id)?;
    let mut skipped = Vec::new();
    for item in mods::list(store, id)? {
        job.check()?;
        if !item.enabled {
            continue;
        }
        if item.side == ModSide::Unknown {
            skipped.push(item.file);
            continue;
        }
        if item.side.folders().contains(&"server") {
            let source = game.join("mods").join(&item.file);
            storage::regular_file(&source)?;
            fs::copy(source, path.join("mods").join(item.file))?;
        }
    }
    for name in [
        "config",
        "defaultconfigs",
        "kubejs",
        "scripts",
        "global_packs",
        "openloader",
    ] {
        job.check()?;
        let source = game.join(name);
        if source.try_exists()? {
            storage::copy_tree(&source, &path.join(name))?;
        }
    }
    fs::write(
        path.join("server.properties"),
        format!("server-port={port}\nonline-mode=true\nwhite-list=true\nenforce-whitelist=true\n"),
    )?;
    fs::write(path.join("eula.txt"), "eula=true\n")?;
    let config = ServerConfig {
        options,
        port,
        ram_mib: Some(ram),
        skipped,
    };
    storage::atomic_json(&path.join("voltra-server.json"), &config)?;
    job.check()?;
    fs::rename(path, destination).context("Saving the separate server folder")?;
    Ok(config)
}

pub fn start(app: &App, id: Uuid, port: u16, ram: u32, eula: bool) -> Result<Uuid> {
    ensure!(
        eula,
        "Accept the Minecraft EULA before starting the server."
    );
    ensure!(
        port != 0 && (512..=65536).contains(&ram),
        "Enter a valid port and 512–65536 MiB of memory."
    );
    let mut running = app.0.servers.lock().expect("server lock");
    ensure!(
        !running.contains_key(&id),
        "This server is already starting or running."
    );
    let guard = app.gate(id)?;
    let job = app.0.jobs.start(Some(id), "Minecraft server".into());
    let (commands, inbox) = mpsc::channel(12);
    let pid = Arc::new(AtomicU32::new(0));
    running.insert(
        id,
        RunningServer {
            job: job.clone(),
            commands,
            pid: pid.clone(),
        },
    );
    let job_id = job.id;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = run(&app, id, port, ram, &job, inbox, guard, pid).await;
        app.0.servers.lock().expect("server lock").remove(&id);
        job.finish(&result);
    });
    Ok(job_id)
}

async fn run(
    app: &App,
    id: Uuid,
    port: u16,
    ram: u32,
    job: &Job,
    mut inbox: mpsc::Receiver<String>,
    guard: OwnedMutexGuard<()>,
    pid: Arc<AtomicU32>,
) -> Result<()> {
    let client = &app.0.client;
    let store = &app.0.store;
    let target = directory(store, id);
    let (instance, existing, settings) = app
        .local(move |store| {
            Ok((
                store.instance(id)?,
                config_at(&directory(&store, id))?,
                store.settings()?,
            ))
        })
        .await?;
    let config = if let Some(config) = existing {
        config
    } else {
        let mut options = instance.options;
        ensure!(
            matches!(options.loader, Loader::Vanilla | Loader::Fabric),
            "Local hosting currently supports Vanilla and Fabric."
        );
        if options.loader == Loader::Fabric && options.loader_version.is_none() {
            options.loader_version =
                minecraft::loader_versions(client, Loader::Fabric, &options.minecraft_version)
                    .await?
                    .into_iter()
                    .next();
        }
        options.launch = Default::default();
        options.jvm_args.clear();
        job.stage("Copying server-compatible mods and configuration", 0);
        let job = job.clone();
        app.local(move |store| initialize(&store, id, options, port, ram, &job))
            .await?
    };
    drop(guard);
    let ram = config.ram_mib.unwrap_or(ram);
    let options = config.options;
    let setup = async {
        let major = minecraft::required_java(client, store, &options.minecraft_version).await?;
        let _java_gate = app.0.java_gate.lock().await;
        let java = minecraft::java::resolve(client, store, &settings.java_path, major, job).await?;
        drop(_java_gate);
        job.stage("Getting the dedicated server launcher", 0);
        let jar = minecraft::prepare_server(client, store, &options, &target, job).await?;
        Ok::<_, anyhow::Error>((java, jar))
    };
    let (java, jar) =
        tokio::select! { result = setup => result?, _ = job.cancel.cancelled() => return Ok(()) };
    job.check()?;
    let log = target.join("voltra-console.log");
    if log.try_exists()? {
        storage::regular_file(&log)?;
    }
    let output = fs::File::create(log)?;
    let mut command = Command::new(java);
    command
        .arg(format!("-Xmx{ram}M"))
        .arg("-jar")
        .arg(jar)
        .arg("nogui")
        .current_dir(&target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Starting the dedicated Minecraft server")?;
    let mut stdin = child
        .stdin
        .take()
        .context("The server console is unavailable.")?;
    let logs = capture_console(&mut child, output, None)?;
    pid.store(
        child
            .id()
            .context("The Minecraft server has no process ID.")?,
        Ordering::Release,
    );
    job.stage("Server process running", 0);
    job.finish(&Ok(()));
    let status = loop {
        tokio::select! {
            result = child.wait() => break result?,
            Some(command) = inbox.recv() => {
                let _ = timeout(Duration::from_secs(2), stdin.write_all(format!("{command}\n").as_bytes())).await;
            }
            _ = job.cancel.cancelled() => {
                job.stage("Saving the server world and stopping", 0);
                let _ = timeout(Duration::from_secs(2), stdin.write_all(b"stop\n")).await;
                // Wait for Java to finish saving the world.
                break child.wait().await?;
            }
        }
    };
    pid.store(0, Ordering::Release);
    for logger in logs {
        let _ = logger.await;
    }
    ensure!(
        status.success() || job.cancel.is_cancelled(),
        "The server exited with {status}. Check the server console."
    );
    job.stage("Minecraft server stopped", 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn server_copy_preserves_a_separate_world_and_selects_compatible_mods() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(temp.path().to_owned())?;
        let instance = store.create(serde_json::from_value(serde_json::json!({
            "name": "Server fixture", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096
        }))?)?.instance;
        let game = store.game_dir(instance.id)?;
        for directory in ["mods", "saves/example", "config"] {
            fs::create_dir_all(game.join(directory))?;
        }
        fs::write(game.join("saves/example/level.dat"), "client world")?;
        fs::write(game.join("config/sample.toml"), "enabled = true")?;
        for (file, metadata) in [
            ("shared.jar", r#"{"environment":"*"}"#),
            ("client.jar", r#"{"environment":"client"}"#),
            ("unknown.jar", "{}"),
        ] {
            let mut jar = zip::ZipWriter::new(fs::File::create(game.join("mods").join(file))?);
            jar.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())?;
            jar.write_all(metadata.as_bytes())?;
            jar.finish()?;
        }
        let job = crate::downloads::Jobs::default().start(None, "Fixture".into());
        let previous = instance.options.clone();
        let initial = initialize(&store, instance.id, instance.options, 25566, 4096, &job)?;
        let server = directory(&store, instance.id);
        assert_eq!(initial.skipped, ["unknown.jar"]);
        assert!(server.join("mods/shared.jar").is_file());
        assert!(!server.join("mods/client.jar").exists());
        assert!(!server.join("saves").exists());
        assert_eq!(
            fs::read_to_string(server.join("config/sample.toml"))?,
            "enabled = true"
        );
        assert!(fs::read_to_string(server.join("server.properties"))?.contains("white-list=true"));
        fs::create_dir(server.join("world"))?;
        fs::write(server.join("world/level.dat"), "server world")?;
        assert_eq!(
            initialize(&store, instance.id, previous, 25567, 4096, &job)?.port,
            25566
        );
        assert_eq!(
            fs::read_to_string(server.join("world/level.dat"))?,
            "server world"
        );
        Ok(())
    }
}
