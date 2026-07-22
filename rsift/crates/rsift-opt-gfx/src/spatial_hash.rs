//! 3D spatial hash for entity queries (Tier 5).

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SpatialHashGrid3D {
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
    /// entity_id → cell key for O(1) remove/move
    locations: HashMap<u32, (i32, i32, i32)>,
}

impl SpatialHashGrid3D {
    pub fn new(cell_size: f32) -> Self {
        Self {
            cell: cell_size.max(0.5),
            cells: HashMap::new(),
            locations: HashMap::new(),
        }
    }

    /// セルキー。負座標は floor で i32 の負側に正しく割れる
    /// ((x/cell).floor() as i32 — Rust の float→int cast は有限値で飽和)。
    /// 非有限 (NaN/±inf) の座標は NaN→0 / inf→端飽和の「謎セル」に堕ちるため
    /// 公開 API で契約拒否している (wave 26 監査)。
    #[inline]
    fn key(&self, x: f32, y: f32, z: f32) -> (i32, i32, i32) {
        (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        )
    }

    /// 契約 (2026-07-22 wave 26 監査で追加): 座標は有限 f32。
    /// NaN だと key() の cast が 0 セルに、±inf だと端飽和セルに静かに分類され、
    /// 本来の世界位置と無関係なクエリに混入する旧動作を fail-loud 化。
    pub fn insert(&mut self, id: u32, x: f32, y: f32, z: f32) {
        assert!(
            x.is_finite() && y.is_finite() && z.is_finite(),
            "SpatialHashGrid3D::insert: coords must be finite (got {x},{y},{z})"
        );
        self.remove(id);
        let k = self.key(x, y, z);
        self.cells.entry(k).or_default().push(id);
        self.locations.insert(id, k);
    }

    pub fn remove(&mut self, id: u32) {
        if let Some(k) = self.locations.remove(&id) {
            if let Some(list) = self.cells.get_mut(&k) {
                list.retain(|&e| e != id);
                if list.is_empty() {
                    self.cells.remove(&k);
                }
            }
        }
    }

    pub fn move_entity(&mut self, id: u32, x: f32, y: f32, z: f32) {
        let nk = self.key(x, y, z);
        if self.locations.get(&id) == Some(&nk) {
            return;
        }
        self.insert(id, x, y, z);
    }

