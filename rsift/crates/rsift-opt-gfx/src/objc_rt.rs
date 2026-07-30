//! ObjC ランタイム dispatcher 注入層 — 直 binding 検証アーキテクチャの中核。
//!
//! 【wave 194 GN (2026-07-30)】ユーザー要求「METAL/GL 完全実装・直 binding・
//! Mac 実機なしで動作保証する独自の静的解析マシーン」を受けた設計。
//!
//! ## なぜ dispatcher 注入か
//! Metal/CGL 呼出は全て ObjC メッセージ送信 (objc_msgSend) に帰着する。
//! その全呼出を「注入可能な trait オブジェクト経由」で行うことで:
//! - 本番 (macOS): extern objc_msgSend 実シンボルへの typed transmute 経路。
//! - 検証 (Linux CI / Mock): 同じ本番コード全行を Mock ランタイムに流し、
//!   呼出 selector 列・引数形状・retain 規則遵守を記録・機械検証できる。
//! すなわち「Mac 実機なしで、実コードが Apple API 契約通り動くこと」
//! (selector 名・引数個数・所有規則・呼出順序) を CI で動的に保証する。
//!
//! ## extern 宣言の扱い
//! libobjc の extern symbol を参照する実装は `#[cfg(target_os = "macos")]`
//! 配下のみとし、Linux ビルドではリンク対象外 (= Linux CI では Mock のみ
//! linked)。宣言ブロック自体は型検査のため全 OS で公開する。
//!
//! ## ABI 上の重要注意 (一次情報: Apple ドキュメント + metal-rs 0.28 実装)
//! - objc_msgSend の正しい呼出は「目的シグネチャの fn ポインタへの
//!   transmute」であり、variadic 宣言での直接呼出は UB (SysV/AAPCS)。
//! - 構造体 by-value 引数 (MTLClearColor=32B/MTLViewport=48B 等) は
//!   C ABI 準拠の fn 型を transmute すれば正しく渡る。
//! - 大構造体戻り値 (stret) 変種は本設計では使用しない (全戻り値は
//!   ポインタ/整数/f64/void のみ = 通常 objc_msgSend で正しい)。

use core::ffi::c_void;

/// ObjC オブジェクト実体ポインタ (`id` 相当)。
pub type ObjcId = *mut c_void;
/// ObjC クラス実体ポインタ (`Class` 相当)。
pub type ObjcClass = *mut c_void;
/// ObjC selector 実体ポインタ (`SEL` 相当)。
pub type Sel = *mut c_void;

extern "C" {
    /// libobjc: autorelease pool push。戻りは pool token。
    /// (一次情報: Objective-C Runtime Reference objc_autoreleasePoolPush)
    #[cfg(target_os = "macos")]
    #[link_name = "objc_autoreleasePoolPush"]
    fn objc_autorelease_pool_push_link() -> *mut c_void;
    /// libobjc: autorelease pool pop (token 指定)。pool 内の
    /// autoreleased オブジェクトが解放される。
    #[cfg(target_os = "macos")]
    #[link_name = "objc_autoreleasePoolPop"]
    fn objc_autorelease_pool_pop_link(token: *mut c_void);
    /// Metal framework: 既定デバイス取得 (autoreleased 返却)。
    /// canon CANON_METAL_CFNS argc=0。
    #[cfg(target_os = "macos")]
    #[link_name = "MTLCreateSystemDefaultDevice"]
    fn mtl_create_system_default_device_link() -> ObjcId;
    /// libobjc: メッセージディスパッチャ本体。直接呼出厳禁
    /// (variadic) — 目的シグネチャに transmute して使う。
    #[cfg(target_os = "macos")]
    #[link_name = "objc_msgSend"]
    fn objc_msgSend_link();
    /// libobjc: 名前からクラスを引く。見つからなければ null。
    #[cfg(target_os = "macos")]
    #[link_name = "objc_getClass"]
    fn objc_get_class_link(name: *const core::ffi::c_char) -> ObjcClass;
    /// libobjc: 名前から selector を登録取得 (冪等)。
    #[cfg(target_os = "macos")]
    #[link_name = "sel_registerName"]
    fn sel_register_name_link(name: *const core::ffi::c_char) -> Sel;
}

