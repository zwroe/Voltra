use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    mods::{self, InstalledMod},
    storage::{self, Store},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub nodes: Vec<Node>,
    pub issues: Vec<Issue>,
    pub metadata_count: usize,
    pub enable_candidates: BTreeSet<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub file: String,
    pub name: String,
    pub mod_id: Option<String>,
    pub version: Option<String>,
    pub enabled: bool,
    pub edges: Vec<Edge>,
}

#[derive(Serialize)]
pub struct Edge {
    pub target: Option<String>,
    pub requirement: String,
    pub incompatible: bool,
}

#[derive(Serialize)]
pub struct Issue {
    pub level: &'static str,
    pub title: &'static str,
    pub file: String,
    pub evidence: String,
    pub recommendation: &'static str,
}

#[derive(Default)]
struct Manifest {
    id: Option<String>,
    version: Option<String>,
    provides: Vec<String>,
    depends: Vec<String>,
    breaks: Vec<String>,
}

pub fn inspect(store: &Store, id: Uuid) -> Result<Health> {
    inspect_enabled(store, id, &HashSet::new())
}

fn inspect_enabled(store: &Store, id: Uuid, enabled: &HashSet<&str>) -> Result<Health> {
    let instance = store.instance(id)?;
    let directory = store.game_dir(id)?.join("mods");
    let installed = mods::list(store, id)?;
    let active = |entry: &InstalledMod| entry.enabled || enabled.contains(entry.file.as_str());
    let mut metadata_count = 0;
    let manifests = installed
        .iter()
        .map(|item| {
            let value = crate::mod_sides::fabric_manifest(&directory.join(&item.file))
                .ok()
                .flatten();
            if item.managed.is_some() || value.is_some() {
                metadata_count += 1;
            }
            let Some(value) = value else {
                return Manifest::default();
            };
            let read = |key| {
                value
                    .get(key)
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            };
            let is_library =
                |key: &str| matches!(key, "minecraft" | "java" | "fabricloader" | "quilt_loader");
            Manifest {
                id: read("id"),
                version: read("version"),
                provides: value
                    .get("provides")
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect(),
                depends: value
                    .get("depends")
                    .and_then(|value| value.as_object())
                    .into_iter()
                    .flat_map(|rows| rows.keys())
                    .filter(|key| !is_library(key))
                    .cloned()
                    .collect(),
                breaks: value
                    .get("breaks")
                    .and_then(|value| value.as_object())
                    .into_iter()
                    .flatten()
                    .filter(|(_, constraint)| constraint.as_str() == Some("*"))
                    .map(|(key, _)| key.clone())
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    let mut by_mod_id: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_project: HashMap<_, Vec<usize>> = HashMap::new();
    let mut by_version: HashMap<_, Vec<usize>> = HashMap::new();
    for (index, (item, manifest)) in installed.iter().zip(&manifests).enumerate() {
        for key in manifest.id.iter().chain(&manifest.provides) {
            by_mod_id.entry(key).or_default().push(index);
        }
        if let Some(managed) = &item.managed {
            by_project
                .entry((managed.platform, managed.project.as_str()))
                .or_default()
                .push(index);
            by_version
                .entry((managed.platform, managed.version.as_str()))
                .or_default()
                .push(index);
        }
    }
    let mut issues = Vec::new();
    let mut nodes = Vec::new();
    let mut enable_candidates = BTreeSet::new();
    for (index, (item, manifest)) in installed.iter().zip(&manifests).enumerate() {
        let mut requirements = Vec::new();
        let mut issue = |level, title, evidence, recommendation| {
            issues.push(Issue {
                level,
                title,
                evidence,
                recommendation,
                file: item.file.clone(),
            })
        };
        let managed = item.managed.as_ref();
        if let Some(managed) = managed {
            if active(item)
                && (managed.loader != instance.options.loader
                    || managed.minecraft_version != instance.options.minecraft_version)
            {
                issue(
                    "warning",
                    "Different installation target",
                    format!(
                        "Installed for Minecraft {} and {}.",
                        managed.minecraft_version,
                        managed.loader.as_str()
                    ),
                    "Preview compatible versions before launching.",
                );
            }
            for dependency in &managed.dependencies {
                let matches = if let Some(version) = dependency.version.as_deref() {
                    by_version.get(&(managed.platform, version))
                } else {
                    dependency
                        .project
                        .as_deref()
                        .and_then(|project| by_project.get(&(managed.platform, project)))
                };
                let candidates = matches
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|candidate| {
                        dependency.project.as_deref().is_none_or(|project| {
                            installed[*candidate]
                                .managed
                                .as_ref()
                                .is_some_and(|entry| entry.project == project)
                        })
                    })
                    .collect::<Vec<_>>();
                let requirement = dependency
                    .project
                    .as_deref()
                    .or(dependency.version.as_deref())
                    .unwrap_or("Unidentified catalog project");
                requirements.push((
                    candidates,
                    format!("{} {requirement}", managed.platform.label()),
                    dependency.incompatible,
                    true,
                ));
            }
        }
        for (requirement, incompatible) in manifest
            .depends
            .iter()
            .map(|key| (key, false))
            .chain(manifest.breaks.iter().map(|key| (key, true)))
        {
            requirements.push((
                by_mod_id
                    .get(requirement.as_str())
                    .cloned()
                    .unwrap_or_default(),
                requirement.clone(),
                incompatible,
                false,
            ));
        }
        if let Some(mod_id) = &manifest.id {
            if active(item)
                && by_mod_id
                    .get(mod_id.as_str())
                    .into_iter()
                    .flatten()
                    .any(|&other| {
                        other < index
                            && active(&installed[other])
                            && manifests[other].id.as_ref() == Some(mod_id)
                    })
            {
                issue("error", "Duplicate enabled mod ID", format!("More than one enabled JAR declares {mod_id}."), "Keep the version required by this modpack and disable the duplicate after reviewing its dependents.");
            }
        }
        let mut edges = Vec::new();
        for (candidates, requirement, incompatible, catalog) in requirements {
            let target = candidates
                .iter()
                .copied()
                .find(|&other| active(&installed[other]))
                .or_else(|| candidates.first().copied());
            edges.push(Edge {
                target: target.map(|other| installed[other].file.clone()),
                requirement: requirement.clone(),
                incompatible,
            });
            if !active(item) {
                continue;
            }
            let present = target.is_some_and(|other| active(&installed[other]));
            if incompatible && present {
                issue("error", "Declared incompatibility", format!("This mod declares a conflict with {requirement}."), "Review the dependency explorer and test a compatible version or a mod selection in isolation.");
            } else if !incompatible && !present {
                if let Some(other) = target {
                    if catalog
                        && candidates.len() == 1
                        && installed[other].managed.as_ref().is_some_and(|entry| {
                            entry.loader == instance.options.loader
                                && entry.minecraft_version == instance.options.minecraft_version
                        })
                    {
                        enable_candidates.insert(installed[other].file.clone());
                    }
                    issue(
                        "error",
                        "Required mod is disabled",
                        format!(
                            "{requirement} is required. {} is disabled.",
                            installed[other].file
                        ),
                        "Preview enabling the required mod and review any new conflicts.",
                    );
                } else {
                    issue("warning", "Dependency could not be verified", format!("This mod requires {requirement}. No matching enabled JAR was found in the available metadata."), "Check the mod project and loader log for the exact dependency and version.");
                }
            }
        }
        nodes.push(Node {
            file: item.file.clone(),
            name: managed.map_or_else(|| item.file.clone(), |entry| entry.title.clone()),
            mod_id: manifest.id.clone(),
            version: managed
                .and_then(|entry| entry.display_version.clone())
                .or_else(|| manifest.version.clone()),
            enabled: active(item),
            edges,
        });
    }
    Ok(Health {
        nodes,
        issues,
        metadata_count,
        enable_candidates,
    })
}

pub fn enable_plan(store: &Store, id: Uuid) -> Result<Vec<String>> {
    let before = inspect(store, id)?;
    let selected: HashSet<_> = before
        .enable_candidates
        .iter()
        .map(String::as_str)
        .collect();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let after = inspect_enabled(store, id, &selected)?;
    let previous: HashSet<_> = before
        .issues
        .iter()
        .map(|issue| (&issue.file, issue.title, &issue.evidence))
        .collect();
    ensure!(after.issues.iter().all(|issue| previous.contains(&(&issue.file, issue.title, &issue.evidence))),
        "Enabling these dependencies would introduce another unresolved issue. Check the dependency explorer first.");
    Ok(before.enable_candidates.into_iter().collect())
}

pub fn apply_enable_plan(store: &Store, id: Uuid, selected: &[String]) -> Result<()> {
    ensure!(
        !selected.is_empty() && selected == enable_plan(store, id)?,
        "The fixes changed. Review a new preview."
    );
    mods::enable_files(store, id, selected)
}

pub fn isolation_files(store: &Store, id: Uuid, chosen: &[String]) -> Result<BTreeSet<String>> {
    let health = inspect(store, id)?;
    let mut included = BTreeSet::new();
    let nodes: HashMap<_, _> = health
        .nodes
        .iter()
        .filter(|node| node.enabled)
        .map(|node| (node.file.as_str(), node))
        .collect();
    let mut pending: Vec<&str> = chosen.iter().map(String::as_str).collect();
    while let Some(file) = pending.pop() {
        storage::check_filename(file)?;
        if included.insert(file.to_owned()) {
            let node = nodes
                .get(file)
                .with_context(|| format!("{file} is no longer enabled."))?;
            pending.extend(
                node.edges
                    .iter()
                    .filter(|edge| !edge.incompatible)
                    .filter_map(|edge| edge.target.as_deref())
                    .filter(|target| nodes.contains_key(target)),
            );
        }
    }
    Ok(included)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, path::Path};

    fn jar(path: &Path, manifest: &serde_json::Value) -> Result<()> {
        let mut zip = zip::ZipWriter::new(fs::File::create(path)?);
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())?;
        zip.write_all(manifest.to_string().as_bytes())?;
        zip.finish()?;
        Ok(())
    }

    #[test]
    fn reads_declared_conflicts_duplicates_and_isolation_dependencies() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let store = Store::new(workspace.path().to_owned())?;
        let id = store.create(serde_json::from_value(serde_json::json!({"name":"Fixture", "minecraftVersion":"1.21.1", "loader":"fabric", "ramMib":4096}))?)?.instance.id;
        let folder = store.game_dir(id)?.join("mods");
        fs::create_dir(&folder)?;
        jar(
            &folder.join("base.jar"),
            &serde_json::json!({"id":"base","version":"1.0.0"}),
        )?;
        jar(
            &folder.join("mod.jar"),
            &serde_json::json!({"id":"feature","depends":{"base":"*","missing":"*"},"breaks":{"conflicting":"*"}}),
        )?;
        jar(
            &folder.join("other.jar"),
            &serde_json::json!({"id":"conflicting"}),
        )?;
        jar(
            &folder.join("duplicate.jar"),
            &serde_json::json!({"id":"base"}),
        )?;
        let report = inspect(&store, id)?;
        assert_eq!(report.metadata_count, 4);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.title == "Duplicate enabled mod ID"));
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.title == "Declared incompatibility"));
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.evidence.contains("missing")));
        assert_eq!(
            isolation_files(&store, id, &["mod.jar".into()])?
                .into_iter()
                .collect::<Vec<_>>(),
            ["base.jar", "mod.jar"]
        );
        Ok(())
    }
}
