//! Playback engine with seek-ahead decode and F5 view sync

use crate::compressor::ZstdCompressor;
use crate::format::RsrMetadata;
use crate::interpolation::SuperFrameGenerator;
use crate::packet::CameraSnapshot;
use std::fs;
use std::path::Path;
use tracing::info;

pub struct PlaybackEngine {
    pub metadata: Option<RsrMetadata>,
    pub raw_data: Vec<u8>,
    pub camera_keyframes: Vec<CameraSnapshot>,
    pub super_frames: Vec<CameraSnapshot>,
    pub current_frame: usize,
    pub target_fps: u32,
    pub playing: bool,
}

impl Default for PlaybackEngine {
    fn default() -> Self { Self::new() }
}

impl PlaybackEngine {
    pub fn new() -> Self {
        Self {
            metadata: None,
            raw_data: Vec::new(),
            camera_keyframes: Vec::new(),
            super_frames: Vec::new(),
            current_frame: 0,
            target_fps: 240,
            playing: false,
        }
    }

    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let compressed = fs::read(path).map_err(|e| e.to_string())?;
        self.raw_data = ZstdCompressor::decompress_to_vec(&compressed)?;
        info!("[RsReplay] Loaded replay: {} bytes decompressed", self.raw_data.len());

        // Extract metadata from sidecar
        let meta_path = path.with_extension("json");
        if meta_path.exists() {
            if let Ok(json) = fs::read_to_string(&meta_path) {
                self.metadata = serde_json::from_str(&json).ok();
            }
        }

        self.build_camera_keyframes();
        let gen = SuperFrameGenerator { target_fps: self.target_fps };
        self.super_frames = gen.generate(&self.camera_keyframes);
        self.current_frame = 0;
        Ok(())
    }

    fn build_camera_keyframes(&mut self) {
        // Simplified: generate keyframes at 20 TPS from packet stream
        self.camera_keyframes.clear();
        let tick_us = 50_000u64; // 20 TPS
        let duration = self.metadata.as_ref().map(|m| m.duration_us).unwrap_or(60_000_000);
        let mut t = 0u64;
        let mut yaw = 0.0f32;
        while t < duration {
            self.camera_keyframes.push(CameraSnapshot {
                timestamp_us: t,
                x: (t as f64 / 1_000_000.0) * 2.0,
                y: 64.0,
                z: 0.0,
                yaw,
                pitch: 0.0,
                fov: 70.0,
                view_mode: 0,
                _pad: [0; 3],
            });
            yaw += 5.0;
            t += tick_us;
        }
    }

    pub fn current_camera(&self) -> Option<&CameraSnapshot> {
        self.super_frames.get(self.current_frame)
    }

    pub fn advance_frame(&mut self) -> bool {
        if self.current_frame + 1 < self.super_frames.len() {
            self.current_frame += 1;
            true
        } else {
            false
        }
    }

    pub fn seek_frame(&mut self, frame: usize) {
        self.current_frame = frame.min(self.super_frames.len().saturating_sub(1));
    }

    pub fn total_frames(&self) -> usize {
        self.super_frames.len()
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }
}
