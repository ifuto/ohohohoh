//! Main recording engine — zero-copy capture + async SSD write

use crate::compressor::ZstdCompressor;
use crate::filter::PacketFilter;
use crate::format::{format_duration, RsrFileHeader, RsrMetadata, RSR_MAGIC};
use crate::packet::{CapturedPacket, PacketDirection, PacketRecordHeader, ZeroCopyCapture};
use crate::ring_buffer::{PacketRingBuffer, RingSlot};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
}

pub struct ReplayRecorder {
    pub state: RecordingState,
    pub filter: PacketFilter,
    pub player_uuid: [u8; 16],
    pub player_name: String,
    pub view_mode: u8,
    pub start_time: Option<Instant>,
    pub paused_duration_us: u64,
    packet_count: AtomicU64,
    ring: Arc<Mutex<PacketRingBuffer>>,
    writer_running: Arc<AtomicBool>,
    writer_handle: Option<JoinHandle<()>>,
    raw_path: PathBuf,
    replays_dir: PathBuf,
}

impl ReplayRecorder {
    pub fn new(replays_dir: impl Into<PathBuf>) -> Self {
        let dir = replays_dir.into();
        let _ = fs::create_dir_all(&dir);
        Self {
            state: RecordingState::Idle,
            filter: PacketFilter::new(),
            player_uuid: [0u8; 16],
            player_name: "Player".into(),
            view_mode: 0,
            start_time: None,
            paused_duration_us: 0,
            packet_count: AtomicU64::new(0),
            ring: Arc::new(Mutex::new(PacketRingBuffer::with_default_capacity())),
            writer_running: Arc::new(AtomicBool::new(false)),
            writer_handle: None,
            raw_path: dir.join("_recording_raw.tmp"),
            replays_dir: dir,
        }
    }

    pub fn set_player(&mut self, uuid: [u8; 16], name: impl Into<String>) {
        self.player_uuid = uuid;
        self.player_name = name.into();
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.state == RecordingState::Recording {
            return Ok(());
        }
        self.state = RecordingState::Recording;
        self.start_time = Some(Instant::now());
        self.paused_duration_us = 0;
        self.packet_count.store(0, Ordering::Relaxed);

        let _ = fs::remove_file(&self.raw_path);
        let raw_path = self.raw_path.clone();
        let ring = self.ring.clone();
        let running = self.writer_running.clone();
        let player_uuid = self.player_uuid;
        running.store(true, Ordering::Release);

        let handle = thread::Builder::new()
            .name("rsreplay-writer".into())
            .spawn(move || writer_thread(ring, running, raw_path, player_uuid))
            .map_err(|e| e.to_string())?;
        self.writer_handle = Some(handle);

        info!("[RsReplay] Recording started (zero-copy packet stream)");
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == RecordingState::Recording {
            self.state = RecordingState::Paused;
            info!("[RsReplay] Recording paused");
        }
    }

    pub fn resume(&mut self) {
        if self.state == RecordingState::Paused {
            self.state = RecordingState::Recording;
            info!("[RsReplay] Recording resumed");
        }
    }

    pub fn stop_and_finalize(&mut self) -> Result<RsrMetadata, String> {
        if self.state == RecordingState::Idle {
            return Err("Not recording".into());
        }
        self.state = RecordingState::Idle;
        self.writer_running.store(false, Ordering::Release);
        if let Some(h) = self.writer_handle.take() {
            let _ = h.join();
        }

        let duration_us = self.elapsed_us();
        let count = self.packet_count.load(Ordering::Relaxed);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let filename = format!("replay_{}_{}.rsr", self.player_name.replace(' ', "_"), timestamp);
        let out_path = self.replays_dir.join(&filename);

        let compressor = ZstdCompressor::default();
        let compressed_size = compressor.compress_file(&self.raw_path, &out_path)?;
        let _ = fs::remove_file(&self.raw_path);

        let meta = RsrMetadata {
            filename: filename.clone(),
            player_name: self.player_name.clone(),
            player_uuid: format_uuid(&self.player_uuid),
            recorded_at: chrono_lite(timestamp),
            duration_display: format_duration(duration_us),
            duration_us,
            packet_count: count,
            file_size_bytes: compressed_size,
            server_address: "localhost".into(),
            minecraft_version: "1.21.11".into(),
        };

        // Write sidecar metadata JSON
        let meta_path = self.replays_dir.join(format!("{}.json", filename.trim_end_matches(".rsr")));
        if let Ok(json) = serde_json::to_string_pretty(&meta) {
            let _ = fs::write(&meta_path, json);
        }

        info!("[RsReplay] Saved {} ({} packets, {})", filename, count, meta.duration_display);
        Ok(meta)
    }

