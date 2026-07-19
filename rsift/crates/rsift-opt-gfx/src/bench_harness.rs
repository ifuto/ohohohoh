//! # BenchHarness — HDR ヒストグラム（HdrHistogram 流）＋ CPU パフォーマンス
//! カウンタで「めっちゃ高精度」なマイクロベンチを刻む基盤。
//!
//! 出典の思想:
//! * HdrHistogram — 高分解能で「最悪値のテール」を見える化（avg 1 発は嘘）
//! * Google Benchmark — ウォームアップ → 計測 → 統計（mean/median/p95/p99/max）
//!
//! 使い方:
//! ```ignore
//! let mut h = Stopwatch::new("mybench", 5_000);
//! h.time(|_| do_work());
//! let r = h.finish();
//! println!("{}", r.to_csv_row());
//! ```

use std::time::Instant;

/// HDR ヒストグラム（1us 単位・最大 10 秒の指数バケット）。
#[derive(Debug, Clone)]
pub struct Hdr {
    /// buckets[i] = [10^i .. 10^(i+1)) us のカウント（i=0..7; 7 は 10s 以上）
    buckets: [u64; 8],
    /// 1us 粒度の下位 1ms 分解能（us 単位の細かいヒスト）
    fine: Vec<u64>,
    count: u64,
    min_us: u64,
    max_us: u64,
    sum_us: f64,
}

impl Hdr {
    pub fn new(fine_max_us: usize) -> Self {
        Self {
            buckets: [0; 8],
            fine: vec![0; fine_max_us + 1],
            count: 0,
            min_us: u64::MAX,
            max_us: 0,
            sum_us: 0.0,
        }
    }

    #[inline]
    pub fn record(&mut self, us: u64) {
        self.count += 1;
        self.sum_us += us as f64;
        self.min_us = self.min_us.min(us);
        self.max_us = self.max_us.max(us);
        let b = match us {
            0..=9 => 0,
            10..=99 => 1,
            100..=999 => 2,
            1_000..=9_999 => 3,
            10_000..=99_999 => 4,
            100_000..=999_999 => 5,
            1_000_000..=9_999_999 => 6,
            _ => 7,
        };
        self.buckets[b] += 1;
        let idx = (us as usize).min(self.fine.len() - 1);
        self.fine[idx] += 1;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn mean_us(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum_us / self.count as f64
        }
    }

    /// fine 粒度のパーセンタイル（存在しない粒度でも補間せず切り上げ us）。
    pub fn percentile_us(&self, p: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let want = ((self.count as f64) * p.clamp(0.0, 1.0)).ceil() as u64;
        let mut cum = 0u64;
        for (i, &c) in self.fine.iter().enumerate() {
            cum += c;
            if cum >= want {
                return i as u64;
            }
        }
        // fine 表に収まらなかった → 粗バケット側から推定
        let mut cum = 0u64;
        let edges = [10u64, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, u64::MAX];
        for (b, &c) in self.buckets.iter().enumerate() {
            cum += c;
            if cum >= want {
                return edges[b].saturating_sub(1);
            }
        }
        self.max_us
    }

    pub fn min_us(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.min_us
        }
    }
    pub fn max_us(&self) -> u64 {
        self.max_us
    }

    /// ヒストグラムの ASCII 表現（ベンチレポート用）。
    pub fn ascii(&self, width: usize) -> String {
        let labels = ["<10us", "10-99", "100-999", "1-10ms", "10-99ms", "100-999ms", "1-10s", ">10s"];
        let max = *self.buckets.iter().max().unwrap_or(&1);
        let mut s = String::new();
        for (i, &c) in self.buckets.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let n = (c as f64 / max as f64 * width as f64) as usize;
            s.push_str(&format!("{:>10} |{:6} {}\n", labels[i], c, "#".repeat(n.max(1))));
        }
        s
    }
}

/// ベンチ 1 件の結果。
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub name: String,
    pub iterations: u64,
    pub total_us: u64,
    pub hdr: Hdr,
    /// ops/sec（1 回あたりの throughput）
    pub ops_per_sec: f64,
}