    /// 返り値は id 昇順・重複無し (HashMap 反復順に依らず完全決定的)。
    /// 契約 (wave 26): min/max とも有限かつ軸毎に min<=max。逆転 AABB は
    /// 呼び出し側バグとして fail-loud する (旧: range が空で静かに 0 件)。
    /// 計算量は包含セル数 Θ(∏(k1-k0+1)) — 巨大 AABB の全走査は呼び出し側で
    /// 半径を制御すること (空間ハッシュの前提設計)。
    pub fn query_aabb(&self, min: [f32; 3], max: [f32; 3]) -> Vec<u32> {
        assert!(
            min.iter().all(|v| v.is_finite())
                && max.iter().all(|v| v.is_finite())
                && min.iter().zip(max.iter()).all(|(lo, hi)| lo <= hi),
            "SpatialHashGrid3D::query_aabb: need finite min<=max (got {min:?}..{max:?})"
        );
        let k0 = self.key(min[0], min[1], min[2]);
        let k1 = self.key(max[0], max[1], max[2]);
        let mut out = Vec::new();
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for z in k0.2..=k1.2 {
                    if let Some(list) = self.cells.get(&(x, y, z)) {
                        out.extend(list.iter().copied());
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 球クエリは包含 AABB で近似 (cell 実体ではなく id 収集段なので
    /// 実距離フィルタは掛けない — 呼び出し側で距離精査する契約)。
    /// 契約 (wave 26): center/radius は有限。半径負は 0 に正規化
    /// (旧からの仕様)、NaN は拒否 (f32::max が NaN→0.0 に直す静寂を閉塞)。
    pub fn query_radius(&self, cx: f32, cy: f32, cz: f32, radius: f32) -> Vec<u32> {
        assert!(
            cx.is_finite() && cy.is_finite() && cz.is_finite() && radius.is_finite(),
            "SpatialHashGrid3D::query_radius: center/radius must be finite"
        );
        let r = radius.max(0.0);
        self.query_aabb([cx - r, cy - r, cz - r], [cx + r, cy + r, cz + r])
    }

    pub fn len(&self) -> usize {
        self.locations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_query() {
        let mut g = SpatialHashGrid3D::new(8.0);
        g.insert(1, 0.0, 0.0, 0.0);
        g.insert(2, 100.0, 0.0, 0.0);
        let near = g.query_radius(0.0, 0.0, 0.0, 16.0);
        assert!(near.contains(&1));
        assert!(!near.contains(&2));
    }

    /// wave 26-4: churn 列 (配置・移動・重回登録・除去) の各段で
    /// クエリ結果が手計算期待と完全一致 (負側 cell の floor 規則含む) し、
    /// HashMap 反復順に依らず常に昇順で返ることの機械ピン。
    /// cell=8: id1 (0.5)→(0,0,0), id2 (8.5)→(1,0,0), id3 (-0.5)→(-1,0,0)。
    #[test]
    fn churn_sequence_matches_hand_derived_results() {
        let mut g = SpatialHashGrid3D::new(8.0);
        g.insert(1, 0.5, 0.5, 0.5);
        g.insert(2, 8.5, 0.5, 0.5);
        g.insert(3, -0.5, 0.5, 0.5);
        assert_eq!(
            g.query_aabb([-1.0, 0.0, 0.0], [16.0, 1.0, 1.0]),
            vec![1, 2, 3],
            "cells (-1..2,0,0) all hit, sorted"
        );
        // id1 を cell (12,0,0) へ移動 → 範囲外
        g.move_entity(1, 100.5, 0.5, 0.5);
        assert_eq!(g.query_aabb([-1.0, 0.0, 0.0], [16.0, 1.0, 1.0]), vec![2, 3]);
        // 同セル move は no-op に見える (再配置なしでも結果同一)
        g.move_entity(2, 9.0, 0.5, 0.5);
        assert_eq!(g.query_aabb([-1.0, 0.0, 0.0], [16.0, 1.0, 1.0]), vec![2, 3]);
        // 重複 insert は移動として振る舞う (旧セルから消える)
        g.insert(1, 0.5, 0.5, 0.5);
        assert_eq!(
            g.query_aabb([-1.0, 0.0, 0.0], [16.0, 1.0, 1.0]),
            vec![1, 2, 3]
        );
        // 除去後の残員と len の帳簿一致
        g.remove(2);
        assert_eq!(g.query_aabb([-1.0, 0.0, 0.0], [16.0, 1.0, 1.0]), vec![1, 3]);
        assert_eq!(g.len(), 2, "locations ledger after remove");
        // 半径クエリの AABB 近似契約: (0.5,0.5,0.5) r=1.0 は
        // aabb [-0.5,1.5]^2 → cells (-1..0, -1..0, -1..0) に 1 と 3
        assert_eq!(g.query_radius(0.5, 0.5, 0.5, 1.0), vec![1, 3]);
    }

    /// wave 26-5: 非有限座標/AABB 逆転は fail-loud (旧: 謎セル混入 /
    /// 空 range で静かに 0 件)。
    #[test]
    #[should_panic(expected = "coords must be finite")]
    fn insert_rejects_nan_coord() {
        let mut g = SpatialHashGrid3D::new(8.0);
        g.insert(1, f32::NAN, 0.0, 0.0);
    }

    #[test]
    #[should_panic(expected = "need finite min<=max")]
    fn query_aabb_rejects_inverted_bounds() {
        let g = SpatialHashGrid3D::new(8.0);
        let _ = g.query_aabb([1.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
    }

    #[test]
    #[should_panic(expected = "center/radius must be finite")]
    fn query_radius_rejects_nan_radius() {
        let g = SpatialHashGrid3D::new(8.0);
        let _ = g.query_radius(0.0, 0.0, 0.0, f32::NAN);
    }
}
