//! Phase 3 — Folia-inspired region tick: per-region queues, cross-region events.

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::thread::{self, JoinHandle};
use tracing::debug;

#[derive(Debug, Clone)]
pub enum RegionEvent {
    EntityTransfer {
        entity_id: u32,
        from: (i32, i32),
        to: (i32, i32),
    },
    BlockNotify {
        x: i32,
        y: i32,
        z: i32,
    },
    Custom {
        code: u32,
        payload: u64,
    },
}

#[derive(Debug)]
struct RegionWorker {
    rx: i32,
    rz: i32,
    inbox: Receiver<RegionEvent>,
    local_queue: Mutex<Vec<RegionEvent>>,
}

pub struct RegionTickScheduler {
    regions: Mutex<FxHashMap<(i32, i32), Sender<RegionEvent>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    event_capacity: usize,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl RegionTickScheduler {
    pub fn new(event_capacity: usize) -> Self {
        Self {
            regions: Mutex::new(FxHashMap::default()),
            workers: Mutex::new(Vec::new()),
            event_capacity: event_capacity.max(32),
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    pub fn ensure_region(&self, rx: i32, rz: i32) {
        let mut map = self.regions.lock();
        if map.contains_key(&(rx, rz)) {
            return;
        }
        let (tx, rx_ch) = bounded(self.event_capacity);
        map.insert((rx, rz), tx);
        let running = self.running.clone();
        let h = thread::Builder::new()
            .name(format!("region-{rx}-{rz}"))
            .spawn(move || {
                let worker = RegionWorker {
                    rx,
                    rz,
                    inbox: rx_ch,
                    local_queue: Mutex::new(Vec::new()),
                };
                while running.load(std::sync::atomic::Ordering::Acquire) {
                    match worker.inbox.recv_timeout(std::time::Duration::from_millis(5)) {
                        Ok(ev) => {
                            worker.local_queue.lock().push(ev);
                        }
                        Err(_) => {}
                    }
                    // Drain local queue as one region tick slice
                    let batch: Vec<_> = worker.local_queue.lock().drain(..).collect();
                    if !batch.is_empty() {
                        debug!(region = ?(worker.rx, worker.rz), n = batch.len(), "region tick");
                        let _ = batch;
                    }
                }
            })
            .expect("region worker");
        self.workers.lock().push(h);
    }

    pub fn send(&self, region: (i32, i32), event: RegionEvent) -> Result<(), RegionEvent> {
        self.ensure_region(region.0, region.1);
        let map = self.regions.lock();
        match map.get(&region) {
            Some(tx) => tx.try_send(event).map_err(|e| match e {
                crossbeam_channel::TrySendError::Full(ev)
                | crossbeam_channel::TrySendError::Disconnected(ev) => ev,
            }),
            None => Err(event),
        }
    }

    pub fn chunk_to_region(cx: i32, cz: i32) -> (i32, i32) {
        (cx.div_euclid(32), cz.div_euclid(32))
    }

    pub fn shutdown(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for RegionTickScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_cross_region() {
        let sched = RegionTickScheduler::new(8);
        assert!(sched
            .send(
                (0, 0),
                RegionEvent::Custom {
                    code: 1,
                    payload: 2
                }
            )
            .is_ok());
        sched.shutdown();
    }
}
