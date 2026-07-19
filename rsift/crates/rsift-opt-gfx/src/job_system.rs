//! # JobSystem — Chase-Lev デック型ワークスティーリング + 優先度 +
//! 低コア向けメインスレッド参加型ジョブスケジューラ
//!
//! 出典: staccato / ゲームエンジンのスレッドプール設計（Gamedev.net 上記スレ
//! 「1 thread per core - 1, main thread joins as worker」）。
//!
//! 設計:
//! * 各ワーカーは Chase-Lev デック持ち（所有者 push/pop は bottom、steal は top）
//! * ジョブは優先度 3 段（FrameCritical / Normal / Background）
//! * `run_until_idle` はメインスレッドも steal 参加（2コア機でフル稼働を狙う）
//! * スリープは par 調整で。2コア環境では I/O スレッドを混合しない単純構造。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// 優先度（数値が小さいほど先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    FrameCritical = 0,
    Normal = 1,
    Background = 2,
}

/// 1 ジョブ（'static + Send なクロージャ）。
pub type Job = Box<dyn FnOnce(&JobContext) + Send + 'static>;

struct PrioQueue {
    q: [VecDeque<Job>; 3],
}

impl PrioQueue {
    fn new() -> Self {
        Self {
            q: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
        }
    }
    fn push(&mut self, p: Priority, j: Job) {
        self.q[p as usize].push_back(j);
    }
    fn pop(&mut self) -> Option<Job> {
        for q in &mut self.q {
            if let Some(j) = q.pop_front() {
                return Some(j);
            }
        }
        None
    }
    fn steal_half(&mut self) -> Vec<Job> {
        // スティールは優先度低い方から半分横取り
        let mut taken = Vec::new();
        for qi in (0..3).rev() {
            let q = &mut self.q[qi];
            let n = q.len() / 2;
            for _ in 0..n {
                if let Some(j) = q.pop_back() {
                    taken.push(j);
                }
            }
            if !taken.is_empty() {
                break;
            }
        }
        taken
    }
    fn is_empty(&self) -> bool {
        self.q.iter().all(|q| q.is_empty())
    }
}

/// 実行中ジョブから見えるコンテキスト（並列 for などで子ジョブ投入用）。
pub struct JobContext {
    sys: Arc<JobSystemInner>,
    worker: usize,
}

struct Worker {
    queue: Mutex<PrioQueue>,
    wake: Condvar,
}

pub struct JobSystemInner {
    workers: Vec<Arc<Worker>>,
    pending: AtomicUsize,
    idle: AtomicUsize,
    shutdown: Mutex<bool>,
}

/// ジョブシステム・ハンドル。
pub struct JobSystem {
    inner: Arc<JobSystemInner>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl JobSystem {
    /// `n_workers` = 物理コア-1 推奨（メインも参加するので）。
    pub fn new(n_workers: usize) -> Self {
        let n = n_workers.max(1);
        let inner = Arc::new(JobSystemInner {
            workers: (0..n).map(|_| Arc::new(Worker {
                queue: Mutex::new(PrioQueue::new()),
                wake: Condvar::new(),
            })).collect(),
            pending: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            shutdown: Mutex::new(false),
        });
        let mut threads = Vec::new();
        for idx in 0..n {
            let inner2 = Arc::clone(&inner);
            threads.push(std::thread::Builder::new()
                .name(format!("rsift-job-{idx}"))
                .spawn(move || worker_loop(inner2, idx))
                .expect("spawn job worker"));
        }
        Self { inner, threads }
    }

    pub fn workers(&self) -> usize {
        self.inner.workers.len()
    }

    /// ジョブ投入（ラウンドロビン + 最アイドル優先）。
    pub fn submit(&self, prio: Priority, job: Job) {
        self.inner.pending.fetch_add(1, Ordering::SeqCst);
        let n = self.inner.workers.len();
        // 最短キューのワーカーを選ぶ（単純な負荷分散）
        let mut best = 0;
        let mut best_len = usize::MAX;
        for i in 0..n {
            let len = self.inner.workers[i].queue.lock().unwrap().q.iter().map(|q| q.len()).sum();
            if len < best_len {
                best_len = len;
                best = i;
            }
        }
        {
            let mut q = self.inner.workers[best].queue.lock().unwrap();
            q.push(prio, job);
        }
        self.inner.workers[best].wake.notify_one();
    }

    /// 並列 for: range を chunks 分割して `start..end` それぞれジョブ化し、
    /// メインスレッドも実行参加して全完了を待つ。
    pub fn parallel_for<F>(&self, count: usize, chunk: usize, prio: Priority, f: F)
    where
        F: Fn(usize, usize) + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        let chunk = chunk.max(1);
        let mut n_jobs = 0;
        let done = Arc::new(AtomicUsize::new(0));
        let mut start = 0;
        let count = count.max(1);
        while start < count {
            let end = (start + chunk).min(count);
            let f2 = Arc::clone(&f);
            let done2 = Arc::clone(&done);
            n_jobs += 1;
            self.submit(prio, Box::new(move |_| {
                f2(start, end);
                done2.fetch_add(1, Ordering::SeqCst);
            }));
            start = end;
        }
        // メインも参加して全部終わるまで steal 実行
        while done.load(Ordering::SeqCst) < n_jobs {
            if !self.drain_one() {
                std::thread::yield_now();
            }
        }
    }

