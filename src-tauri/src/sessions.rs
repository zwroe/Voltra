use std::{fs, path::PathBuf};

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{self, Store};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub elapsed_seconds: u64,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub fps: Option<f32>,
    pub frame_ms: Option<f32>,
    pub stutters: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Telemetry {
    session: Uuid,
    sequence: u64,
    fps: Option<f32>,
    frame_ms: Option<f32>,
    stutters: Option<u32>,
    world: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    id: Uuid,
    started_at: u64,
    duration_seconds: u64,
    preparation_seconds: u64,
    pub outcome: Outcome,
    exit_code: Option<i32>,
    configured_ram_mib: u32,
    sample_count: u64,
    mean_cpu_percent: f32,
    peak_memory_bytes: u64,
    mean_fps: Option<f32>,
    mean_frame_ms: Option<f32>,
    stutters: Option<u32>,
    #[serde(default)]
    diagnostic_signals: Vec<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Interrupted,
    Closed,
    Crashed,
    LaunchFailed,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(flatten)]
    summary: Summary,
    samples: Vec<Sample>,
    world_seconds: std::collections::BTreeMap<String, u64>,
    #[serde(skip)]
    telemetry_sequence: Option<u64>,
    #[serde(skip)]
    previous_world: Option<(String, u64)>,
    #[serde(skip)]
    fps_count: u32,
    #[serde(skip)]
    frame_count: u32,
    #[serde(skip)]
    sample_stride: u64,
}

impl Session {
    pub fn new(
        id: Uuid,
        started_at: u64,
        preparation_seconds: u64,
        configured_ram_mib: u32,
    ) -> Self {
        Self {
            summary: Summary {
                id,
                started_at,
                preparation_seconds,
                configured_ram_mib,
                duration_seconds: 0,
                outcome: Outcome::Interrupted,
                exit_code: None,
                sample_count: 0,
                mean_cpu_percent: 0.0,
                peak_memory_bytes: 0,
                mean_fps: None,
                mean_frame_ms: None,
                stutters: None,
                diagnostic_signals: Vec::new(),
            },
            samples: Vec::new(),
            world_seconds: Default::default(),
            telemetry_sequence: None,
            previous_world: None,
            fps_count: 0,
            frame_count: 0,
            sample_stride: 1,
        }
    }

    pub fn update(
        &mut self,
        store: &Store,
        instance: Uuid,
        elapsed: u64,
        process: Option<(f32, u64)>,
    ) {
        self.summary.duration_seconds = elapsed;
        let Some((cpu, memory)) = process else {
            return;
        };
        let mut sample = Sample {
            elapsed_seconds: elapsed,
            cpu_percent: cpu.max(0.0),
            memory_bytes: memory,
            fps: None,
            frame_ms: None,
            stutters: None,
        };
        let telemetry = telemetry(store, instance, self.summary.id).filter(|frame| {
            self.telemetry_sequence
                .is_none_or(|sequence| frame.sequence > sequence)
        });
        if let Some(frame) = telemetry {
            self.telemetry_sequence = Some(frame.sequence);
            sample.fps = frame
                .fps
                .filter(|value| value.is_finite() && (0.0..=10_000.0).contains(value));
            sample.frame_ms = frame
                .frame_ms
                .filter(|value| value.is_finite() && (0.0..=60_000.0).contains(value));
            sample.stutters = frame.stutters;
            if let Some(value) = frame.stutters {
                self.summary.stutters =
                    Some(self.summary.stutters.map_or(value, |old| old.max(value)));
            }
            if let Some(value) = sample.fps {
                running_mean(&mut self.summary.mean_fps, &mut self.fps_count, value);
            }
            if let Some(value) = sample.frame_ms {
                running_mean(
                    &mut self.summary.mean_frame_ms,
                    &mut self.frame_count,
                    value,
                );
            }
            let world = frame.world.filter(|name| {
                !name.is_empty()
                    && name.chars().count() <= 100
                    && !name.chars().any(char::is_control)
            });
            if let Some((previous, at)) = self.previous_world.take() {
                if world.as_deref() == Some(&previous) && elapsed.saturating_sub(at) <= 15 {
                    *self.world_seconds.entry(previous).or_default() += elapsed.saturating_sub(at);
                }
            }
            self.previous_world = world.map(|name| (name, elapsed));
        }
        self.summary.sample_count += 1;
        self.summary.mean_cpu_percent +=
            (sample.cpu_percent - self.summary.mean_cpu_percent) / self.summary.sample_count as f32;
        self.summary.peak_memory_bytes = self.summary.peak_memory_bytes.max(memory);
        if self.summary.sample_count % self.sample_stride == 0 {
            self.samples.push(sample);
        }
        if self.samples.len() >= 360 {
            self.samples = self.samples.drain(..).step_by(2).collect();
            self.sample_stride *= 2;
        }
    }

