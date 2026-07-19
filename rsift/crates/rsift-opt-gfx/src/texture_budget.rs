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