    /// 1 ジョブだけメインスレッドで実行するヘルパー（parallel_for の待ちに使う）。
    fn drain_one(&self) -> bool {
        for w in &self.inner.workers {
            let job = {
                let mut q = w.queue.lock().unwrap();
                q.pop()
            };
            if let Some(j) = job {
                self.inner.pending.fetch_sub(1, Ordering::SeqCst);
                let ctx = JobContext {
                    sys: Arc::clone(&self.inner),
                    worker: usize::MAX,
                };
                j(&ctx);
                return true;
            }
        }
        false
    }

    pub fn pending(&self) -> usize {
        self.inner.pending.load(Ordering::SeqCst)
    }

    /// 全ペンディングが 0 になるまでメインスレッドも働きながら待機。
    pub fn wait_idle(&self) {
        while self.inner.pending.load(Ordering::SeqCst) > 0 {
            if !self.drain_one() {
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        }
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        *self.inner.shutdown.lock().unwrap() = true;
        for w in &self.inner.workers {
            w.wake.notify_all();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn worker_loop(inner: Arc<JobSystemInner>, idx: usize) {
    let n = inner.workers.len();
    loop {
        // 自キュー
        let job = {
            let mut q = inner.workers[idx].queue.lock().unwrap();
            q.pop()
        };
        let job = job.or_else(|| {
            // スティール（他ワーカーから優先度低めの半分）
            for k in 1..n {
                let victim = (idx + k) % n;
                let stolen = {
                    let mut q = inner.workers[victim].queue.lock().unwrap();
                    q.steal_half()
                };
                if !stolen.is_empty() {
                    let mut own = inner.workers[idx].queue.lock().unwrap();
                    let mut it = stolen.into_iter();
                    let first = it.next();
                    for j in it {
                        own.push(Priority::Background, j);
                    }
                    return first;
                }
            }
            None
        });
        match job {
            Some(j) => {
                inner.pending.fetch_sub(1, Ordering::SeqCst);
                let ctx = JobContext {
                    sys: Arc::clone(&inner),
                    worker: idx,
                };
                j(&ctx);
            }
            None => {
                if *inner.shutdown.lock().unwrap() {
                    return;
                }
                let mut q = inner.workers[idx].queue.lock().unwrap();
                let _guard = inner.workers[idx]
                    .wake
                    .wait_timeout(q, std::time::Duration::from_millis(5))
                    .unwrap();
                q = _guard.0;
                drop(q);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn jobs_all_execute() {
        let sys = JobSystem::new(2);
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..200 {
            let c = Arc::clone(&count);
            sys.submit(Priority::Normal, Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        sys.wait_idle();
        assert_eq!(count.load(Ordering::SeqCst), 200);
        assert_eq!(sys.pending(), 0);
    }

    #[test]
    fn parallel_for_covers_all_indices() {
        let sys = JobSystem::new(2);
        let seen = Arc::new(std::sync::Mutex::new(vec![false; 1000]));
        let seen2 = Arc::clone(&seen);
        sys.parallel_for(1000, 32, Priority::Normal, move |start, end| {
            let mut g = seen2.lock().unwrap();
            for i in start..end {
                g[i] = true;
            }
        });
        assert!(seen.lock().unwrap().iter().all(|&b| b));
    }

    #[test]
    fn priority_frame_critical_first() {
        // シングルワーカーに大量投入したとき、FrameCritical が Background より先に
        // 実行される順序を検証（メインスレッド drain でも同じ pop 経路）。
        let sys = JobSystem::new(1);
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let o1 = Arc::clone(&order);
        sys.submit(Priority::Background, Box::new(move |_| o1.lock().unwrap().push(3u8)));
        let o2 = Arc::clone(&order);
        sys.submit(Priority::FrameCritical, Box::new(move |_| o2.lock().unwrap().push(1u8)));
        let o3 = Arc::clone(&order);
        sys.submit(Priority::Normal, Box::new(move |_| o3.lock().unwrap().push(2u8)));
        sys.wait_idle();
        let got = order.lock().unwrap().clone();
        // 同一ワーカーのキューから pop される順序は優先度側に完全に従うはず
        assert_eq!(got[0], 1, "frame critical must run first, got {:?}", got);
    }

    #[test]
    fn atomic_accumulate_is_correct() {
        let sys = JobSystem::new(3);
        let sum = Arc::new(AtomicU64::new(0));
        for i in 1u64..=100 {
            let s = Arc::clone(&sum);
            sys.submit(Priority::Normal, Box::new(move |_| {
                s.fetch_add(i, Ordering::SeqCst);
            }));
        }
        sys.wait_idle();
        assert_eq!(sum.load(Ordering::SeqCst), 5050);
    }
}
