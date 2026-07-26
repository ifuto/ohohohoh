//! FastChest 逆輸入 — ブロックエンティティ (チェスト等) の静的メッシュ化。
//!
//! BE 描画コストの原因 = 毎フレームの model パーツ評価・蓋アニメ・個別描画。
//! FastChest は「閉まってる/開閉しそうにない BE を通常ブロックとして固化」する。
//! ここではインタラクト距離・アニメ時計・使用頻度で昇格/降格する決定状態機械.
//!
//! # 監査 2026-07-26 (wave 121 DU) — 契約公表
//!
//! - **DU-1**: `tick()` の境界契約を厳密 pin: 昇格猶予 40 tick
//!   (39→動的 / 40→昇格、共に初期 `last_anim_tick=0` 基準)・interact
//!   鮮度窓 `promote_after_ticks*4=160` (159→降格 / 160→通過)・近距離
//!   3.5 inclusive (`0x40600000` 降格 / `0x40600001`=3.5000002 通過)・
//!   burst 間隔 20 inclusive 累積 (21 でリセット)。**時計逆行**
//!   (`now < last`) は `saturating_sub→0` で安全側 (動的) 化。
//!   **NaN camera_distance** は全比較 false で近距離降格が不発し
//!   昇格側へ倒れる (fail-safe ではない: 誠実公表。SSR NaN とは逆側)。
//! - **DU-2**: `pos_pack` レイアウト厳密 pin (x: bit63-38・z: 37-12・
//!   y: 11-0 の排他 3 領域、rq 導出厳密値 5 件)。**領域外は折り畳み
//!   衝突**する (x=2^26≡0・y=±2048≡2048、共にピン): MC 世界境界
//!   ±30M < 2^25 内では単射だが契約として公表。
//! - **DU-3**: wiring 構造公表 — 唯一の Rust 側呼出
//!   `full_graph_wiring.rs:1369-1387` は `take(4)`・kind 常時 Chest・
//!   `opened=false, interacted=false, ready=true` 固定・戻り値は
//!   `let _mode` で破棄。ただし `be_entries: HashMap` への副作用
//!   (mode 遷移) は永続するため、恒等クラス (DM-2/DQ-1/DS-1) や常時
//!   miss (DT-1) とも別型の「決定破棄・状態機械のみ進行」構造。
//!   `dist` 欠損時の `unwrap_or(0.0)` は 0.0<=3.5 で近距離側に倒れる
//!   安全既定だが、interact 無しでは鮮度窓不発のため昇格を妨げない。
//! - **DU-4**: `static_mesh_ready=false` 時は現 mode を保持して返す
//!   (Dynamic 維持だけでなく **Static 維持**も)。Static からの降格経路は
//!   burst/anim/interact の 3 系統のみで「メッシュ喪失」降格は存在しない。
//! - **DU-5**: `promote_after_ticks*4` は u64 直接乗算 (既定 40→160 で
//!   不発)。policy を 2^62 超に変更すると debug で overflow panic
//!   (release では wrap)。契約記録のみ、既定設定では到達不能。
//! - **DU-6**: `closed_model_id` 全表 pin + `Other` が chest へ
//!   フォールバックする近似の公表 (リソースパックに汎用 BE が無いため)。

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
        if camera_distance <= self.interact_demote_distance
            && since_interact < self.promote_after_ticks * 4
        {
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
        let p = BeStaticPolicy {
            burst_guard: 3,
            ..Default::default()
        };
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

    // ================= wave 121 DU: 厳密契約ピン群 =================
    // 全厳密値は rq (tools/rspeed.rs rq, RQ.md v2 参照) で事前導出済。
    // du_pospack.rq / du_bounds.rq の機械出力に 1:1 で対応。

    /// DU-1: 昇格猶予 40 tick の境界 (39=動的 / 40=昇格)。初期 last_anim=0 基準。
    #[test]
    fn promote_boundary_40_ticks_exact() {
        let p = BeStaticPolicy::default();
        let mut e39 = entry(StaticPromoteKind::Chest);
        // since_anim = 39 < 40 → 動的維持
        assert_eq!(
            p.tick(&mut e39, 39, false, false, 8.0, true),
            BeRenderMode::DynamicBlockEntity,
            "39 tick 猶予内は動的"
        );
        let mut e40 = entry(StaticPromoteKind::Chest);
        // since_anim = 40 → 猶予切れ、dist=8.0>3.5 で近距離不発 → 昇格
        assert_eq!(
            p.tick(&mut e40, 40, false, false, 8.0, true),
            BeRenderMode::StaticChunkMesh,
            "40 tick で昇格 (40*1 = promote_after_ticks)"
        );
    }

    /// DU-1: interact 鮮度窓 = promote_after_ticks*4 = 160 の境界
    /// (159=降格 / 160=通過→昇格)。rq: 40*4 = 160。
    #[test]
    fn interact_freshness_boundary_160_exact() {
        let p = BeStaticPolicy::default();
        let mut e = entry(StaticPromoteKind::Chest);
        // t=50 で interact (last_interact=50, dist 近い → 降格)
        assert_eq!(
            p.tick(&mut e, 50, false, true, 3.0, true),
            BeRenderMode::DynamicBlockEntity
        );
        // since_interact = 159 < 160 → まだ降格
        assert_eq!(
            p.tick(&mut e, 209, false, false, 3.0, true),
            BeRenderMode::DynamicBlockEntity,
            "鮮度 159 は窓内で降格"
        );
        // since_interact = 160 → 窓外 → 昇格 (since_anim=210>=40)
        assert_eq!(
            p.tick(&mut e, 210, false, false, 3.0, true),
            BeRenderMode::StaticChunkMesh,
            "鮮度 160 で窓外に出て昇格"
        );
    }

    /// DU-1: 近距離閾値 3.5 は inclusive。**Static→Dynamic 降格経路の実証**を
    /// 兼ねる。3.5=0x40600000 降格 / 0x40600001=3.5000002 通過 (rq 導出)。
    #[test]
    fn interact_demote_distance_inclusive_exact_bits() {
        let p = BeStaticPolicy::default();
        let mut e = entry(StaticPromoteKind::Chest);
        // t=1000 で interact (遠いので不発) → 同時に Static 昇格を得る
        assert_eq!(
            p.tick(&mut e, 1000, false, true, 8.0, true),
            BeRenderMode::StaticChunkMesh
        );
        // 近距離へ接近: dist == 3.5 (inclusive) は降格 → Static から Dynamic へ
        assert_eq!(
            p.tick(&mut e, 1100, false, false, 3.5, true),
            BeRenderMode::DynamicBlockEntity,
            "dist==3.5 は inclusive で降格 (鮮度 100<160)"
        );
        // 1 ulp 上 (3.5000002) は閾値を escape: dist>3.5 で近距離ルール不発
        // → since_anim=1150>=40 により昇格へ復帰 (捕捉 47: 期待値を誤記し
        // 初回実行で機械捕捉された後に実装一致へ修正)
        assert_eq!(
            p.tick(
                &mut e,
                1150,
                false,
                false,
                f32::from_bits(0x4060_0001),
                true
            ),
            BeRenderMode::StaticChunkMesh,
            "1 ulp 上は inclusive 窓を escape して静昇格へ復帰"
        );
    }

    /// DU-1: burst 間隔 20 inclusive 累積 / 21 でリセット / guard 到達 /
    /// 非開閉 tick では score 減衰しない維持契約。
    #[test]
    fn burst_interval_boundary_and_guard_semantics() {
        let p = BeStaticPolicy {
            burst_guard: 3,
            ..Default::default()
        };
        let mut e = entry(StaticPromoteKind::Chest);
        // t=0 (0-0=0<=20 → 1), t=20 (丁度 20<=20 → 2), t=40 (→3)
        for t in [0u64, 20, 40] {
            p.tick(&mut e, t, true, false, 8.0, true);
        }
        assert_eq!(e.anim_burst_score, 3, "間隔 20 丁度も累積 (inclusive)");
        // score=3 >= guard=3 → 以降無期限動的 (t=100000 でも)
        assert_eq!(
            p.tick(&mut e, 100_000, false, false, 8.0, true),
            BeRenderMode::DynamicBlockEntity,
            "guard 到達は恒久的動的"
        );
        // リセット境界: 新 entry で t=0 open (score=1)、t=21 open (21>20 → 0 リセット)
        let mut r = entry(StaticPromoteKind::Chest);
        let p6 = BeStaticPolicy {
            burst_guard: 6,
            ..Default::default()
        };
        p6.tick(&mut r, 0, true, false, 8.0, true);
        assert_eq!(r.anim_burst_score, 1);
        p6.tick(&mut r, 21, true, false, 8.0, true);
        assert_eq!(r.anim_burst_score, 0, "間隔 21 でリセット");
        // 維持契約: opened=false の tick では score は減衰もリセットもされない
        let mut k = entry(StaticPromoteKind::Chest);
        k.anim_burst_score = 2;
        k.last_anim_tick = 9_000;
        p6.tick(&mut k, 10_000, false, false, 8.0, true);
        assert_eq!(
            k.anim_burst_score, 2,
            "非開閉 tick では score 維持 (時間減衰なし: 設計公表)"
        );
    }

    /// DU-1: 時計逆行は saturating_sub→0 で安全側 (動的) 化。
    /// NaN camera_distance は近距離降格不発 → 昇格側。誠実公表ピン。
    #[test]
    fn clock_regression_and_nan_distance_failsafes() {
        let p = BeStaticPolicy::default();
        let mut e = entry(StaticPromoteKind::Chest);
        e.last_anim_tick = 500; // 未来刻印
        assert_eq!(
            p.tick(&mut e, 100, false, false, 8.0, true),
            BeRenderMode::DynamicBlockEntity,
            "now<last_anim は saturating→0<40 で安全側 (動的)"
        );
        // NaN 距離: 全比較 false で近距離降格が発動せず昇格する
        let mut n = entry(StaticPromoteKind::Chest);
        assert_eq!(
            p.tick(&mut n, 1000, false, false, f32::NAN, true),
            BeRenderMode::StaticChunkMesh,
            "NaN 距離は昇格側 (SSR の NaN miss fail-safe とは逆: 公表)"
        );
    }

    /// DU-4: static_mesh_ready=false は現 mode を**保持**して返す。どちら向きも。
    #[test]
    fn mesh_not_ready_preserves_current_mode_both_ways() {
        let p = BeStaticPolicy::default();
        // Dynamic のまま維持 (昇格しない)
        let mut d = entry(StaticPromoteKind::Chest);
        assert_eq!(
            p.tick(&mut d, 1000, false, false, 8.0, false),
            BeRenderMode::DynamicBlockEntity,
            "ready=false では昇格しない"
        );
        // 一度 Static にしてから ready=false で呼んでも Static 維持
        // (メッシュ喪失による降格経路は存在しない設計のピン)
        let mut s = entry(StaticPromoteKind::Barrel);
        assert_eq!(
            p.tick(&mut s, 1000, false, false, 8.0, true),
            BeRenderMode::StaticChunkMesh
        );
        assert_eq!(
            p.tick(&mut s, 2000, false, false, 8.0, false),
            BeRenderMode::StaticChunkMesh,
            "ready=false で Static は維持される (降格経路なし)"
        );
    }

    /// DU-2: pos_pack 厳密レイアウト (x:63-38/z:37-12/y:11-0 排他) と
    /// 領域外折り畳み衝突の公表ピン。全値 rq du_pospack.rq 導出。
    #[test]
    fn pos_pack_exact_layout_and_fold_collisions() {
        // 厳密値 (rq 機械出力)
        assert_eq!(pos_pack(1, 64, 2), 274_877_915_200u64, "0x0000004000002040");
        assert_eq!(
            pos_pack(-1, 0, 0),
            0xFFFF_FFC0_0000_0000u64,
            "x 全ビット (u64=18446743798831644672)"
        );
        assert_eq!(pos_pack(0, -1, 0), 4095u64, "y 折り畳み 0xFFF");
        assert_eq!(pos_pack(7, 100, 9), 1_924_145_385_572u64, "領域混在代表");
        // 領域排他性: 各フィールドを取り出して元値に一致 (射影復元)
        let v = pos_pack(7, 100, 9);
        assert_eq!(((v >> 38) & 0x3FF_FFFF) as i32, 7, "x 領域");
        assert_eq!(((v >> 12) & 0x3FF_FFFF) as i32, 9, "z 領域");
        assert_eq!((v & 0xFFF) as i32, 100, "y 領域");
        // 領域内最大: x=2^25 は有効 (bit63) = 2^63
        assert_eq!(
            pos_pack(1 << 25, 0, 0),
            0x8000_0000_0000_0000u64,
            "x=2^25 は有効 (u64 2^63)"
        );
        // 折り畳み衝突 (公表): x=2^26 ≡ 0、y=±2048 ≡ 2048
        assert_eq!(pos_pack(1 << 26, 0, 0), pos_pack(0, 0, 0), "x 折り畳み衝突");
        assert_eq!(pos_pack(0, 2048, 0), 2048u64);
        assert_eq!(
            pos_pack(0, 2048, 0),
            pos_pack(0, -2048, 0),
            "y ±2048 折り畳み衝突"
        );
    }

    /// DU-6: closed_model_id 全表 + Other→chest フォールバック。
    #[test]
    fn closed_model_id_full_table_and_fallback() {
        assert_eq!(
            StaticPromoteKind::Chest.closed_model_id(),
            "minecraft:block/chest"
        );
        assert_eq!(
            StaticPromoteKind::TrappedChest.closed_model_id(),
            "minecraft:block/chest_trapped"
        );
        assert_eq!(
            StaticPromoteKind::EnderChest.closed_model_id(),
            "minecraft:block/ender_chest"
        );
        assert_eq!(
            StaticPromoteKind::Barrel.closed_model_id(),
            "minecraft:block/barrel"
        );
        assert_eq!(
            StaticPromoteKind::ShulkerBox.closed_model_id(),
            "minecraft:block/shulker_box"
        );
        // フォールバック公表: Other は chest を返す (汎用 BE モデルは存在しない)
        assert_eq!(
            StaticPromoteKind::Other.closed_model_id(),
            "minecraft:block/chest",
            "Other→chest フォールバック (近似、誠実公表)"
        );
    }

    /// DU-3: wiring 同型 soak — `full_graph_wiring.rs:1369-1387` と同一形状
    /// (take(4)・kind Chest 固定・opened/interacted=false・ready=true・
    /// dist 欠損 unwrap_or(0.0)・戻り値破棄)。決定は破棄されるが副作用の
    /// 状態機械は実進行し、長期静止で全エントリが静昇格する構造の実証。
    #[test]
    fn wiring_shape_soak_promotes_and_retains_side_effects() {
        use std::collections::HashMap;
        let p = BeStaticPolicy::default();
        let mut entries: HashMap<u64, BePromotionEntry> = HashMap::new();
        let chunk_keys = [(3i32, 5i32), (7, -2), (100, 40), (-8, 9)];
        let chunk_dists = [8.0f32]; // 2 件目以降は欠損 → unwrap_or(0.0)
        let mut tick_now = 1u64;
        for _ in 0..200 {
            for (i, k) in chunk_keys.iter().take(4).enumerate() {
                let id = pos_pack(k.0, i as i32, k.1);
                let entry = entries.entry(id).or_insert(BePromotionEntry {
                    kind: StaticPromoteKind::Chest,
                    last_anim_tick: 0,
                    last_interact_tick: 0,
                    mode: BeRenderMode::DynamicBlockEntity,
                    anim_burst_score: 0,
                });
                let dist = chunk_dists.get(i).copied().unwrap_or(0.0);
                let _mode = p.tick(entry, tick_now, false, false, dist, true); // 決定は破棄
            }
            tick_now += 1;
        }
        assert_eq!(entries.len(), 4, "4 区画キーは全て distinct");
        for (id, e) in &entries {
            assert_eq!(
                e.mode,
                BeRenderMode::StaticChunkMesh,
                "200 tick 無操作で全エントリ静昇格 (副作用は永続): id={id:#x}"
            );
        }
        // dist 欠損 (=0.0→近距離側) でも interact なしでは鮮度窓不発で昇格した
        let missing_dist_id = pos_pack(7, 1, -2);
        assert_eq!(
            entries[&missing_dist_id].mode,
            BeRenderMode::StaticChunkMesh,
            "dist 欠損=0.0 でも昇格 (安全側既定だが昇格は妨げない)"
        );
    }
}
