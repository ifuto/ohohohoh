//! wave 107 仮設差分ファズ (コミット対象外): BitpackedSection vs 単純参照モデル。
//! 目的: 既存テスト未踏の 10..=16 bit 幅・palette 境界 (id == 2^bits) ・
//! 跨ぎワード spill を、決定論 PRNG で網羅的に突き、set/get の完全可逆性を破壊検査する。

use rsift_opt_gfx::bitpacked_section::{CompactChunkSection, SECTION_VOL};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn full_verify(cs: &CompactChunkSection, model: &[u16; SECTION_VOL], tag: &str) {
    for i in 0..SECTION_VOL {
        let got = cs.get(i % 16, (i / 16) % 16, i / 256);
        assert_eq!(got, model[i], "MISMATCH cell={i} tag={tag}");
    }
}

fn bits_of(cs: &CompactChunkSection) -> usize {
    match cs {
        CompactChunkSection::SingleValue(_) => 0,
        CompactChunkSection::Bitpacked(b) => b.bits_per_block,
    }
}

fn main() {
    // Phase A: ランダム ops (重複 state 多用 + 時々新 state) 10 万 ops。
    let mut cs = CompactChunkSection::new_air();
    let mut model = [0u16; SECTION_VOL];
    let mut rng = Rng(0xC0FFEE);
    let mut pool_max = 1u16; // 徐々に拡大する値域 → width 境界の一再往復
    for op in 0..100_000u64 {
        let cell = rng.below(SECTION_VOL as u64) as usize;
        let (x, y, z) = (cell % 16, (cell / 16) % 16, cell / 256);
        let r = rng.below(100);
        let state = if r < 70 {
            (rng.below(pool_max as u64 + 1)) as u16
        } else {
            pool_max = (pool_max + 1).min(9000); // 最大 ~13+bit
            pool_max
        };
        cs.set(x, y, z, state);
        model[cell] = state;
        if op % 5_000 == 4_999 {
            full_verify(&cs, &model, "phaseA");
        }
    }
    full_verify(&cs, &model, "phaseA-final");
    assert!(bits_of(&cs) >= 13, "13bit 未達: {}", bits_of(&cs));

    // Phase B: palette 境界 (states 数 = 2^k, 2^k±1) を細かく掃引。
    for n_states in [1u32, 2, 3, 15, 16, 17, 31, 32, 33, 255, 256, 257, 511, 512, 513, 1023, 1024, 2048, 4095, 4096] {
        for phase in 0..2 {
            let mut cs = CompactChunkSection::new_air();
            let mut model = [0u16; SECTION_VOL];
            for t in 0..SECTION_VOL {
                let v = ((t + phase * 7919) % n_states as usize) as u16;
                model[t] = v;
                cs.set(t % 16, (t / 16) % 16, t / 256, v);
            }
            full_verify(&cs, &model, "phaseB");
        }
    }

    // Phase C: u16 多重全域 — 40,000 連番 state で 15→16 bit 境界を越える。
    // (palette は履歴累積するため 4,096 セルでも 32,768+ 状態へ到達可能)
    let mut cs = CompactChunkSection::new_air();
    let mut model = [0u16; SECTION_VOL];
    let mut width_hits: Vec<usize> = Vec::new();
    let mut last_w = 4;
    for t in 0..40_300u32 {
        let state = t as u16; // 0,1,...,40300 (wrapping せず u16 域内)
        let cell = (t as usize) % SECTION_VOL;
        model[cell] = state;
        cs.set(cell % 16, (cell / 16) % 16, cell / 256, state);
        let w = bits_of(&cs);
        if w != last_w {
            width_hits.push(w);
            full_verify(&cs, &model, "phaseC-transition");
            last_w = w;
        }
    }
    full_verify(&cs, &model, "phaseC-final");
    assert!(width_hits.contains(&16), "16bit に未到達: {:?}", width_hits);
    println!(
        "DG FUZZ ALL PASS — phaseA 100k ops / phaseB 境界 20×2 / phaseC 40,300 states widths {:?}",
        width_hits
    );
}
