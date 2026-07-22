//! SoA entity layout + XZY indexing + 64-byte alignment (Tier 3).

/// SoA エンティティ列。
///
/// **契約 (2026-07-22 wave 33 明文化)**: 7 配列は全て等長必須。
/// `push` / `from_aos` 経由なら自動的に満たされるが、pub フィールドの
/// 手組みで不等長にすると `integrate` は範囲外 index panic、
/// `to_aos` は `get` の None で**静寂に打ち切られた短い出力**を返す。
/// 両入口で `assert_uniform_len` が fail-loud に契約強制する。
#[derive(Debug, Default, Clone)]
pub struct EntitySoa {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub z: Vec<f32>,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub vz: Vec<f32>,
    pub flags: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityAos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub flags: u32,
}

impl EntitySoa {
    /// 7 配列の等長契約を検査 (fail-loud)。
    #[inline]
    #[track_caller]
    fn assert_uniform_len(&self) {
        let n = self.x.len();
        assert!(
            self.y.len() == n
                && self.z.len() == n
                && self.vx.len() == n
                && self.vy.len() == n
                && self.vz.len() == n
                && self.flags.len() == n,
            "EntitySoa 契約違反: 7 配列は等長必須 (x={}, y={}, z={}, vx={}, vy={}, vz={}, flags={})",
            self.x.len(),
            self.y.len(),
            self.z.len(),
            self.vx.len(),
            self.vy.len(),
            self.vz.len(),
            self.flags.len()
        );
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            x: Vec::with_capacity(n),
            y: Vec::with_capacity(n),
            z: Vec::with_capacity(n),
            vx: Vec::with_capacity(n),
            vy: Vec::with_capacity(n),
            vz: Vec::with_capacity(n),
            flags: Vec::with_capacity(n),
        }
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn push(&mut self, e: EntityAos) {
        self.x.push(e.x);
        self.y.push(e.y);
        self.z.push(e.z);
        self.vx.push(e.vx);
        self.vy.push(e.vy);
        self.vz.push(e.vz);
        self.flags.push(e.flags);
    }

    pub fn get(&self, i: usize) -> Option<EntityAos> {
        Some(EntityAos {
            x: *self.x.get(i)?,
            y: *self.y.get(i)?,
            z: *self.z.get(i)?,
            vx: *self.vx.get(i)?,
            vy: *self.vy.get(i)?,
            vz: *self.vz.get(i)?,
            flags: *self.flags.get(i)?,
        })
    }

    pub fn integrate(&mut self, dt: f32) {
        self.assert_uniform_len();
        for i in 0..self.len() {
            self.x[i] += self.vx[i] * dt;
            self.y[i] += self.vy[i] * dt;
            self.z[i] += self.vz[i] * dt;
        }
    }

    pub fn from_aos(list: &[EntityAos]) -> Self {
        let mut s = Self::with_capacity(list.len());
        for e in list {
            s.push(*e);
        }
        s
    }

    pub fn to_aos(&self) -> Vec<EntityAos> {
        self.assert_uniform_len();
        (0..self.len()).filter_map(|i| self.get(i)).collect()
    }
}

/// XZY linear index — Y contiguous for column scans (Minecraft section friendly).
///
/// **契約 (2026-07-22 wave 33 明文化)**: `x, z < sx` かつ `y < sy` 必須
/// (xz 平面は sz == sx の正方形断面前提)。`z >= sx` 等の範囲外指定は
/// 異なるセルを同一 index に**静寂衝突**させるため fail-loud で拒否する
/// (旧実装は黙って aliasing し、例: (1,0,0) と (0,0,16) が同一 index)。
/// `xzy_decode` は有効 index を一貫して復元する (相互逆写像)。
#[inline]
pub fn xzy_index(x: u32, y: u32, z: u32, sx: u32, sy: u32) -> usize {
    assert!(
        x < sx && z < sx && y < sy,
        "xzy_index 契約違反: (x,z) < sx かつ y < sy 必須 (x={x}, y={y}, z={z}, sx={sx}, sy={sy})"
    );
    ((x * sx + z) * sy + y) as usize
}

