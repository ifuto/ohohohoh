
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
            flags: 0x1, // DENY_HS|DS|GS等最適化
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
        assert_eq!(rs.flags, 0x1, "DENY 系フラグ 0x1 固定");
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

    #[test]
    fn graphics_root_cost_within_d3d12_budget() {
        // D3D12 の root signature 上限は 64 DWORD — 超過は API レベルの生成失敗を
        // 意味するため不変式として固定する。
        let cost = OptimizedRootSignature::rs_graphics().root_cost();
        assert_eq!(cost, 19, "16 (Camera) + 1 (bindless Table) + 2 (Material)");
        assert!(cost <= 64, "D3D12 64 DWORD 制限内であること");
    }
}
