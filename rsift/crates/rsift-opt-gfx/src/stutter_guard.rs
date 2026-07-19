//! # StutterGuard — フレーム内一時オブジェクトの撲滅（バンプアリーナ＋
//! チャンク更新の time-slice＋GC 圧の除去）で「カクつき」を物理的に断つ
//!
//! 出典:
//! * bumpalo（1 フレーム分の短命オブジェクトを arena に押し込み、
//!   frame end で `reset()` して全解放 O(1)）
//! * C2ME / Sodium の「チャンクビルドを時間切れで中断して次フレームに回す」
//!
//! 低スペック環境のカクつきの 8 割は「フレーム中の急なアロケーション +
//! 1 フレームに積みすぎた作業」なので、この 2 つを予算管理する。

use std::alloc::{alloc, dealloc, Layout};
use std::cell::Cell;
use std::ptr::NonNull;

/// 1 フレーム使い捨てバンプアリーナ（bumpalo の necko 版: 依存ゼロ）。
pub struct FrameArena {
    chunk: Cell<NonNull<u8>>,
    cap: Cell<usize>,
    used: Cell<usize>,
    chunk_size: usize,
    allocations: Cell<u32>,
}

impl FrameArena {
    pub fn new(chunk_size: usize) -> Self {
        let chunk_size = chunk_size.max(4096);
        let layout = Layout::from_size_align(chunk_size, 64).unwrap();
        let p = unsafe { alloc(layout) };
        Self {
            chunk: Cell::new(NonNull::new(p).expect("alloc arena")),
            cap: Cell::new(chunk_size),
            used: Cell::new(0),
            chunk_size,
            allocations: Cell::new(0),
        }
    }

    /// 生メモリを 1 ブロック切り出す（8B アライン）。
    #[inline]
    pub fn alloc_bytes(&self, n: usize) -> Option<NonNull<u8>> {
        let n = (n + 7) & !7;
        let used = self.used.get();
        let cap = self.cap.get();
        if used + n > cap {
            // 実機: ここで追加チャンクを取る。検証を簡潔に保つため None
            // （呼び側は fall back to Vec）
            return None;
        }
        let p = unsafe { self.chunk.get().as_ptr().add(used) };
        self.used.set(used + n);
        self.allocations.set(self.allocations.get() + 1);
        NonNull::new(p)
    }

    /// 「u32 配列」など POD の切り出し（長さ n のスライス相当領域）。
    #[inline]
    pub fn alloc_pod_slice<T: Sized>(&self, n: usize) -> Option<NonNull<T>> {
        let bytes = n * std::mem::size_of::<T>();
        self.alloc_bytes(bytes).map(|p| p.cast())
    }

    /// フレーム終了: 全解放 O(1)。
    pub fn reset(&self) {
        self.used.set(0);
        self.allocations.set(0);
    }

    pub fn used(&self) -> usize {
        self.used.get()
    }

    pub fn allocations(&self) -> u32 {
        self.allocations.get()
    }

    pub fn capacity(&self) -> usize {
        self.cap.get()
    }
}

impl Drop for FrameArena {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.cap.get(), 64).unwrap();
        unsafe { dealloc(self.chunk.get().as_ptr(), layout) };
    }
}

/// 1 フレームに許すチャンク作業の時間予算。切れたら残りは次フレーム。
/// 「低スペックでは毎フレーム少しずつ」がカクつかない鉄則。
pub struct TimeSlice<T> {
    pub budget_us: u32,
    backlog: std::collections::VecDeque<T>,
    pub completed: u64,
    pub deferred: u64,
}

impl<T> TimeSlice<T> {
    pub fn new(budget_us: u32) -> Self {
        Self {
            budget_us: budget_us.max(200),
            backlog: std::collections::VecDeque::new(),
            completed: 0,
            deferred: 0,
        }
    }

    pub fn push(&mut self, item: T) {
        self.backlog.push_back(item);
    }

    pub fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    /// フレーム冒頭に呼ぶ: 予算内で処理できるだけ処理し、切れた時点で中断。
    /// `process(item) -> 実消費マイクロ秒` を返す関数を渡す。
    pub fn run_frame<F: FnMut(&T) -> u32>(&mut self, mut process: F) -> usize {
        let mut spent = 0u32;
        let mut done = 0usize;
        while let Some(item) = self.backlog.front() {
            if spent > self.budget_us {
                break;
            }
            let cost = process(item);
            spent += cost;
            self.backlog.pop_front();
            done += 1;
            self.completed += 1;
        }
        if !self.backlog.is_empty() {
            self.deferred += self.backlog.len() as u64;
        }
        done
    }
}

/// フレームごとの統計（これを quality_governor に食わせる）。
#[derive(Debug, Clone, Copy, Default)]
pub struct FramePerfSample {
    pub frame_us: u32,
    pub arena_used: usize,
    pub arena_allocs: u32,
    pub deferred_work: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_bump_and_reset() {
        let a = FrameArena::new(1024);
        let p1 = a.alloc_bytes(100).unwrap();
        let p2 = a.alloc_bytes(200).unwrap();
        assert_ne!(p1, p2);
        assert!(a.used() >= 300);
        assert_eq!(a.allocations(), 2);
        a.reset();
        assert_eq!(a.used(), 0);
        let p3 = a.alloc_bytes(16).unwrap();
        assert_eq!(p3, p1, "after reset must reuse the same base");
    }

    #[test]
    fn arena_full_returns_none_not_panic() {
        let a = FrameArena::new(4096);
        let mut total = 0;
        while a.alloc_bytes(512).is_some() {
            total += 512;
        }
        assert_eq!(total, 4096);
        assert!(a.alloc_bytes(1).is_none());
    }

    #[test]
    fn pod_slice_layout() {
        let a = FrameArena::new(4096);
        let s = a.alloc_pod_slice::<u64>(10).unwrap();
        unsafe {
            for i in 0..10 {
                s.as_ptr().add(i).write(i as u64);
            }
            assert_eq!(*s.as_ptr().add(7), 7u64);
        }
    }

    #[test]
    fn time_slice_defers_over_budget() {
        let mut ts: TimeSlice<u32> = TimeSlice::new(1000);
        for i in 0..10 {
            ts.push(i);
        }
        let done = ts.run_frame(|_| 400); // 400us/個 → 1000us 予算で 2 個 + α
        assert!(done <= 3, "must stop around budget, done={done}");
        assert!(ts.backlog_len() >= 7);
        // 次フレームで継続
        let done2 = ts.run_frame(|_| 400);
        assert!(done2 >= 2);
        assert_eq!(ts.completed as usize, done + done2);
    }
}