// 構造体 by-value 引数の正典レイアウト (一次情報: objc2-metal 生成 struct、
// Apple ヘッダ MTLTypes.h 由来)。全て repr(C) で C ABI 同一配置。
/// MTLClearColor (4×f64 = 32B)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MtlClearColor {
    /// 赤。
    pub red: f64,
    /// 緑。
    pub green: f64,
    /// 青。
    pub blue: f64,
    /// アルファ。
    pub alpha: f64,
}

/// MTLViewport (6×f64 = 48B)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MtlViewport {
    /// 原点 X。
    pub origin_x: f64,
    /// 原点 Y。
    pub origin_y: f64,
    /// 幅。
    pub width: f64,
    /// 高さ。
    pub height: f64,
    /// 深度 near。
    pub znear: f64,
    /// 深度 far。
    pub zfar: f64,
}

/// MTLScissorRect (4×u64 = 32B)。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MtlScissorRect {
    /// 原点 X。
    pub x: u64,
    /// 原点 Y。
    pub y: u64,
    /// 幅。
    pub width: u64,
    /// 高さ。
    pub height: u64,
}

/// CGSize (2×f64 = 16B) — CoreGraphics 寸法 (CAMetalLayer drawableSize)。
/// SysV/AAPCS 両 ABI で 16B struct by-value はレジスタペア渡し。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CgSize {
    /// 幅。
    pub width: f64,
    /// 高さ。
    pub height: f64,
}

/// MTLSize (3×u64 = 24B) — texture サイズ指定用。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtlSize {
    /// 幅。
    pub width: u64,
    /// 高さ。
    pub height: u64,
    /// 深さ。
    pub depth: u64,
}

/// MTLRegion (origin 3×u64 + size 3×u64 = 48B) — texture upload 用。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtlRegion {
    /// 起点サイズ (x,y,z)。
    pub origin: MtlSize,
    /// 寸法。
    pub size: MtlSize,
}

