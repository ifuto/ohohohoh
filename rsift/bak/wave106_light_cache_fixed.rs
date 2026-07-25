//! Block light propagation cache (Tier 6).
//! Dirty BFS (label-correcting) flood — only recompute dirty sections (Minecraft light engine idea).
//! 正直注記 (wave 106): sky ニブルは格納のみで sky flood は未実装 (scaffold 温存)。
//! また消灯/減衰の伝播は未実装 (除去 BFS が必要) — set_emitter による上書きは
//! 増分方向の再 flood のみで、古い溢れ光は除去されない (契約として明記)。

const SEC: usize = 16;
const SEC_VOL: usize = SEC * SEC * SEC;

#[derive(Debug, Clone)]
pub struct SectionLights {
    /// Packed: low4 = block, high4 = sky
    pub packed: Vec<u8>,
    pub dirty: bool,
}

impl SectionLights {
    pub fn new() -> Self {
        Self {
            packed: vec![0; SEC_VOL],
            dirty: true,
        }
    }

    #[inline]
    fn idx(x: usize, y: usize, z: usize) -> usize {
        (y * SEC + z) * SEC + x
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> (u8, u8) {
        let v = self.packed[Self::idx(x, y, z)];
        (v & 0x0f, v >> 4)
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, block: u8, sky: u8) {
        let i = Self::idx(x, y, z);
        let v = (block & 0x0f) | ((sky & 0x0f) << 4);
        // DF-2: 同値上書きは no-op (dirty を汚染して無駄な re-flood を誘発しない。
        // wave 102 DB-4 同型。値が変わる場合のみ packed 更新 + dirty 宣言)
        if self.packed[i] == v {
            return;
        }
        self.packed[i] = v;
        self.dirty = true;
    }
}

impl Default for SectionLights {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct LightPropagationCache {
    sections: std::collections::HashMap<(i32, i32, i32), SectionLights>,
}

impl LightPropagationCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn section_mut(&mut self, cx: i32, cy: i32, cz: i32) -> &mut SectionLights {
        self.sections
            .entry((cx, cy, cz))
            .or_insert_with(SectionLights::new)
    }

    pub fn get_light(&self, wx: i32, wy: i32, wz: i32) -> (u8, u8) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        self.sections
            .get(&(cx, cy, cz))
            .map(|s| s.get(lx, ly, lz))
            .unwrap_or((0, 0))
    }

