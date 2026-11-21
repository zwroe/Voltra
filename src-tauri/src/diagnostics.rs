use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::LazyLock,
};
use uuid::Uuid;

use crate::storage::{list_files, regular_file, Store};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogKind {
    Console,
    Game,
    Crash,
    Test,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub name: String,
    pub text: String,
    pub truncated: bool,
    pub findings: Vec<Finding>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub certainty: &'static str,
    pub title: &'static str,
    pub evidence: String,
    pub severity: &'static str,
    pub confidence: &'static str,
    pub recommendation: &'static str,
    pub suspected_mod: Option<String>,
}

pub fn read(store: &Store, id: Uuid, kind: LogKind) -> Result<Option<Log>> {
    let game = store.game_dir(id)?;
    let path = match kind {
        LogKind::Console => store.root.join("logs").join(format!("{id}.log")),
        LogKind::Test => store.root.join("logs").join(format!("{id}-test.log")),
        LogKind::Game => game.join("logs/latest.log"),
        LogKind::Crash => {
            let folder = game.join("crash-reports");
            match list_files(&folder)?
                .into_iter()
                .filter(|name| name.starts_with("crash-") && name.ends_with(".txt"))
                .max()
            {
                Some(name) => folder.join(name),
                None => return Ok(None),
            }
        }
    };
    if !path.exists() {
        return Ok(None);
    }
    let (text, truncated) = tail(&path)?;
    Ok(Some(Log {
        name: path
            .file_name()
            .context("Missing log filename.")?
            .to_string_lossy()
            .into_owned(),
        findings: analyze(&text),
        text,
        truncated,
    }))
}

pub(crate) fn tail(path: &Path) -> Result<(String, bool)> {
    regular_file(path)?;
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let limit = 512 * 1024;
    let start = len.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    let first_line = if start > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    Ok((
        String::from_utf8_lossy(&bytes[first_line..]).into_owned(),
        start > 0,
    ))
}

pub fn analyze(text: &str) -> Vec<Finding> {
    let lines: Vec<_> = text.lines().collect();
    let mut findings = Vec::new();
    for (needle, certainty, title, severity, recommendation) in [
        (
            "java.lang.OutOfMemoryError",
            "confirmed",
            "Java reported an out-of-memory error",
            "error", "Review the session's memory graph and heap limit. Check the exact memory error before changing RAM or JVM flags.",
        ),
        (
            "UnsupportedClassVersionError",
            "confirmed",
            "A component requires a newer Java class format",
            "error", "Check the required Java version in instance settings and enable automatic runtime selection.",
        ),
        (
            "Incompatible mods found",
            "confirmed",
            "The loader rejected the installed mod set",
            "error", "Review the dependency details in the log and check this modpack's Health tab.",
        ),
        (
            "Mod resolution encountered an incompatible mod set",
            "confirmed",
            "The loader reported a dependency conflict",
            "error", "Review the exact mod names and versions in Health. Preview matching versions before updating.",
        ),
        (
            "Missing or unsupported mandatory dependencies",
            "confirmed",
            "The loader reported missing or unsupported dependencies",
            "error", "Check required versions in Health and preview fixes for dependencies already installed but disabled.",
        ),
        (
            "MixinApplyError",
            "suspected",
            "A mod mixin may be incompatible",
            "error", "Check the loader and mod versions. Use an isolation trial if the crash is repeatable.",
        ),
        (
            "Can't keep up!",
            "suspected",
            "Server ticks may be falling behind",
            "warning", "Compare the same world and settings across sessions. Tick lag can have several causes.",
        ),
        ("JsonSyntaxException", "suspected", "A JSON file may be invalid", "warning", "Check the file named near the error. Compare it with the last working snapshot before editing."),
        ("DuplicateModsFoundException", "confirmed", "The loader reported duplicate mods", "error", "Use Health to find duplicate IDs and review their dependents before disabling anything."),
        ("Registry remapping failed", "suspected", "Registry remapping failed", "error", "Back up your world before launching again. Compare removed mods with the last working snapshot."),
        ("Unsupported Java", "suspected", "The log reports a Java compatibility problem", "warning", "Check the Minecraft version's required Java runtime in instance settings."),
        ("IncompatibleClassChangeError", "confirmed", "Java found incompatible classes", "error", "Check the reported mod and its dependencies for matching builds."),
    ] {
        if let Some(line) = lines.iter().find(|line| line.contains(needle)) {
            static ATTRIBUTION: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#"(?i)\bfrom mod\s+['"]?([a-z][a-z0-9_-]{1,63})"#).unwrap());
            let suspected_mod = if needle == "MixinApplyError" {
                ATTRIBUTION.captures(line).and_then(|match_| match_.get(1)).map(|value| value.as_str().to_owned())
            } else { None };
            findings.push(Finding {
                certainty,
                title,
                evidence: clip(line),
                severity,
                confidence: if certainty == "confirmed" { "high" } else { "medium" },
                recommendation,
                suspected_mod,
            });
        }
    }
    let mut repeated = HashMap::new();
    for line in lines {
        let message = line
            .split_once("ERROR]: ")
            .or_else(|| line.split_once("ERROR] "))
            .map(|(_, message)| ("error", message))
            .or_else(|| {
                line.split_once("WARN]: ")
                    .or_else(|| line.split_once("WARN] "))
                    .map(|(_, message)| ("info", message))
            });
        if let Some((level, message)) = message.filter(|(_, message)| !message.trim().is_empty()) {
            *repeated.entry((level, message)).or_insert(0) += 1;
        }
    }
    let mut repeated: Vec<_> = repeated
        .into_iter()
        .filter(|((level, _), count)| *count >= if *level == "error" { 4 } else { 10 })
        .collect();
    repeated.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for ((level, message), count) in repeated.into_iter().take(5) {
        findings.push(Finding {
            certainty: "confirmed",
            title: if level == "error" { "Repeated error in this log" } else { "Repeated warning in this log" },
            evidence: format!("{count} occurrences · {}", clip(message)),
            severity: if level == "error" { "warning" } else { "info" },
            confidence: "high",
            recommendation: "Compare this message with the failure time. Repetition alone does not establish a cause.",
            suspected_mod: None,
        });
    }
    findings
}

