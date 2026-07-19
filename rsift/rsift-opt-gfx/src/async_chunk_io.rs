//! Async chunk I/O with LZ4 + LRU priority load (Tier 3).
//! Near-player chunks dequeue first; background thread never blocks render.

use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::fs;

#[derive(Debug, Clone, Eq, PartialEq)]
struct PrioritizedKey {
    /// Lower = higher priority (distance²).
    dist2: i64,
    cx: i32,
    cz: i32,
}

impl Ord for PrioritizedKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // Max-heap by inverted distance → BinaryHeap pops smallest dist first via Reverse semantics
        other
            .dist2
            .cmp(&self.dist2)
            .then(self.cx.cmp(&other.cx))
            .then(self.cz.cmp(&other.cz))
    }
}
impl PartialOrd for PrioritizedKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

enum IoCmd {
    Load(PrioritizedKey),
    Store {
        cx: i32,
        cz: i32,
        bytes: Vec<u8>,
    },
    Stop,
}

enum IoEvent {
    Loaded {
        cx: i32,
        cz: i32,
        bytes: Vec<u8>,
    },
    Failed {
        cx: i32,
        cz: i32,
    },
    Stored {
        cx: i32,
        cz: i32,
    },
}

struct LruEntry {
    key: (i32, i32),
}

pub struct AsyncChunkIo {
    root: PathBuf,
    capacity: usize,
    cache: HashMap<(i32, i32), Vec<u8>>,
    lru: VecDeque<(i32, i32)>,
    pending: BinaryHeap<PrioritizedKey>,
    cmd_tx: Sender<IoCmd>,
    evt_rx: Receiver<IoEvent>,
    _worker: JoinHandle<()>,
    hits: u64,
    misses: u64,
}

impl AsyncChunkIo {
    pub fn new(root: impl Into<PathBuf>, capacity: usize) -> Self {
        let root = root.into();
        let _ = fs::create_dir_all(&root);
        let (cmd_tx, cmd_rx) = mpsc::channel::<IoCmd>();
        let (evt_tx, evt_rx) = mpsc::channel::<IoEvent>();
        let root_cl = root.clone();
        let worker = thread::Builder::new()
            .name("rsift-chunk-io".into())
            .spawn(move || worker_loop(root_cl, cmd_rx, evt_tx))
            .expect("spawn chunk io");
        Self {
            root,
            capacity: capacity.max(8),
            cache: HashMap::new(),
            lru: VecDeque::new(),
            pending: BinaryHeap::new(),
            cmd_tx,
            evt_rx,
            _worker: worker,
            hits: 0,
            misses: 0,
        }
    }

    fn path(root: &Path, cx: i32, cz: i32) -> PathBuf {
        root.join(format!("c.{}.{}.lz4", cx, cz))
    }

    pub fn request_load(&mut self, cx: i32, cz: i32, player_cx: i32, player_cz: i32) {
        if self.cache.contains_key(&(cx, cz)) {
            self.touch(cx, cz);
            self.hits += 1;
            return;
        }
        self.misses += 1;
        let dx = (cx - player_cx) as i64;
        let dz = (cz - player_cz) as i64;
        let key = PrioritizedKey {
            dist2: dx * dx + dz * dz,
            cx,
            cz,
        };
        let _ = self.cmd_tx.send(IoCmd::Load(key.clone()));
        self.pending.push(key);
    }

    pub fn store(&mut self, cx: i32, cz: i32, raw: &[u8]) {
        let compressed = compress_prepend_size(raw);
        self.insert_cache(cx, cz, raw.to_vec());
        let _ = self.cmd_tx.send(IoCmd::Store {
            cx,
            cz,
            bytes: compressed,
        });
    }