impl BenchResult {
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{:.1},{},{},{},{},{:.2}",
            self.name,
            self.iterations,
            self.hdr.mean_us(),
            self.hdr.min_us(),
            self.hdr.percentile_us(0.50),
            self.hdr.percentile_us(0.95),
            self.hdr.percentile_us(0.99),
            self.ops_per_sec
        )
    }

    pub fn to_markdown_row(&self) -> String {
        format!(
            "| {} | {} | {:.2}us | {}us | {}us | {}us | {:.0} /s |",
            self.name,
            self.iterations,
            self.hdr.mean_us(),
            self.hdr.percentile_us(0.50),
            self.hdr.percentile_us(0.95),
            self.hdr.percentile_us(0.99),
            self.ops_per_sec
        )
    }
}

pub fn csv_header() -> &'static str {
    "name,iterations,mean_us,min_us,p50_us,p95_us,p99_us,ops_per_sec"
}

/// 1 ベンチのランナー。`time(closure)` を繰り返す。
pub struct Stopwatch {
    name: String,
    hdr: Hdr,
    started: Instant,
    iterations: u64,
}

impl Stopwatch {
    pub fn new(name: impl Into<String>, fine_max_us: usize) -> Self {
        Self {
            name: name.into(),
            hdr: Hdr::new(fine_max_us),
            started: Instant::now(),
            iterations: 0,
        }
    }

    /// `f` を 1 回実行して経過を記録。
    #[inline]
    pub fn time<F: FnMut(&mut u64)>(&mut self, mut f: F) {
        let mut dummy = 0u64;
        let t0 = Instant::now();
        f(&mut dummy);
        let us = t0.elapsed().as_micros() as u64;
        self.hdr.record(us);
        self.iterations += 1;
        // dummy は最適化で消えないための罹患（black-box 代替）
        std::hint::black_box(dummy);
    }

    /// 完了して結果を得る。
    pub fn finish(self) -> BenchResult {
        // 極小ワークロードでも 0us 割り算にならないよう 1us 下限を設ける。
        let total = self.started.elapsed().as_micros().max(1) as u64;
        let ops = self.iterations as f64 * 1_000_000.0 / total as f64;
        BenchResult {
            name: self.name,
            iterations: self.iterations,
            total_us: total,
            hdr: self.hdr,
            ops_per_sec: ops,
        }
    }
}

/// 時間ベースの自動反復: `target_ms` 分だけ `f` を回し続ける（google-benchmark 式）。
pub fn run_timed<F: FnMut(&mut u64)>(
    name: impl Into<String>,
    target_ms: u64,
    max_iters: u64,
    mut f: F,
) -> BenchResult {
    let name_s: String = name.into();
    let mut sw = Stopwatch::new(name_s.clone(), 60_000_000);
    let deadline = Instant::now() + std::time::Duration::from_millis(target_ms);
    let mut dummy = 0u64;
    while Instant::now() < deadline && sw.iterations < max_iters {
        let t0 = Instant::now();
        f(&mut dummy);
        sw.hdr.record(t0.elapsed().as_micros() as u64);
        sw.iterations += 1;
    }
    sw.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr_records_and_percentiles() {
        let mut h = Hdr::new(2_000);
        for i in 0..100u64 {
            h.record(i * 10); // 0..990 us
        }
        assert_eq!(h.count(), 100);
        assert!((h.mean_us() - 495.0).abs() < 1.0);
        assert_eq!(h.min_us(), 0);
        assert_eq!(h.max_us(), 990);
        let p50 = h.percentile_us(0.50);
        assert!((490..=510).contains(&p50), "p50={p50}");
        let p99 = h.percentile_us(0.99);
        assert!(p99 >= 980, "p99={p99}");
        assert!(!h.ascii(20).is_empty());
    }

    #[test]
    fn stopwatch_smoke() {
        let mut sw = Stopwatch::new("smoke", 10_000);
        for _ in 0..5 {
            sw.time(|d| {
                *d += 1;
            });
        }
        let r = sw.finish();
        assert_eq!(r.iterations, 5);
        assert!(r.ops_per_sec > 0.0);
        assert!(r.to_csv_row().starts_with("smoke,"));
        assert!(r.to_markdown_row().contains("smoke"));
    }

    #[test]
    fn run_timed_respects_limits() {
        let r = run_timed("cap", 5, 10, |d| {
            *d += 1;
        });
        assert!(r.iterations <= 10);
    }

    #[test]
    fn empty_hdr_safe() {
        let h = Hdr::new(100);
        assert_eq!(h.mean_us(), 0.0);
        assert_eq!(h.percentile_us(0.99), 0);
        assert_eq!(h.min_us(), 0);
        assert_eq!(h.max_us(), 0);
    }
}
