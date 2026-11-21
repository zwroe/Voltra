use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use reqwest::{Client, RequestBuilder};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;

use crate::{
    downloads::Hash,
    mod_sides::ModSide,
    storage::{check_id, InstanceOptions, Loader, UpdateChannel},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Modrinth,
    Curseforge,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modrinth => "Modrinth",
            Self::Curseforge => "CurseForge",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub platform: Platform,
    pub icon_url: Option<String>,
    pub categories: Vec<String>,
    pub updated_at: Option<String>,
    pub url: Option<String>,
    pub can_download: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetails {
    summary: String,
    body: Option<String>,
    description_unavailable: bool,
    gallery: Vec<GalleryImage>,
    url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GalleryImage {
    url: String,
    thumbnail_url: String,
    title: Option<String>,
    description: Option<String>,
}

fn gallery_url(platform: Platform, raw: &str) -> Option<String> {
    let url = crate::downloads::https_url(raw).ok()?;
    let allowed = match platform {
        Platform::Modrinth => url.host_str() == Some("cdn.modrinth.com"),
        Platform::Curseforge => matches!(
            url.host_str(),
            Some("media.forgecdn.net" | "mediafilez.forgecdn.net")
        ),
    };
    (allowed && url.port().is_none()).then(|| url.to_string())
}

#[derive(Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Mod,
    Modpack,
    Resourcepack,
    Shader,
    Datapack,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Modpack => "modpack",
            Self::Resourcepack => "resourcepack",
            Self::Shader => "shader",
            Self::Datapack => "datapack",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Mod => "6",
            Self::Modpack => "4471",
            Self::Resourcepack => "12",
            Self::Shader => "6552",
            Self::Datapack => "6945",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Mod => ".jar",
            Self::Modpack => ".mrpack",
            _ => ".zip",
        }
    }

    pub fn folder(self) -> Option<&'static str> {
        match self {
            Self::Resourcepack => Some("resourcepacks"),
            Self::Shader => Some("shaderpacks"),
            Self::Datapack => Some("datapacks"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Relevance,
    Downloads,
    Updated,
    Newest,
}

impl Sort {
    fn modrinth(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Downloads => "downloads",
            Self::Updated => "updated",
            Self::Newest => "newest",
        }
    }
    fn curseforge(self) -> &'static str {
        match self {
            Self::Relevance => "1",
            Self::Downloads => "6",
            Self::Updated => "3",
            Self::Newest => "11",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    pub kind: Kind,
    pub game: Option<String>,
    pub loader: Option<Loader>,
    pub category: Option<String>,
    pub sort: Sort,
    pub limit: u32,
}

pub struct ContentFile {
    pub filename: String,
    pub url: String,
    pub hash: Hash,
    pub size: Option<u64>,
}

#[derive(Clone)]
pub struct ModFile {
    pub project: String,
    pub version: String,
    pub title: String,
    pub icon_url: Option<String>,
    pub display_version: Option<String>,
    pub filename: String,
    pub url: String,
    pub hash: Hash,
    pub size: Option<u64>,
    pub dependencies: Vec<Dependency>,
    pub side: crate::mod_sides::ModSide,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Dependency {
    pub project: Option<String>,
    pub version: Option<String>,
    pub incompatible: bool,
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn search(&self, query: &str, filters: &Filters, offset: u32) -> Result<Vec<Project>>;
    async fn details(&self, project: &str) -> Result<ProjectDetails>;
    async fn file(
        &self,
        project: Option<&str>,
        version: Option<&str>,
        target: &InstanceOptions,
        update: Option<UpdateChannel>,
    ) -> Result<Option<ModFile>>;
    async fn content_file(
        &self,
        project: &str,
        kind: Kind,
        game: Option<&str>,
    ) -> Result<ContentFile>;
}

pub async fn provider(
    platform: Platform,
    client: &Client,
    private_client: &Client,
) -> Result<Box<dyn Provider>> {
    match platform {
        Platform::Modrinth => Ok(Box::new(Modrinth(client.clone()))),
        Platform::Curseforge => {
            let key = tokio::task::spawn_blocking(get_key)
                .await??
                .context("Add an authorized CurseForge API key in Settings.")?;
            Ok(Box::new(CurseForge {
                client: private_client.clone(),
                key,
            }))
        }
    }
}

fn key() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(
        "app.voltra.launcher",
        "curseforge-api-key",
    )?)
}