/// ObjC ランタイム dispatcher — 本番/モックで差替える注入面。
///
/// メソッド名は `<ret>_<args>` 形状 (引数文字は p=ポインタ, u=u64, d=f64)。
/// 各メソッド契約:
/// - `obj` は非 null (null 呼出は safety の観点で許すが wrapper が防ぐ)。
/// - `sel` は本 trait の `reg_sel` で取得したもののみ。
/// - 戻り値 id 系は null 可 (メソッド応答 null = 失敗等)。
pub trait ObjcRt {
    /// 名前からクラスを引く (ObjCClass = null 可 = クラス不在)。
    fn get_class(&mut self, name: &'static core::ffi::CStr) -> ObjcClass;
    /// 名前から selector を登録取得 (必ず非 null)。
    fn reg_sel(&mut self, name: &'static core::ffi::CStr) -> Sel;

    // ---- 戻り値 id ----
    /// 0 引数 id 返却 (newCommandQueue, commandBuffer, …)。
    fn id_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> ObjcId;
    /// 1 ポインタ引数 id 返却 (newTextureWithDescriptor:, …)。
    fn id_1p(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId) -> ObjcId;
    /// 1 u64 引数 id 返却 (objectAtIndexedSubscript:、objectAtIndex:)。
    fn id_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64) -> ObjcId;
    /// u64×3+bool 引数 id 返却 (texture2DDescriptorWithPixelFormat:width:height:mipmapped:)。
    fn id_3uuub(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: u64,
        b: u64,
        c: u64,
        d: bool,
    ) -> ObjcId;
    /// ポインタ+u64 引数 id 返却 (newBufferWithBytes:length:options: 系)。
    fn id_2pu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64) -> ObjcId;
    /// ポインタ+u64×2 引数 id 返却 (newBufferWithBytes:length:options:)。
    fn id_3puu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: u64,
        c: u64,
    ) -> ObjcId;
    /// u64×2 引数 id 返却 (newBufferWithLength:options:)。
    fn id_2uu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64, b: u64) -> ObjcId;
    /// ポインタ×3 引数 id 返却 (newLibraryWithSource:options:error:)。
    fn id_3ppp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
        c: ObjcId,
    ) -> ObjcId;
    /// ポインタ×2 引数 id 返却 (newRenderPipelineStateWithDescriptor:error:)。
    fn id_2pp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
    ) -> ObjcId;
    /// ポインタ×4 引数 id 返却
    /// (newBufferWithBytesNoCopy:length:options:deallocator:)。
    fn id_4ppuu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
        c: u64,
        d: u64,
    ) -> ObjcId;

    // ---- 戻り値 void ----
    /// 0 引数 void (commit, endEncoding, release, …)。
    fn void_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr);
    /// 1 ポインタ引数 void (presentDrawable:, setRenderPipelineState:, …)。
    fn void_1p(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId);
    /// ポインタ+u64×2 引数 void (setVertexBuffer:offset:atIndex:)。
    fn void_3puu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64, c: u64);
    /// u64×3 引数 void (drawPrimitives:vertexStart:vertexCount:)。
    fn void_3uuu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64, b: u64, c: u64);
    /// u64×2+ポインタ+u64×2 引数 void
    /// (drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:)。
    fn void_5uuupu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: u64,
        b: u64,
        c: u64,
        d: ObjcId,
        e: u64,
    );
    /// ポインタ+u64 引数 void (setFragmentTexture:atIndex: 等)。
    fn void_2pu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64);
    /// 1 u64 引数 void (setPixelFormat:、setMaximumDrawableCount:、
    /// setLoadAction:、setStoreAction:、setUsage: 等)。
    fn void_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64);
    /// 1 bool 引数 void (setDisplaySyncEnabled:、setFramebufferOnly:、
    /// setPresentsWithTransaction:)。ObjC BOOL は 1 バイト符号付き 0/1。
    fn void_1b(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: bool);
    /// CGSize by-value 引数 void (setDrawableSize:)。
    fn void_cgsize(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: CgSize);
    /// texture readback (getBytes:bytesPerRow:fromRegion:mipmapLevel:)。
    /// out へ bpr 行バイトで region 分コピーされる。
    fn get_bytes_region(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        out: *mut c_void,
        bpr: u64,
        region: MtlRegion,
        level: u64,
    );
    /// autorelease pool push → token 返却 (pool 範囲開始)。
    fn pool_push(&mut self) -> *mut c_void;
    /// autorelease pool pop (token 範囲の autoreleased 解放)。
    fn pool_pop(&mut self, token: *mut c_void);
    /// MTLClearColor by-value 引数 void (setClearColor:)。
    fn void_clearcolor(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlClearColor);
    /// MTLViewport by-value 引数 void (setViewport:)。
    fn void_viewport(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlViewport);
    /// MTLScissorRect by-value 引数 void (setScissorRect:)。
    fn void_scissor(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlScissorRect);
    /// MTLRegion by-value + ポインタ + u64×2 引数 void
    /// (replaceRegion:mipmapLevel:withBytes:bytesPerRow: — 4 引数)。
    fn void_region_update(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        r: MtlRegion,
        level: u64,
        bytes: ObjcId,
        bpr: u64,
    );

    // ---- 戻り値スカラ ----
    /// 0 引数 u64 返却 (status, length, …)。
    fn u64_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> u64;
    /// 0 引数ポインタ返却 (contents, UTF8String, …)。
    fn ptr_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> *mut c_void;
    /// 1 u64 引数 bool 返却 (supportsFamily:)。
    fn bool_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64) -> bool;
    /// C エントリ MTLCreateSystemDefaultDevice() の注入トンネル
    /// (canon CANON_METAL_CFNS に name+argc=0 で存在 → 監査対象)。
    /// 返却は autoreleased device (Borrowed 規則、release 禁止)。
    fn c_mtl_default_device(&mut self) -> ObjcId;
}

