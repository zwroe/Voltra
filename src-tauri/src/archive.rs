use anyhow::{ensure, Context, Result};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

use crate::storage::check_filename;

pub fn safe_relative(name: &str) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for part in name.split('/') {
        check_filename(part)?;
        result.push(part);
    }
    ensure!(!result.as_os_str().is_empty(), "Missing relative path.");
    Ok(result)
}

pub fn extract_zip(
    archive: &Path,
    target: &Path,
    prefixes: &[String],
    cancel: &CancellationToken,
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    fs::create_dir_all(target)?;
    let mut size = 0u64;
    for index in 0..zip.len() {
        ensure!(!cancel.is_cancelled(), "Cancelled.");
        let mut entry = zip.by_index(index)?;
        if prefixes
            .iter()
            .any(|prefix| entry.name().starts_with(prefix))
        {
            continue;
        }
        let relative = entry
            .enclosed_name()
            .context("Archive contains a path outside its destination.")?;
        ensure!(relative.components().all(|part| matches!(part, Component::Normal(name) if name.to_str().is_some_and(|n| check_filename(n).is_ok()))),
            "Archive contains an unsafe file or directory name.");
        ensure!(
            !entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000),
            "Archive contains a symbolic link."
        );
        let destination = target.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&destination)?;
        } else {
            size = size.saturating_add(entry.size());
            ensure!(
                size <= 2 * 1024 * 1024 * 1024,
                "Archive is too large to extract."
            );
            fs::create_dir_all(
                destination
                    .parent()
                    .context("Archive file has no parent.")?,
            )?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(())
}
