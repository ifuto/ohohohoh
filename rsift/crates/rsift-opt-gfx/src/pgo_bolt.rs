
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
