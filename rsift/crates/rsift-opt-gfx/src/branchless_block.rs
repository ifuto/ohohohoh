
//! Branchless Block Logic - if(block==water)をLUT + cmovに
//! 低スペCPUの分岐予測ミスペナルティ回避

pub struct BlockLut {
    pub opaque: [bool; 4096],
    pub transparent: [bool; 4096],
    pub light: [u8; 4096],
}

impl BlockLut {
    pub fn new() -> Self {
        let mut opaque = [false; 4096];
        let mut transparent = [false; 4096];
        let mut light = [0u8; 4096];
        for i in 0..4096 {
            opaque[i] = i % 3 != 0;
            transparent[i] = i % 5 == 0;
            light[i] = (i % 16) as u8;
        }
        Self { opaque, transparent, light }
    }

    #[inline(always)]
    pub fn is_opaque_branchless(&self, id: u16) -> bool {
        // 分岐無し: テーブル参照のみ
        self.opaque[id as usize & 4095]
    }

    #[inline(always)]
    pub fn light_branchless(&self, id: u16) -> u8 {
        self.light[id as usize & 4095]
    }

    #[inline(always)]
    pub fn select_branchless(cond: bool, a: u32, b: u32) -> u32 {
        // cmov相当: (cond as u32 * a) + (!cond as u32 * b)を算術で
        let m = cond as u32;
        (m * a) | ((1-m) * b)
    }
}