    pub fn mark_dirty(&mut self, wx: i32, wy: i32, wz: i32) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        self.section_mut(cx, cy, cz).dirty = true;
    }

    pub fn set_emitter(&mut self, wx: i32, wy: i32, wz: i32, level: u8) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        let s = self.section_mut(cx, cy, cz);
        let (_, sky) = s.get(lx, ly, lz);
        s.set(lx, ly, lz, level.min(15), sky);
    }

    /// Label-correcting flood of block light within a section (opaque mask: true = blocks light)。
    ///
    /// 戻り値は **改善書込み数** (隣接セルの block ニブルを増加させた回数。wave 106 で
    /// 「pop 数」から意味変更 — 外部消費者ゼロ、テストの進捗観測のみ)。
    ///
    /// 設計 (DF-1 + DF-5 根治): 永続 queue を持たず、各呼出が全セル i=0..4096 を
    /// 走査して improvement (書込み) のみを予算 `max_steps` の対象とする pass 反復。
    /// 改善は各セル高々 15 回 (値域 1..=15) → 総数 ≤ 15×4,096 = 61,440 (as u32 飽和なし)。
    /// - 冪等な再訪は予算を消費しないため、小さい max_steps でも継続呼出が必ず
    ///   前進する (旧 full re-seed 設計は先頭冪等セルに予算が燃えて飢餓 = livelock)。
    /// - fixpoint は改善飽和の唯一解で、実行順・予算分割に依らず同一 (confluence、
    ///   Python 機械検算: budget 64 → 50 calls 収束・最終 packed bit 一致)。
    /// - dirty は「収束したか」に忠実: 予算打ち切り → true 維持、無変更 pass 到達 → false。
    /// opaque 点灯セル自身は発光し得る設計 (MC の光源ブロック相当) — 近傍への
    /// 書込みのみ opaque を遮る。
    pub fn propagate_dirty(
        &mut self,
        cx: i32,
        cy: i32,
        cz: i32,
        opaque: &[bool; SEC_VOL],
        max_steps: usize,
    ) -> u32 {
        let Some(sec) = self.sections.get_mut(&(cx, cy, cz)) else {
            return 0;
        };
        if !sec.dirty {
            return 0;
        }
        // DF-1/DF-5: 旧 VecDeque flood は ①pop 後 break で先頭アイテムを未処理破棄、
        // ②max_steps=0 や丁度境界で「queue 空 × 未処理残」でも dirty=false に確定
        // (以後 `!dirty → return 0` で永久 no-op = 収束詐称)、③full re-seed +
        // 小さい max_steps で先頭冪等セルに予算が燃え frontier が進まない飢餓、
        // の三重欠陥があった。予算は「書込み直前」に判定 (budget-before) するため
        // max_steps=0 は 0 writes で dirty=true を維持する。
        let mut writes = 0usize;
        let mut truncated = false;
        'passes: loop {
            let mut changed = false;
            for i in 0..SEC_VOL {
                let level = sec.packed[i] & 0x0f;
                if level <= 1 {
                    continue;
                }
                let x = i % SEC;
                let z = (i / SEC) % SEC;
                let y = i / (SEC * SEC);
                // wrapping_sub は underflow で巨大値となり `>= SEC` フィルタに載る
                // (負にはならない設計 — ny/nz 側も同様)
                let neighbors = [
                    (x.wrapping_sub(1), y, z),
                    (x + 1, y, z),
                    (x, y.wrapping_sub(1), z),
                    (x, y + 1, z),
                    (x, y, z.wrapping_sub(1)),
                    (x, y, z + 1),
                ];
                for (nx, ny, nz) in neighbors {
                    if nx >= SEC || ny >= SEC || nz >= SEC {
                        continue;
                    }
                    let ni = (ny * SEC + nz) * SEC + nx;
                    if opaque[ni] {
                        continue;
                    }
                    let next = level - 1;
                    let (cur_b, sky) = {
                        let v = sec.packed[ni];
                        (v & 0x0f, v >> 4)
                    };
                    if next > cur_b {
                        if writes >= max_steps {
                            truncated = true;
                            break 'passes;
                        }
                        sec.packed[ni] = next | (sky << 4);
                        writes += 1;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        sec.dirty = truncated;
        writes as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_spreads() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(0, 0, 0, 15);
        let opaque = [false; SEC_VOL];
        let steps = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert!(steps > 0);
        let (b, _) = c.get_light(2, 0, 0);
        assert!(b > 0 && b < 15);
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn missing_section_reads_zero_even_at_negative_coords() {
        let c = LightPropagationCache::new();
        assert_eq!(c.get_light(0, 0, 0), (0, 0));
        assert_eq!(c.get_light(-100, 320, 7), (0, 0));
        assert_eq!(c.get_light(i32::MIN, i32::MIN, i32::MIN), (0, 0));
    }

    #[test]
    fn set_emitter_clamps_preserves_sky_and_addresses_negative() {
        let mut c = LightPropagationCache::new();
        assert_eq!(c.set_emitter(3, 70, -5, 14), ());
        assert_eq!(c.get_light(3, 70, -5), (14, 0));
        c.set_emitter(3, 70, -5, 250);
        assert_eq!(c.get_light(3, 70, -5).0, 15, "level は 15 に clamp");
        // 負座標のアドレッシング (div_euclid/rem_euclid 規約) も往復する。
        c.set_emitter(-8, -1, -3, 12);
        assert_eq!(c.get_light(-8, -1, -3), (12, 0));
        assert_eq!(c.get_light(-8, -1, -4), (0, 0), "近傍は非影響");
        // sky ニブルは set_emitter が保持する。
        {
            let s = c.section_mut(0, 0, 0);
            let (b, _) = s.get(5, 5, 5);
            s.set(5, 5, 5, b, 9);
        }
        c.set_emitter(5, 5, 5, 7);
        assert_eq!(c.get_light(5, 5, 5), (7, 9), "set_emitter は sky を保持");
    }

    #[test]
    fn propagate_decrements_by_manhattan_distance_and_zero_beyond_15() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(1, 1, 1, 15);
        let opaque = [false; SEC_VOL];
        let _ = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert_eq!(c.get_light(2, 1, 1).0, 14, "dist 1");
        assert_eq!(c.get_light(1, 3, 2).0, 12, "manhattan 3 → 15-3");
        assert_eq!(c.get_light(1, 1, 15).0, 1, "dist 14 → 1");
        assert_eq!(
            c.get_light(15, 15, 15).0,
            0,
            "dist 42 > 15 → 減衰しきって 0"
        );
    }

    #[test]
    fn opaque_wall_blocks_propagation() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(1, 1, 1, 15);
        let mut opaque = [false; SEC_VOL];
        // x=2 平面を全面壁に。
        for y in 0..16 {
            for z in 0..16 {
                opaque[(y * 16 + z) * 16 + 2] = true;
            }
        }
        let _ = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert_eq!(c.get_light(3, 1, 1).0, 0, "壁の向こうは照らされない");
        assert_eq!(c.get_light(1, 1, 1).0, 15, "エミッタ自身は残る");
        assert_eq!(c.get_light(1, 2, 1).0, 14, "壁の手前側は通常どおり減衰");
    }

    #[test]
    fn dirty_cleared_after_full_flood_and_zero_steps_is_noop() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(1, 1, 1, 15);
        let opaque = [false; SEC_VOL];
        let first = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert!(first > 0);
        let second = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert_eq!(second, 0, "完遂後 dirty=false で再 flood は no-op");
        // max_steps=0: 何も処理されない (dirty は立ったまま消費されない)。
        let mut c2 = LightPropagationCache::new();
        c2.set_emitter(1, 1, 1, 15);
        let s = c2.propagate_dirty(0, 0, 0, &opaque, 0);
        assert_eq!(s, 0);
        assert_eq!(c2.get_light(2, 1, 1).0, 0, "steps 消費 0 で伝播なし");
    }

    /// DF-1 根治ピン: max_steps=0 は未処理のまま dirty を消費しない
    /// (旧実装は dirty=false に確定して以後の呼出が永久 no-op = 収束詐称)。
    #[test]
    fn zero_steps_preserves_dirty_and_later_converges() {
        let opaque = [false; SEC_VOL];
        let mut c = LightPropagationCache::new();
        c.set_emitter(1, 1, 1, 15);
        let s = c.propagate_dirty(0, 0, 0, &opaque, 0);
        assert_eq!(s, 0);
        assert!(
            c.sections.get(&(0, 0, 0)).unwrap().dirty,
            "max_steps=0 で dirty=false にされると継続不能 = DF-1 詐称"
        );
        assert_eq!(c.get_light(2, 1, 1).0, 0, "まだ伝播していない");
        let _ = c.propagate_dirty(0, 0, 0, &opaque, 100_000);
        assert_eq!(c.get_light(2, 1, 1).0, 14, "継続呼出で収束到達");
    }

    /// DF-1+DF-5 根治ピン: 小さい max_steps (64) の打ち切り継続でも飢餓せず収束し、
    /// 最終 packed は一括 flood と bit 一致 (confluence)。旧 re-seed 設計は
    /// 先頭冪等セルに予算が燃えて livelock、writes-budget 設計で構造排除。
    #[test]
    fn small_budget_truncation_still_converges() {
        let opaque = [false; SEC_VOL];
        let mut full = LightPropagationCache::new();
        full.set_emitter(1, 1, 1, 15);
        full.set_emitter(10, 10, 10, 12);
        let full_writes = full.propagate_dirty(0, 0, 0, &opaque, 1_000_000);
        // 改善書込み総数 (2 emitters シナリオ) の厳密ピン: Python 独立シムで一致確認
        assert_eq!(full_writes, 2_639);
        assert!(!full.sections.get(&(0, 0, 0)).unwrap().dirty);

        let mut part = LightPropagationCache::new();
        part.set_emitter(1, 1, 1, 15);
        part.set_emitter(10, 10, 10, 12);
        let mut calls = 0usize;
        loop {
            let s = part.propagate_dirty(0, 0, 0, &opaque, 64);
            let d = part.sections.get(&(0, 0, 0)).unwrap().dirty;
            calls += 1;
            if !d {
                break; // 収束
            }
            assert!(
                s >= 1,
                "未完 (dirty=true) なら継続呼出は必ず前進 (writes ≥ 1)"
            );
            assert!(
                calls <= 200,
                "飢餓せず収束するはず (機械検算: 50 calls 確定)"
            );
        }
        assert_eq!(
            full.sections[&(0, 0, 0)].packed,
            part.sections[&(0, 0, 0)].packed,
            "最終 packed は一括 flood と bit 一致"
        );
    }

    /// DF-2 根治ピン: 同値 no-op set は dirty を汚染せず値も不変。
    #[test]
    fn noop_set_does_not_remark_dirty() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(1, 1, 1, 15);
        let opaque = [false; SEC_VOL];
        let _ = c.propagate_dirty(0, 0, 0, &opaque, 100_000);
        assert!(
            !c.sections.get(&(0, 0, 0)).unwrap().dirty,
            "完遂後は dirty=false"
        );
        let before = c.sections[&(0, 0, 0)].packed.clone();
        {
            let s = c.section_mut(0, 0, 0);
            let (b, sky) = s.get(2, 1, 1);
            s.set(2, 1, 1, b, sky); // 同一値 (14, 0)
        }
        assert!(
            !c.sections.get(&(0, 0, 0)).unwrap().dirty,
            "同値 no-op で dirty を立てない"
        );
        assert_eq!(c.sections[&(0, 0, 0)].packed, before, "値も不変");
    }
}
