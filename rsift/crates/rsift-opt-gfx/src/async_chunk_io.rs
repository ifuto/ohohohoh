//! Async chunk I/O with LZ4 + LRU priority load (Tier 3).
//! Near-player chunks dequeue first; background thread never blocks render.

use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

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
    Store { cx: i32, cz: i32, bytes: Vec<u8> },
    Stop,
}

enum IoEvent {
    Loaded { cx: i32, cz: i32, bytes: Vec<u8> },
    // 注: Failed/Stored の cx/cz ペイロードは唯一の消費者 (poll_ready) が
    // 読まないデッドデータだったため unit 化 (2026-07-21 監査)。
    Failed,
    Stored,
}

pub struct AsyncChunkIo {
    // 注: root フィールドは new() 内で create_dir_all 済み + ワーカーが root_cl
    // クローンを保持するため、構造体にも残すと一度も読まれないデッド状態になる
    // (2026-07-21 監査)。同じく未構築の `struct LruEntry` は削除済み。
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

/// 現在の配線状態: 本モジュールは自己完結の実装 (ワークスレッド + 優先度 heap +
/// LRU + 実 disk I/O) だが、ワークスペース内に消費者は存在しない
/// (2026-07-21 監査。エンジン統合は今後の課題)。単体テストは実動作を検証済み。
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
                IoEvent::Failed | IoEvent::Stored => {}
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
                            let _ = tx.send(IoEvent::Failed);
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
                                let _ = tx.send(IoEvent::Stored);
                            }
                        }
                    }
                }
            }
            IoCmd::Store { cx, cz, bytes } => {
                let path = AsyncChunkIo::path(&root, cx, cz);
                let _ = fs::write(path, bytes);
                let _ = tx.send(IoEvent::Stored);
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
        // 【wave 132 EF-1】固定 sleep 方式は CI 高負荷でワーカ未完了のまま
        // 判定に入り**断続フレーク**する (c6e838c CI 紅の最有力候補として
        // 時刻依存パターン走査で特定、同 crate 唯一)。成功条件は不変のまま
        // 5 秒 deadline ポーリングへ堅牢化 (load/store は数 ms で済む通常系、
        // 上限に達した場合だけ失敗 = ワーカ異常の検出力は維持)。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !AsyncChunkIo::path(&dir, 1, 2).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "store 書込待機が 5s 超過 (ワーカ異常)"
            );
            let _ = io.poll_ready(); // Stored イベント排水 (詰まり防止)
            thread::sleep(Duration::from_millis(5));
        }
        // Force disk load
        io.cache.clear();
        io.lru.clear();
        io.request_load(1, 2, 0, 0);
        let deadline2 = std::time::Instant::now() + Duration::from_secs(5);
        let ready = loop {
            let r = io.poll_ready();
            if !r.is_empty() || io.get_cached(1, 2).is_some() {
                break r;
            }
            assert!(
                std::time::Instant::now() < deadline2,
                "load 完了待機が 5s 超過 (ワーカ異常)"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(!ready.is_empty() || io.get_cached(1, 2).is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
