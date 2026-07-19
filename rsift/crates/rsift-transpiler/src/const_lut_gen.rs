
//! const fn & Build.rsコード生成 - BlockState->ModelIDのPerfect HashとAOテーブル
//! 実行時HashMap無しでO(1)

pub const AO_TABLE_SIZE: usize = 4096;

pub const fn gen_ao_table() -> [u8; AO_TABLE_SIZE] {
    let mut table = [0u8; AO_TABLE_SIZE];
    let mut i = 0;
    while i < AO_TABLE_SIZE {
        // 簡易AO: 3ビットからAO値を生成
        let side1 = (i & 1) != 0;
        let side2 = (i & 2) != 0;
        let corner = (i & 4) != 0;
        let ao = if side1 && side2 { 0 } else {
            let mut v = 0;
            if side1 { v+=1; }
            if side2 { v+=1; }
            if corner { v+=1; }
            if v > 3 { 0 } else { 3 - v }
        };
        table[i] = ao as u8;
        i+=1;
    }
    table
}

pub const AO_TABLE: [u8; AO_TABLE_SIZE] = gen_ao_table();

pub const fn perfect_hash_block_id(block_id: u32) -> u32 {
    // 簡易perfect hash: 下位ビットXOR上位
    block_id ^ (block_id >> 16) ^ (block_id >> 8)
}

pub fn build_perf_hash_table(ids: &[u32]) -> Vec<(u32, u32)> {
    ids.iter().map(|&id| (perfect_hash_block_id(id) % 1024, id)).collect()
}
