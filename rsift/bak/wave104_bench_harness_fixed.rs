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

/// HDR ヒストグラム（1us 単位の fine 表＋指数バケットの二段構成）。
#[derive(Debug, Clone)]
pub struct Hdr {
    /// 指数バケット: buckets[0] = [0 .. 10) us（0us サンプルを含む。旧 doc「[10^0..10^1)」は
    /// 0 を含まない誤りだった — DD-4 訂正）、buckets[i] = [10^i .. 10^(i+1)) us（i=1..=6）、
    /// buckets[7] = [10^7 ..= u64::MAX] us（10s 以上。10,000,000us 丁度もここ）
    buckets: [u64; 8],
    /// 1us 粒度のヒスト [0 ..= fine_max_us]（確保メモリ 8·(fine_max_us+1) B）。
    /// fine_max_us 超過サンプルは **記録しない**（percentile は粗バケット推定へ委譲。
    /// 旧実装は末尾スロットに min(us, fine_max_us) で静寂飽和させており、
    /// 粗フォールバック到達不能＋超過サンプルの過小報告という二重の虚偽だった — DD-1 根治）
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
        // DD-1: fine レンジ [0, fine_max_us] 内のみ precise 記録。旧実装は
        // `min(us, len-1)` で末尾スロットに飽和させており、5s サンプルが
        // fine_max=1000 の表で 1000us と過小報告され得た（粗フォールバックも死にコード化）。
        if let Some(slot) = self.fine.get_mut(us as usize) {
            *slot += 1;
        }
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

