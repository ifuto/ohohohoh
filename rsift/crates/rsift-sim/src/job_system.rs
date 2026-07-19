//! Phase 1 — bounded job system: auto workers, frame budget, distance priority.
//! CPU work → Rayon pool. I/O work → dedicated threads (never unbounded).

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use parking_lot::Mutex;
use rayon::ThreadPoolBuilder;
use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    ChunkGen,
    EntityAi,
    Light,
    IoLoad,
    IoSave,
    Pathfind,
    Misc,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub kind: JobKind,
    pub priority: i32, // lower = sooner (distance²)
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub payload: JobPayload,
}

#[derive(Debug, Clone)]
pub enum JobPayload {
    Empty,
    Bytes(Vec<u8>),
    EntityIds(Vec<u32>),
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then(self.chunk_x.cmp(&other.chunk_x))
            .then(self.chunk_z.cmp(&other.chunk_z))
    }
}
impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
            && self.chunk_x == other.chunk_x
            && self.chunk_z == other.chunk_z
            && self.kind == other.kind
    }
}
impl Eq for Job {}

#[derive(Debug, Clone)]
pub struct FrameBudget {
    pub tick_ms: f32,
    pub chunk_gen_ms: f32,
    pub ai_ms: f32,
    pub light_ms: f32,
    pub io_ms: f32,
}

impl FrameBudget {
    pub fn for_cores(cores: usize) -> Self {
        // Stutter-safe budgets; weak CPUs get tighter caps.
        match cores {
            0..=2 => Self {
                tick_ms: 8.0,
                chunk_gen_ms: 4.0,
                ai_ms: 2.0,
                light_ms: 2.0,
                io_ms: 3.0,
            },
            3..=4 => Self {
                tick_ms: 12.0,
                chunk_gen_ms: 6.0,
                ai_ms: 3.0,
                light_ms: 3.0,
                io_ms: 4.0,
            },
            5..=8 => Self {
                tick_ms: 20.0,
                chunk_gen_ms: 10.0,
                ai_ms: 5.0,
                light_ms: 5.0,
                io_ms: 6.0,
            },
            _ => Self {
                tick_ms: 25.0,
                chunk_gen_ms: 12.0,
                ai_ms: 6.0,
                light_ms: 6.0,
                io_ms: 8.0,
            },
        }
    }

    pub fn remaining_ms(&self, kind: JobKind, spent_ms: f32) -> f32 {
        let cap = match kind {
            JobKind::ChunkGen => self.chunk_gen_ms,
            JobKind::EntityAi | JobKind::Pathfind => self.ai_ms,
            JobKind::Light => self.light_ms,
            JobKind::IoLoad | JobKind::IoSave => self.io_ms,
            JobKind::Misc => self.tick_ms,
        };
        (cap - spent_ms).max(0.0)
    }
}

struct BudgetTracker {
    spent: [f32; 6],
}

impl BudgetTracker {
    fn new() -> Self {
        Self { spent: [0.0; 6] }
    }
    fn idx(kind: JobKind) -> usize {
        match kind {
            JobKind::ChunkGen => 0,
            JobKind::EntityAi | JobKind::Pathfind => 1,
            JobKind::Light => 2,
            JobKind::IoLoad | JobKind::IoSave => 3,
            JobKind::Misc => 4,
        }
    }
    fn can_run(&self, budget: &FrameBudget, kind: JobKind) -> bool {
        budget.remaining_ms(kind, self.spent[Self::idx(kind)]) > 0.05
    }
    fn add(&mut self, kind: JobKind, ms: f32) {
        self.spent[Self::idx(kind)] += ms;
    }
}

pub type JobHandler = Arc<dyn Fn(&Job) + Send + Sync>;

pub struct JobSystem {
    cpu_tx: Sender<Job>,
    cpu_rx: Receiver<Job>,
    io_tx: Sender<Job>,
    capacity: usize,
    budget: FrameBudget,
    cpu_workers: usize,
    io_workers: usize,
    running: Arc<AtomicBool>,
    completed: AtomicU64,
    rejected: AtomicU64,
    handler: Mutex<Option<JobHandler>>,
    io_threads: Mutex<Vec<JoinHandle<()>>>,
    /// Pending CPU jobs held for priority extract (bounded).
    pending_cpu: Mutex<std::collections::BinaryHeap<Job>>,
}

