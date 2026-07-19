
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
