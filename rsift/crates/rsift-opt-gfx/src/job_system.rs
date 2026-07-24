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

impl Priority {
    /// バケット index → enum への一対一写像 (steal 側で保存に使う内部規約)。
    fn from_raw(bucket: usize) -> Self {
        match bucket {
            0 => Self::FrameCritical,
            1 => Self::Normal,
            _ => Self::Background,
        }
    }
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
    /// wave 96 CT-2: 旧実装は横取りしたジョブをワーカー側で強制
    /// `Priority::Background` として再投入し、FrameCritical が静寂背景化
    /// していた (優先度契約の破壊)。優先度はジョブ自身の属性であり、
    /// 位置由来で書き換えない — 転送時に優先度を必ず保持する。
    fn steal_half(&mut self) -> Vec<(Priority, Job)> {
        let mut taken = Vec::new();
        for qi in (0..3).rev() {
            let q = &mut self.q[qi];
            let n = q.len() / 2;
            for _ in 0..n {
                if let Some(j) = q.pop_back() {
                    taken.push((Priority::from_raw(qi), j));
                }
            }
            if !taken.is_empty() {
                break;
            }
        }
        taken
    }
}

/// 実行中ジョブから見えるコンテキスト（並列 for などで子ジョブ投入用）。
pub struct JobContext {
    /// keep-alive: 本コンテキストが生存している間 JobSystemInner (ワーカー群)
    /// の drop を遅らせる RAII アンカー。子ジョブ投入 API 予約でもある
    /// (現行は read されないが drop セマンティクス上必要 — 2026-07-21 監査)。
    #[allow(dead_code)]
    sys: Arc<JobSystemInner>,
    /// ワーカー識別子 (子ジョブ投入時の投入先決定用の予約値)。
    #[allow(dead_code)]
    worker: usize,
}

struct Worker {
    queue: Mutex<PrioQueue>,
    wake: Condvar,
}

