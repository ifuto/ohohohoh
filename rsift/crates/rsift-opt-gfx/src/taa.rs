//! Lightweight temporal AA with neighborhood clamp (Tier 6).

#[derive(Debug, Clone)]
pub struct LightweightTaa {
    pub blend: f32,
    pub enabled: bool,
}

impl Default for LightweightTaa {
    fn default() -> Self {
        Self {
            blend: 0.1,
            enabled: true,
        }
    }
}

impl LightweightTaa {
    pub fn for_low_spec() -> Self {
        Self {
            blend: 0.15,
            enabled: true,
        }
    }

    /// Reproject UV: uv_hist = uv - velocity (vel は current→history 方向、
    /// taa.wgsl `hist_uv = uv - vel` と一致。旧 doc の「uv + velocity」表記は
    /// 符号が逆の偽だった — 2026-07-22 wave 23 監査で訂正)。
    /// 返り値はヒストリ側サンプル座標 (now - displacement)。
    #[inline]
    pub fn reproject_uv(uv: [f32; 2], velocity: [f32; 2]) -> [f32; 2] {
        [uv[0] - velocity[0], uv[1] - velocity[1]]
    }

    /// Neighborhood clamp — keep history inside min/max of 3×3 neighborhood (reduces ghosting).
    /// 契約 (mirror): nbr_min/max は **current フレーム中心 (uv 自身を含む) 3×3 近傍
    /// 9 サンプル**の min/max — taa.wgsl fs_main のループ規則と同一。呼び出し側が
    /// 別範囲 (history 側等) を与えると GPU/CPU 相互検証が壊れる。
    pub fn neighborhood_clamp(history_rgb: [f32; 3], nbr_min: [f32; 3], nbr_max: [f32; 3]) -> [f32; 3] {
        [
            history_rgb[0].clamp(nbr_min[0], nbr_max[0]),
            history_rgb[1].clamp(nbr_min[1], nbr_max[1]),
            history_rgb[2].clamp(nbr_min[2], nbr_max[2]),
        ]
    }

    pub fn resolve(
        &self,
        current: [f32; 3],
        history: [f32; 3],
        nbr_min: [f32; 3],
        nbr_max: [f32; 3],
    ) -> [f32; 3] {
        if !self.enabled {
            return current;
        }
        let h = Self::neighborhood_clamp(history, nbr_min, nbr_max);
        // 契約: 非有限 blend (NaN/±inf。NaN は f32::clamp を素通りする) は
        // a=0 (current 素通し) に正規化する。設定ミスの NaN を画面出力へ
        // 静かに伝搬させないための境界 (2026-07-22 wave 23 監査で追加)。
        // なお taa.wgsl 側の mix() は blend clamp 無しなので、params を
        // 供給するホストはこの規則で事前正規化すること (両側同一契約)。
        let a = if self.blend.is_finite() {
            self.blend.clamp(0.0, 1.0)
        } else {
            0.0
        };
        [
            current[0] * (1.0 - a) + h[0] * a,
            current[1] * (1.0 - a) + h[1] * a,
            current[2] * (1.0 - a) + h[2] * a,
        ]
    }

    pub fn wgsl_source(&self) -> &'static str {
        TAA_WGSL
    }
}

pub const TAA_WGSL: &str = include_str!("../shaders/taa.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_kills_ghost() {
        let taa = LightweightTaa::default();
        let out = taa.resolve(
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.4, 0.4, 0.4],
            [0.6, 0.6, 0.6],
        );
        assert!(out[0] <= 0.6 + 1e-5);
    }

    /// wave 23-1: resolve が WGSL `mix(cur, clamp(hist, mn, mx), blend)` と
    /// 同一演算順のときの厳密ビット値 (f32 単一回丸め規則から exact rational で
    /// 厳密導出 — float64 近似エミュレーション禁止、W-3 教訓)。
    #[test]
    fn resolve_exact_bits_matching_wgsl_mix() {
        let taa = LightweightTaa::default(); // blend 0.1
        let out = taa.resolve(
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0], // 3x3 clamp で [0.6, 0.4, 0.4] へ
            [0.4, 0.4, 0.4],
            [0.6, 0.6, 0.6],
        );
        assert_eq!(out[0].to_bits(), 0x3f028f5c, "r: 0.5*0.9 + 0.6*0.1");
        assert_eq!(out[1].to_bits(), 0x3efae147, "g: 0.5*0.9 + 0.4*0.1");
        assert_eq!(out[2].to_bits(), 0x3efae147, "b: 0.5*0.9 + 0.4*0.1");
        // blend=1.0 は純ヒストリ (クランプ後)、bit 厳密
        let taa_h = LightweightTaa {
            blend: 1.0,
            enabled: true,
        };
        let out = taa_h.resolve(
            [0.2, 0.2, 0.2],
            [0.9, 0.1, 0.5],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        );
        let bits = [out[0].to_bits(), out[1].to_bits(), out[2].to_bits()];
        assert_eq!(
            bits,
            [0.9f32.to_bits(), 0.1f32.to_bits(), 0.5f32.to_bits()],
            "blend=1 must be pure clamped history"
        );
    }

    /// wave 23-2: 非有限 blend は a=0 (current 素通し、bit 厳密) に正規化 —
    /// NaN を画面に流さない契約の機械ピン。
    #[test]
    fn non_finite_blend_normalizes_to_passthrough() {
        let cur = [0.25f32, 0.5, 0.75];
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let taa = LightweightTaa {
                blend: bad,
                enabled: true,
            };
            let out = taa.resolve(cur, [0.9, 0.9, 0.9], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
            for c in 0..3 {
                assert_eq!(
                    out[c].to_bits(),
                    cur[c].to_bits(),
                    "blend={bad} must act as 0.0 (passthrough)"
                );
            }
        }
    }

    /// wave 23-3: taa.wgsl の実妥当性 (naga パース + 2 エントリ) と
    /// ミラー契約トークン (uv - vel 添字・3x3 ループ・mix) の存在検査。
    #[test]
    fn taa_wgsl_parses_and_mirror_tokens_present() {
        let module = naga::front::wgsl::parse_str(TAA_WGSL).expect("taa.wgsl must parse");
        for ep in ["vs_main", "fs_main"] {
            assert!(
                module.entry_points.iter().any(|f| f.name == ep),
                "taa.wgsl missing entry point {ep}"
            );
        }
        assert!(
            TAA_WGSL.contains("uv - vel"),
            "reprojection must be uv - vel"
        );
        assert!(TAA_WGSL.contains("mix("), "resolve must be mix()");
        for tok in ["y = -1; y <= 1", "x = -1; x <= 1"] {
            assert!(
                TAA_WGSL.contains(tok),
                "3x3 neighborhood loop missing: {tok}"
            );
        }
    }
}
