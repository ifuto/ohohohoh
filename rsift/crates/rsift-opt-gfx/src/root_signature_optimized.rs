
//! Root Signature 1.1 + Static Samplers最適化
//! Rootコストを最小化: CameraはRootConstants、BindlessはDescriptorTable、SamplerはStatic。
//! 低スペGPUでRoot Signature変更コストをゼロに。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootParamType {
    Constants,
    DescriptorTable,
    RootDescriptor,
    StaticSampler,
}

#[derive(Debug, Clone)]
pub struct RootParameter {
    pub param_type: RootParamType,
    pub shader_visibility: u32,
    pub num_descriptors: u32,
    pub register: u32,
    pub space: u32,
}

#[derive(Debug, Clone)]
pub struct StaticSampler {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub shader_register: u32,
    pub register_space: u32,
}

#[derive(Debug, Clone)]
pub struct OptimizedRootSignature {
    pub params: Vec<RootParameter>,
    pub static_samplers: Vec<StaticSampler>,
    pub flags: u32,
}

impl OptimizedRootSignature {
    pub fn rs_graphics() -> Self {
        // Camera Matrix(16 floats=64 bytes)をRootConstants 16 DWORDに、Bindless IndexをDescriptorTable 1つに、Material CBVをRootDescriptorに
        Self {
            params: vec![
                RootParameter { param_type: RootParamType::Constants, shader_visibility: 0, num_descriptors: 16, register: 0, space: 0 }, // b0: Camera
                RootParameter { param_type: RootParamType::DescriptorTable, shader_visibility: 0, num_descriptors: 1024, register: 0, space: 0 }, // t0: bindless textures
                RootParameter { param_type: RootParamType::RootDescriptor, shader_visibility: 0, num_descriptors: 1, register: 1, space: 0 }, // b1: Material SSBO
            ],
            static_samplers: vec![
                StaticSampler { filter: 0, address_u: 1, address_v: 1, address_w: 1, shader_register: 0, register_space: 0 }, // Linear Wrap
                StaticSampler { filter: 1, address_u: 1, address_v: 1, address_w: 1, shader_register: 1, register_space: 0 }, // Point
            ],
            // 【wave 168 FN 捕捉 104】MS Learn 一次情報: 0x1 は ALLOW_INPUT_ASSEMBLER_
            // INPUT_LAYOUT (IA opt-in) で DENY 系ではない。設計意図 DENY_HS|DS|GS
            // は 0x4|0x8|0x10 = 0x1C。vertex pull 主パイプラインで IA 非使用の
            // ため ALLOW_IA は opt-in しない。
            flags: 0x1C, // DENY_HULL(0x4)|DENY_DOMAIN(0x8)|DENY_GEOMETRY(0x10)
        }
    }

