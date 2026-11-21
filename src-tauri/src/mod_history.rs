use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{collections::BTreeSet, fs, io, path::Path};
use uuid::Uuid;

use crate::{
    mods::ManagedMod,
    providers::Dependency,
    storage::{check_filename, is_mod, now, regular_file, Instance, Loader},
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Revision {
    pub file: String,
    pub managed: Option<ManagedMod>,
    pub digest: String,
}

impl Revision {
    pub fn read(directory: &Path, file: &str, managed: Option<ManagedMod>) -> Result<Self> {
        Ok(Self {
            file: file.into(),
            managed,
            digest: digest(directory, file)?,
        })
    }

    pub fn check(&self, directory: &Path) -> Result<()> {
        ensure!(
            digest(directory, &self.file)? == self.digest,
            "{} has changed since this was recorded.",
            self.file
        );
        Ok(())
    }

    fn label(&self) -> FileLabel {
        FileLabel {
            file: self.file.clone(),
            version: self.managed.as_ref().map(|item| {
                item.display_version
                    .clone()
                    .unwrap_or_else(|| item.version.clone())
            }),
        }
    }
}

fn digest(directory: &Path, file: &str) -> Result<String> {
    use sha2::Digest;
    check_filename(file)?;
    ensure!(is_mod(file), "Expected a mod JAR.");
    let path = directory.join(file);
    regular_file(&path)?;
    let mut hash = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hash)?;
    Ok(hex::encode(hash.finalize()))
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Install,
    Update,
    Remove,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Change {
    pub before: Option<Revision>,
    pub after: Option<Revision>,
    pub undone: bool,
}

impl Change {
    pub fn new(before: Option<Revision>, after: Option<Revision>) -> Self {
        Self {
            before,
            after,
            undone: false,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct Record {
    pub id: Uuid,
    pub at: u64,
    pub action: Action,
    pub changes: Vec<Change>,
    pub backup: Option<Uuid>,
    pub game: String,
    pub loader: Loader,
}

impl Record {
    pub fn new(
        instance: &Instance,
        action: Action,
        backup: Option<Uuid>,
        changes: Vec<Change>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            at: now(),
            action,
            changes,
            backup,
            game: instance.options.minecraft_version.clone(),
            loader: instance.options.loader,
        }
    }

    pub fn compatible(&self, instance: &Instance) -> bool {
        self.game == instance.options.minecraft_version && self.loader == instance.options.loader
    }

    pub fn undo_group(&self, index: usize, current: &[ManagedMod]) -> Result<BTreeSet<usize>> {
        let selected = self
            .changes
            .get(index)
            .context("This mod change is missing.")?;
        ensure!(!selected.undone, "This mod change was already undone.");
        let mut group = BTreeSet::new();
        let mut pending = vec![index];
        while let Some(index) = pending.pop() {
            if !group.insert(index) {
                continue;
            }
            let change = &self.changes[index];
            for revision in change.before.iter().chain(&change.after) {
                let Some(modified) = &revision.managed else {
                    continue;
                };
                for requirement in modified.dependencies.iter().filter(|dep| !dep.incompatible) {
                    for (other_index, other) in self.changes.iter().enumerate() {
                        if !other.undone
                            && other.before.iter().chain(&other.after).any(|candidate| {
                                candidate
                                    .managed
                                    .as_ref()
                                    .is_some_and(|item| satisfies(modified, requirement, item))
                            })
                        {
                            pending.push(other_index);
                        }
                    }
                }
            }
        }
        loop {
            let affected = |mod_file: &ManagedMod| {
                group.iter().any(|index| {
                    self.changes[*index]
                        .after
                        .as_ref()
                        .and_then(|revision| revision.managed.as_ref())
                        == Some(mod_file)
                })
            };
            let restored = group
                .iter()
                .filter_map(|index| self.changes[*index].before.as_ref()?.managed.as_ref());
            let remaining = current
                .iter()
                .filter(|entry| !affected(entry))
                .chain(restored);
            let shared = group.iter().copied().find(|candidate| {
                let change = &self.changes[*candidate];
                if *candidate == index || change.before.is_some() {
                    return false;
                }
                change
                    .after
                    .as_ref()
                    .and_then(|revision| revision.managed.as_ref())
                    .is_some_and(|added| {
                        remaining
                            .clone()
                            .filter(|entry| enabled(entry))
                            .any(|owner| {
                                owner
                                    .dependencies
                                    .iter()
                                    .any(|dep| !dep.incompatible && satisfies(owner, dep, added))
                            })
                    })
            });
            let Some(shared) = shared else {
                break;
            };
            group.remove(&shared);
        }
        Ok(group)
    }

    pub fn view(&self, instance: &Instance) -> RecordView {
        RecordView {
            id: self.id,
            at: self.at,
            action: self.action,
            compatible: self.compatible(instance),
            changes: self
                .changes
                .iter()
                .map(|change| {
                    let title = change
                        .after
                        .as_ref()
                        .or(change.before.as_ref())
                        .map_or_else(
                            || "Mod".into(),
                            |revision| {
                                revision.managed.as_ref().map_or_else(
                                    || revision.file.clone(),
                                    |item| item.title.clone(),
                                )
                            },
                        );
                    ChangeView {
                        title,
                        before: change.before.as_ref().map(Revision::label),
                        after: change.after.as_ref().map(Revision::label),
                        undone: change.undone,
                    }
                })
                .collect(),
        }
    }
}

fn satisfies(owner: &ManagedMod, requirement: &Dependency, candidate: &ManagedMod) -> bool {
    owner.platform == candidate.platform
        && (requirement.project.is_some() || requirement.version.is_some())
        && requirement
            .project
            .as_ref()
            .is_none_or(|project| project == &candidate.project)
        && requirement
            .version
            .as_ref()
            .is_none_or(|version| version == &candidate.version)
}

pub fn verify_dependencies(current: &[ManagedMod], proposed: &[ManagedMod]) -> Result<()> {
    for entry in proposed.iter().filter(|entry| enabled(entry)) {
        for dep in &entry.dependencies {
            let matches = |items: &[ManagedMod]| {
                items
                    .iter()
                    .any(|item| enabled(item) && satisfies(entry, dep, item))
            };
            let retained = current.iter().any(|old| old == entry);
            let allowed = if dep.incompatible {
                !matches(proposed) || (retained && matches(current))
            } else {
                matches(proposed) || (retained && !matches(current))
            };
            ensure!(
                allowed,
                "Undo would conflict with a dependency of {}.",
                entry.title
            );
        }
    }
    Ok(())
}

fn enabled(entry: &ManagedMod) -> bool {
    !entry.file.to_ascii_lowercase().ends_with(".disabled")
}

#[derive(Serialize)]
pub struct RecordView {
    id: Uuid,
    at: u64,
    action: Action,
    compatible: bool,
    changes: Vec<ChangeView>,
}

#[derive(Serialize)]
pub struct ChangeView {
    title: String,
    before: Option<FileLabel>,
    after: Option<FileLabel>,
    undone: bool,
}

#[derive(Serialize)]
pub struct FileLabel {
    file: String,
    version: Option<String>,
}