/// production dispatcher (macOS 実シンボル) — typed transmute 経路。
///
/// 全ての ObjC メッセージが objc_msgSend に帰着する一次情報
/// (Apple objc runtime 仕様) に基づく。各メソッドでは `objc_msgSend`
/// のアドレスを「そのシグネチャちょうどの extern "C" fn 型」に
/// transmute して呼ぶ = 唯一の正しい呼出形。
#[cfg(target_os = "macos")]
pub struct NativeObjcRt;

#[cfg(target_os = "macos")]
impl ObjcRt for NativeObjcRt {
    fn get_class(&mut self, name: &'static core::ffi::CStr) -> ObjcClass {
        unsafe { objc_get_class_link(name.as_ptr()) }
    }
    fn reg_sel(&mut self, name: &'static core::ffi::CStr) -> Sel {
        unsafe { sel_register_name_link(name.as_ptr()) }
    }
    fn id_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel)
        }
    }
    fn id_1p(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn id_2pu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, u64) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b)
        }
    }
    fn id_3puu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: u64,
        c: u64,
    ) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, u64, u64) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c)
        }
    }
    fn id_2uu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64, b: u64) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64, u64) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b)
        }
    }
    fn id_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn id_3uuub(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: u64,
        b: u64,
        c: u64,
        d: bool,
    ) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64, u64, u64, bool) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c, d)
        }
    }
    fn id_3ppp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
        c: ObjcId,
    ) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, ObjcId, ObjcId) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c)
        }
    }
    fn id_2pp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
    ) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, ObjcId) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b)
        }
    }
    fn id_4ppuu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: ObjcId,
        b: ObjcId,
        c: u64,
        d: u64,
    ) -> ObjcId {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, ObjcId, u64, u64) -> ObjcId =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c, d)
        }
    }
    fn void_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel)
        }
    }
    fn void_1p(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn void_3puu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64, c: u64) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, u64, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c)
        }
    }
    fn void_3uuu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64, b: u64, c: u64) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64, u64, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c)
        }
    }
    fn void_5uuupu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        a: u64,
        b: u64,
        c: u64,
        d: ObjcId,
        e: u64,
    ) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64, u64, u64, ObjcId, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b, c, d, e)
        }
    }
    fn void_2pu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: ObjcId, b: u64) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, ObjcId, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a, b)
        }
    }
    fn void_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn void_1b(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: bool) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, bool) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn void_cgsize(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: CgSize) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, CgSize) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, v)
        }
    }
    fn get_bytes_region(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        out: *mut c_void,
        bpr: u64,
        region: MtlRegion,
        level: u64,
    ) {
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, *mut c_void, u64, MtlRegion, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, out, bpr, region, level)
        }
    }
    fn pool_push(&mut self) -> *mut c_void {
        unsafe { objc_autorelease_pool_push_link() }
    }
    fn pool_pop(&mut self, token: *mut c_void) {
        unsafe { objc_autorelease_pool_pop_link(token) }
    }
    fn void_clearcolor(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlClearColor) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, MtlClearColor) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, v)
        }
    }
    fn void_viewport(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlViewport) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, MtlViewport) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, v)
        }
    }
    fn void_scissor(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, v: MtlScissorRect) {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, MtlScissorRect) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, v)
        }
    }
    fn void_region_update(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        r: MtlRegion,
        level: u64,
        bytes: ObjcId,
        bpr: u64,
    ) {
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, MtlRegion, u64, ObjcId, u64) =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, r, level, bytes, bpr)
        }
    }
    fn u64_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> u64 {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel) -> u64 =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel)
        }
    }
    fn ptr_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> *mut c_void {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel) -> *mut c_void =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel)
        }
    }
    fn bool_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, a: u64) -> bool {
        let sel = unsafe { sel_register_name_link(sel.as_ptr()) };
        unsafe {
            let f: extern "C" fn(ObjcId, Sel, u64) -> bool =
                core::mem::transmute(objc_msgSend_link as *mut c_void);
            f(obj, sel, a)
        }
    }
    fn c_mtl_default_device(&mut self) -> ObjcId {
        unsafe { mtl_create_system_default_device_link() }
    }
}

