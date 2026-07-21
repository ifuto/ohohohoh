
//! PGO + BOLT + LTO設定 - Hypixelワークロードでプロファイル取得
//! ビルド時の最適化フラグを提供

#[derive(Debug, Clone)]
pub struct PgoConfig {
    pub lto: String,
    pub codegen_units: u32,
    pub target_cpu: String,
    pub pgo_instrument: bool,
    pub bolt: bool,
}

impl Default for PgoConfig {
    fn default() -> Self {
        Self {
            lto: "thin".into(),
            codegen_units: 1,
            target_cpu: "x86-64-v3".into(),
            pgo_instrument: false,
            bolt: false,
        }
    }
}

impl PgoConfig {
    pub fn release() -> Self {
        Self { lto: "fat".into(), codegen_units: 1, target_cpu: "x86-64-v3".into(), pgo_instrument: false, bolt: true }
    }

    pub fn dev() -> Self {
        Self { lto: "thin".into(), codegen_units: 16, target_cpu: "native".into(), pgo_instrument: false, bolt: false }
    }

    pub fn rustflags(&self) -> String {
        format!("-C lto={} -C codegen-units={} -C target-cpu={}", self.lto, self.codegen_units, self.target_cpu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_field_tables() {
        let rel = PgoConfig::release();
        assert_eq!(rel.lto, "fat");
        assert_eq!(rel.codegen_units, 1);
        assert_eq!(rel.target_cpu, "x86-64-v3");
        assert!(rel.bolt && !rel.pgo_instrument);
        let dev = PgoConfig::dev();
        assert_eq!(dev.lto, "thin");
        assert_eq!(dev.codegen_units, 16);
        assert_eq!(dev.target_cpu, "native");
        assert!(!dev.bolt);
        let def = PgoConfig::default();
        assert_eq!((def.lto.as_str(), def.codegen_units), ("thin", 1));
        assert!(!def.bolt && !def.pgo_instrument);
    }

    #[test]
    fn rustflags_string_exact() {
        assert_eq!(
            PgoConfig::release().rustflags(),
            "-C lto=fat -C codegen-units=1 -C target-cpu=x86-64-v3"
        );
        assert_eq!(
            PgoConfig::dev().rustflags(),
            "-C lto=thin -C codegen-units=16 -C target-cpu=native"
        );
    }
}