fn get_key() -> Result<Option<String>> {
    if let Ok(value) = std::env::var("VOLTRA_CURSEFORGE_API_KEY") {
        if !value.trim().is_empty() {
            return Ok(Some(value));
        }
    }
    match key()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Reading the CurseForge credential"),
    }
}

pub async fn key_status() -> Result<bool> {
    Ok(tokio::task::spawn_blocking(get_key).await??.is_some())
}

pub async fn set_key(value: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        let value = value.trim();
        if value.is_empty() {
            match key()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => (),
                Err(error) => return Err(error).context("Removing the saved CurseForge key"),
            }
        } else {
            ensure!(
                value.len() <= 512 && value.bytes().all(|byte| byte.is_ascii_graphic()),
                "Enter a valid CurseForge API key."
            );
            key()?
                .set_password(value)
                .context("Saving the CurseForge key")?;
        }
        Ok(get_key()?.is_some())
    })
    .await?
}

const MR: &str = "https://api.modrinth.com/v2";
struct Modrinth(Client);

#[derive(Deserialize)]
struct PublishedLicense {
    id: String,
}

pub fn open_source_license(text: &str) -> bool {
    spdx::Expression::parse(text.trim()).is_ok_and(|expression| {
        expression.evaluate(|requirement| {
            requirement
                .license
                .id()
                .is_some_and(|id| id.is_osi_approved())
        })
    })
}

#[cfg(test)]
#[test]
fn open_source_filter_evaluates_multiple_licenses() {
    assert!(open_source_license("MIT"));
    assert!(open_source_license("MIT OR LicenseRef-Custom"));
    assert!(!open_source_license("MIT AND LicenseRef-Custom"));
    assert!(!open_source_license("All Rights Reserved"));
}

async fn modrinth_batch<T: DeserializeOwned>(
    client: &Client,
    endpoint: &str,
    ids: &[&str],
) -> Result<Vec<T>> {
    let mut entries = Vec::new();
    for chunk in ids.chunks(80) {
        let page: Vec<T> = json(
            client
                .get(format!("{MR}/{endpoint}"))
                .query(&[("ids", serde_json::to_string(chunk)?)]),
            "Modrinth",
        )
        .await?;
        entries.extend(page);
    }
    Ok(entries)
}

pub async fn modrinth_open_source(client: &Client, ids: &[&str]) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ProjectLicense {
        id: String,
        license: Option<PublishedLicense>,
    }

    let projects = modrinth_batch::<ProjectLicense>(client, "projects", ids).await?;
    Ok(projects
        .into_iter()
        .filter_map(|project| {
            project
                .license
                .is_some_and(|license| open_source_license(&license.id))
                .then_some(project.id)
        })
        .collect())
}

pub async fn modrinth_environments(
    client: &Client,
    ids: &[&str],
) -> Result<HashMap<String, ModSide>> {
    #[derive(Deserialize)]
    struct VersionEnvironment {
        id: String,
        environment: Option<String>,
    }

    Ok(
        modrinth_batch::<VersionEnvironment>(client, "versions", ids)
            .await?
            .into_iter()
            .filter_map(|version| {
                let side = ModSide::from_modrinth(version.environment.as_deref(), None, None);
                (side != ModSide::Unknown).then_some((version.id, side))
            })
            .collect(),
    )
}