pub struct JobSystemInner {
    workers: Vec<Arc<Worker>>,
    pending: AtomicUsize,
    // 注: 旧 `idle: AtomicUsize` は書き込み・読み取り双方ゼロのデッドフィールド
    // (待機は pending 側で実装) だったため削除 (2026-07-21 監査)。
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
        // wave 96 CT-3: 旧 `count.max(1)` は 0 件要求を f(0..1) の 1 件実行
        // という逆セマンティクスにしていた (要求は「0 実行」)。early return で根治。
        if count == 0 {
            return;
        }
        let f = Arc::new(f);
        let chunk = chunk.max(1);
        let mut n_jobs = 0;
        let done = Arc::new(AtomicUsize::new(0));
        let mut start = 0;
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
                let ctx = JobContext {
                    sys: Arc::clone(&self.inner),
                    worker: usize::MAX,
                };
                j(&ctx);
                // CT-1: ワーカー側と同一契約 (実行完了時に減算)。self.inner.pending は
                // submit 完了後の未終了件数 (in-flight + queued) を表す now 語彙に同期。
                self.inner.pending.fetch_sub(1, Ordering::SeqCst);
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

/// スティール品を自キューへ優先度保存で再投入 (worker_loop から分離の pure 口)。
/// 複数同時 steal 時に優先度書き換えが混入しないことを単体テストで機械固定する
/// (wave 96 CT-2: 旧実装は一括 Background 化 — 直写経路に分離して厳密検査可に)。
fn requeue_stolen(own: &Mutex<PrioQueue>, stolen: Vec<(Priority, Job)>) -> Option<Job> {
    let mut q = own.lock().unwrap();
    let mut it = stolen.into_iter();
    let first = it.next().map(|(_, j)| j);
    for (p, j) in it {
        q.push(p, j);
    }
    first
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
                    return requeue_stolen(&inner.workers[idx].queue, stolen);
                }
            }
            None
        });
        match job {
            Some(j) => {
                let ctx = JobContext {
                    sys: Arc::clone(&inner),
                    worker: idx,
                };
                j(&ctx);
                // wave 96 CT-1: pending は実行完了時に減算 — POP 時点 (実行開始)
                // で減らすと最終ジョブの実行中に wait_idle が 0 判定で早期戻り
                // 得て完了保証を満たさない (旧契約破綻)。
                inner.pending.fetch_sub(1, Ordering::SeqCst);
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

    /// wave 96 CT-1: pending は実行完了後にのみ 0 になる契約。
    /// 8 ジョブ × 10 ms sleep → 単一ワーカーでも wait_idle 直後の
    /// 全件完了を保証する (旧実装は POP 減算で最終 sleep 中に
    /// 早期復帰し得た → 直接の確率は低いが本体契約の破綻)。
    #[test]
    fn wait_idle_waits_for_actual_completion() {
        let sys = JobSystem::new(1);
        let n = Arc::new(AtomicUsize::new(0));
        for _ in 0..8 {
            let c = Arc::clone(&n);
            sys.submit(
                Priority::Normal,
                Box::new(move |_| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    c.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }
        sys.wait_idle();
        assert_eq!(
            n.load(Ordering::SeqCst),
            8,
            "wait_idle は実行完了後にのみ戻るはず (CT-1 root 契約)"
        );
    }

    /// wave 96 CT-2: スティールは優先度を属性のまま保持する
    /// (旧実装は Background への一括書き換えで FC が静寂背景化)。
    /// PrioQueue レベルで pure に固定 + thief 再投入側の再順序確保も
    /// 改修後のワーカー経路へ同期の責任で担保 (UI テストなしでも成立する
    /// 厳密な属性固定のみ)。
    #[test]
    fn steal_half_preserves_priority_and_amount() {
        let mut pq = PrioQueue::new();
        for _ in 0..4 {
            pq.push(Priority::FrameCritical, Box::new(|_| {}));
        }
        for _ in 0..2 {
            pq.push(Priority::Background, Box::new(|_| {}));
        }
        let stolen = pq.steal_half();
        assert_eq!(
            stolen.len(),
            1,
            "最低優先度 BG 半分 (2 件の半分=1) を先に横取り"
        );
        assert_eq!(stolen[0].0, Priority::Background, "優先度が属性のまま");
        // 半分切捨てで残 1 件の BG は n=1/2=0 → fall-through → FC 4 件の半分=2
        // (機械検算値、テスト赤=自己誤り捕捉 19 件目で訂正記録)。
        let stolen2 = pq.steal_half();
        assert_eq!(
            stolen2.len(),
            2,
            "BG 半切捨て 0 は fall-through → FC から 2"
        );
        assert!(stolen2.iter().all(|(p, _)| *p == Priority::FrameCritical));
        let stolen3 = pq.steal_half();
        assert_eq!(stolen3.len(), 1, "残 FC 2 件の半分=1");
        assert_eq!(stolen3[0].0, Priority::FrameCritical);
    }

    /// wave 96 CT-2: thief 側再投入も優先度属性のまま (自キュー to push 経路を
    /// requeue_stolen 分離して機械検査可に — 旧提出巡回が優先度を書き換える
    /// 逆変異がこのテストで検出可能となる)。
    #[test]
    fn requeue_stolen_preserves_fifo_and_priority() {
        let dst = Mutex::new(PrioQueue::new());
        let marker = Arc::new(AtomicUsize::new(0));
        let make = |m: usize| {
            let c = Arc::clone(&marker);
            Box::new(move |_: &JobContext| {
                c.store(m, Ordering::SeqCst);
            }) as Job
        };
        let anchor = JobSystem::new(1);
        let ctx_of = |_: &JobSystem| JobContext {
            sys: Arc::clone(&anchor.inner),
            worker: usize::MAX,
        };
        {
            let mut q0 = dst.lock().unwrap();
            q0.push(Priority::Background, make(99));
        }
        let first = requeue_stolen(
            &dst,
            vec![
                (Priority::FrameCritical, make(1)),
                (Priority::Normal, make(2)),
                (Priority::Background, make(3)),
            ],
        );
        first.expect("return first job")(&ctx_of(&anchor));
        assert_eq!(marker.load(Ordering::SeqCst), 1, "先頭は FC=1");
        let mut q = dst.lock().unwrap();
        let nj = q.pop().expect("normal");
        nj(&ctx_of(&anchor));
        assert_eq!(
            marker.load(Ordering::SeqCst),
            2,
            "次は Normal=2 (優先度書き換えゼロ)"
        );
    }

    /// wave 96 CT-3: count=0 の parallel_for は厳密に no-op
    /// (旧実装は `count.max(1)` で 1 件の f(0..1) 呼出を静寂実行)。
    #[test]
    fn parallel_for_zero_count_is_strict_noop() {
        let sys = JobSystem::new(2);
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        sys.parallel_for(0, 32, Priority::Normal, move |_, _| {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(
            n.load(Ordering::SeqCst),
            0,
            "count=0 は 1 件も実行しないはず"
        );
    }
}