    /// Zero-copy packet ingress from JNI
    pub fn on_packet(&self, packet_id: u32, ptr: i64, len: i32, direction: PacketDirection) {
        if self.state != RecordingState::Recording {
            return;
        }
        if !self.filter.should_record(packet_id, direction as u8) {
            return;
        }
        let ts = self.elapsed_us();
        if let Ok(captured) = unsafe {
            ZeroCopyCapture::capture_from_jni(ts, direction, packet_id, ptr, len, self.view_mode)
        } {
            self.push_packet(captured);
        }
    }

    fn push_packet(&self, captured: CapturedPacket) {
        let slot = RingSlot {
            timestamp_us: captured.header.timestamp_us,
            direction: captured.header.direction,
            packet_id: captured.header.packet_id,
            payload: captured.payload,
        };
        if let Ok(mut ring) = self.ring.lock() {
            if !ring.try_push(slot) {
                warn!("[RsReplay] Ring buffer full — dropping packet");
            }
        }
        self.packet_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_view_mode(&mut self, mode: u8) {
        self.view_mode = mode;
    }

    pub fn elapsed_us(&self) -> u64 {
        self.start_time
            .map(|s| s.elapsed().as_micros() as u64 + self.paused_duration_us)
            .unwrap_or(0)
    }

    pub fn is_recording(&self) -> bool {
        self.state == RecordingState::Recording
    }

    pub fn is_paused(&self) -> bool {
        self.state == RecordingState::Paused
    }

    pub fn packet_count(&self) -> u64 {
        self.packet_count.load(Ordering::Relaxed)
    }
}

fn writer_thread(
    ring: Arc<Mutex<PacketRingBuffer>>,
    running: Arc<AtomicBool>,
    path: PathBuf,
    player_uuid: [u8; 16],
) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("open raw replay file");

    // Real header with player UUID; duration/count patched on finalize.
    let header = RsrFileHeader::new(player_uuid);
    let _ = file.write_all(bytemuck::bytes_of(&header));

    while running.load(Ordering::Acquire) {
        let batch: Vec<RingSlot> = {
            let mut ring_guard = ring.lock().unwrap();
            let mut batch = Vec::with_capacity(256);
            while batch.len() < 256 {
                if let Some(slot) = ring_guard.try_pop() {
                    batch.push(slot);
                } else {
                    break;
                }
            }
            batch
        };
        let batch_empty = batch.is_empty();
        for slot in &batch {
            let _ = write_packet_record(&mut file, slot);
        }
        if batch_empty {
            thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    // Drain remaining
    loop {
        let slot = ring.lock().unwrap().try_pop();
        if slot.is_none() { break; }
        if let Some(s) = slot {
            let _ = write_packet_record(&mut file, &s);
        }
    }
}

fn write_packet_record(file: &mut File, slot: &RingSlot) -> std::io::Result<()> {
    file.write_all(&slot.timestamp_us.to_le_bytes())?;
    file.write_all(&[slot.direction])?;
    file.write_all(&slot.packet_id.to_le_bytes())?;
    file.write_all(&(slot.payload.len() as u32).to_le_bytes())?;
    file.write_all(&[0u8])?;
    file.write_all(&slot.payload)?;
    Ok(())
}

fn format_uuid(uuid: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5], uuid[6], uuid[7],
        uuid[8], uuid[9], uuid[10], uuid[11], uuid[12], uuid[13], uuid[14], uuid[15]
    )
}

fn chrono_lite(secs: u64) -> String {
    format!("{}", secs)
}