    pub fn root_cost(&self) -> u32 {
        // Root Signatureコスト計算: ConstantsはDWORD数、DescriptorTableは1、RootDescriptorは2
        self.params.iter().map(|p| match p.param_type {
            RootParamType::Constants => p.num_descriptors,
            RootParamType::DescriptorTable => 1,
            RootParamType::RootDescriptor => 2,
            RootParamType::StaticSampler => 0,
        }).sum()
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn graphics_layout_fields_bit_exact() {
        let rs = OptimizedRootSignature::rs_graphics();
        assert_eq!(rs.params.len(), 3);
        let p0 = &rs.params[0];
        assert_eq!(p0.param_type, RootParamType::Constants, "b0 Camera は RootConstants");
        assert_eq!((p0.shader_visibility, p0.num_descriptors, p0.register, p0.space), (0, 16, 0, 0),
            "Camera 行列 16 floats = 16 DWORD");
        let p1 = &rs.params[1];
        assert_eq!(p1.param_type, RootParamType::DescriptorTable, "t0 bindless は Table");
        assert_eq!((p1.shader_visibility, p1.num_descriptors, p1.register, p1.space), (0, 1024, 0, 0));
        let p2 = &rs.params[2];
        assert_eq!(p2.param_type, RootParamType::RootDescriptor, "b1 Material は RootDescriptor");
        assert_eq!((p2.shader_visibility, p2.num_descriptors, p2.register, p2.space), (0, 1, 1, 0));

        assert_eq!(rs.static_samplers.len(), 2);
        let s0 = &rs.static_samplers[0];
        assert_eq!(
            (s0.filter, s0.address_u, s0.address_v, s0.address_w, s0.shader_register, s0.register_space),
            (0, 1, 1, 1, 0, 0),
            "s0 = Linear Wrap"
        );
        let s1 = &rs.static_samplers[1];
        assert_eq!(
            (s1.filter, s1.address_u, s1.address_v, s1.address_w, s1.shader_register, s1.register_space),
            (1, 1, 1, 1, 1, 0),
            "s1 = Point Wrap"
        );
        assert_eq!(rs.flags, 0x1C, "DENY_HS|DS|GS = 0x1C (一次情報真値)");
    }

    #[test]
    fn root_cost_weighting_table() {
        let mk = |param_type: RootParamType, num: u32| RootParameter {
            param_type, shader_visibility: 0, num_descriptors: num, register: 0, space: 0,
        };
        let rs = OptimizedRootSignature {
            params: vec![
                mk(RootParamType::Constants, 5),
                mk(RootParamType::DescriptorTable, 999), // 個数に依らず 1
                mk(RootParamType::RootDescriptor, 7),    // 個数に依らず 2
                mk(RootParamType::StaticSampler, 100),   // コスト 0
            ],
            static_samplers: Vec::new(),
            flags: 0,
        };
        assert_eq!(rs.root_cost(), 5 + 1 + 2 + 0, "Constants=DWORD数, Table=1, RootDesc=2, Sampler=0");
    }

    /// 【wave 168 FN 捕捉 104】flags の真値 pin (一次情報 = MS Learn
    /// D3D12_ROOT_SIGNATURE_FLAGS): 0x1 = ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
    /// (vertex pull 主パイプラインで IA 非使用のため opt-in しない)、
    /// DENY_HS=0x4 / DENY_DS=0x8 / DENY_GS=0x10 → 設計意図 DENY_HS|DS|GS = 0x1C。
    /// 現行 0x1 + 「DENY 系」コメント/旧 golden は一次情報と正矛盾。
    #[test]
    fn fn_flags_truth_from_first_source() {
        let rs = OptimizedRootSignature::rs_graphics();
        assert_eq!(
            rs.flags, 0x1C,
            "DENY_HULL(0x4)|DENY_DOMAIN(0x8)|DENY_GEOMETRY(0x10) = 0x1C"
        );
        assert_eq!(
            rs.flags & 0x1,
            0,
            "ALLOW_IA(0x1) は opt-in しない (vertex pull)"
        );
        assert_eq!(rs.flags & !0x1C, 0, "既知外のフラグは立たない");
    }

    #[test]
    fn graphics_root_cost_within_d3d12_budget() {
        // D3D12 の root signature 上限は 64 DWORD — 超過は API レベルの生成失敗を
        // 意味するため不変式として固定する。
        let cost = OptimizedRootSignature::rs_graphics().root_cost();
        assert_eq!(cost, 19, "16 (Camera) + 1 (bindless Table) + 2 (Material)");
        assert!(cost <= 64, "D3D12 64 DWORD 制限内であること");
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 17 例目 (wave 168 FN adversarial (d)
    /// 死救出 非検出) の回収。変異スクリプト一次資料 (当時 /tmp) 消失で対象語彙が一意
    /// 確定できないため (誠実記録)、代置として構造網羅 pin を立てる: 現存公開構造
    /// (RootParamType/RootParameter/StaticSampler/OptimizedRootSignature/rs_graphics/
    /// root_cost) の存在 pin + pub fn/struct/enum 宣言総数 pin で新規公開構造 (当時の
    /// 死救出型変更の再来) を将来検出する。
    #[test]
    fn ge_structure_census_pin() {
        let src = include_str!("root_signature_optimized.rs");
        // 現存公開構造の存在 pin (全消失は trajectory 異常として検出)
        assert!(
            src.contains(concat!("pub e", "num RootParamType")),
            "現存公開構造の識別子消失を検出"
        );
        assert!(
            src.contains(concat!("pub str", "uct RootParameter")),
            "現存公開構造の識別子消失を検出"
        );
        assert!(
            src.contains(concat!("pub str", "uct StaticSampler")),
            "現存公開構造の識別子消失を検出"
        );
        assert!(
            src.contains(concat!("pub str", "uct OptimizedRootSignature")),
            "現存公開構造の識別子消失を検出"
        );
        assert!(
            src.contains(concat!("fn rs_", "graphics")),
            "現存公開構造の識別子消失を検出"
        );
        assert!(
            src.contains(concat!("fn root_", "cost")),
            "現存公開構造の識別子消失を検出"
        );
        assert_eq!(
            src.matches(concat!("pub f", "n ")).count(),
            2,
            "pub メソッド宣言 の宣言総数 pin: 新規公開構造 (dead code 復活候補) の追加を検出"
        );
        assert_eq!(
            src.matches(concat!("pub str", "uct ")).count(),
            3,
            "公開 struct 宣言 の宣言総数 pin: 新規公開構造 (dead code 復活候補) の追加を検出"
        );
        assert_eq!(
            src.matches(concat!("pub en", "um ")).count(),
            1,
            "公開 enum 宣言 の宣言総数 pin: 新規公開構造 (dead code 復活候補) の追加を検出"
        );
    }
}