    /// パーセンタイル（nearest-rank 法: rank = ceil(p·count) ∈ [1, count]、p=0 → 最小値）。
    /// fine レンジ内は 1us 精确。range 外サンプルは粗バケットの **上限値** を返す
    /// （保守的上振れ — 真値を過小には返さない。例: [1000..9999]us 帯なら 9999）。
    pub fn percentile_us(&self, p: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        // DD-3: p=0 で want=0 となり先頭スロット（未観測の 0us を含む）を即返す虚偽があった。
        // nearest-rank 定義では rank ∈ [1, count] なので下限 1 にクランプ（p=0 → 最小値）。
        let want = ((self.count as f64) * p.clamp(0.0, 1.0)).ceil().max(1.0) as u64;
        let mut cum = 0u64;
        for (i, &c) in self.fine.iter().enumerate() {
            cum += c;
            if cum >= want {
                return i as u64;
            }
        }
        // fine 表に収まらなかった → 粗バケット側から推定（上限値返却）
        let mut cum = 0u64;
        let edges = [10u64, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, u64::MAX];
        for (b, &c) in self.buckets.iter().enumerate() {
            cum += c;
            if cum >= want {
                return edges[b].saturating_sub(1);
            }
        }
        // 構築上到達不能: 全バケット合計 == count ≥ want（want ≤ ceil(count·1.0) = count、
        // count < 2^53 で f64 乘算・ceil は厳密）で必ず上のループが return する。防御として残す。
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
        // DD-4: bucket7 は 10,000,000us 丁度を含む「10s 以上」→ ラベルは ">10s" ではなく ">=10s"
        let labels = [
            "<10us",
            "10-99",
            "100-999",
            "1-10ms",
            "10-99ms",
            "100-999ms",
            "1-10s",
            ">=10s",
        ];
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
    /// CSV 1 行。name はエスケープしない（ベンチ名はコード内定数・カンマ不含の前提 — DD 観察。
    /// 外部入力名を流す場合は呼出側でクォートすること）。
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
    /// 構築時点（Instant::now()）で壁時計を開始する。
    /// ops/sec は new()〜finish() の **全期間** ベースなので、構築〜計測開始の間に
    /// 別処理を挟むと throughput が薄まる。計測直前に new すること（DD 観察）。
    /// fine 表メモリ: 8·(fine_max_us+1) B（Hdr::new と同契約）。
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
        // as u64: as_micros() は u128 だが u64::MAX us ≈ 584,542 年でしか切捨てが
        // 起きないため実質到達不能（防御は置かず証明に委ねる — DD 観察）
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

/// run_timed の fine 表最大 us（DD-2）: 全測定窓（target_ms）を通常カバーしつつ
/// max(60ms, target_ms)・上限 1s レンジに抑える = fine 表は最大 8,000,008 B。
/// 旧固定値 60_000_000us は **呼出毎に 480,000,008 B（457.8 MiB）を確保** しており、
/// rsift_bench 9 bench 連続実行・lib テストで毎回の無駄だった（低スペック PC 敵対）。
fn run_timed_fine_max_us(target_ms: u64) -> usize {
    (target_ms.saturating_mul(1_000)).clamp(60_000, 1_000_000) as usize
}

/// 時間ベースの自動反復: `target_ms` 分だけ `f` を回し続ける（google-benchmark 式）。
/// fine 解像度は max(60ms, target_ms)・上限 1s（fine 表は最大 8MB）。
/// `Instant + Duration` は target_ms が u64::MAX 級のとき加算パニックし得る
/// （fail-loud 側の挙動 — DD 観察）。
pub fn run_timed<F: FnMut(&mut u64)>(
    name: impl Into<String>,
    target_ms: u64,
    max_iters: u64,
    mut f: F,
) -> BenchResult {
    let name_s: String = name.into();
    let mut sw = Stopwatch::new(name_s.clone(), run_timed_fine_max_us(target_ms));
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

    #[test]
    fn overflow_samples_skip_fine_and_percentile_uses_bucket_ceiling() {
        // DD-1 根治の厳密ピン: fine_max=2000 に対し 5,000us / 500,000us は
        // fine に記録されず (旧実装は末尾スロット 2000 へ 2 件静寂飽和)、
        // percentile は粗バケット上限値へ委譲される。
        let mut h = Hdr::new(2_000);
        for us in [10u64, 20, 30, 5_000, 500_000] {
            h.record(us);
        }
        assert_eq!(h.count(), 5);
        assert_eq!(h.fine[10], 1);
        assert_eq!(h.fine[20], 1);
        assert_eq!(h.fine[30], 1);
        assert_eq!(h.fine[2000], 0, "旧実装はここに 2 件飽和させていた");
        assert_eq!(h.buckets[1], 3, "10/20/30 → [10..100)");
        assert_eq!(h.buckets[3], 1, "5,000 → [1000..10000)");
        assert_eq!(h.buckets[5], 1, "500,000 → [100000..1000000)");
        // nearest-rank: count=5 → rank=ceil(0.5·5)=3 → 30us (fine 精确)
        assert_eq!(h.percentile_us(0.50), 30);
        assert_eq!(h.percentile_us(0.60), 30, "rank=ceil(3)=3");
        // rank=ceil(0.8·5)=4 → fine 累計 3 で不足 → bucket3 上限 9,999 (保守的上振れ。
        // 旧実装は 2,000us を返し真値 5,000us を 2.5 倍過小報告していた)
        assert_eq!(h.percentile_us(0.80), 9_999);
        // rank=5 → bucket5 上限 999,999 (真値 500,000; 旧実装は 2,000us と 250 倍過小報告)
        assert_eq!(h.percentile_us(1.00), 999_999);
        assert_eq!(h.max_us(), 500_000);
        assert_eq!(h.min_us(), 10);
        assert!((h.mean_us() - 101_012.0).abs() < 1e-9, "505060/5");
    }

    #[test]
    fn percentile_zero_is_min_nearest_rank() {
        // DD-3 根治の厳密ピン: p=0 の rank は 1 (最小値)。旧実装は want=0 で
        // 先頭スロットを即返し、未観測の 0us を報告していた。
        let mut h = Hdr::new(1_000);
        for us in [10u64, 20, 30, 40, 50] {
            h.record(us);
        }
        assert_eq!(h.percentile_us(0.0), 10, "p0 = min (nearest-rank)");
        assert_eq!(h.percentile_us(0.2), 10, "rank=ceil(0.2·5)=1");
        assert_eq!(h.percentile_us(0.6), 30, "rank=ceil(0.6·5)=3");
        assert_eq!(h.percentile_us(1.0), 50, "rank=5");
    }

    #[test]
    fn run_timed_fine_max_us_contract() {
        // DD-2 根治の厳密ピン: fine 表は max(60ms, target_ms)・上限 1s (= 最大 8MB)。
        assert_eq!(run_timed_fine_max_us(0), 60_000);
        assert_eq!(run_timed_fine_max_us(5), 60_000);
        assert_eq!(run_timed_fine_max_us(350), 350_000, "rsift_bench --medium");
        assert_eq!(run_timed_fine_max_us(1_000), 1_000_000);
        assert_eq!(
            run_timed_fine_max_us(1_500),
            1_000_000,
            "--heavy でも 1s cap"
        );
        assert_eq!(
            run_timed_fine_max_us(u64::MAX),
            1_000_000,
            "saturating で飽和"
        );
        // メモリ契約: fine 表は最大 8,000,008 B。旧 480,000,008 B との商は
        // 480,000,008 / 8,000,008 = 59.99995… (整数除算商 59) = 「約 1/60」。
        // 「1/60 未満」と断言するのは数学的に誤り (60×8,000,008 = 480,000,480 > 480,000,008
        // なので厳密には 1/60 より僅かに大きい) — 捕捉 22 件目: 自分の断言がテスト赤で捕捉。
        assert_eq!(480_000_008usize / (8 * (1_000_000 + 1)), 59);
        assert!(60 * 8 * (1_000_000 + 1) > 480_000_008usize);
        assert!(59 * 8 * (1_000_000 + 1) < 480_000_008usize);
    }

    #[test]
    fn ascii_bucket_labels_are_interval_honest() {
        // DD-4 根治の厳密ピン: bucket7 は 10s 以上（10,000,000us 丁度を含む）
        // → ラベルは ">10s" ではなく ">=10s"。bucket0 は 0us を含む "<10us"。
        let mut h = Hdr::new(100);
        h.record(5);
        h.record(10_000_000);
        let s = h.ascii(8);
        assert!(s.contains("<10us"), "{s}");
        assert!(s.contains(">=10s"), "{s}");
        assert_eq!(s.lines().count(), 2, "空バケット行は出さない");
    }
}