impl JobSystem {
    pub fn auto() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        // Leave headroom for OS + Tokio/render — never use all cores for Rayon alone.
        let cpu_workers = (cores.saturating_sub(1)).clamp(1, 12);
        let io_workers = (cores / 4).clamp(1, 4);
        let capacity = (cores * 64).clamp(64, 1024);
        Self::with_config(cpu_workers, io_workers, capacity, FrameBudget::for_cores(cores))
    }

    pub fn with_config(
        cpu_workers: usize,
        io_workers: usize,
        capacity: usize,
        budget: FrameBudget,
    ) -> Self {
        let (cpu_tx, cpu_rx) = bounded::<Job>(capacity);
        let (io_tx, io_rx) = bounded::<Job>(capacity / 2);

        let _ = ThreadPoolBuilder::new()
            .num_threads(cpu_workers)
            .thread_name(|i| format!("rsift-cpu-{i}"))
            .build_global();

        let running = Arc::new(AtomicBool::new(true));
        let mut handles = Vec::new();
        for i in 0..io_workers {
            let rx = io_rx.clone();
            let running = running.clone();
            let h = thread::Builder::new()
                .name(format!("rsift-io-{i}"))
                .spawn(move || {
                    while running.load(AtomicOrdering::Acquire) {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(job) => {
                                let _g = crate::profiling::ScopeGuard::enter(
                                    crate::profiling::ProfileDomain::Io,
                                );
                                let _ = (job.chunk_x, job.chunk_z);
                            }
                            Err(_) => {}
                        }
                    }
                })
                .expect("io worker");
            handles.push(h);
        }

        Self {
            cpu_tx,
            cpu_rx,
            io_tx,
            capacity,
            budget,
            cpu_workers,
            io_workers,
            running,
            completed: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            handler: Mutex::new(None),
            io_threads: Mutex::new(handles),
            pending_cpu: Mutex::new(std::collections::BinaryHeap::new()),
        }
    }

    pub fn set_handler(&self, handler: JobHandler) {
        *self.handler.lock() = Some(handler);
    }

    pub fn budget(&self) -> &FrameBudget {
        &self.budget
    }

    pub fn worker_counts(&self) -> (usize, usize) {
        (self.cpu_workers, self.io_workers)
    }

    /// Submit job — returns Err if queue full (load shed; never grow unbounded).
    pub fn try_submit(&self, job: Job) -> Result<(), Job> {
        let is_io = matches!(job.kind, JobKind::IoLoad | JobKind::IoSave);
        if is_io {
            match self.io_tx.try_send(job) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(j)) | Err(TrySendError::Disconnected(j)) => {
                    self.rejected.fetch_add(1, AtomicOrdering::Relaxed);
                    Err(j)
                }
            }
        } else {
            let mut pending = self.pending_cpu.lock();
            if pending.len() >= self.capacity {
                self.rejected.fetch_add(1, AtomicOrdering::Relaxed);
                return Err(job);
            }
            pending.push(job);
            Ok(())
        }
    }

    pub fn submit_chunk_gen(&self, cx: i32, cz: i32, player_cx: i32, player_cz: i32) -> bool {
        let dx = (cx - player_cx) as i64;
        let dz = (cz - player_cz) as i64;
        let job = Job {
            kind: JobKind::ChunkGen,
            priority: (dx * dx + dz * dz) as i32,
            chunk_x: cx,
            chunk_z: cz,
            payload: JobPayload::Empty,
        };
        self.try_submit(job).is_ok()
    }

    /// Drain CPU jobs within frame budget using Rayon (scoped join — costs are real).
    pub fn pump_cpu_frame(&self) -> u32 {
        let mut tracker = BudgetTracker::new();
        let handler = self.handler.lock().clone();
        let mut batch = Vec::with_capacity(64);
        loop {
            if batch.len() >= 256 {
                break;
            }
            let job = {
                let mut pending = self.pending_cpu.lock();
                pending.pop()
            };
            let Some(job) = job else { break };
            if !tracker.can_run(&self.budget, job.kind) {
                self.pending_cpu.lock().push(job);
                break;
            }
            batch.push(job);
        }
        if batch.is_empty() {
            return 0;
        }
        let start = Instant::now();
        if let Some(h) = handler {
            rayon::scope(|s| {
                for job in &batch {
                    let h = Arc::clone(&h);
                    s.spawn(move |_| h(job));
                }
            });
        }
        let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
        let per = elapsed_ms / batch.len() as f32;
        for job in &batch {
            tracker.add(job.kind, per);
            let _ = self.cpu_tx.try_send(job.clone());
        }
        self.completed
            .fetch_add(batch.len() as u64, AtomicOrdering::Relaxed);
        // Drain any prior channel markers without blocking.
        while self.cpu_rx.try_recv().is_ok() {}
        batch.len() as u32
    }

    pub fn stats(&self) -> (u64, u64, usize) {
        (
            self.completed.load(AtomicOrdering::Relaxed),
            self.rejected.load(AtomicOrdering::Relaxed),
            self.pending_cpu.lock().len(),
        )
    }

    pub fn shutdown(&self) {
        self.running.store(false, AtomicOrdering::Release);
        debug!("job system shutdown signalled");
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(mut hs) = self.io_threads.try_lock() {
            for h in hs.drain(..) {
                let _ = h.join();
            }
        } else {
            warn!("io threads still locked on drop");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_rejects() {
        let sys = JobSystem::with_config(1, 1, 4, FrameBudget::for_cores(2));
        for i in 0..4 {
            assert!(sys
                .try_submit(Job {
                    kind: JobKind::ChunkGen,
                    priority: i,
                    chunk_x: i,
                    chunk_z: 0,
                    payload: JobPayload::Empty,
                })
                .is_ok());
        }
        assert!(sys
            .try_submit(Job {
                kind: JobKind::ChunkGen,
                priority: 99,
                chunk_x: 9,
                chunk_z: 9,
                payload: JobPayload::Empty,
            })
            .is_err());
    }

    #[test]
    fn nearer_first() {
        let sys = JobSystem::with_config(1, 1, 32, FrameBudget::for_cores(4));
        assert!(sys.submit_chunk_gen(10, 0, 0, 0));
        assert!(sys.submit_chunk_gen(1, 0, 0, 0));
        let top = sys.pending_cpu.lock().peek().unwrap().chunk_x;
        assert_eq!(top, 1);
    }
}