/// 呼出 1 件の記録 (検証用ミニ DSL)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRec {
    /// 呼出 dispatcher メソッド名 (例 "id_3ppp")。
    pub method: &'static str,
    /// 対象 selector 名 (例 "newLibraryWithSource:options:error:")。
    pub sel: String,
    /// 引数の形状ダイジェスト (型種別列、例 "p,p,p")。
    pub args_shape: &'static str,
}

/// Mock ObjC ランタイム — シナリオ駆動の検証器。
///
/// 使い方: `script` に (class_name, sel_name) → 応答 (新規生成オブジェクトの
/// class 名) を登録した状態で本番コードを全行実行し、完了後に `calls` の
/// 呼出列・`live` オブジェクト残存 (release 漏れ検出) を検査する。
pub struct MockObjcRt {
    /// sel 名 → Sel ポインタ (mut 参照の一意アドレス)。
    sels: Vec<&'static core::ffi::CStr>,
    /// 呼出記録。
    pub calls: Vec<CallRec>,
    /// 生成したオブジェクトの class 名 (index が fake ポインタ値+1)。
    pub objs: Vec<&'static str>,
    /// 生存オブジェクト (未 release) の fake id 集合。
    pub live: Vec<usize>,
    /// 応答スクリプト: (recv_class, sel, 生成 class, live 追跡するか)。
    /// live 追跡=false は autoreleased 等の借用応答 (canon RetainRule::Borrowed)
    /// を表し、テスト終了時の live 突合対象から外れる。
    pub script: Vec<(&'static str, &'static str, &'static str, bool)>,
    /// オブジェクト id → class 名。
    obj_class: Vec<&'static str>,
    /// get_class で登場したクラス名の安定 id 表 (mock のみ)。
    classes: Vec<&'static str>,
    /// MTLCreateSystemDefaultDevice 応答用の device fake id (遅延生成)。
    default_device: Option<ObjcId>,
    /// u64 応答 override: (recv_class, sel) → 値。
    /// 未登録の u64 問合せは 0 を返す ( ObjC 実態のゼロ初期化に準拠)。
    u64_overrides: Vec<(&'static str, &'static str, u64)>,
    /// ptr 応答 override: (recv_class, sel)。一致時は arena 先頭を返す
    /// (MTLBuffer contents 相当の「CPU から普通に書ける実メモリ」)。
    ptr_overrides: Vec<(&'static str, &'static str)>,
    /// contents 応答用の実メモリ (64KiB)。new 後は resize しないため
    /// 先頭ポインタは安定。buffer 書込みの動作検証を Linux 上で実物理行する。
    arena: Vec<u8>,
}

impl MockObjcRt {
    /// 空 mock。
    pub fn new() -> Self {
        Self {
            sels: Vec::new(),
            calls: Vec::new(),
            objs: Vec::new(),
            live: Vec::new(),
            script: Vec::new(),
            obj_class: Vec::new(),
            classes: Vec::new(),
            default_device: None,
            u64_overrides: Vec::new(),
            ptr_overrides: Vec::new(),
            arena: vec![0u8; 64 * 1024],
        }
    }
    /// arena 先頭から len バイトのスナップショット (contents 書込み検証用)。
    pub fn arena_bytes(&self, len: usize) -> &[u8] {
        &self.arena[..len.min(self.arena.len())]
    }
    /// fake id → class 名 (0x1000+ はクラスオブジェクト空間)。
    pub fn class_of(&self, id: ObjcId) -> Option<&'static str> {
        let v = id as usize;
        if v >= 0x1000 {
            return self.classes.get(v - 0x1000).copied();
        }
        let idx = v.checked_sub(1)?;
        self.obj_class.get(idx).copied()
    }
    /// 応答登録 (owned=canon Owned 規則): 生成物を live で追跡する。
    pub fn on_make(&mut self, recv: &'static str, sel: &'static str, made: &'static str) {
        self.script.push((recv, sel, made, true));
    }
    /// 応答登録 (borrowed=canon Borrowed 規則): live 追跡しない。
    pub fn on_borrow(&mut self, recv: &'static str, sel: &'static str, made: &'static str) {
        self.script.push((recv, sel, made, false));
    }
    /// u64 応答 override 登録: `recv` class の `sel` 問合せに `val` を返す。
    /// 同名 selector を持つ他 class との衝突を避けるため class 付きキーとする。
    pub fn on_u64(&mut self, recv: &'static str, sel: &'static str, val: u64) {
        self.u64_overrides.push((recv, sel, val));
    }
    /// ptr 応答 override 登録: `recv` class の `sel` 問合せに arena 先頭
    /// (実書込み可能メモリ) を返す。MTLBuffer contents の動作検証に使う。
    pub fn on_ptr(&mut self, recv: &'static str, sel: &'static str) {
        self.ptr_overrides.push((recv, sel));
    }
    /// 応答登録なしで直接 class 付き fake オブジェクトを生成 (テスト補助)。
    pub fn mk(&mut self, class: &'static str) -> ObjcId {
        self.objs.push(class);
        self.obj_class.push(class);
        let id = self.objs.len();
        self.live.push(id);
        id as ObjcId
    }
    fn spawn(&mut self, recv: ObjcId, sel_name: &str) -> ObjcId {
        // init family 契約: レシーバ (alloc 直後のオブジェクト) をそのまま
        // 返す (新規 live を作らない = alloc の +1 を init が absorb する
        // ObjC メモリ管理規則の mock 的模倣)。
        if sel_name == "init" || sel_name.starts_with("initWith") {
            return recv;
        }
        let rc = self.class_of(recv).unwrap_or("?");
        for (r, s, made, track) in &self.script {
            if *r == rc && *s == sel_name {
                self.objs.push(made);
                self.obj_class.push(made);
                let id = self.objs.len();
                if *track {
                    self.live.push(id);
                }
                return id as ObjcId;
            }
        }
        core::ptr::null_mut()
    }
    fn note_release(&mut self, obj: ObjcId, sel_name: &str) {
        if sel_name == "release" {
            let id = obj as usize;
            self.live.retain(|x| *x != id);
        }
    }
}

