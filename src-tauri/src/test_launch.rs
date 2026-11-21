use std::{collections::BTreeSet, fs, io, path::Path};

use anyhow::{ensure, Context, Result};
use tempfile::TempDir;
use uuid::Uuid;

use crate::{
    capture_console, minecraft,
    storage::{self, Store},
    App, Job, Running,
};

#[cfg(feature = "dev-test-launch")]
pub fn start(app: &App, id: Uuid) -> Result<Uuid> {
    start_trial(app, id, None)
}

pub fn start_isolation(app: &App, id: Uuid, chosen: Vec<String>) -> Result<Uuid> {
    ensure!(
        chosen.len() <= 2000,
        "Choose up to 2,000 mods for one trial."
    );
    start_trial(app, id, Some(chosen))
}

fn start_trial(app: &App, id: Uuid, chosen: Option<Vec<String>>) -> Result<Uuid> {
    let guard = app.gate(id)?;
    let app = app.clone();
    let title = if chosen.is_some() {
        "Mod isolation trial"
    } else {
        "Test Play"
    };
    let job = app.0.jobs.start(Some(id), title.into());
    let job_id = job.id;
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let result = run(&app, id, &job, chosen).await;
        job.finish(&result);
    });
    Ok(job_id)
}

async fn run(app: &App, id: Uuid, job: &Job, chosen: Option<Vec<String>>) -> Result<()> {
    let isolation = chosen.is_some();
    job.stage("Copying modpack files for the test", 0);
    let (scratch, mut instance, mut settings, log_path, output) = app
        .local({
            let job = job.clone();
            move |store| {
                let instance = store.instance(id)?;
                let settings = store.settings()?;
                let selected = chosen
                    .as_deref()
                    .map(|files| crate::health::isolation_files(&store, id, files))
                    .transpose()?;
                let scratch = scratch_game(&store, id, &job, selected.as_ref())?;
                let log_path = store.root.join("logs").join(format!("{id}-test.log"));
                let log = tempfile::NamedTempFile::new_in(store.root.join("logs"))?
                    .persist(&log_path)
                    .map_err(|error| error.error)?;
                Ok((scratch, instance, settings, log_path, log))
            }
        })
        .await?;
    let game = scratch.path().to_path_buf();
    instance.options.launch.hooks = Default::default();
    instance.options.launch.environment = None;
    instance.options.jvm_args.clear();
    settings.hooks = Default::default();
    settings.environment.clear();
    settings.jvm_args.clear();
    #[cfg(feature = "dev-test-launch")]
    let signed_in = isolation && app.0.auth.accounts().await?.active_id.is_some();
    #[cfg(not(feature = "dev-test-launch"))]
    let signed_in = isolation;
    let session = if signed_in {
        Some(tokio::select! {
            _ = job.cancel.cancelled() => anyhow::bail!("Cancelled."),
            result = app.0.auth.session() => result?,
        })
    } else {
        None
    };
    let token = session.as_ref().map(|session| session.token.clone());
    let prepared = minecraft::prepare(
        &app.0.client,
        &app.0.store,
        &instance,
        game,
        &settings,
        &app.0.java_gate,
        &app.0.loader_gate,
        job,
    )
    .await?;
    job.check()?;
    job.stage("Starting the Minecraft test profile", 0);
    let version = instance.options.minecraft_version.clone();
    let pack = instance.options.name.clone();
    let (mut child, _prepared, _scratch) = tokio::task::spawn_blocking({
        let store = app.0.store.clone();
        move || {
            let identity = match session.as_ref() {
                Some(session) => minecraft::LaunchIdentity::Microsoft(session),
                #[cfg(feature = "dev-test-launch")]
                None => minecraft::LaunchIdentity::LocalTest,
                #[cfg(not(feature = "dev-test-launch"))]
                None => {
                    anyhow::bail!("Connect a Minecraft account before starting an isolation trial.")
                }
            };
            let child = prepared.launch(&store, &instance, &settings, identity, None, None)?;
            Ok::<_, anyhow::Error>((child, prepared, scratch))
        }
    })
    .await??;
    let pid = child.id().context("Test Minecraft process has no ID.")?;
    let consoles = capture_console(&mut child, output, token)?;
    let started_at = storage::now();
    app.0.running.lock().expect("running lock").insert(
        id,
        Running {
            instance_id: id,
            pid,
            started_at,
            playtime_seconds: 0,
            clock: None,
            test: true,
            discord_game: Some(crate::discord::Game {
                version,
                pack,
                started_at,
                test: true,
            }),
        },
    );
    app.refresh_discord();
    job.stage("Minecraft test profile is running", 0);
    let status = tokio::select! {
        status = child.wait() => Some(status),
        _ = job.cancel.cancelled() => {
            if let Err(error) = child.kill().await {
                tracing::warn!(instance = %id, %error, "Could not stop the Minecraft test process");
            }
            None
        },
    };
    if let Some(game) = app.0.running.lock().expect("running lock").get_mut(&id) {
        game.discord_game = None;
    }
    app.refresh_discord();
    drop(child);
    for logger in consoles {
        if let Ok(Err(error)) = logger.await {
            tracing::warn!(instance = %id, %error, "Could not write the Minecraft test log");
        }
    }
    app.0.running.lock().expect("running lock").remove(&id);
    job.check()?;
    let status = status.context("The Minecraft test was cancelled.")??;
    ensure!(
        status.success(),
        "Test Minecraft exited with {status}. Log: {}",
        log_path.display()
    );
    job.stage("Minecraft test profile closed", 0);
    Ok(())
}