    pub fn finish(&mut self, elapsed: u64, status: &std::io::Result<std::process::ExitStatus>) {
        self.summary.duration_seconds = elapsed;
        self.summary.exit_code = status.as_ref().ok().and_then(|value| value.code());
        self.summary.outcome = match status {
            Ok(value) if value.success() => Outcome::Closed,
            Ok(_) => Outcome::Crashed,
            Err(_) => Outcome::Interrupted,
        };
    }

    pub fn failed(&mut self) {
        self.summary.outcome = Outcome::LaunchFailed;
    }

    pub fn add_signals(&mut self, signals: Vec<String>) {
        self.summary.diagnostic_signals = signals;
    }

    pub fn save(&self, store: &Store, instance: Uuid) -> Result<()> {
        let directory = directory(store, instance);
        storage::check_directory(&store.instance_dir(instance))?;
        fs::create_dir_all(&directory)?;
        storage::check_directory(&directory)?;
        storage::atomic_json(&directory.join(format!("{}.json", self.summary.id)), self)
    }
}

fn running_mean(mean: &mut Option<f32>, count: &mut u32, value: f32) {
    *count += 1;
    let average = mean.get_or_insert(value);
    *average += (value - *average) / *count as f32;
}

fn telemetry(store: &Store, instance: Uuid, session: Uuid) -> Option<Telemetry> {
    let path = store.game_dir(instance).ok()?.join("voltra-telemetry.json");
    storage::regular_file(&path).ok()?;
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > 8192 {
        return None;
    }
    let value: Telemetry = storage::read_json(&path).ok()?;
    (value.session == session).then_some(value)
}

fn directory(store: &Store, instance: Uuid) -> PathBuf {
    store.instance_dir(instance).join("sessions")
}

pub fn detail(store: &Store, instance: Uuid, session: Uuid) -> Result<Session> {
    store.instance(instance)?;
    let dir = directory(store, instance);
    storage::check_directory(&dir)?;
    let path = dir.join(format!("{session}.json"));
    storage::regular_file(&path)?;
    let metadata = fs::metadata(&path)?;
    ensure!(metadata.len() <= 256 * 1024, "Session record is too large.");
    storage::read_json(&path)
}

pub fn list(store: &Store, instance: Uuid) -> Result<Vec<Summary>> {
    store.instance(instance)?;
    let dir = directory(store, instance);
    let mut rows = Vec::new();
    for name in storage::list_files(&dir)? {
        if let Some(id) = name
            .strip_suffix(".json")
            .and_then(|name| Uuid::parse_str(name).ok())
        {
            if let Ok(session) = detail(store, instance, id) {
                rows.push(session.summary);
            }
        }
    }
    rows.sort_unstable_by_key(|row| std::cmp::Reverse((row.started_at, row.id)));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_samples_and_accepts_only_current_telemetry() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::new(root.path().to_owned())?;
        let instance = store.create(serde_json::from_value(serde_json::json!({"name":"Test pack", "minecraftVersion":"1.21.1", "loader":"fabric", "ramMib":4096}))?)?.instance.id;
        let file = store.game_dir(instance)?.join("voltra-telemetry.json");
        let id = Uuid::new_v4();
        let mut record = Session::new(id, 150, 6, 4096);
        storage::atomic_json(
            &file,
            &serde_json::json!({"session":Uuid::new_v4(), "sequence":1, "fps":500}),
        )?;
        record.update(&store, instance, 6, Some((20.0, 100)));
        for (elapsed, sequence, fps, stutters) in [(12, 1, 60, 1), (18, 2, 90, 3)] {
            storage::atomic_json(
                &file,
                &serde_json::json!({"session":id, "sequence":sequence, "fps":fps, "frameMs":10, "stutters":stutters, "world":"Survival"}),
            )?;
            record.update(&store, instance, elapsed, Some((40.0, 300)));
        }
        record.update(&store, instance, 24, Some((60.0, 200)));
        record.save(&store, instance)?;
        let loaded = detail(&store, instance, id)?;
        assert_eq!(loaded.summary.peak_memory_bytes, 300);
        assert_eq!(loaded.summary.mean_cpu_percent, 40.0);
        assert_eq!(loaded.summary.mean_fps, Some(75.0));
        assert_eq!(loaded.summary.stutters, Some(3));
        assert_eq!(loaded.world_seconds["Survival"], 6);
        assert_eq!(list(&store, instance)?.len(), 1);
        Ok(())
    }
}
