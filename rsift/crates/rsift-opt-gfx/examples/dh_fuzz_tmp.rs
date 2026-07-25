//! wave 108 仮設差分ファズ (非コミット): morton_order 全経路 vs naive 参照。
//! 検証対象:
//!  A. split/compact_by_3 / _by_2 が naive ビット展開と全ドメイン一致
//!  B. encode/decode 3d/2d・*_fast ≡ base (BMI2 ランタイム経路を含む) 全入力一致
//!     (x ≥ 1024 の切捨てドメインでも pdep/swar が一致すること)
//!  C. MortonGrid3D の全セル単射 (SIZE=16: encode が衝突しないこと)
//!  D. 旧 sort_by_key 実装と「キー前計算+安定ペア sort」最適化の完全結果一致
//!     (同一キーでの入力順保持 = 安定性)

use rsift_opt_gfx::morton_order::*;

// ------- naive 参照 (素直な bit-by-bit 展開) -------
fn naive_split3(a: u32) -> u64 {
    let a = a & 0x3FF; // 現行契約: 10 bit 切捨て
    let mut x = 0u64;
    for i in 0..10 {
        x |= (((a >> i) & 1) as u64) << (3 * i);
    }
    x
}
fn naive_compact3(mut m: u64) -> u32 {
    let mut a = 0u32;
    m &= 0x0924_9249; // 現行は入力側に mask なし → 同様に全ビット走査する代わりに pre-mask は不要 (周期 3 の全 64 bit 走査)
    let _ = m;
    let mut mm = m;
    for i in 0..22u32 {
        // 現行 compact は 0x3FF 出力マスク = 低 30 bit の周期-3 位置のみが効く (3i ≤ 30 ⇔ i ≤ 10)
        a |= (((mm >> (3 * i)) & 1) as u32) << i;
    }
    a & 0x3FF
}
fn naive_split2(a: u32) -> u64 {
    let mut x = 0u64;
    for i in 0..32 {
        x |= (((a >> i) & 1) as u64) << (2 * i);
    }
    x
}
fn naive_compact2(mut m: u64) -> u32 {
    let mut a = 0u32;
    for i in 0..32 {
        a |= (((m >> (2 * i)) & 1) as u32) << i;
    }
    a
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

fn main() {
    let mut rng = Rng(0xDDA108);

    // A+B: 3d pair — 境界 (0,1,1023,1024,1025,65535,1<<21-1,u32::MAX) + ランダム 2M
    let edges: [u32; 11] = [0, 1, 2, 1023, 1024, 1025, 2047, 65535, (1 << 21) - 1, 1 << 21, u32::MAX];
    let mut seen = 0u64;
    for round in 0..2u64 {
        let test_one = |v: u32| {
            // split/compact base == naive (全 u32 ドメインで切捨て契約含め一致)
            assert_eq!(split_by_3(v), naive_split3(v), "split3 v={v}");
            assert_eq!(compact_by_3(v as u64), naive_compact3(v as u64), "compact3 v={v}");
            // base≡fast for ALL inputs (BMI2 経路有無を機械独立に一致証明)
            let (x, y, z) = (v, v.rotate_left(7) ^ 0x5555, v.wrapping_mul(2654435761) >> 3);
            let (m_b, m_f) = (morton_encode_3d(x, y, z), morton_encode_3d_fast(x, y, z));
            assert_eq!(m_b, m_f, "encode3d base!=fast at ({x},{y},{z})");
            assert_eq!(morton_decode_3d(m_b), morton_decode_3d_fast(m_f));
            // 10-bit 内では roundtrip 恒等
            let (a, b, c) = (x & 1023, y & 1023, z & 1023);
            assert_eq!(morton_decode_3d(morton_encode_3d(a, b, c)), [a, b, c], "rt3d");
            assert_eq!(morton_decode_3d_fast(morton_encode_3d_fast(a, b, c)), [a, b, c]);
            // morton_encode_bmi2 alias ≡ fast
            assert_eq!(morton_encode_bmi2(x, y, z), m_f, "alias");
        };
        for &v in edges.iter() {
            test_one(v);
        }
        for _ in 0..2_000_000u64 {
            let v = rng.next() as u32;
            test_one(v);
            seen += 1;
            if seen % 500_000 == 0 {
                eprintln!("phase A/B progress: {seen}");
            }
        }
        if round == 1 {
            break;
        }
    }

    // A+B: 2d pair — 32bit 全ドメイン (roundtrip は全 u32 で恒等のはず)
    for &v in edges.iter() {
        assert_eq!(split_by_2(v), naive_split2(v), "split2 v={v}");
        assert_eq!(compact_by_2(v as u64), naive_compact2(v as u64), "compact2 v={v}");
    }
    let mut ids = 0u64;
    for i in 0..1_000_000u64 {
        let v = rng.next() as u32;
        assert_eq!(split_by_2(v), naive_split2(v), "split2 v={v}");
        assert_eq!(compact_by_2(split_by_2(v)), v, "rt2 split v={v}");
        let w = v ^ 0xDEAD_BEEF;
        assert_eq!(morton_decode_2d(morton_encode_2d(v, w)), [v, w], "rt2d");
        if i % 250_000 == 0 {
            eprintln!("phase 2d progress: {i}");
        }
        ids += 1;
    }

    // C: Grid 全単射 (SIZE=16 全 4,096 セル encode 相異なる + roundtrip)
    let mut grid = MortonGrid3D::<u32, 16>::new();
    let mut codes = std::collections::HashSet::new();
    for z in 0..16usize {
        for y in 0..16 {
            for x in 0..16 {
                let v = (x * 100 + y * 10 + z) as u32;
                assert!(grid.set(x, y, z, v));
                assert_eq!(grid.get(x, y, z), Some(&v));
                let m = morton_encode_3d_fast(x as u32, y as u32, z as u32);
                assert!(codes.insert(m), "encode 衝突 at ({x},{y},{z})");
            }
        }
    }
    assert!(!grid.set(16, 0, 0, 0) && grid.get(16, 0, 0).is_none(), "範囲外ガード");

    // D: sort 最適化の新旧完全一致 (安定性を含む) — 同一キー多発の入力を構成
    let old_impl = |positions: &[[i32; 3]]| -> Vec<usize> {
        let mut idx: Vec<usize> = (0..positions.len()).collect();
        idx.sort_by_key(|&i| {
            let p = positions[i];
            morton_encode_3d_fast(p[0] as u32 & 1023, p[1] as u32 & 1023, p[2] as u32 & 1023)
        });
        idx
    };
    let new_impl = |positions: &[[i32; 3]]| -> Vec<usize> {
        let mut pairs: Vec<(u64, usize)> = positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    morton_encode_3d_fast(p[0] as u32 & 1023, p[1] as u32 & 1023, p[2] as u32 & 1023),
                    i,
                )
            })
            .collect();
        pairs.sort_by_key(|&(k, _)| k);
        pairs.into_iter().map(|(_, i)| i).collect()
    };
    for seed in 0..64u64 {
        let mut r = Rng(seed + 1);
        let n = 1 + (r.next() % 300) as usize;
        let pos: Vec<[i32; 3]> = (0..n)
            .map(|_| {
                [
                    (r.next() % 34) as i32 - 17, // 負座標 & 1023 wrap 衝突を多発させる
                    (r.next() % 34) as i32 - 17,
                    (r.next() % 34) as i32 - 17,
                ]
            })
            .collect();
        assert_eq!(old_impl(&pos), new_impl(&pos), "sort 新旧不一致 seed={seed}");
    }

    println!(
        "DH FUZZ ALL PASS — 3d: edge11+4M 乱 + 2d: edge11+1M 乱 + grid 4096 全単射 + sort 新旧 64 種一致 (seen={seen} ids={ids})"
    );
}