pub async fn modrinth_updates(
    client: &Client,
    hashes: &[&str],
    target: &InstanceOptions,
) -> Result<HashMap<String, (String, String)>> {
    #[derive(Deserialize)]
    struct LatestVersion {
        id: String,
        project_id: String,
        version_type: String,
    }

    let version_types = match target.update_channel {
        UpdateChannel::Release => &["release"][..],
        UpdateChannel::Beta => &["release", "beta"][..],
        UpdateChannel::Alpha => &["release", "beta", "alpha"][..],
    };
    let mut result = HashMap::new();
    for chunk in hashes.chunks(100) {
        let page: HashMap<String, LatestVersion> = json(
            client
                .post(format!("{MR}/version_files/update"))
                .json(&serde_json::json!({
                    "hashes": chunk,
                    "algorithm": "sha512",
                    "loaders": [target.loader.as_str()],
                    "game_versions": [&target.minecraft_version],
                    "version_types": version_types,
                })),
            "Modrinth",
        )
        .await?;
        result.extend(
            page.into_iter()
                .filter(|(_, version)| version_types.contains(&version.version_type.as_str()))
                .map(|(hash, version)| (hash, (version.project_id, version.id))),
        );
    }
    Ok(result)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthArtwork {
    pub project: String,
    pub title: String,
    pub icon_url: Option<String>,
    pub display_version: Option<String>,
}

pub async fn modrinth_artwork(
    client: &Client,
    hashes: &[&str],
) -> Result<HashMap<String, ModrinthArtwork>> {
    #[derive(Deserialize)]
    struct Version {
        project_id: String,
        version_number: Option<String>,
    }

    #[derive(Deserialize)]
    struct Project {
        id: String,
        title: String,
        icon_url: Option<String>,
    }

    let mut versions = HashMap::new();
    for chunk in hashes.chunks(100) {
        let page: HashMap<String, Version> = json(
            client
                .post(format!("{MR}/version_files"))
                .json(&serde_json::json!({ "hashes": chunk, "algorithm": "sha512" })),
            "Modrinth",
        )
        .await?;
        versions.extend(page);
    }
    let mut ids: Vec<_> = versions
        .values()
        .map(|version| version.project_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let projects: HashMap<_, _> = modrinth_batch::<Project>(client, "projects", &ids)
        .await?
        .into_iter()
        .map(|project| (project.id.clone(), project))
        .collect();
    Ok(versions
        .into_iter()
        .filter_map(|(hash, version)| {
            let project = projects.get(&version.project_id)?;
            Some((
                hash,
                ModrinthArtwork {
                    project: version.project_id,
                    title: project.title.clone(),
                    icon_url: project.icon_url.clone(),
                    display_version: version.version_number,
                },
            ))
        })
        .collect())
}

#[derive(Deserialize)]
struct MrSearch {
    hits: Vec<MrProject>,
}

#[derive(Deserialize)]
struct MrProject {
    project_id: String,
    title: String,
    description: String,
    author: String,
    downloads: u64,
    icon_url: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    date_modified: Option<String>,
    slug: Option<String>,
}

#[derive(Deserialize)]
struct MrDetails {
    title: String,
    client_side: Option<String>,
    server_side: Option<String>,
    icon_url: Option<String>,
}

#[derive(Deserialize)]
struct MrProjectPage {
    description: String,
    body: Option<String>,
    project_type: Option<String>,
    #[serde(default)]
    gallery: Vec<MrGalleryImage>,
}

#[derive(Deserialize)]
struct MrGalleryImage {
    url: String,
    raw_url: Option<String>,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    ordering: i32,
}

#[derive(Deserialize)]
struct MrVersion {
    id: String,
    project_id: String,
    version_type: String,
    version_number: Option<String>,
    date_published: String,
    environment: Option<String>,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    files: Vec<MrFile>,
    #[serde(default)]
    dependencies: Vec<MrDependency>,
}

#[derive(Deserialize)]
struct MrFile {
    url: String,
    filename: String,
    primary: bool,
    size: u64,
    hashes: MrHashes,
}

#[derive(Deserialize)]
struct MrHashes {
    sha512: String,
}

#[derive(Deserialize)]
struct MrDependency {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[async_trait]
impl Provider for Modrinth {
    async fn search(&self, query: &str, filters: &Filters, offset: u32) -> Result<Vec<Project>> {
        let mut facets = vec![vec![format!("project_type:{}", filters.kind.tag())]];
        if filters.kind == Kind::Mod {
            facets.push(vec![
                "client_side:optional".into(),
                "client_side:required".into(),
                "client_side:unknown".into(),
            ]);
        }
        if let Some(game) = &filters.game {
            facets.push(vec![format!("versions:{game}")]);
        }
        if let Some(loader) = filters.loader.filter(|loader| *loader != Loader::Vanilla) {
            facets.push(vec![format!("categories:{}", loader.as_str())]);
        }
        if let Some(category) = &filters.category {
            facets.push(vec![format!("categories:{category}")]);
        }
        let response: MrSearch = json(
            self.0.get(format!("{MR}/search")).query(&[
                ("query", query.to_owned()),
                ("facets", serde_json::to_string(&facets)?),
                ("limit", filters.limit.to_string()),
                ("offset", offset.to_string()),
                ("index", filters.sort.modrinth().into()),
            ]),
            "Modrinth",
        )
        .await?;
        Ok(response
            .hits
            .into_iter()
            .map(|project| Project {
                url: Some(format!(
                    "https://modrinth.com/{}/{}",
                    filters.kind.tag(),
                    project.slug.as_deref().unwrap_or(&project.project_id)
                )),
                id: project.project_id,
                title: project.title,
                description: project.description,
                author: project.author,
                downloads: project.downloads,
                platform: Platform::Modrinth,
                icon_url: project.icon_url,
                categories: project.categories,
                updated_at: project.date_modified,
                can_download: true,
            })
            .collect())
    }

    async fn details(&self, project: &str) -> Result<ProjectDetails> {
        check_id(project)?;
        let mut page: MrProjectPage =
            json(self.0.get(format!("{MR}/project/{project}")), "Modrinth").await?;
        page.gallery
            .sort_by(|a, b| a.ordering.cmp(&b.ordering).then(a.title.cmp(&b.title)));
        let gallery = page
            .gallery
            .into_iter()
            .filter_map(|image| {
                let thumbnail = gallery_url(Platform::Modrinth, &image.url);
                let url = image
                    .raw_url
                    .as_deref()
                    .and_then(|raw| gallery_url(Platform::Modrinth, raw))
                    .or_else(|| thumbnail.clone())?;
                Some(GalleryImage {
                    thumbnail_url: thumbnail.unwrap_or_else(|| url.clone()),
                    url,
                    title: image.title,
                    description: image.description,
                })
            })
            .collect();
        Ok(ProjectDetails {
            summary: page.description,
            body: page.body,
            description_unavailable: false,
            gallery,
            url: page
                .project_type
                .as_deref()
                .filter(|kind| {
                    matches!(
                        *kind,
                        "mod" | "modpack" | "resourcepack" | "shader" | "datapack"
                    )
                })
                .map(|kind| format!("https://modrinth.com/{kind}/{project}")),
        })
    }

    async fn file(
        &self,
        project: Option<&str>,
        version: Option<&str>,
        target: &InstanceOptions,
        update: Option<UpdateChannel>,
    ) -> Result<Option<ModFile>> {
        ensure!(
            target.loader != Loader::Vanilla,
            "Mods need a supported mod loader."
        );
        let version = if let Some(version) = version {
            check_id(version)?;
            json(self.0.get(format!("{MR}/version/{version}")), "Modrinth").await?
        } else {
            let project = project.context("A Modrinth project ID is required.")?;
            check_id(project)?;
            let mut versions: Vec<MrVersion> = json(
                self.0
                    .get(format!("{MR}/project/{project}/version"))
                    .query(&[
                        ("loaders", serde_json::to_string(&[target.loader.as_str()])?),
                        (
                            "game_versions",
                            serde_json::to_string(&[&target.minecraft_version])?,
                        ),
                    ]),
                "Modrinth",
            )
            .await?;
            versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
            if let Some(channel) = update {
                versions.retain(|version| match version.version_type.as_str() {
                    "release" => true,
                    "beta" => channel >= UpdateChannel::Beta,
                    "alpha" => channel >= UpdateChannel::Alpha,
                    _ => false,
                });
                let Some(version) = versions.into_iter().next() else {
                    return Ok(None);
                };
                version
            } else {
                versions
                    .iter()
                    .position(|version| version.version_type == "release")
                    .or_else(|| (!versions.is_empty()).then_some(0))
                    .map(|index| versions.swap_remove(index))
                    .with_context(|| {
                        format!("No version of Modrinth project {project} supports this instance.")
                    })?
            }
        };
        ensure!(
            project.is_none_or(|id| id == version.project_id),
            "The required version belongs to a different Modrinth project."
        );
        ensure!(
            version.game_versions.contains(&target.minecraft_version)
                && version
                    .loaders
                    .iter()
                    .any(|loader| loader == target.loader.as_str()),
            "A Modrinth dependency does not support this Minecraft version and loader."
        );
        check_id(&version.project_id)?;
        let details: MrDetails = json(
            self.0.get(format!("{MR}/project/{}", version.project_id)),
            "Modrinth",
        )
        .await?;
        ensure!(
            details.client_side.as_deref() != Some("unsupported"),
            "{} is a server-only mod.",
            details.title
        );
        let file = version
            .files
            .iter()
            .filter(|file| file.filename.to_ascii_lowercase().ends_with(".jar"))
            .max_by_key(|file| file.primary)
            .context("This project has no mod JAR for this version.")?;
        Ok(Some(ModFile {
            side: crate::mod_sides::ModSide::from_modrinth(
                version.environment.as_deref(),
                details.client_side.as_deref(),
                details.server_side.as_deref(),
            ),
            project: version.project_id,
            version: version.id,
            title: details.title,
            icon_url: details.icon_url,
            display_version: version.version_number,
            filename: file.filename.clone(),
            url: file.url.clone(),
            hash: Hash::Sha512(file.hashes.sha512.clone()),
            size: Some(file.size),
            dependencies: version
                .dependencies
                .into_iter()
                .filter(|dep| matches!(dep.dependency_type.as_str(), "required" | "incompatible"))
                .map(|dep| Dependency {
                    project: dep.project_id,
                    version: dep.version_id,
                    incompatible: dep.dependency_type == "incompatible",
                })
                .collect(),
        }))
    }

    async fn content_file(
        &self,
        project: &str,
        kind: Kind,
        game: Option<&str>,
    ) -> Result<ContentFile> {
        check_id(project)?;
        let mut request = self.0.get(format!("{MR}/project/{project}/version"));
        if let Some(game) = game {
            request = request.query(&[("game_versions", serde_json::to_string(&[game])?)]);
        }
        let mut versions: Vec<MrVersion> = json(request, "Modrinth").await?;
        versions.sort_by(|a, b| {
            (b.version_type == "release", &b.date_published)
                .cmp(&(a.version_type == "release", &a.date_published))
        });
        let file = versions
            .iter()
            .find_map(|version| {
                version
                    .files
                    .iter()
                    .filter(|file| {
                        file.filename
                            .to_ascii_lowercase()
                            .ends_with(kind.extension())
                    })
                    .max_by_key(|file| file.primary)
            })
            .context("No compatible download was found for this project.")?;
        Ok(ContentFile {
            filename: file.filename.clone(),
            url: file.url.clone(),
            hash: Hash::Sha512(file.hashes.sha512.clone()),
            size: Some(file.size),
        })
    }
}

const CF: &str = "https://api.curseforge.com/v1";
struct CurseForge {
    client: Client,
    key: String,
}

#[derive(Deserialize)]
struct CfResponse<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfProject {
    id: u64,
    name: String,
    summary: String,
    download_count: u64,
    authors: Vec<CfAuthor>,
    allow_mod_distribution: Option<bool>,
    logo: Option<CfLogo>,
    date_modified: Option<String>,
    links: Option<CfLinks>,
    #[serde(default)]
    categories: Vec<CfCategory>,
}

#[derive(Deserialize)]
struct CfProjectPage {
    summary: String,
    links: Option<CfLinks>,
    #[serde(default)]
    screenshots: Vec<CfScreenshot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfScreenshot {
    url: Option<String>,
    thumbnail_url: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLogo {
    thumbnail_url: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfLinks {
    website_url: Option<String>,
}

#[derive(Deserialize)]
struct CfCategory {
    id: u64,
    name: String,
    slug: String,
}

#[derive(Deserialize)]
struct CfAuthor {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    id: u64,
    file_name: String,
    download_url: Option<String>,
    file_length: u64,
    file_date: String,
    release_type: u8,
    display_name: Option<String>,
    #[serde(default)]
    dependencies: Vec<CfDependency>,
    hashes: Vec<CfHash>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDependency {
    mod_id: u64,
    relation_type: u8,
}

#[derive(Deserialize)]
struct CfHash {
    algo: u8,
    value: String,
}

impl CurseForge {
    fn get(&self, path: &str) -> RequestBuilder {
        self.client
            .get(format!("{CF}/{path}"))
            .header("x-api-key", &self.key)
    }
}

fn cf_loader(loader: Loader) -> Option<&'static str> {
    match loader {
        Loader::Vanilla => None,
        Loader::Fabric => Some("4"),
        Loader::Quilt => Some("5"),
        Loader::Forge => Some("1"),
        Loader::Neoforge => Some("6"),
    }
}

#[async_trait]
impl Provider for CurseForge {
    async fn search(&self, query: &str, filters: &Filters, offset: u32) -> Result<Vec<Project>> {
        let offset = offset.to_string();
        let limit = filters.limit.to_string();
        let mut params = vec![
            ("gameId", "432"),
            ("classId", filters.kind.class()),
            ("pageSize", limit.as_str()),
            ("index", offset.as_str()),
            ("searchFilter", query),
            ("sortField", filters.sort.curseforge()),
            ("sortOrder", "desc"),
        ];
        if let Some(game) = &filters.game {
            params.push(("gameVersion", game));
        }
        if let Some(loader) = filters.loader.and_then(cf_loader) {
            params.push(("modLoaderType", loader));
        }
        let category;
        if let Some(slug) = &filters.category {
            let available: CfResponse<Vec<CfCategory>> = json(
                self.get("categories")
                    .query(&[("gameId", "432"), ("classId", filters.kind.class())]),
                "CurseForge",
            )
            .await?;
            let Some(item) = available.data.into_iter().find(|item| item.slug == *slug) else {
                return Ok(vec![]);
            };
            category = item.id.to_string();
            params.push(("categoryId", category.as_str()));
        }
        let results: CfResponse<Vec<CfProject>> =
            json(self.get("mods/search").query(&params), "CurseForge").await?;
        Ok(results
            .data
            .into_iter()
            .map(|project| Project {
                id: project.id.to_string(),
                title: project.name,
                description: project.summary,
                author: project
                    .authors
                    .into_iter()
                    .map(|author| author.name)
                    .collect::<Vec<_>>()
                    .join(", "),
                downloads: project.download_count,
                platform: Platform::Curseforge,
                icon_url: project
                    .logo
                    .and_then(|logo| logo.thumbnail_url.or(logo.url)),
                categories: project
                    .categories
                    .into_iter()
                    .map(|category| category.name)
                    .collect(),
                updated_at: project.date_modified,
                url: project.links.and_then(|links| links.website_url),
                can_download: filters.kind != Kind::Modpack
                    && project.allow_mod_distribution != Some(false),
            })
            .collect())
    }

    async fn details(&self, project: &str) -> Result<ProjectDetails> {
        let id = project
            .parse::<u64>()
            .context("Invalid CurseForge project ID.")?;
        let (page, description) = tokio::join!(
            json::<CfResponse<CfProjectPage>>(self.get(&format!("mods/{id}")), "CurseForge"),
            json::<CfResponse<String>>(self.get(&format!("mods/{id}/description")), "CurseForge"),
        );
        let page = page?.data;
        let gallery = page
            .screenshots
            .into_iter()
            .filter_map(|image| {
                let thumbnail = image
                    .thumbnail_url
                    .as_deref()
                    .and_then(|raw| gallery_url(Platform::Curseforge, raw));
                let url = image
                    .url
                    .as_deref()
                    .and_then(|raw| gallery_url(Platform::Curseforge, raw))
                    .or_else(|| thumbnail.clone())?;
                Some(GalleryImage {
                    thumbnail_url: thumbnail.unwrap_or_else(|| url.clone()),
                    url,
                    title: image.title,
                    description: image.description,
                })
            })
            .collect();
        Ok(ProjectDetails {
            summary: page.summary,
            description_unavailable: description.is_err(),
            body: description.ok().map(|response| response.data),
            gallery,
            url: page.links.and_then(|links| links.website_url),
        })
    }

    async fn file(
        &self,
        project: Option<&str>,
        version: Option<&str>,
        target: &InstanceOptions,
        update: Option<UpdateChannel>,
    ) -> Result<Option<ModFile>> {
        let id = project
            .context("A CurseForge project ID is required.")?
            .parse::<u64>()?;
        let loader = cf_loader(target.loader).context("Mods need a supported mod loader.")?;
        let project: CfResponse<CfProject> =
            json(self.get(&format!("mods/{id}")), "CurseForge").await?;
        ensure!(project.data.allow_mod_distribution != Some(false), "{} has disabled third-party downloads. Download the file from CurseForge and add the JAR in the instance's mods folder.", project.data.name);
        let params = [
            ("gameVersion", target.minecraft_version.as_str()),
            ("modLoaderType", loader),
            ("pageSize", "50"),
        ];
        let mut files: CfResponse<Vec<CfFile>> = json(
            self.get(&format!("mods/{id}/files")).query(&params),
            "CurseForge",
        )
        .await?;
        files
            .data
            .retain(|file| file.file_name.to_ascii_lowercase().ends_with(".jar"));
        files.data.sort_by(|a, b| b.file_date.cmp(&a.file_date));
        let file = if let Some(version) = version {
            let id = version.parse::<u64>()?;
            files.data.into_iter().find(|file| file.id == id)
        } else if let Some(channel) = update {
            let Some(file) = files.data.into_iter().find(|file| {
                matches!(file.release_type, 1..=3) && file.release_type <= channel as u8 + 1
            }) else {
                return Ok(None);
            };
            Some(file)
        } else {
            let index = files
                .data
                .iter()
                .position(|file| file.release_type == 1)
                .or_else(|| (!files.data.is_empty()).then_some(0));
            index.map(|index| files.data.swap_remove(index))
        }
        .with_context(|| {
            format!(
                "No version of {} supports this instance.",
                project.data.name
            )
        })?;
        let url = file.download_url.with_context(|| {
            format!(
                "CurseForge provides no authorized download URL for {}.",
                project.data.name
            )
        })?;
        let hash = file
            .hashes
            .into_iter()
            .find(|hash| hash.algo == 1)
            .context("CurseForge provided no SHA-1 checksum for this file.")?;
        Ok(Some(ModFile {
            side: crate::mod_sides::ModSide::Unknown,
            project: id.to_string(),
            version: file.id.to_string(),
            title: project.data.name,
            icon_url: project
                .data
                .logo
                .and_then(|logo| logo.thumbnail_url.or(logo.url)),
            display_version: file.display_name,
            filename: file.file_name,
            url,
            hash: Hash::Sha1(hash.value),
            size: Some(file.file_length),
            dependencies: file
                .dependencies
                .into_iter()
                .filter(|dep| matches!(dep.relation_type, 3 | 5))
                .map(|dep| Dependency {
                    project: Some(dep.mod_id.to_string()),
                    version: None,
                    incompatible: dep.relation_type == 5,
                })
                .collect(),
        }))
    }

    async fn content_file(
        &self,
        project: &str,
        kind: Kind,
        game: Option<&str>,
    ) -> Result<ContentFile> {
        let id = project.parse::<u64>()?;
        ensure!(
            kind != Kind::Modpack,
            "Import Modrinth modpacks from Discover."
        );
        let project: CfResponse<CfProject> =
            json(self.get(&format!("mods/{id}")), "CurseForge").await?;
        ensure!(
            project.data.allow_mod_distribution != Some(false),
            "This project has disabled third-party downloads."
        );
        let mut params = vec![("pageSize", "50")];
        if let Some(game) = game {
            params.push(("gameVersion", game));
        }
        let mut files: CfResponse<Vec<CfFile>> = json(
            self.get(&format!("mods/{id}/files")).query(&params),
            "CurseForge",
        )
        .await?;
        files.data.sort_by(|a, b| {
            (b.release_type == 1, &b.file_date).cmp(&(a.release_type == 1, &a.file_date))
        });
        let file = files
            .data
            .into_iter()
            .find(|file| {
                file.file_name
                    .to_ascii_lowercase()
                    .ends_with(kind.extension())
            })
            .context("No compatible download was found for this project.")?;
        let url = file
            .download_url
            .context("CurseForge provides no authorized download URL for this file.")?;
        let hash = file
            .hashes
            .into_iter()
            .find(|hash| hash.algo == 1)
            .context("CurseForge provided no SHA-1 checksum.")?;
        Ok(ContentFile {
            filename: file.file_name,
            url,
            hash: Hash::Sha1(hash.value),
            size: Some(file.file_length),
        })
    }
}

async fn json<T: DeserializeOwned>(request: RequestBuilder, platform: &str) -> Result<T> {
    let response = request
        .send()
        .await
        .with_context(|| format!("Connecting to {platform}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!("{platform} denied access ({status}). Check your API authorization.");
    }
    ensure!(status.is_success(), "{platform} returned {status}.");
    response
        .json()
        .await
        .with_context(|| format!("Reading the {platform} response"))
}
