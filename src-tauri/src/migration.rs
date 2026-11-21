use anyhow::{ensure, Context, Result};
use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    mods,
    providers::{self, Filters, Kind, Platform, Project, Sort},
    storage::{self, Store},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    file: String,
    name: String,
    version: Option<String>,
    state: &'static str,
    reason: Option<String>,
}

pub async fn preview(
    client: &Client,
    private: &Client,
    store: Store,
    id: Uuid,
    game: String,
) -> Result<Vec<Entry>> {
    storage::check_id(&game)?;
    let (mut options, mods) = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>((store.instance(id)?.options, mods::list(&store, id)?))
    })
    .await??;
    options.minecraft_version = game;
    options.loader_version = None;
    options.validate()?;
    let mut entries = Vec::new();
    for item in mods.iter().filter(|item| item.managed.is_none()) {
        entries.push(Entry {
            file: item.file.clone(),
            name: item.file.clone(),
            version: None,
            state: "manual",
            reason: Some("This file has no catalog identity.".into()),
        });
    }
    for platform in [Platform::Modrinth, Platform::Curseforge] {
        let items = mods
            .iter()
            .filter_map(|item| item.managed.as_ref())
            .filter(|item| item.platform == platform)
            .map(|item| (item.project.clone(), item.file.clone(), item.title.clone()))
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }
        let provider = match providers::provider(platform, client, private).await {
            Ok(provider) => provider,
            Err(error) => {
                entries.extend(items.into_iter().map(|(_, file, name)| Entry {
                    file,
                    name,
                    version: None,
                    state: "unknown",
                    reason: Some(error.to_string()),
                }));
                continue;
            }
        };
        let target = &options;
        let provider = provider.as_ref();
        let checked: Vec<_> = stream::iter(items)
            .map(|(id, file, name)| async move {
                let result = provider
                    .file(Some(&id), None, target, Some(target.update_channel))
                    .await;
                match result {
                    Ok(Some(version)) => Entry {
                        file,
                        name,
                        version: version.display_version.or(Some(version.version)),
                        state: "available",
                        reason: None,
                    },
                    Ok(None) => Entry {
                        file,
                        name,
                        version: None,
                        state: "unavailable",
                        reason: Some(
                            "No compatible version in the selected update channel.".into(),
                        ),
                    },
                    Err(error) => Entry {
                        file,
                        name,
                        version: None,
                        state: "unknown",
                        reason: Some(error.to_string()),
                    },
                }
            })
            .buffer_unordered(6)
            .collect()
            .await;
        entries.extend(checked);
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

pub async fn candidates(
    client: &Client,
    private: &Client,
    store: Store,
    id: Uuid,
    game: String,
    file: String,
) -> Result<Vec<Project>> {
    storage::check_id(&game)?;
    storage::check_filename(&file)?;
    let (mut options, item) = tokio::task::spawn_blocking(move || {
        let options = store.instance(id)?.options;
        let item = mods::list(&store, id)?
            .into_iter()
            .find(|entry| entry.file == file)
            .context("Mod is no longer installed.")?;
        Ok::<_, anyhow::Error>((options, item))
    })
    .await??;
    options.minecraft_version = game;
    options.loader_version = None;
    options.validate()?;
    ensure!(
        options.loader != storage::Loader::Vanilla,
        "Choose a modded instance."
    );
    let name = item
        .managed
        .as_ref()
        .map_or(item.file.as_str(), |item| item.title.as_str());
    let mut results = Vec::new();
    let mut last_error = None;
    let mut checked = false;
    for platform in [Platform::Modrinth, Platform::Curseforge] {
        let query = async {
            let provider = providers::provider(platform, client, private).await?;
            let filters = Filters {
                kind: Kind::Mod,
                game: Some(options.minecraft_version.clone()),
                loader: Some(options.loader),
                category: None,
                sort: Sort::Relevance,
                limit: 8,
            };
            let projects = provider.search(name, &filters, 0).await?;
            let mut verified = Vec::new();
            for project in projects.into_iter().filter(|project| {
                item.managed
                    .as_ref()
                    .is_none_or(|item| item.platform != platform || item.project != project.id)
            }) {
                if provider
                    .file(
                        Some(&project.id),
                        None,
                        &options,
                        Some(options.update_channel),
                    )
                    .await
                    .is_ok_and(|file| file.is_some())
                {
                    verified.push(project);
                }
            }
            Ok::<_, anyhow::Error>(verified)
        }
        .await;
        match query {
            Ok(projects) => {
                checked = true;
                results.extend(projects);
            }
            Err(error) => last_error = Some(error),
        }
    }
    if !checked {
        return Err(last_error.context("Could not search the catalogs.")?);
    }
    Ok(results)
}
