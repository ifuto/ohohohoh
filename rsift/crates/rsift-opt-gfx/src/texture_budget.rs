//! Texture bandwidth budget — compressed atlas + mipmap bias (Feather weak-PC).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasCompression {
    Uncompressed,
    /// Desktop + modern iGPU.
    Bc7,
    /// Mobile / Apple Silicon preferred.
    Astc4x4,
}

pub struct TextureBudget {
    pub compression: AtlasCompression,
    pub mipmap_bias: f32,
    pub use_mipmaps: bool,
}

impl TextureBudget {
    /// **契約 (2026-07-23 wave 50 厳格化)**: `mipmap_bias` は有限値必須
    /// (NaN/±inf を格納するとサンプラ LOD bias に NaN が流れ下流の挙動が
    /// 不定になる。非 feather 経路では bias 自体を破棄するが、入力契約は
    /// 統一して検証する)。
    pub fn from_profile(feather_enabled: bool, compressed: bool, mipmap_bias: f32) -> Self {
        assert!(
            mipmap_bias.is_finite(),
            "TextureBudget::from_profile 契約違反: mipmap_bias が非有限 ({mipmap_bias})"
        );
        if !feather_enabled {
            return Self {
                compression: AtlasCompression::Uncompressed,
                mipmap_bias: 0.0,
                use_mipmaps: true,
            };
        }
        let compression = if compressed {
            #[cfg(target_os = "macos")]
            {
                AtlasCompression::Astc4x4
            }
            #[cfg(not(target_os = "macos"))]
            {
                AtlasCompression::Bc7
            }
        } else {
            AtlasCompression::Uncompressed
        };
        Self {
            compression,
            mipmap_bias,
            use_mipmaps: true,
        }
    }

    /// RGBA8 (= 32 bpp) との帯域比。導出 (bpp は全形式で厳密):
    /// RGBA8 = 32 bpp、BC7 = 128 bit / 16 texel = 8 bpp → 8/32 = **0.25**、
    /// ASTC 4×4 = 128 bit / 16 texel = 8 bpp → **0.25** (ASTC は全 block
    /// サイズで 128 bit 固定、4×4 = 16 texel)。
    /// **2026-07-23 wave 50 訂正**: Astc4x4 は旧実装で 0.2 だったが、
    /// 6.4 bpp は ASTC **5×4** (128/20 bit/texel) の値でラベル「4×4」と
    /// 矛盾していた。帯域モデルは bpp に厳密一致させる。
    pub fn bandwidth_factor(&self) -> f32 {
        match self.compression {
            AtlasCompression::Uncompressed => 1.0,
            AtlasCompression::Bc7 => 0.25,
            AtlasCompression::Astc4x4 => 0.25,
        }
    }

    pub fn label(&self) -> &'static str {
        match self.compression {
            AtlasCompression::Uncompressed => "RGBA8 atlas",
            AtlasCompression::Bc7 => "BC7 compressed atlas",
            AtlasCompression::Astc4x4 => "ASTC 4×4 atlas",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_feather_is_passthrough_uncompressed() {
        let t = TextureBudget::from_profile(false, true, 2.0);
        assert_eq!(t.compression, AtlasCompression::Uncompressed);
        assert_eq!(t.mipmap_bias, 0.0); // bias 指定は破棄される
        assert!(t.use_mipmaps);
    }

    #[test]
    fn feather_compressed_uses_platform_format() {
        let b = TextureBudget::from_profile(true, true, 1.5);
        let want = if cfg!(target_os = "macos") {
            AtlasCompression::Astc4x4
        } else {
            AtlasCompression::Bc7
        };
        assert_eq!(b.compression, want);
        assert_eq!(b.mipmap_bias, 1.5);
        assert!(b.use_mipmaps);
    }

    #[test]
    fn feather_uncompressed_keeps_bias() {
        let b = TextureBudget::from_profile(true, false, 1.5);
        assert_eq!(b.compression, AtlasCompression::Uncompressed);
        assert_eq!(b.mipmap_bias, 1.5);
    }

    #[test]
    fn bandwidth_factor_and_labels_exact() {
        assert_eq!(TextureBudget::from_profile(false, false, 0.0).bandwidth_factor(), 1.0);
        let mut t = TextureBudget::from_profile(false, false, 0.0);
        assert_eq!(t.label(), "RGBA8 atlas");
        t.compression = AtlasCompression::Bc7;
        assert_eq!(t.bandwidth_factor(), 0.25);
        assert_eq!(t.label(), "BC7 compressed atlas");
        t.compression = AtlasCompression::Astc4x4;
        // wave 50 訂正: ASTC 4×4 は 128bit/16texel = 8bpp → BC7 と同じ 0.25
        // (旧 0.2 は 6.4bpp = ASTC 5×4 の値でラベルと矛盾していた)
        assert_eq!(t.bandwidth_factor(), 0.25);
        assert_eq!(t.label(), "ASTC 4×4 atlas");
    }

    /// wave 50-1: 帯域モデルは bpp に厳密一致 (factor × 32bpp = 形式 bpp)。
    #[test]
    fn bandwidth_factor_matches_exact_bpp() {
        let mut t = TextureBudget::from_profile(false, false, 0.0);
        assert_eq!(t.bandwidth_factor() * 32.0, 32.0, "RGBA8 = 32 bpp");
        t.compression = AtlasCompression::Bc7;
        assert_eq!(t.bandwidth_factor() * 32.0, 8.0, "BC7 = 8 bpp");
        t.compression = AtlasCompression::Astc4x4;
        assert_eq!(t.bandwidth_factor() * 32.0, 8.0, "ASTC 4×4 = 8 bpp");
    }

    /// wave 50-2: mipmap_bias 非有限は fail-loud (NaN LOD bias の下流不定を遮断)。
    #[test]
    #[should_panic(expected = "mipmap_bias が非有限")]
    fn from_profile_rejects_nan_bias() {
        let _ = TextureBudget::from_profile(true, true, f32::NAN);
    }

    #[test]
    #[should_panic(expected = "mipmap_bias が非有限")]
    fn from_profile_rejects_inf_bias() {
        let _ = TextureBudget::from_profile(true, false, f32::INFINITY);
    }

    #[test]
    #[should_panic(expected = "mipmap_bias が非有限")]
    fn from_profile_rejects_nan_bias_even_when_discarded() {
        // 非 feather でも契約は統一 (bias は破棄されるが入力は検証する)
        let _ = TextureBudget::from_profile(false, false, f32::NAN);
    }
}