fn scratch_game(
    store: &Store,
    id: Uuid,
    job: &Job,
    selected: Option<&BTreeSet<String>>,
) -> Result<TempDir> {
    let source = store.game_dir(id)?;
    let scratch = tempfile::Builder::new()
        .prefix("test-game-")
        .tempdir_in(store.root.join("cache"))?;
    let folders = [
        "mods",
        "config",
        "defaultconfigs",
        "resourcepacks",
        "shaderpacks",
        "resources",
        "datapacks",
        "kubejs",
        "scripts",
        "global_packs",
        "openloader",
        "patchouli_books",
    ];
    for name in folders {
        job.check()?;
        let from = source.join(name);
        if name == "mods" && selected.is_some() {
            if let Some(files) = selected {
                let target = scratch.path().join(name);
                fs::create_dir(&target)?;
                for file in files {
                    job.check()?;
                    storage::regular_file(&from.join(file))?;
                    fs::copy(from.join(file), target.join(file))?;
                }
            }
        } else if exists(&from)? {
            storage::copy_tree(&from, &scratch.path().join(name))?;
        }
    }
    for name in ["options.txt", "optionsof.txt", "optionsshaders.txt"] {
        let from = source.join(name);
        if exists(&from)? {
            storage::regular_file(&from)?;
            fs::copy(from, scratch.path().join(name))?;
        }
    }
    Ok(scratch)
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::Jobs;

    #[test]
    fn test_game_copies_mods_and_settings_without_existing_worlds() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::new(root.path().to_owned())?;
        let instance = store.create(serde_json::from_value(serde_json::json!({
            "name": "Example pack", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096,
        }))?)?.instance;
        let game = store.game_dir(instance.id)?;
        for folder in ["mods", "config", "saves/world", "kubejs/server_scripts"] {
            fs::create_dir_all(game.join(folder))?;
        }
        for file in [
            "mods/demo.jar",
            "config/demo.toml",
            "saves/world/level.dat",
            "kubejs/server_scripts/script.js",
            "servers.dat",
            "options.txt",
        ] {
            fs::write(game.join(file), b"fixture")?;
        }
        let job = Jobs::default().start(Some(instance.id), "Fixture".into());
        let scratch = scratch_game(&store, instance.id, &job, None)?;
        for file in [
            "mods/demo.jar",
            "config/demo.toml",
            "kubejs/server_scripts/script.js",
            "options.txt",
        ] {
            assert_eq!(fs::read(scratch.path().join(file))?, b"fixture");
        }
        assert!(!scratch.path().join("saves").exists());
        assert!(!scratch.path().join("servers.dat").exists());
        fs::write(scratch.path().join("config/demo.toml"), b"changed")?;
        assert_eq!(fs::read(game.join("config/demo.toml"))?, b"fixture");
        let scratch_path = scratch.path().to_owned();
        drop(scratch);
        assert!(!scratch_path.exists());
        assert_eq!(store.instances()?.len(), 1);
        Ok(())
    }
}