    pub fn poll_ready(&mut self) -> Vec<(i32, i32, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                IoEvent::Loaded { cx, cz, bytes } => {
                    if let Ok(raw) = decompress_size_prepended(&bytes) {
                        self.insert_cache(cx, cz, raw.clone());
                        out.push((cx, cz, raw));
                    }
                }
                IoEvent::Failed { .. } | IoEvent::Stored { .. } => {}
            }
        }
        out
    }

    pub fn get_cached(&mut self, cx: i32, cz: i32) -> Option<&Vec<u8>> {
        if self.cache.contains_key(&(cx, cz)) {
            self.touch(cx, cz);
            self.cache.get(&(cx, cz))
        } else {
            None
        }
    }

    fn touch(&mut self, cx: i32, cz: i32) {
        let key = (cx, cz);
        if let Some(pos) = self.lru.iter().position(|k| *k == key) {
            self.lru.remove(pos);
        }
        self.lru.push_back(key);
    }

    fn insert_cache(&mut self, cx: i32, cz: i32, data: Vec<u8>) {
        let key = (cx, cz);
        self.cache.insert(key, data);
        self.touch(cx, cz);
        while self.cache.len() > self.capacity {
            if let Some(old) = self.lru.pop_front() {
                self.cache.remove(&old);
            } else {
                break;
            }
        }
    }

    pub fn evict_lru(&mut self) {
        if let Some(old) = self.lru.pop_front() {
            self.cache.remove(&old);
        }
    }

    pub fn stats(&self) -> (u64, u64, usize) {
        (self.hits, self.misses, self.cache.len())
    }
}

impl Drop for AsyncChunkIo {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(IoCmd::Stop);
    }
}

fn worker_loop(root: PathBuf, rx: Receiver<IoCmd>, tx: Sender<IoEvent>) {
    let queue: Arc<Mutex<BinaryHeap<PrioritizedKey>>> = Arc::new(Mutex::new(BinaryHeap::new()));
    while let Ok(cmd) = rx.recv() {
        match cmd {
            IoCmd::Stop => break,
            IoCmd::Load(key) => {
                queue.lock().unwrap().push(key);
                // Drain all pending loads in priority order
                while let Some(k) = queue.lock().unwrap().pop() {
                    let path = AsyncChunkIo::path(&root, k.cx, k.cz);
                    match fs::read(&path) {
                        Ok(bytes) => {
                            let _ = tx.send(IoEvent::Loaded {
                                cx: k.cx,
                                cz: k.cz,
                                bytes,
                            });
                        }
                        Err(_) => {
                            let _ = tx.send(IoEvent::Failed {
                                cx: k.cx,
                                cz: k.cz,
                            });
                        }
                    }
                    // Also process any cmds that arrived
                    while let Ok(extra) = rx.try_recv() {
                        match extra {
                            IoCmd::Stop => return,
                            IoCmd::Load(k2) => queue.lock().unwrap().push(k2),
                            IoCmd::Store { cx, cz, bytes } => {
                                let path = AsyncChunkIo::path(&root, cx, cz);
                                let _ = fs::write(path, bytes);
                                let _ = tx.send(IoEvent::Stored { cx, cz });
                            }
                        }
                    }
                }
            }
            IoCmd::Store { cx, cz, bytes } => {
                let path = AsyncChunkIo::path(&root, cx, cz);
                let _ = fs::write(path, bytes);
                let _ = tx.send(IoEvent::Stored { cx, cz });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lz4_roundtrip_store_load() {
        let dir = std::env::temp_dir().join("rsift_chunk_io_test");
        let _ = fs::remove_dir_all(&dir);
        let mut io = AsyncChunkIo::new(&dir, 16);
        let payload = vec![7u8; 4096];
        io.store(1, 2, &payload);
        thread::sleep(Duration::from_millis(50));
        let _ = io.poll_ready();
        // Force disk load
        io.cache.clear();
        io.lru.clear();
        io.request_load(1, 2, 0, 0);
        thread::sleep(Duration::from_millis(80));
        let ready = io.poll_ready();
        assert!(!ready.is_empty() || io.get_cached(1, 2).is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