#[inline]
pub fn xzy_decode(index: usize, sx: u32, sy: u32) -> (u32, u32, u32) {
    let sy = sy as usize;
    let sx = sx as usize;
    let y = index % sy;
    let xz = index / sy;
    let z = xz % sx;
    let x = xz / sx;
    (x as u32, y as u32, z as u32)
}

/// 64B (cache line) アライン保証のバイトバッファ (std::alloc + 自前 Drop)。
///
/// 旧 `alloc_aligned_64` (2026-07-22 wave 33 で置換) は drain による頭出しで
/// 調整を試みたが、**Vec の先頭ポインタは drain しても不変**のため一度も
/// アライン補正を達成していなかった (事後検査が必ず非整列を検出して
/// fallback に落ち、fallback の `vec![0u8; len]` も 64B 非保証)。
/// aligned SIMD load 前提の呼び出し側は実機で crash し得た。
/// Layout align=64 の専用アロケーションに根治。
#[derive(Debug)]
pub struct Aligned64 {
    ptr: std::ptr::NonNull<u8>,
    len: usize,
}

// SAFETY: 排他所有の他者非共有アロケーションであり、公開面は &[u8] /
// &mut [u8] の通常借用規則のみのため、Vec<u8> と同等に送受・共有して安全。
unsafe impl Send for Aligned64 {}
unsafe impl Sync for Aligned64 {}

impl Aligned64 {
    fn layout_of(len: usize) -> std::alloc::Layout {
        // len == 0 でも 1B 確保する (Layout のサイズ 0 は alloc 系で不定のため)。
        std::alloc::Layout::from_size_align(len.max(1), 64).expect("Aligned64: size/align overflow")
    }