pub fn share_summary(store: &Store, id: Uuid) -> Result<String> {
    let instance = store.instance(id)?;
    let mut report = format!(
        "Voltra diagnostic summary\nMinecraft {}\nLoader {}\n",
        instance.options.minecraft_version,
        instance.options.loader.as_str()
    );
    let mut total = 0;
    for (label, kind) in [
        ("Crash report", LogKind::Crash),
        ("Game log", LogKind::Game),
        ("Launcher console", LogKind::Console),
        ("Test console", LogKind::Test),
    ] {
        if let Some(log) = read(store, id, kind)? {
            report.push_str(&format!(
                "\n{label}{}\n",
                if log.truncated {
                    " (recent output)"
                } else {
                    ""
                }
            ));
            if log.findings.is_empty() {
                report.push_str("No rules matched.\n");
            }
            for finding in log.findings {
                total += 1;
                report.push_str(&format!(
                    "- {} ({} confidence). {}\n",
                    finding.title, finding.confidence, finding.recommendation
                ));
            }
        }
    }
    report.push_str(&format!(
        "\n{total} observations. Raw logs and identifying details are omitted.\n"
    ));
    Ok(report)
}

pub fn session_signals(store: &Store, id: Uuid, started_at: u64) -> Result<Vec<String>> {
    let game = store.game_dir(id)?;
    let crash_dir = game.join("crash-reports");
    let mut paths = vec![
        store.root.join("logs").join(format!("{id}.log")),
        game.join("logs/latest.log"),
    ];
    let mut crashes = list_files(&crash_dir)?;
    crashes.retain(|name| name.starts_with("crash-") && name.ends_with(".txt"));
    if let Some(name) = crashes.into_iter().max() {
        paths.push(crash_dir.join(name));
    }
    let mut titles = std::collections::BTreeSet::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        regular_file(&path)?;
        let modified = fs::metadata(&path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if modified < started_at {
            continue;
        }
        let (text, _) = tail(&path)?;
        titles.extend(
            analyze(&text)
                .into_iter()
                .filter(|item| item.severity == "error")
                .map(|item| item.title.to_owned()),
        );
    }
    Ok(titles.into_iter().collect())
}

fn clip(value: &str) -> String {
    value.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings_distinguish_observations_from_causes() {
        let lines = [
            "[12:00:01] [Server/ERROR]: Test",
            "[12:00:02] [Server/ERROR]: Test",
            "[12:00:03] [Server/ERROR]: Test",
            "[12:00:04] [Server/ERROR]: Test",
            "Caused by MixinApplyError",
        ];
        let result = analyze(&lines.join("\n"));
        assert_eq!(result[0].certainty, "suspected");
        assert_eq!(result[1].certainty, "confirmed");
        assert!(result[1].evidence.starts_with("4 occurrences"));
    }

    #[test]
    fn share_summary_excludes_version_log_text_and_instance_name() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::new(root.path().to_owned())?;
        let id = store.create(serde_json::from_value(serde_json::json!({"name":"private-pack-name", "minecraftVersion":"1.21.1", "loader":"fabric", "ramMib":4096}))?)?.instance.id;
        fs::write(
            store.root.join("logs").join(format!("{id}.log")),
            "java.lang.OutOfMemoryError in C:/Users/private-user; token=secret-value",
        )?;
        let summary = share_summary(&store, id)?;
        assert!(summary.contains("out-of-memory error"));
        for secret in [
            "private-pack-name",
            "private-user",
            "secret-value",
            &id.to_string(),
        ] {
            assert!(!summary.contains(secret));
        }
        Ok(())
    }
}
