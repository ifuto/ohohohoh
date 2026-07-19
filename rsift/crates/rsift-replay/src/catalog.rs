//! Replay catalog — list past recordings with duration

use crate::format::{format_duration, RsrMetadata};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ReplayCatalog {
    pub replays_dir: PathBuf,
    pub entries: Vec<RsrMetadata>,
}

impl ReplayCatalog {
    pub fn new(replays_dir: impl Into<PathBuf>) -> Self {
        let dir = replays_dir.into();
        let mut catalog = Self {
            replays_dir: dir.clone(),
            entries: Vec::new(),
        };
        catalog.refresh();
        catalog
    }

    pub fn refresh(&mut self) {
        self.entries.clear();
        let Ok(read_dir) = fs::read_dir(&self.replays_dir) else { return };

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<RsrMetadata>(&data) {
                    self.entries.push(meta);
                }
            }
        }
        self.entries.sort_by(|a, b| b.recorded_at.cmp(&a.recorded_at));
        info!("[RsReplay] Catalog: {} replays", self.entries.len());
    }

    pub fn list_display(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|e| (e.filename.clone(), e.duration_display.clone()))
            .collect()
    }

    pub fn replay_path(&self, filename: &str) -> PathBuf {
        self.replays_dir.join(filename)
    }
}