    /// `len` バイトをゼロ初期化で確保 (先頭は必ず 64 の倍数)。
    pub fn zeroed(len: usize) -> Self {
        let layout = Self::layout_of(len);
        // SAFETY: layout は非零 size・align 64 で構築済み。
        // null は handle_alloc_error で絶対に進まない。
        let raw = unsafe { std::alloc::alloc_zeroed(layout) };
        let ptr = match std::ptr::NonNull::new(raw) {
            Some(p) => p,
            None => std::alloc::handle_alloc_error(layout),
        };
        Self { ptr, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr は len.max(1)B の生存アロケーションで、self と
        // 同寿命。共有参照のため書き込み競合なし。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: 排他借用でエイリアスなし。領域有効性は上記同様。
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl std::ops::Deref for Aligned64 {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::ops::DerefMut for Aligned64 {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl Drop for Aligned64 {
    fn drop(&mut self) {
        let layout = Self::layout_of(self.len);
        // SAFETY: 構築時と同一 layout で割り当てた領域を解放。
        unsafe { std::alloc::dealloc(self.ptr.as_ptr(), layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soa_xzy() {
        let mut s = EntitySoa::with_capacity(2);
        s.push(EntityAos {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            vx: 0.5,
            vy: 0.0,
            vz: 0.0,
            flags: 1,
        });
        s.integrate(2.0);
        assert!((s.x[0] - 2.0).abs() < 1e-5);
        let i = xzy_index(1, 2, 3, 16, 16);
        let (x, y, z) = xzy_decode(i, 16, 16);
        assert_eq!((x, y, z), (1, 2, 3));
    }

    /// wave 33-1: xzy_index/decode の厳密値 table (相互逆写像、一意性)。
    #[test]
    fn xzy_index_decode_exact_table() {
        assert_eq!(xzy_index(0, 0, 0, 16, 16), 0);
        assert_eq!(xzy_index(1, 2, 3, 16, 16), (1 * 16 + 3) * 16 + 2);
        assert_eq!(xzy_index(15, 15, 15, 16, 16), 4095);
        // y 内側連続 (column scan): (x,z) 固定で y++ が index++ に一致
        assert_eq!(xzy_index(2, 5, 9, 16, 16) + 1, xzy_index(2, 6, 9, 16, 16));
        // 逆写像: 全域代表点で decode∘index == id
        for (x, y, z) in [(0, 0, 0), (1, 2, 3), (15, 0, 15), (7, 15, 4), (15, 15, 15)] {
            assert_eq!(xzy_decode(xzy_index(x, y, z, 16, 16), 16, 16), (x, y, z));
        }
    }

    /// wave 33-2: xzy_index 範囲外は aliasing せず fail-loud (3 軸)。
    #[test]
    #[should_panic(expected = "xzy_index 契約違反")]
    fn xzy_index_rejects_x_out_of_range() {
        let _ = xzy_index(16, 0, 0, 16, 16);
    }

    #[test]
    #[should_panic(expected = "xzy_index 契約違反")]
    fn xzy_index_rejects_y_out_of_range() {
        let _ = xzy_index(0, 16, 0, 16, 16);
    }

    #[test]
    #[should_panic(expected = "xzy_index 契約違反")]
    fn xzy_index_rejects_z_out_of_range() {
        // 旧実装は (0,0,16) が (1,0,0) に静寂衝突していた
        let _ = xzy_index(0, 0, 16, 16, 16);
    }

    /// wave 33-3: Aligned64 は任意長で先頭 64 の倍数 + ゼロ初期化 + RW 疎通。
    #[test]
    fn aligned64_actually_aligned_zeroed_and_writable() {
        for len in [0usize, 1, 63, 64, 65, 4096] {
            let mut b = Aligned64::zeroed(len);
            assert_eq!(b.len(), len);
            assert_eq!(b.is_empty(), len == 0);
            let ptr = b.as_slice().as_ptr() as usize;
            assert_eq!(ptr % 64, 0, "len={len}: 先頭が 64B アラインでない");
            assert!(b.as_slice().iter().all(|&v| v == 0));
            for v in b.as_mut_slice().iter_mut() {
                *v = 0xAB;
            }
            assert!(b.iter().all(|&v| v == 0xAB), "Deref/DerefMut で RW 疎通");
        }
    }

    /// wave 33-4 用の簡潔コンストラクタ (rustfmt 正準形との両立)。
    fn ent(x: f32, y: f32, z: f32, vx: f32, vy: f32, vz: f32, flags: u32) -> EntityAos {
        EntityAos {
            x,
            y,
            z,
            vx,
            vy,
            vz,
            flags,
        }
    }

    /// wave 33-4: integrate 厳密値 + AoS 往復一致。
    #[test]
    fn integrate_exact_and_aos_roundtrip() {
        let list = [
            ent(1.0, 2.0, 3.0, 0.5, -0.25, 4.0, 7),
            ent(-8.0, 0.0, 100.0, 4.0, 1.5, 0.0, 3),
        ];
        let mut s = EntitySoa::from_aos(&list);
        s.integrate(2.0);
        // x += vx*dt (左結合 1 回) の厳密値
        assert_eq!(s.x[0], 2.0); // 1.0 + 0.5*2
        assert_eq!(s.y[0], 1.5); // 2.0 + (-0.25)*2
        assert_eq!(s.z[0], 11.0); // 3.0 + 4.0*2
        assert_eq!(s.x[1], 0.0); // -8.0 + 4.0*2
        assert_eq!(s.y[1], 3.0);
        assert_eq!(s.z[1], 100.0);
        // 速度・フラグは不変、往復で一致 (位置部分のみ更新)
        let back = s.to_aos();
        let expect0 = EntityAos {
            x: 2.0,
            y: 1.5,
            z: 11.0,
            ..list[0]
        };
        let expect1 = EntityAos {
            x: 0.0,
            y: 3.0,
            z: 100.0,
            ..list[1]
        };
        assert_eq!(back, vec![expect0, expect1]);
    }

    /// wave 33-5: EntitySoa 等長契約 fail-loud (integrate / to_aos)。
    #[test]
    #[should_panic(expected = "EntitySoa 契約違反")]
    fn integrate_rejects_non_uniform_soa() {
        let mut s = EntitySoa::from_aos(&[
            ent(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0),
            ent(1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1),
        ]);
        s.y.pop(); // 不等長化
        s.integrate(0.016);
    }

    #[test]
    #[should_panic(expected = "EntitySoa 契約違反")]
    fn to_aos_rejects_non_uniform_soa() {
        let mut s = EntitySoa::from_aos(&[
            ent(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0),
            ent(1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1),
        ]);
        s.flags.pop(); // 不等長化 (旧実装は get の None で 1 件に静寂打切り)
        let _ = s.to_aos();
    }
}
