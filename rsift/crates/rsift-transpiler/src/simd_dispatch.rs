
//! Target-CPU Dispatch: x86-64-v3 Baseline + v4 Runtime Dispatch
//! 起動時にis_x86_feature_detectedで切替、multiversionクレート相当

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuLevel { X86_64_V3, X86_64_V4, Avx2, Avx512 }

pub fn detect_cpu_level() -> CpuLevel {
    #[cfg(target_arch="x86_64")]
    {
        if std::is_x86_feature_detected!("avx512f") { return CpuLevel::X86_64_V4; }
        if std::is_x86_feature_detected!("avx2") { return CpuLevel::Avx2; }
    }
    CpuLevel::X86_64_V3
}

pub fn dispatch_greedy<F, R>(v3: F, v4: F) -> R
where F: Fn() -> R {
    match detect_cpu_level() {
        CpuLevel::X86_64_V4 | CpuLevel::Avx512 => v4(),
        _ => v3(),
    }
}

/// Greedy MeshingのAVX2ビットマスク版と通常版の切替
pub fn greedy_dispatch(palette: &[u16; 4096]) -> Vec<(u32,u32)> {
    match detect_cpu_level() {
        CpuLevel::Avx2 | CpuLevel::X86_64_V4 | CpuLevel::Avx512 => {
            // AVX2パス: ビットボード判定
            greedy_avx2(palette)
        },
        _ => {
            greedy_scalar(palette)
        }
    }
}

fn greedy_avx2(palette: &[u16; 4096]) -> Vec<(u32,u32)> {
    // 簡易: 空でないレイヤーをビットマスクで検出
    let mut masks = [0u16; 16];
    for z in 0..16 {
        let mut m = 0u16;
        for x in 0..16 {
            for y in 0..16 {
                if palette[x + y*16 + z*256] != 0 { m |= 1<<x; break; }
            }
        }
        masks[z] = m;
    }
    masks.iter().enumerate().filter(|(_, &mm)| mm!=0).map(|(z,_)| (z as u32, 0)).collect()
}

fn greedy_scalar(palette: &[u16; 4096]) -> Vec<(u32,u32)> {
    palette.iter().enumerate().filter(|(_, &b)| b!=0).map(|(i,_)| (i as u32 / 256, i as u32 % 16)).collect()
}