impl Default for MockObjcRt {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjcRt for MockObjcRt {
    fn get_class(&mut self, name: &'static core::ffi::CStr) -> ObjcClass {
        // mock では class オブジェクトを 0x1000+index 空間で表す。
        // 初見のクラス名は mock 独自空間に登録して安定 id にする。
        let n = name.to_str().unwrap_or("?");
        let idx = self
            .classes
            .iter()
            .position(|c| *c == n)
            .unwrap_or_else(|| {
                self.classes.push(n);
                self.classes.len() - 1
            });
        (0x1000 + idx) as ObjcClass
    }
    fn reg_sel(&mut self, name: &'static core::ffi::CStr) -> Sel {
        if let Some(i) = self.sels.iter().position(|c| *c == name) {
            return i as Sel;
        }
        self.sels.push(name);
        (self.sels.len() - 1) as Sel
    }
    fn id_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_0",
            sel: n.clone(),
            args_shape: "",
        });
        self.note_release(obj, &n);
        self.spawn(obj, &n)
    }
    fn id_1u(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, _a: u64) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_1u",
            sel: n.clone(),
            args_shape: "u",
        });
        self.spawn(obj, &n)
    }
    fn id_3uuub(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: u64,
        _b: u64,
        _c: u64,
        _d: bool,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_3uuub",
            sel: n.clone(),
            args_shape: "u,u,u,b",
        });
        self.spawn(obj, &n)
    }
    fn id_1p(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, _a: ObjcId) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_1p",
            sel: n.clone(),
            args_shape: "p",
        });
        self.spawn(obj, &n)
    }
    fn id_2pu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: u64,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_2pu",
            sel: n.clone(),
            args_shape: "p,u",
        });
        self.spawn(obj, &n)
    }
    fn id_3puu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: u64,
        _c: u64,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_3puu",
            sel: n.clone(),
            args_shape: "p,u,u",
        });
        self.spawn(obj, &n)
    }
    fn id_2uu(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr, _a: u64, _b: u64) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_2uu",
            sel: n.clone(),
            args_shape: "u,u",
        });
        self.spawn(obj, &n)
    }
    fn id_3ppp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: ObjcId,
        _c: ObjcId,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_3ppp",
            sel: n.clone(),
            args_shape: "p,p,p",
        });
        self.spawn(obj, &n)
    }
    fn id_2pp(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: ObjcId,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_2pp",
            sel: n.clone(),
            args_shape: "p,p",
        });
        self.spawn(obj, &n)
    }
    fn id_4ppuu(
        &mut self,
        obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: ObjcId,
        _c: u64,
        _d: u64,
    ) -> ObjcId {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "id_4ppuu",
            sel: n.clone(),
            args_shape: "p,p,u,u",
        });
        self.spawn(obj, &n)
    }
    fn void_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_0",
            sel: n.clone(),
            args_shape: "",
        });
        self.note_release(obj, &n);
    }
    fn void_1p(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _a: ObjcId) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_1p",
            sel: n,
            args_shape: "p",
        });
    }
    fn void_3puu(
        &mut self,
        _obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: ObjcId,
        _b: u64,
        _c: u64,
    ) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_3puu",
            sel: n,
            args_shape: "p,u,u",
        });
    }
    fn void_3uuu(
        &mut self,
        _obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: u64,
        _b: u64,
        _c: u64,
    ) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_3uuu",
            sel: n,
            args_shape: "u,u,u",
        });
    }
    fn void_5uuupu(
        &mut self,
        _obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _a: u64,
        _b: u64,
        _c: u64,
        _d: ObjcId,
        _e: u64,
    ) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_5uuupu",
            sel: n,
            args_shape: "u,u,u,p,u",
        });
    }
    fn void_clearcolor(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _v: MtlClearColor) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_clearcolor",
            sel: n,
            args_shape: "clearcolor",
        });
    }
    fn void_viewport(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _v: MtlViewport) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_viewport",
            sel: n,
            args_shape: "viewport",
        });
    }
    fn void_scissor(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _v: MtlScissorRect) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_scissor",
            sel: n,
            args_shape: "scissor",
        });
    }
    fn void_region_update(
        &mut self,
        _obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _r: MtlRegion,
        _level: u64,
        _bytes: ObjcId,
        _bpr: u64,
    ) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_region_update",
            sel: n,
            args_shape: "region,u,p,u",
        });
    }
    fn void_2pu(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _a: ObjcId, _b: u64) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_2pu",
            sel: n,
            args_shape: "p,u",
        });
    }
    fn void_1u(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _a: u64) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_1u",
            sel: n,
            args_shape: "u",
        });
    }
    fn void_1b(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _a: bool) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_1b",
            sel: n,
            args_shape: "b",
        });
    }
    fn void_cgsize(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _v: CgSize) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "void_cgsize",
            sel: n,
            args_shape: "cgsize",
        });
    }
    fn get_bytes_region(
        &mut self,
        _obj: ObjcId,
        sel: &'static core::ffi::CStr,
        _out: *mut c_void,
        _bpr: u64,
        _region: MtlRegion,
        _level: u64,
    ) {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "get_bytes_region",
            sel: n,
            args_shape: "p,u,region,u",
        });
    }
    fn pool_push(&mut self) -> *mut c_void {
        self.calls.push(CallRec {
            method: "pool_push",
            sel: String::new(),
            args_shape: "",
        });
        1 as *mut c_void
    }
    fn pool_pop(&mut self, _token: *mut c_void) {
        self.calls.push(CallRec {
            method: "pool_pop",
            sel: String::new(),
            args_shape: "",
        });
    }
    fn u64_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> u64 {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "u64_0",
            sel: n.clone(),
            args_shape: "",
        });
        let rc = self.class_of(obj).unwrap_or("?");
        for (r, s, v) in &self.u64_overrides {
            if *r == rc && *s == n {
                return *v;
            }
        }
        0
    }
    fn ptr_0(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr) -> *mut c_void {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "ptr_0",
            sel: n.clone(),
            args_shape: "",
        });
        let rc = self.class_of(obj).unwrap_or("?");
        for (r, s) in &self.ptr_overrides {
            if *r == rc && *s == n {
                return self.arena.as_mut_ptr() as *mut c_void;
            }
        }
        core::ptr::null_mut()
    }
    fn bool_1u(&mut self, _obj: ObjcId, sel: &'static core::ffi::CStr, _a: u64) -> bool {
        let n = sel.to_str().unwrap_or("?").to_string();
        self.calls.push(CallRec {
            method: "bool_1u",
            sel: n,
            args_shape: "u",
        });
        true
    }
    fn c_mtl_default_device(&mut self) -> ObjcId {
        self.calls.push(CallRec {
            method: "c_mtl_default_device",
            sel: String::new(),
            args_shape: "",
        });
        if let Some(d) = self.default_device {
            return d;
        }
        self.objs.push("MTLDevice");
        self.obj_class.push("MTLDevice");
        let d = self.objs.len() as ObjcId;
        self.default_device = Some(d);
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gn_mock_records_calls_and_release() {
        let mut rt = MockObjcRt::new();
        rt.on_make("MTLDevice", "newCommandQueue", "MTLCommandQueue");
        let dev = rt.mk("MTLDevice");
        let q = rt.id_0(dev, c"newCommandQueue");
        assert!(!q.is_null(), "script 応答で queue が返る");
        assert_eq!(rt.class_of(q), Some("MTLCommandQueue"));
        assert_eq!(rt.calls.len(), 1);
        assert_eq!(rt.calls[0].sel, "newCommandQueue");
        assert_eq!(rt.calls[0].method, "id_0");
        // release で live から消える
        rt.void_0(q, c"release");
        assert!(rt.live.iter().all(|id| *id != q as usize));
        // 未 release の dev が残存 = リーク検出対象になること
        assert_eq!(rt.live.len(), 1);
    }

    #[test]
    fn gn_mock_reg_sel_interning_stable() {
        let mut rt = MockObjcRt::new();
        let a1 = rt.reg_sel(c"commit");
        let a2 = rt.reg_sel(c"commit");
        let b = rt.reg_sel(c"waitUntilCompleted");
        assert_eq!(a1, a2, "同名 selector は同一 Sel (冪等インターン)");
        assert_ne!(a1, b, "別名は別 Sel");
        assert_eq!(rt.sels.len(), 2, "インターン表は重複なし");
        // 表の内容 (CStr 実体) も登録順に一致。
        assert_eq!(rt.sels[0].to_bytes(), b"commit");
        assert_eq!(rt.sels[1].to_bytes(), b"waitUntilCompleted");
    }

    #[test]
    fn gn_mock_shape_digest() {
        let mut rt = MockObjcRt::new();
        let sel_lib = c"newLibraryWithSource:options:error:";
        let dev = {
            rt.objs.push("MTLDevice");
            rt.obj_class.push("MTLDevice");
            rt.objs.len() as ObjcId
        };
        rt.id_3ppp(
            dev,
            sel_lib,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(rt.calls[0].args_shape, "p,p,p");
        assert_eq!(rt.calls[0].sel, "newLibraryWithSource:options:error:");
    }
}
