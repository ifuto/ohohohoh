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
    pub fn from_profile(feather_enabled: bool, compressed: bool, mipmap_bias: f32) -> Self {
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

    pub fn bandwidth_factor(&self) -> f32 {
        match self.compression {
            AtlasCompression::Uncompressed => 1.0,
            AtlasCompression::Bc7 => 0.25,
            AtlasCompression::Astc4x4 => 0.2,
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
        assert_eq!(t.bandwidth_factor(), 0.2);
        assert_eq!(t.label(), "ASTC 4×4 atlas");
    }
}
