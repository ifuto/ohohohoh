//! FastChest 逆輸入 — ブロックエンティティ (チェスト等) の静的メッシュ化。
//!
//! BE 描画コストの原因 = 毎フレームの model パーツ評価・蓋アニメ・個別描画。
//! FastChest は「閉まってる/開閉しそうにない BE を通常ブロックとして固化」する。
//! ここではインタラクト距離・アニメ時計・使用頻度で昇格/降格する決定状態機械.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticPromoteKind {
    Chest,
    TrappedChest,
    EnderChest,
    Barrel,
    ShulkerBox,
    Other,
}

impl StaticPromoteKind {
    /// 閉蓋状態の静的モデル名と 1:1。リソースパックの closed モデル id。
    pub fn closed_model_id(&self) -> &'static str {
        match self {
            Self::Chest => "minecraft:block/chest",
            Self::TrappedChest => "minecraft:block/chest_trapped",
            Self::EnderChest => "minecraft:block/ender_chest",
            Self::Barrel => "minecraft:block/barrel",
            Self::ShulkerBox => "minecraft:block/shulker_box",
            Self::Other => "minecraft:block/chest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeRenderMode {
    /// BE として毎フレーム個別描画 (開いたまま・カメラ近距離)
    DynamicBlockEntity,
    /// 通常ブロックメッシュに昇格 (蓋閉モデルでチャンクメッシュに吸収)
    StaticChunkMesh,
}

#[derive(Debug, Clone)]
pub struct BePromotionEntry {
    pub kind: StaticPromoteKind,
    /// 最後に開閉があった tick
    pub last_anim_tick: u64,
    /// 最近インタラクトされた tick
    pub last_interact_tick: u64,
    pub mode: BeRenderMode,
    /// 高速点滅 (何度も開閉) 検知 — この場合動的のまま
    pub anim_burst_score: u32,
}

pub struct BeStaticPolicy {
    /// この tick 数だけアニメ無しで昇格 (既定 40 tick = 2秒)
    pub promote_after_ticks: u64,
    /// 降格するインタラクト距離 (m)
    pub interact_demote_distance: f32,
    /// 連続開閉数がこの閾値を超えたら無期限動的
    pub burst_guard: u32,
}

impl Default for BeStaticPolicy {
    fn default() -> Self {
        Self {
            promote_after_ticks: 40,
            interact_demote_distance: 3.5,
            burst_guard: 6,
        }
    }
}

impl BeStaticPolicy {
    /// BE 登録/更新。返り値はこのフレームの描画モード。
    pub fn tick(
        &self,
        entry: &mut BePromotionEntry,
        now_tick: u64,
        opened_now: bool,
        interacted_now: bool,
        camera_distance: f32,
        static_mesh_ready: bool,
    ) -> BeRenderMode {
        if opened_now {
            if now_tick.saturating_sub(entry.last_anim_tick) <= 20 {
                entry.anim_burst_score = entry.anim_burst_score.saturating_add(1);
            } else {
                entry.anim_burst_score = 0;
            }
            entry.last_anim_tick = now_tick;
        }
        if interacted_now {
            entry.last_interact_tick = now_tick;
        }

        // 降格ルール (優先度順): burst > 近距離インタラクト直後 > 直近アニメ
        if entry.anim_burst_score >= self.burst_guard {
            entry.mode = BeRenderMode::DynamicBlockEntity;
            return entry.mode;
        }
        let since_anim = now_tick.saturating_sub(entry.last_anim_tick);
        let since_interact = now_tick.saturating_sub(entry.last_interact_tick);
        if since_anim < self.promote_after_ticks {
            entry.mode = BeRenderMode::DynamicBlockEntity;
            return entry.mode;
        }
        if camera_distance <= self.interact_demote_distance && since_interact < self.promote_after_ticks * 4 {
            // 触りに来た瞬間を動的 BE のまま扱う (蓋の開きを必ず見せる)
            entry.mode = BeRenderMode::DynamicBlockEntity;
            return entry.mode;
        }

        // 全条件クリア → 静的メッシュに昇格
        if static_mesh_ready {
            entry.mode = BeRenderMode::StaticChunkMesh;
        }
        entry.mode
    }

    /// 静的昇格時のモデル id (リソースパックの closed モデル)。
    pub fn promoted_model(entry: &BePromotionEntry) -> &'static str {
        entry.kind.closed_model_id()
    }
}

/// ブロック位置のキー (64bit パック)。
#[inline]
pub fn pos_pack(x: i32, y: i32, z: i32) -> u64 {
    let a = ((x as u64) & 0x3FF_FFFF) << 38;
    let b = ((z as u64) & 0x3FF_FFFF) << 12;
    let c = (y as u64) & 0xFFF;
    a | b | c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: StaticPromoteKind) -> BePromotionEntry {
        BePromotionEntry {
            kind,
            last_anim_tick: 0,
            last_interact_tick: 0,
            mode: BeRenderMode::DynamicBlockEntity,
            anim_burst_score: 0,
        }
    }

    #[test]
    fn idle_chest_promotes_to_static() {
        let p = BeStaticPolicy::default();
        let mut e = entry(StaticPromoteKind::Chest);
        // 初回 anim & interact は 0=古い
        let m = p.tick(&mut e, 1000, false, false, 8.0, true);
        assert_eq!(m, BeRenderMode::StaticChunkMesh);
        assert_eq!(BeStaticPolicy::promoted_model(&e), "minecraft:block/chest");
    }

    #[test]
    fn recent_anim_stays_dynamic() {
        let p = BeStaticPolicy::default();
        let mut e = entry(StaticPromoteKind::Chest);
        let m = p.tick(&mut e, 30, true, false, 8.0, true);
        assert_eq!(m, BeRenderMode::DynamicBlockEntity);
        let m2 = p.tick(&mut e, 45, false, false, 8.0, true);
        assert_eq!(m2, BeRenderMode::DynamicBlockEntity);
    }

    #[test]
    fn burst_stays_dynamic_forever() {
        let p = BeStaticPolicy { burst_guard: 3, ..Default::default() };
        let mut e = entry(StaticPromoteKind::Chest);
        // 短い間隔で 4 回開閉
        for t in [1u64, 10, 20, 30] {
            p.tick(&mut e, t, true, false, 8.0, true);
        }
        let m = p.tick(&mut e, 10_000, false, false, 8.0, true);
        assert_eq!(m, BeRenderMode::DynamicBlockEntity);
    }

    #[test]
    fn pos_pack_distinct() {
        assert_ne!(pos_pack(1, 64, 2), pos_pack(2, 64, 1));
        assert_ne!(pos_pack(-1, 0, 0), pos_pack(1, 0, 0));
    }
}
