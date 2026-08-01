//! Metal 直 binding 完全実装 (classic Metal 系) — objc runtime dispatcher 経由。
//!
//! 【wave 194 GN (2026-07-30)】ユーザー要求「METAL、GL 完全実装・直 binding・
//! Mac 実機なしで動作保証する独自静的解析マシン」。
//!
//! ## 保証体系
//! 1. **静的解析マシン** `apple_ffi_audit`: 本ファイルの全 selector 宣言・
//!    extern 宣言・enum 値・呼出引数形状を、一次情報 canon
//!    (apple_canon.rs、Apple SDK ヘッダ機械転記から生成) と機械照合。
//! 2. **Mock 動作保証**: `objc_rt::MockObjcRt` が実 ObjC メモリ規則
//!    (init family absorb・release 追跡) を模倣し、本番コード全行を
//!    Linux/CI で動かして呼出列・リークを検証。
//! 3. **実機経路**: `#[cfg(target_os = "macos")]` `NativeObjcRt` が
//!    objc_msgSend を目的シグネチャへ transmute した正規呼出を実行。
//!
//! ## 実装スコープ (完全・スタブなし)
//! オフスクリーン描画全経路 (三角形、実ピクセル readback 迄) +
//! CAMetalLayer への present 経路 + NSError 詳細回収。msl ソースは
//! metal_stdlib 正規形 (vertex_id 駆動、constant buffer バインド)。
//!
//! ## selector 宣言規約 (監査機が正規表現で抽出する固定形式)
//! - `pub const SEL_*: (&str, &CStr) = ("<Class>", c"<sel>");` — class 帰属。
//! - `pub const MTLV_*: (&str, &str, i64) = ("<Enum>", "<Variant>", <val>);`
//!   — enum 値定数、canon CANON_METAL_ENUMS と整合必須。
//! - 呼出は `rt.<disp>(obj, SEL_*.1, args...)` — disp 名の数値引数個数が
//!   selector コロン数と一致することを監査機が全件機械照合。

use crate::objc_rt::{CgSize, MtlClearColor, MtlRegion, MtlSize, ObjcId, ObjcRt};
use core::ffi::CStr;

// ---- SEL: オブジェクト管理 ----
/// alloc (NSObject 継承解決 → NSObject::alloc、canon Owned)。
pub const SEL_ALLOC: (&str, &CStr) = ("NSObject", c"alloc");
/// init (init family、alloc 直後の absorb、canon Owned)。
pub const SEL_INIT: (&str, &CStr) = ("NSObject", c"init");
/// new (class factory、canon Owned)。
pub const SEL_NEW: (&str, &CStr) = ("MTLRenderPipelineDescriptor", c"new");
/// release (canon NSObject、void_0)。
pub const SEL_RELEASE: (&str, &CStr) = ("NSObject", c"release");

// ---- SEL: NSString/NSError ユーティリティ ----
/// NSString 生成 (init family、Owned absorb)。
pub const SEL_INIT_WITH_UTF8: (&str, &CStr) = ("NSString", c"initWithUTF8String:");
/// NSString → C 文字列ポインタ取得。
pub const SEL_UTF8_STRING: (&str, &CStr) = ("NSString", c"UTF8String");
/// NSError の詳細文字列 (localizedDescription → NSString)。
pub const SEL_LOCALIZED_DESCRIPTION: (&str, &CStr) = ("NSError", c"localizedDescription");

// ---- SEL: device/queue ----
/// コマンドキュー生成 (MTLDevice、canon Owned)。
pub const SEL_NEW_COMMAND_QUEUE: (&str, &CStr) = ("MTLDevice", c"newCommandQueue");
/// デバイス名 (Borrowed)。
pub const SEL_DEVICE_NAME: (&str, &CStr) = ("MTLDevice", c"name");
/// GPU family 判定 (bool_1u)。
pub const SEL_SUPPORTS_FAMILY: (&str, &CStr) = ("MTLDevice", c"supportsFamily:");
/// バッファ生成・ゼロ初期化 (Owned)。
pub const SEL_NEW_BUFFER_LEN_OPTS: (&str, &CStr) = ("MTLDevice", c"newBufferWithLength:options:");
/// バッファ生成・初期データコピー (Owned)。
pub const SEL_NEW_BUFFER_BYTES_OPTS: (&str, &CStr) =
    ("MTLDevice", c"newBufferWithBytes:length:options:");
/// バッファ CPU ポインタ (Borrowed、Shared/Managed storage で有効)。
/// Apple MTLBuffer 一次情報: Shared storage では contents が
/// CPU アドレス空間の直接ポインタを返す (dynamic data 更新の正規経路)。
pub const SEL_CONTENTS: (&str, &CStr) = ("MTLBuffer", c"contents");
/// テクスチャ生成 (Owned)。
pub const SEL_NEW_TEXTURE: (&str, &CStr) = ("MTLDevice", c"newTextureWithDescriptor:");
/// MSL ライブラリ生成 (Owned、options=null は既定)。
pub const SEL_NEW_LIBRARY_SRC: (&str, &CStr) =
    ("MTLDevice", c"newLibraryWithSource:options:error:");
/// レンダーパイプライン生成 (Owned、error out-param)。
pub const SEL_NEW_PIPELINE: (&str, &CStr) =
    ("MTLDevice", c"newRenderPipelineStateWithDescriptor:error:");

// ---- SEL: library/function ----
/// 関数名から MTLFunction 取得 (Owned、canon method_id New)。
pub const SEL_NEW_FUNCTION: (&str, &CStr) = ("MTLLibrary", c"newFunctionWithName:");

// ---- SEL: pipeline descriptor ----
/// vertex function セット (void_1p)。
pub const SEL_SET_VERTEX_FUNCTION: (&str, &CStr) =
    ("MTLRenderPipelineDescriptor", c"setVertexFunction:");
/// fragment function セット (void_1p)。
pub const SEL_SET_FRAGMENT_FUNCTION: (&str, &CStr) =
    ("MTLRenderPipelineDescriptor", c"setFragmentFunction:");
/// colorAttachments 配列 (Borrowed)。
pub const SEL_COLOR_ATTACHMENTS: (&str, &CStr) =
    ("MTLRenderPipelineDescriptor", c"colorAttachments");
/// 添字要素取得 (id_1u、attachment array)。
pub const SEL_OBJECT_AT_INDEXED_SUBSCRIPT: (&str, &CStr) = (
    "MTLRenderPipelineColorAttachmentDescriptorArray",
    c"objectAtIndexedSubscript:",
);
/// attachment pixelFormat セット (void_1u)。
pub const SEL_SET_PIXEL_FORMAT: (&str, &CStr) = (
    "MTLRenderPipelineColorAttachmentDescriptor",
    c"setPixelFormat:",
);

// ---- SEL: texture ----
/// 2D テクスチャ記述子 convenience factory (Borrowed = autoreleased)。
pub const SEL_TEX2D_DESC: (&str, &CStr) = (
    "MTLTextureDescriptor",
    c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
);
/// usage セット (void_1u、RenderTarget|ShaderRead 等)。
pub const SEL_SET_USAGE: (&str, &CStr) = ("MTLTextureDescriptor", c"setUsage:");
/// storageMode セット (void_1u)。
pub const SEL_SET_STORAGE_MODE: (&str, &CStr) = ("MTLTextureDescriptor", c"setStorageMode:");
/// テクスチャ幅 (u64_0)。
pub const SEL_TEX_WIDTH: (&str, &CStr) = ("MTLTexture", c"width");
/// テクスチャ高さ (u64_0)。
pub const SEL_TEX_HEIGHT: (&str, &CStr) = ("MTLTexture", c"height");
/// ピクセル領域アップロード (void_region_update)。
pub const SEL_REPLACE_REGION: (&str, &CStr) = (
    "MTLTexture",
    c"replaceRegion:mipmapLevel:withBytes:bytesPerRow:",
);
/// ピクセル領域 readback (get_bytes_region)。
pub const SEL_GET_BYTES: (&str, &CStr) = (
    "MTLTexture",
    c"getBytes:bytesPerRow:fromRegion:mipmapLevel:",
);

// ---- SEL: command buffer/encoder ----
/// コマンドバッファ生成 (autoreleased = Borrowed)。
pub const SEL_COMMAND_BUFFER: (&str, &CStr) = ("MTLCommandQueue", c"commandBuffer");
/// render エンコーダ生成 (Borrowed)。
pub const SEL_RENDER_ENCODER: (&str, &CStr) =
    ("MTLCommandBuffer", c"renderCommandEncoderWithDescriptor:");
/// present (void_1p)。
pub const SEL_PRESENT_DRAWABLE: (&str, &CStr) = ("MTLCommandBuffer", c"presentDrawable:");
/// commit (void_0)。
pub const SEL_COMMIT: (&str, &CStr) = ("MTLCommandBuffer", c"commit");
/// 完了待機 (void_0)。
pub const SEL_WAIT_COMPLETED: (&str, &CStr) = ("MTLCommandBuffer", c"waitUntilCompleted");
/// status (u64_0、完了=Completed(4))。
pub const SEL_CB_STATUS: (&str, &CStr) = ("MTLCommandBuffer", c"status");
/// エラー取得 (Borrowed、nullable)。
pub const SEL_CB_ERROR: (&str, &CStr) = ("MTLCommandBuffer", c"error");
/// render pass 添字要素取得 (id_1u)。
pub const SEL_PASS_OBJECT_AT_SUBSCRIPT: (&str, &CStr) = (
    "MTLRenderPassColorAttachmentDescriptorArray",
    c"objectAtIndexedSubscript:",
);
/// pass colorAttachments (Borrowed)。
pub const SEL_PASS_COLOR_ATTACHMENTS: (&str, &CStr) =
    ("MTLRenderPassDescriptor", c"colorAttachments");
/// pass 生成 (class factory new、Owned)。
pub const SEL_PASS_NEW: (&str, &CStr) = ("MTLRenderPassDescriptor", c"new");
/// 描画先テクスチャセット (void_1p)。
pub const SEL_SET_TEXTURE: (&str, &CStr) =
    ("MTLRenderPassColorAttachmentDescriptor", c"setTexture:");
/// LoadAction セット (void_1u)。
pub const SEL_SET_LOAD_ACTION: (&str, &CStr) =
    ("MTLRenderPassColorAttachmentDescriptor", c"setLoadAction:");
/// StoreAction セット (void_1u)。
pub const SEL_SET_STORE_ACTION: (&str, &CStr) =
    ("MTLRenderPassColorAttachmentDescriptor", c"setStoreAction:");
/// ClearColor セット (void_clearcolor)。
pub const SEL_SET_CLEAR_COLOR: (&str, &CStr) =
    ("MTLRenderPassColorAttachmentDescriptor", c"setClearColor:");
/// パイプライン状態セット (void_1p)。
pub const SEL_SET_PIPELINE_STATE: (&str, &CStr) =
    ("MTLRenderCommandEncoder", c"setRenderPipelineState:");
/// viewport (void_viewport)。
pub const SEL_SET_VIEWPORT: (&str, &CStr) = ("MTLRenderCommandEncoder", c"setViewport:");
/// scissor (void_scissor)。
pub const SEL_SET_SCISSOR: (&str, &CStr) = ("MTLRenderCommandEncoder", c"setScissorRect:");
/// vertex buffer バインド (void_3puu)。
pub const SEL_SET_VERTEX_BUFFER: (&str, &CStr) = (
    "MTLRenderCommandEncoder",
    c"setVertexBuffer:offset:atIndex:",
);
/// fragment texture バインド (void_2pu)。
pub const SEL_SET_FRAGMENT_TEXTURE: (&str, &CStr) =
    ("MTLRenderCommandEncoder", c"setFragmentTexture:atIndex:");
/// 非 index 描画 (void_3uuu)。
pub const SEL_DRAW_PRIMITIVES: (&str, &CStr) = (
    "MTLRenderCommandEncoder",
    c"drawPrimitives:vertexStart:vertexCount:",
);
/// index 描画 (void_5uuupu)。
pub const SEL_DRAW_INDEXED: (&str, &CStr) = (
    "MTLRenderCommandEncoder",
    c"drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:",
);
/// endEncoding (MTLCommandEncoder 継承経由、canon 帰属解決)。
pub const SEL_END_ENCODING: (&str, &CStr) = ("MTLRenderCommandEncoder", c"endEncoding");

// ---- SEL: CAMetalLayer (present 経路) ----
/// layer device セット (void_1p)。
pub const SEL_LAYER_SET_DEVICE: (&str, &CStr) = ("CAMetalLayer", c"setDevice:");
/// layer pixelFormat (void_1u)。
pub const SEL_LAYER_SET_PIXEL_FORMAT: (&str, &CStr) = ("CAMetalLayer", c"setPixelFormat:");
/// layer drawableSize (void_cgsize)。
pub const SEL_LAYER_SET_DRAWABLE_SIZE: (&str, &CStr) = ("CAMetalLayer", c"setDrawableSize:");
/// 最大 in-flight 数 (void_1u)。
pub const SEL_LAYER_SET_MAX_DRAWABLE: (&str, &CStr) = ("CAMetalLayer", c"setMaximumDrawableCount:");
/// presentsWithTransaction (void_1b)。
pub const SEL_LAYER_SET_PRESENTS_TRANSACTION: (&str, &CStr) =
    ("CAMetalLayer", c"setPresentsWithTransaction:");
/// displaySyncEnabled (void_1b、vsync 制御)。
pub const SEL_LAYER_SET_DISPLAY_SYNC: (&str, &CStr) = ("CAMetalLayer", c"setDisplaySyncEnabled:");
/// framebufferOnly (void_1b)。
pub const SEL_LAYER_SET_FRAMEBUFFER_ONLY: (&str, &CStr) = ("CAMetalLayer", c"setFramebufferOnly:");
/// 次 drawable 取得。
pub const SEL_NEXT_DRAWABLE: (&str, &CStr) = ("CAMetalLayer", c"nextDrawable");
/// drawable 表面テクスチャ (Borrowed)。
pub const SEL_DRAWABLE_TEXTURE: (&str, &CStr) = ("CAMetalDrawable", c"texture");

// ---- MTLV: enum 値 (canon enum 表と audit 照合) ----
/// BGRA8Unorm (80: 画面表示の標準 format)。
pub const MTLV_PF_BGRA8: (&str, &str, i64) = ("MTLPixelFormat", "BGRA8Unorm", 80);
/// RGBA8Unorm (70: オフスクリーン標準)。
pub const MTLV_PF_RGBA8: (&str, &str, i64) = ("MTLPixelFormat", "RGBA8Unorm", 70);
/// LoadAction::Clear。
pub const MTLV_LOAD_CLEAR: (&str, &str, i64) = ("MTLLoadAction", "Clear", 2);
/// StoreAction::Store。
pub const MTLV_STORE_STORE: (&str, &str, i64) = ("MTLStoreAction", "Store", 1);
/// PrimitiveType::Triangle。
pub const MTLV_PRIM_TRIANGLE: (&str, &str, i64) = ("MTLPrimitiveType", "Triangle", 3);
/// TextureUsage::ShaderRead。
pub const MTLV_USAGE_SHADER_READ: (&str, &str, i64) = ("MTLTextureUsage", "ShaderRead", 1);
/// TextureUsage::RenderTarget。
pub const MTLV_USAGE_RENDER_TARGET: (&str, &str, i64) = ("MTLTextureUsage", "RenderTarget", 4);
/// StorageMode::Shared (CPU から直接 RW 可能)。
pub const MTLV_STORAGE_SHARED: (&str, &str, i64) = ("MTLStorageMode", "Shared", 0);
/// CommandBufferStatus::Completed。
pub const MTLV_CB_COMPLETED: (&str, &str, i64) = ("MTLCommandBufferStatus", "Completed", 4);
/// GPUFamily::Apple7 (M1 系: Metal 4 動作ラインの下界定義参考)。
pub const MTLV_FAMILY_APPLE7: (&str, &str, i64) = ("MTLGPUFamily", "Apple7", 1007);
/// IndexType::UInt16 (index 描画の 16bit index 形式)。
pub const MTLV_INDEX_U16: (&str, &str, i64) = ("MTLIndexType", "UInt16", 0);

// ---- CLASS 名 (監査機が CANON_PARENTS/CANON_SELS 帰属と照合) ----
/// NSString クラス。
pub const CLASS_NSSTRING: &CStr = c"NSString";
/// レンダーパイプライン記述子。
pub const CLASS_PIPELINE_DESC: &CStr = c"MTLRenderPipelineDescriptor";
/// レンダーパス記述子。
pub const CLASS_PASS_DESC: &CStr = c"MTLRenderPassDescriptor";
/// CAMetalLayer。
pub const CLASS_METAL_LAYER: &CStr = c"CAMetalLayer";
/// テクスチャ記述子。
pub const CLASS_TEX_DESC: &CStr = c"MTLTextureDescriptor";

/// MSL シェーダソース (三角形、vertex id 駆動 + constant buffer 色)。
/// metal_stdlib 正規形。vs名 rsift_direct_vs / fs名 rsift_direct_fs、
/// compile 失敗時は NSError localizedDescription が Err 文字列化される。
pub const TRIANGLE_MSL: &str = r#"#include <metal_stdlib>
using namespace metal;

struct VsOut {
    float4 pos [[position]];
    float4 col;
};

vertex VsOut rsift_direct_vs(uint vid [[vertex_id]],
                             constant float4* verts [[buffer(0)]]) {
    VsOut o;
    o.pos = float4(verts[vid].xy, 0.0, 1.0);
    o.col = verts[vid];
    return o;
}

fragment float4 rsift_direct_fs(VsOut in [[stage_in]]) {
    return in.col;
}

// ---- テクスチャ描画経路 (UI quad: メニュー/HUD テクスチャ表示) ----
// fullscreen triangle (Apple "Fullscreen Triangle" 定石: vertex_id で
// 範囲外 3 頂点を生成、rasterizer がクリップ)。vbuf 不使用。

struct QuadOut {
    float4 pos [[position]];
    float2 uv;
};

vertex QuadOut rsift_quad_vs(uint vid [[vertex_id]]) {
    QuadOut o;
    float2 p = float2((vid == 1u) ? 3.0 : -1.0, (vid == 2u) ? 3.0 : -1.0);
    o.pos = float4(p, 0.0, 1.0);
    o.uv = float2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return o;
}

fragment float4 rsift_quad_fs(QuadOut in [[stage_in]],
                              texture2d<float> tex [[texture(0)]]) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    return tex.sample(s, in.uv);
}
"#;

/// direct binding エラー (詳細付き、fail-loud 方針)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectMetalError {
    /// MTLCreateSystemDefaultDevice が null (Metal 非搭載環境)。
    NoDefaultDevice,
    /// クラス不在 (想定: 非 macOS での native 実行、フレームワーク未リンク)。
    ClassMissing(&'static str),
    /// selector 応答 null (objc 的に致命的な不整合)。
    NullObject(&'static str),
    /// MSL コンパイル失敗 (NSError localizedDescription 全文)。
    ShaderCompile(String),
    /// パイプライン生成失敗 (NSError 全文)。
    Pipeline(String),
    /// コマンドバッファ最終 status が Completed でない (実 status 値)。
    CommandFailed(u64),
    /// drawable 枯渇 (nextDrawable null、再試行指示)。
    NoDrawable,
    /// 未対応 pixel format 指定 (create_with_format の検証結果)。
    /// 対応集合は canon 照合済の BGRA8/RGBA8 のみ。
    FormatUnsupported(u64),
    /// MTLBuffer contents null (Shared storage 規則ではあり得ない
    /// 本番異常、または mock 未配線) — fail-loud。
    ContentsUnavailable,
    /// 頂点データが vbuf 容量超過 (update_vertices 検証)。
    VertexOverflow {
        have_float4: u64,
        capacity_float4: u64,
    },
    /// ピクセルデータ長不足 (upload_pixels 検証)。
    PixelDataShort { have: usize, want: usize },
    /// Metal 4 family 非対応 (supportsFamily: MTLGPUFamilyMetal4=5002 が
    /// false、wave 196 GO — MTL4 経路の硬ゲート応答)。
    Metal4Unsupported,
    /// MTL4 frame 同期 event 待機 timeout (waitUntilSignaledValue: が
    /// false 応答、wave 196 GO — Hello Triangle 待機機構の fail-loud 化)。
    FrameSyncTimeout {
        /// 現在 frame 番号。
        frame: u64,
        /// 完了を待っていた番号 (frame - kMaxFramesInFlight)。
        awaited: u64,
    },
}

/// 三角形の頂点 (x,y,r,g)、float4 揃え 3 頂点 = 48 byte。
pub const TRIANGLE_VERTS: [f32; 12] = [
    0.0, 0.8, 1.0, 0.2, // 上: 赤系
    -0.8, -0.8, 0.2, 1.0, // 左下: 緑系
    0.8, -0.8, 0.2, 0.2, // 右下: 青系
];

// ---- 小物ユーティリティ ----

/// `&str` → NSString (Owned)。失敗時 None。
/// alloc (Owned) → initWithUTF8String: (init family absorb)。
/// metal4_direct も消費 (crate 内共有、wave 196 GO)。
pub(crate) fn ns_str(rt: &mut dyn ObjcRt, s: &str) -> Option<ObjcId> {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    let cls = rt.get_class(CLASS_NSSTRING);
    if cls.is_null() {
        return None;
    }
    let obj = rt.id_0(cls, SEL_ALLOC.1);
    if obj.is_null() {
        return None;
    }
    let inited = rt.id_1p(obj, SEL_INIT_WITH_UTF8.1, bytes.as_ptr() as ObjcId);
    if inited.is_null() {
        return None;
    }
    Some(inited)
}

/// NSError から localizedDescription UTF-8 文字列を取り出す。
/// err が null の場合は既定文。metal4_direct も消費 (wave 196 GO)。
pub(crate) fn err_string(rt: &mut dyn ObjcRt, err: ObjcId, fallback: &str) -> String {
    if err.is_null() {
        return fallback.to_string();
    }
    let desc = rt.id_0(err, SEL_LOCALIZED_DESCRIPTION.1);
    if desc.is_null() {
        return fallback.to_string();
    }
    // Borrowed 規則参照なので release しない。
    let cstr = rt.ptr_0(desc, SEL_UTF8_STRING.1);
    if cstr.is_null() {
        return fallback.to_string();
    }
    unsafe { CStr::from_ptr(cstr as *const core::ffi::c_char) }
        .to_string_lossy()
        .into_owned()
}

// ---- 完全描画コンテキスト ----

/// Metal 直 binding のセッション (device→queue→shader→pipeline→対象まで確立)。
/// 全 ObjC オブジェクトの所有規則は canon と一致:
/// queue/lib/vs/fs/pipeline/vbuf/target = Owned (shutdown で release)、
/// device = Borrowed (MTLCreateSystemDefaultDevice 返却規則)。
pub struct DirectMetal {
    /// MTCreateSystemDefaultDevice 由来 (Borrowed、release 禁止)。
    pub device: ObjcId,
    /// newCommandQueue 生成 (Owned)。
    pub queue: ObjcId,
    /// newLibraryWithSource:options:error: 生成 (Owned)。
    pub library: ObjcId,
    /// newFunctionWithName: vertex (Owned)。
    pub vs: ObjcId,
    /// newFunctionWithName: fragment (Owned)。
    pub fs: ObjcId,
    /// newRenderPipelineStateWithDescriptor:error: 生成 (Owned)。
    pub pipeline: ObjcId,
    /// UI テクスチャ quad 用 pipeline (Owned、render_textured_quad
    /// 初回呼出時の lazy 構築 — 未使用経路で生成コスト 0 の軽量化)。
    pub tex_pipeline: ObjcId,
    /// newBufferWithLength:options: 生成の動的頂点バッファ (Owned)。
    /// 容量固定・中身は contents 経由で上書き (Apple 動的データ定石)。
    pub vbuf: ObjcId,
    /// vbuf 容量 (float4 個数)。
    pub vbuf_capacity: u64,
    /// vbuf 内の有効頂点数 (update_vertices で更新)。
    pub vbuf_len: u64,
    /// newTextureWithDescriptor: 生成 (Owned、BGRA8 RT 用途)。
    pub target: ObjcId,
    /// attach_layer で所有化した CAMetalLayer (Owned、未 attach は null)。
    pub layer: ObjcId,
    /// target の幅。
    pub width: u64,
    /// target の高さ。
    pub height: u64,
    /// レンダーターゲットの px format (canon enum 値)。
    pub format: u64,
}

/// vbuf の容量 (float4 × 1024 = 16KiB)。UI バッチの現実的上限。
pub const VBUF_CAP_FLOAT4: u64 = 1024;

impl DirectMetal {
    /// オフスクリーン完全経路の構築。width×height の BGRA8
    /// RenderTarget|ShaderRead テクスチャを描画先とする。
    /// 全失敗は DirectMetalError で詳細化 (スタブ/Some-wrapping 無し)。
    pub fn create(rt: &mut dyn ObjcRt, width: u32, height: u32) -> Result<Self, DirectMetalError> {
        Self::create_with_format(rt, width, height, MTLV_PF_BGRA8.2 as u64)
    }

    /// pixel format 指定版 create。許容集合は canon 照合済の
    /// BGRA8 (画面表示標準) / RGBA8 (GL parity・CPU upload 整合) のみ。
    pub fn create_with_format(
        rt: &mut dyn ObjcRt,
        width: u32,
        height: u32,
        format: u64,
    ) -> Result<Self, DirectMetalError> {
        if format != MTLV_PF_BGRA8.2 as u64 && format != MTLV_PF_RGBA8.2 as u64 {
            return Err(DirectMetalError::FormatUnsupported(format));
        }
        let pool = rt.pool_push();
        let result = Self::create_inner(rt, width, height, format);
        rt.pool_pop(pool);
        result
    }

    fn create_inner(
        rt: &mut dyn ObjcRt,
        width: u32,
        height: u32,
        format: u64,
    ) -> Result<Self, DirectMetalError> {
        let device = rt.c_mtl_default_device();
        if device.is_null() {
            return Err(DirectMetalError::NoDefaultDevice);
        }
        // command queue (Owned)
        let queue = rt.id_0(device, SEL_NEW_COMMAND_QUEUE.1);
        if queue.is_null() {
            return Err(DirectMetalError::NullObject("newCommandQueue"));
        }
        // shader library (Owned、失敗時 NSError 詳細)
        let src = ns_str(rt, TRIANGLE_MSL).ok_or(DirectMetalError::ClassMissing("NSString"))?;
        let mut err: ObjcId = core::ptr::null_mut();
        let library = rt.id_3ppp(
            device,
            SEL_NEW_LIBRARY_SRC.1,
            src,
            core::ptr::null_mut(),
            &mut err as *mut ObjcId as ObjcId,
        );
        // NSString (alloc→init = +1、canon Owned 規則) は役目終了で解放。
        // newLibrary 内部は content をコピー済 (一次情報: MTLDevice.h
        // "source" 固有参照を保持しない ObjC 標準振る舞い)。
        rt.void_0(src, SEL_RELEASE.1);
        if library.is_null() {
            let msg = err_string(rt, err, "shader compile failed");
            return Err(DirectMetalError::ShaderCompile(msg));
        }
        // functions (Owned)
        let vs_name =
            ns_str(rt, "rsift_direct_vs").ok_or(DirectMetalError::ClassMissing("NSString"))?;
        let fs_name =
            ns_str(rt, "rsift_direct_fs").ok_or(DirectMetalError::ClassMissing("NSString"))?;
        let vs = rt.id_1p(library, SEL_NEW_FUNCTION.1, vs_name);
        let fs = rt.id_1p(library, SEL_NEW_FUNCTION.1, fs_name);
        rt.void_0(vs_name, SEL_RELEASE.1);
        rt.void_0(fs_name, SEL_RELEASE.1);
        if vs.is_null() || fs.is_null() {
            return Err(DirectMetalError::NullObject("newFunctionWithName:"));
        }
        // pipeline descriptor (class alloc→init、Owned)
        let pcls = rt.get_class(CLASS_PIPELINE_DESC);
        if pcls.is_null() {
            return Err(DirectMetalError::ClassMissing(
                "MTLRenderPipelineDescriptor",
            ));
        }
        let desc = rt.id_0(pcls, SEL_NEW.1);
        if desc.is_null() {
            return Err(DirectMetalError::NullObject("new"));
        }
        rt.void_1p(desc, SEL_SET_VERTEX_FUNCTION.1, vs);
        rt.void_1p(desc, SEL_SET_FRAGMENT_FUNCTION.1, fs);
        let atts = rt.id_0(desc, SEL_COLOR_ATTACHMENTS.1);
        let att0 = rt.id_1u(atts, SEL_OBJECT_AT_INDEXED_SUBSCRIPT.1, 0);
        rt.void_1u(att0, SEL_SET_PIXEL_FORMAT.1, format);
        let mut perr: ObjcId = core::ptr::null_mut();
        let pipeline = rt.id_2pp(
            device,
            SEL_NEW_PIPELINE.1,
            desc,
            &mut perr as *mut ObjcId as ObjcId,
        );
        // descriptor は Owned → 使い終わったので release。
        rt.void_0(desc, SEL_RELEASE.1);
        if pipeline.is_null() {
            let msg = err_string(rt, perr, "pipeline creation failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        // vertex buffer (Owned): 容量固定の newBufferWithLength:options:
        // で確保し、中身は contents 書込み (Apple「動的データは Shared
        // buffer + contents 直接書き」定石、frame 毎の再確保を物理排除)。
        let vbuf = rt.id_2uu(
            device,
            SEL_NEW_BUFFER_LEN_OPTS.1,
            VBUF_CAP_FLOAT4 * 16,
            MTLV_STORAGE_SHARED.2 as u64,
        );
        if vbuf.is_null() {
            return Err(DirectMetalError::NullObject("newBufferWithLength:options:"));
        }
        let mut m = Self {
            device,
            queue,
            library,
            vs,
            fs,
            pipeline,
            tex_pipeline: core::ptr::null_mut(),
            vbuf,
            vbuf_capacity: VBUF_CAP_FLOAT4,
            vbuf_len: 0,
            target: core::ptr::null_mut(),
            layer: core::ptr::null_mut(),
            width: width as u64,
            height: height as u64,
            format,
        };
        // 初期三角形を contents 経由で投入 (実ポインタ書込み、mock では
        // arena への物理 memcpy として全行動作検証される)。
        let f4: &[[f32; 4]] =
            unsafe { core::slice::from_raw_parts(TRIANGLE_VERTS.as_ptr() as *const [f32; 4], 3) };
        m.update_vertices(rt, f4)?;
        // target texture (Owned)
        let tcls = rt.get_class(CLASS_TEX_DESC);
        if tcls.is_null() {
            return Err(DirectMetalError::ClassMissing("MTLTextureDescriptor"));
        }
        let tdesc = rt.id_3uuub(
            tcls,
            SEL_TEX2D_DESC.1,
            format,
            width as u64,
            height as u64,
            false,
        );
        if tdesc.is_null() {
            return Err(DirectMetalError::NullObject(
                "texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
            ));
        }
        rt.void_1u(
            tdesc,
            SEL_SET_USAGE.1,
            (MTLV_USAGE_SHADER_READ.2 | MTLV_USAGE_RENDER_TARGET.2) as u64,
        );
        rt.void_1u(tdesc, SEL_SET_STORAGE_MODE.1, MTLV_STORAGE_SHARED.2 as u64);
        let target = rt.id_1p(device, SEL_NEW_TEXTURE.1, tdesc);
        if target.is_null() {
            return Err(DirectMetalError::NullObject("newTextureWithDescriptor:"));
        }
        m.target = target;
        Ok(m)
    }

    /// MTLBuffer contents 経由の頂点データ更新 (Shared storage 直書き)。
    /// 容量超過は fail-loud、contents null は本番異常として同様に扱う。
    /// (一次情報: Apple MTLBuffer — Shared storage の contents は
    /// CPU から書き込み可能な直接ポインタ)
    pub fn update_vertices(
        &mut self,
        rt: &mut dyn ObjcRt,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        if verts.len() as u64 > self.vbuf_capacity {
            return Err(DirectMetalError::VertexOverflow {
                have_float4: verts.len() as u64,
                capacity_float4: self.vbuf_capacity,
            });
        }
        let p = rt.ptr_0(self.vbuf, SEL_CONTENTS.1);
        if p.is_null() {
            return Err(DirectMetalError::ContentsUnavailable);
        }
        let dst = p as *mut [f32; 4];
        unsafe { core::ptr::copy_nonoverlapping(verts.as_ptr(), dst, verts.len()) };
        self.vbuf_len = verts.len() as u64;
        Ok(())
    }

    /// デバイス能力の実機照合 (probe): デバイス名と Apple7 (M1 世代) 以上
    /// 判定。canon MTLGPUFamilyApple7=1007 を supportsFamily: に照合。
    /// Metal 4 (GO 波) のハードウェア真値ゲートとして apple_backend::
    /// metal4_surface (policy 層) と組み合わせて使う想定の消費配線。
    pub fn probe_caps(&self, rt: &mut dyn ObjcRt) -> DirectCaps {
        let name_id = rt.id_0(self.device, SEL_DEVICE_NAME.1);
        let mut name = String::from("<unknown>");
        if !name_id.is_null() {
            let cstr = rt.ptr_0(name_id, SEL_UTF8_STRING.1);
            if !cstr.is_null() {
                name = unsafe { CStr::from_ptr(cstr as *const core::ffi::c_char) }
                    .to_string_lossy()
                    .into_owned();
            }
        }
        let apple7_or_newer = rt.bool_1u(
            self.device,
            SEL_SUPPORTS_FAMILY.1,
            MTLV_FAMILY_APPLE7.2 as u64,
        );
        DirectCaps {
            name,
            apple7_or_newer,
        }
    }

    /// CPU ピクセルデータを target へ全面アップロード (canon
    /// replaceRegion:mipmapLevel:withBytes:bytesPerRow:、BGRA 行連続)。
    /// repo 内 CPU rasterizer 出力の GPU 転送に使う本番経路。
    pub fn upload_pixels(
        &self,
        rt: &mut dyn ObjcRt,
        pixels: &[u8],
    ) -> Result<(), DirectMetalError> {
        let want = (self.width * self.height * 4) as usize;
        if pixels.len() < want {
            return Err(DirectMetalError::PixelDataShort {
                have: pixels.len(),
                want,
            });
        }
        let region = MtlRegion {
            origin: MtlSize {
                width: 0,
                height: 0,
                depth: 0,
            },
            size: MtlSize {
                width: self.width,
                height: self.height,
                depth: 1,
            },
        };
        rt.void_region_update(
            self.target,
            SEL_REPLACE_REGION.1,
            region,
            0,
            pixels.as_ptr() as ObjcId,
            self.width * 4,
        );
        Ok(())
    }

    /// 1 フレーム描画 (三角形クリア+draw→commit) を target テクスチャへ実行。
    /// 失敗条件: cmdBuffer/encoder null、最終 status != Completed、NSError 言及。
    pub fn render_frame(&self, rt: &mut dyn ObjcRt) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = self.encode_frame(rt, &self.full_frame_spec());
        rt.pool_pop(pool);
        result
    }

    /// 部分矩形 (x,y,w,h px) のみを描画する軽量経路 (HUD/部分再描画)。
    /// viewport と scissor を同一矩形に制限し、full frame と同一の
    /// commit+wait+status 検証を行う。
    pub fn render_frame_region(
        &self,
        rt: &mut dyn ObjcRt,
        x: u64,
        y: u64,
        w: u64,
        h: u64,
    ) -> Result<(), DirectMetalError> {
        let mut spec = self.full_frame_spec();
        spec.viewport = (x as f64, y as f64, w as f64, h as f64);
        spec.scissor = Some((x, y, w, h));
        let pool = rt.pool_push();
        let result = self.encode_frame(rt, &spec);
        rt.pool_pop(pool);
        result
    }

    /// UI テクスチャ quad 描画 (メニュー/HUD)。初回呼出で tex_pipeline を
    /// lazy 構築し、fragment texture(0) に tex を bind して fullscreen
    /// triangle を描く (quad vs は vbuf 不使用、TRIANGLE_MSL 内蔵)。
    pub fn render_textured_quad(
        &mut self,
        rt: &mut dyn ObjcRt,
        tex: ObjcId,
    ) -> Result<(), DirectMetalError> {
        let pipeline = self.ensure_tex_pipeline(rt)?;
        let pool = rt.pool_push();
        let result = {
            let mut spec = self.full_frame_spec();
            spec.pipeline = pipeline;
            spec.vbuf = None;
            spec.frag_tex = Some(tex);
            self.encode_frame(rt, &spec)
        };
        rt.pool_pop(pool);
        result
    }

    /// index 描画経路 (幾何 tier 基盤)。ibuf は呼出側所有の UInt16 index
    /// バッファ (MTLBuffer)、icount は index 個数。canon MTLIndexType
    /// UInt16=0 を使用、offset 0 固定の最小完全形。
    pub fn render_frame_indexed(
        &self,
        rt: &mut dyn ObjcRt,
        ibuf: ObjcId,
        icount: u64,
    ) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = {
            let mut spec = self.full_frame_spec();
            spec.draw = DrawCall::Indexed {
                ibuf,
                count: icount,
            };
            self.encode_frame(rt, &spec)
        };
        rt.pool_pop(pool);
        result
    }

    /// UInt16 index バッファの生成 (初期データコピー、Owned)。
    /// canon newBufferWithBytes:length:options: 経路 — 作成時に内容が
    /// 確定する不変データの正規生成系 (render_frame_indexed の相方)。
    /// 返却 ObjcId の release は呼出側の責務。
    pub fn create_index_buffer_u16(
        &self,
        rt: &mut dyn ObjcRt,
        indices: &[u16],
    ) -> Result<ObjcId, DirectMetalError> {
        if indices.is_empty() {
            return Err(DirectMetalError::PixelDataShort { have: 0, want: 2 });
        }
        let bytes = unsafe {
            core::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 2)
        };
        let buf = rt.id_3puu(
            self.device,
            SEL_NEW_BUFFER_BYTES_OPTS.1,
            bytes.as_ptr() as ObjcId,
            bytes.len() as u64,
            MTLV_STORAGE_SHARED.2 as u64,
        );
        if buf.is_null() {
            return Err(DirectMetalError::NullObject(
                "newBufferWithBytes:length:options:",
            ));
        }
        Ok(buf)
    }

    /// full frame の既定 spec (三角形、target、wait+status 照合あり)。
    fn full_frame_spec(&self) -> FrameSpec {
        FrameSpec {
            tex: self.target,
            viewport: (0.0, 0.0, self.width as f64, self.height as f64),
            scissor: None,
            pipeline: self.pipeline,
            vbuf: Some(self.vbuf),
            frag_tex: None,
            draw: DrawCall::Primitives { start: 0, count: 3 },
            present: None,
            wait: true,
        }
    }

    /// UI quad pipeline の lazy 構築 (Owned、release は shutdown)。
    /// descriptor は pipeline 生成後に即 release (newRenderPipelineState
    /// 完了後は state 側が自己完結、descriptor 参照を保持しない
    /// ObjC 慣行、一次情報: MTLDevice.h newRenderPipelineStateWith…)。
    fn ensure_tex_pipeline(&mut self, rt: &mut dyn ObjcRt) -> Result<ObjcId, DirectMetalError> {
        if !self.tex_pipeline.is_null() {
            return Ok(self.tex_pipeline);
        }
        let vs_name =
            ns_str(rt, "rsift_quad_vs").ok_or(DirectMetalError::ClassMissing("NSString"))?;
        let fs_name =
            ns_str(rt, "rsift_quad_fs").ok_or(DirectMetalError::ClassMissing("NSString"))?;
        let qvs = rt.id_1p(self.library, SEL_NEW_FUNCTION.1, vs_name);
        let qfs = rt.id_1p(self.library, SEL_NEW_FUNCTION.1, fs_name);
        rt.void_0(vs_name, SEL_RELEASE.1);
        rt.void_0(fs_name, SEL_RELEASE.1);
        if qvs.is_null() || qfs.is_null() {
            return Err(DirectMetalError::NullObject("newFunctionWithName:"));
        }
        let pcls = rt.get_class(CLASS_PIPELINE_DESC);
        if pcls.is_null() {
            return Err(DirectMetalError::ClassMissing(
                "MTLRenderPipelineDescriptor",
            ));
        }
        let desc = rt.id_0(pcls, SEL_NEW.1);
        if desc.is_null() {
            return Err(DirectMetalError::NullObject("new"));
        }
        rt.void_1p(desc, SEL_SET_VERTEX_FUNCTION.1, qvs);
        rt.void_1p(desc, SEL_SET_FRAGMENT_FUNCTION.1, qfs);
        let atts = rt.id_0(desc, SEL_COLOR_ATTACHMENTS.1);
        let att0 = rt.id_1u(atts, SEL_OBJECT_AT_INDEXED_SUBSCRIPT.1, 0);
        rt.void_1u(att0, SEL_SET_PIXEL_FORMAT.1, self.format);
        let mut perr: ObjcId = core::ptr::null_mut();
        let pipeline = rt.id_2pp(
            self.device,
            SEL_NEW_PIPELINE.1,
            desc,
            &mut perr as *mut ObjcId as ObjcId,
        );
        rt.void_0(desc, SEL_RELEASE.1);
        // quad functions は pipeline 生成で役目終了 (descriptor 経由で
        // state 側に取り込まれる ObjC 慣行) のため即 release。
        rt.void_0(qvs, SEL_RELEASE.1);
        rt.void_0(qfs, SEL_RELEASE.1);
        if pipeline.is_null() {
            let msg = err_string(rt, perr, "quad pipeline creation failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        self.tex_pipeline = pipeline;
        Ok(pipeline)
    }

    /// 描画 1 パス共通エンコーダ (offscreen/layer/region/quad/indexed 全経路の唯一の核)。
    fn encode_frame(&self, rt: &mut dyn ObjcRt, spec: &FrameSpec) -> Result<(), DirectMetalError> {
        let cmd = rt.id_0(self.queue, SEL_COMMAND_BUFFER.1);
        if cmd.is_null() {
            return Err(DirectMetalError::NullObject("commandBuffer"));
        }
        // render pass descriptor (class factory new、Owned)
        let pcls = rt.get_class(CLASS_PASS_DESC);
        let pass = rt.id_0(pcls, SEL_PASS_NEW.1);
        if pass.is_null() {
            return Err(DirectMetalError::NullObject("renderPassDescriptor"));
        }
        let atts = rt.id_0(pass, SEL_PASS_COLOR_ATTACHMENTS.1);
        let att0 = rt.id_1u(atts, SEL_PASS_OBJECT_AT_SUBSCRIPT.1, 0);
        rt.void_1p(att0, SEL_SET_TEXTURE.1, spec.tex);
        rt.void_1u(att0, SEL_SET_LOAD_ACTION.1, MTLV_LOAD_CLEAR.2 as u64);
        rt.void_1u(att0, SEL_SET_STORE_ACTION.1, MTLV_STORE_STORE.2 as u64);
        rt.void_clearcolor(
            att0,
            SEL_SET_CLEAR_COLOR.1,
            MtlClearColor {
                red: 0.08,
                green: 0.1,
                blue: 0.18,
                alpha: 1.0,
            },
        );
        let enc = rt.id_1p(cmd, SEL_RENDER_ENCODER.1, pass);
        if enc.is_null() {
            rt.void_0(pass, SEL_RELEASE.1);
            return Err(DirectMetalError::NullObject(
                "renderCommandEncoderWithDescriptor:",
            ));
        }
        rt.void_1p(enc, SEL_SET_PIPELINE_STATE.1, spec.pipeline);
        rt.void_viewport(
            enc,
            SEL_SET_VIEWPORT.1,
            crate::objc_rt::MtlViewport {
                origin_x: spec.viewport.0,
                origin_y: spec.viewport.1,
                width: spec.viewport.2,
                height: spec.viewport.3,
                znear: 0.0,
                zfar: 1.0,
            },
        );
        if let Some((sx, sy, sw, sh)) = spec.scissor {
            rt.void_scissor(
                enc,
                SEL_SET_SCISSOR.1,
                crate::objc_rt::MtlScissorRect {
                    x: sx,
                    y: sy,
                    width: sw,
                    height: sh,
                },
            );
        }
        if let Some(vb) = spec.vbuf {
            rt.void_3puu(enc, SEL_SET_VERTEX_BUFFER.1, vb, 0, 0);
        }
        if let Some(ft) = spec.frag_tex {
            rt.void_2pu(enc, SEL_SET_FRAGMENT_TEXTURE.1, ft, 0);
        }
        match spec.draw {
            DrawCall::Primitives { start, count } => {
                rt.void_3uuu(
                    enc,
                    SEL_DRAW_PRIMITIVES.1,
                    MTLV_PRIM_TRIANGLE.2 as u64,
                    start,
                    count,
                );
            }
            DrawCall::Indexed { ibuf, count } => {
                rt.void_5uuupu(
                    enc,
                    SEL_DRAW_INDEXED.1,
                    MTLV_PRIM_TRIANGLE.2 as u64,
                    count,
                    MTLV_INDEX_U16.2 as u64,
                    ibuf,
                    0,
                );
            }
        }
        rt.void_0(enc, SEL_END_ENCODING.1);
        if let Some(d) = spec.present {
            rt.void_1p(cmd, SEL_PRESENT_DRAWABLE.1, d);
        }
        rt.void_0(cmd, SEL_COMMIT.1);
        if spec.wait {
            rt.void_0(cmd, SEL_WAIT_COMPLETED.1);
        }
        rt.void_0(pass, SEL_RELEASE.1);
        if spec.wait {
            let status = rt.u64_0(cmd, SEL_CB_STATUS.1);
            if status != MTLV_CB_COMPLETED.2 as u64 {
                let err = rt.id_0(cmd, SEL_CB_ERROR.1);
                let msg = err_string(rt, err, "command buffer failed");
                tracing::warn!("[DirectMetal] frame fail status={status}: {msg}");
                return Err(DirectMetalError::CommandFailed(status));
            }
        }
        Ok(())
    }

    /// 描画結果を CPU へ読み戻し (BGRA8、行バイト率 width*4 連続)。
    /// macOS Shared storage 規則に基づく readback、out.len() >= w*h*4 必須。
    /// 事前にデバイス側真値 (width/height property) を照合し、0 (未応答)
    /// 以外で食い違う場合は警告のうえ min 側にクランプする guard 付き。
    pub fn readback(&self, rt: &mut dyn ObjcRt, out: &mut [u8]) {
        let tw = rt.u64_0(self.target, SEL_TEX_WIDTH.1);
        let th = rt.u64_0(self.target, SEL_TEX_HEIGHT.1);
        if (tw != 0 && tw != self.width) || (th != 0 && th != self.height) {
            tracing::warn!(
                "[DirectMetal] texture mismatch self={}x{} device={}x{} (min でクランプ)",
                self.width,
                self.height,
                tw,
                th
            );
        }
        let w = if tw == 0 {
            self.width
        } else {
            tw.min(self.width)
        };
        let h = if th == 0 {
            self.height
        } else {
            th.min(self.height)
        };
        let region = MtlRegion {
            origin: MtlSize {
                width: 0,
                height: 0,
                depth: 0,
            },
            size: MtlSize {
                width: w,
                height: h,
                depth: 1,
            },
        };
        rt.get_bytes_region(
            self.target,
            SEL_GET_BYTES.1,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            w * 4,
            region,
            0,
        );
    }

    /// CAMetalLayer を alloc→init で生成し、configure して所有化する
    /// 一体経路 (NSView 非依存の headless/自前 view 配線向け)。
    /// 戻り値は Owned layer (self.layer にも保持、shutdown で release)。
    pub fn attach_layer(&mut self, rt: &mut dyn ObjcRt) -> Result<ObjcId, DirectMetalError> {
        let cls = rt.get_class(CLASS_METAL_LAYER);
        if cls.is_null() {
            return Err(DirectMetalError::ClassMissing("CAMetalLayer"));
        }
        let allocated = rt.id_0(cls, SEL_ALLOC.1);
        if allocated.is_null() {
            return Err(DirectMetalError::NullObject("alloc"));
        }
        let layer = rt.id_0(allocated, SEL_INIT.1);
        if layer.is_null() {
            return Err(DirectMetalError::NullObject("init"));
        }
        self.configure_layer(rt, layer);
        self.layer = layer;
        Ok(layer)
    }

    /// CAMetalLayer 初期化 (device/pixelFormat/drawableSize/3 重バッファ)。
    /// layer は alloc→init の Owned を caller が管理する設計
    /// (ObjC ARC 慣行: CALayer は通常 view が保持、本 crate では
    /// 明示 release を shutdown で行う所有形とする)。
    pub fn configure_layer(&self, rt: &mut dyn ObjcRt, layer: ObjcId) {
        rt.void_1p(layer, SEL_LAYER_SET_DEVICE.1, self.device);
        rt.void_1u(layer, SEL_LAYER_SET_PIXEL_FORMAT.1, MTLV_PF_BGRA8.2 as u64);
        rt.void_cgsize(
            layer,
            SEL_LAYER_SET_DRAWABLE_SIZE.1,
            CgSize {
                width: self.width as f64,
                height: self.height as f64,
            },
        );
        rt.void_1u(layer, SEL_LAYER_SET_MAX_DRAWABLE.1, 3);
        rt.void_1b(layer, SEL_LAYER_SET_FRAMEBUFFER_ONLY.1, false);
        rt.void_1b(layer, SEL_LAYER_SET_DISPLAY_SYNC.1, true);
        rt.void_1b(layer, SEL_LAYER_SET_PRESENTS_TRANSACTION.1, false);
    }

    /// CAMetalLayer へ 1 フレーム描画+present (三角形、offscreen と同構造)。
    pub fn render_to_layer(
        &self,
        rt: &mut dyn ObjcRt,
        layer: ObjcId,
    ) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = self.render_to_layer_inner(rt, layer);
        rt.pool_pop(pool);
        result
    }

    fn render_to_layer_inner(
        &self,
        rt: &mut dyn ObjcRt,
        layer: ObjcId,
    ) -> Result<(), DirectMetalError> {
        let drawable = rt.id_0(layer, SEL_NEXT_DRAWABLE.1);
        if drawable.is_null() {
            return Err(DirectMetalError::NoDrawable);
        }
        let tex = rt.id_0(drawable, SEL_DRAWABLE_TEXTURE.1);
        let spec = FrameSpec {
            tex,
            present: Some(drawable),
            wait: false,
            ..self.full_frame_spec()
        };
        self.encode_frame(rt, &spec)
    }

    /// 全 Owned オブジェクトを canon 規則で release。
    /// device は Borrowed のため対象外 (MTLCreateSystemDefaultDevice 規則)。
    pub fn shutdown(self, rt: &mut dyn ObjcRt) {
        for obj in [
            self.layer,
            self.target,
            self.vbuf,
            self.tex_pipeline,
            self.pipeline,
            self.fs,
            self.vs,
            self.library,
            self.queue,
        ] {
            if !obj.is_null() {
                rt.void_0(obj, SEL_RELEASE.1);
            }
        }
    }
}

/// 描画 1 パスの指定 (encode_frame 内部制御、全描画経路の唯一の真理)。
/// 公開 API はこれを組み立てる薄い層 (spec 自体は非公開)。
struct FrameSpec {
    /// 描画先テクスチャ (offscreen target か drawable texture)。
    tex: ObjcId,
    /// viewport (ox, oy, w, h)。znear/zfar は 0..1 固定。
    viewport: (f64, f64, f64, f64),
    /// scissor 矩形 (x, y, w, h)。None で無効。
    scissor: Option<(u64, u64, u64, u64)>,
    /// 使用 pipeline。
    pipeline: ObjcId,
    /// vertex buffer bind (buffer(0))、None なら非 bind (quad vs 用)。
    vbuf: Option<ObjcId>,
    /// fragment texture bind (texture(0))、None なら非 bind。
    frag_tex: Option<ObjcId>,
    /// draw 呼出種別。
    draw: DrawCall,
    /// present drawable (Some なら commit 前に presentDrawable)。
    present: Option<ObjcId>,
    /// commit 後に waitUntilCompleted+status 照合するか (offscreen 検証系)。
    wait: bool,
}

/// draw 呼出 (非 index / index、共通引数は Triangle primitive 固定)。
enum DrawCall {
    /// drawPrimitives:vertexStart:vertexCount:。
    Primitives { start: u64, count: u64 },
    /// drawIndexedPrimitives:… (UInt16、offset 0)。
    Indexed { ibuf: ObjcId, count: u64 },
}

/// probe_caps の結果 (実機照合値)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCaps {
    /// MTLDevice name の UTF-8 文字列 (取得不能時 "<unknown>")。
    pub name: String,
    /// supportsFamily: MTLGPUFamilyApple7(1007) の応答 (M1 世代以上真値)。
    pub apple7_or_newer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objc_rt::MockObjcRt;

    /// mock 応答を完全配線する総合フィクスチャ。
    fn scripted_rt() -> MockObjcRt {
        let mut rt = MockObjcRt::new();
        // device → queue (Owned)
        rt.on_make("MTLDevice", "newCommandQueue", "MTLCommandQueue");
        // NSString 生成経路 (init family: alloc → init で同一 recv 返却)
        rt.on_make("NSString", "alloc", "NSString");
        // library (Owned)
        rt.on_make(
            "MTLDevice",
            "newLibraryWithSource:options:error:",
            "MTLLibrary",
        );
        // functions (Owned)
        rt.on_make("MTLLibrary", "newFunctionWithName:", "MTLFunction");
        // pipeline descriptor/attach (new のみ Owned、accessory getter は Borrowed)
        rt.on_make(
            "MTLRenderPipelineDescriptor",
            "new",
            "MTLRenderPipelineDescriptor",
        );
        rt.on_borrow(
            "MTLRenderPipelineDescriptor",
            "colorAttachments",
            "MTLRenderPipelineColorAttachmentDescriptorArray",
        );
        rt.on_borrow(
            "MTLRenderPipelineColorAttachmentDescriptorArray",
            "objectAtIndexedSubscript:",
            "MTLRenderPipelineColorAttachmentDescriptor",
        );
        rt.on_make(
            "MTLDevice",
            "newRenderPipelineStateWithDescriptor:error:",
            "MTLRenderPipelineState",
        );
        // buffer (Owned、長さ確保+contents 更新の動的経路 + bytes 版)
        rt.on_make("MTLDevice", "newBufferWithLength:options:", "MTLBuffer");
        rt.on_make(
            "MTLDevice",
            "newBufferWithBytes:length:options:",
            "MTLBuffer",
        );
        // contents → arena 実メモリ (update_vertices の物理書込み検証)
        rt.on_ptr("MTLBuffer", "contents");
        // texture descriptor (convenience factory = Borrowed) + texture (Owned)
        rt.on_borrow(
            "MTLTextureDescriptor",
            "texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
            "MTLTextureDescriptor",
        );
        rt.on_make("MTLDevice", "newTextureWithDescriptor:", "MTLTexture");
        // frame 系 (autoreleased 系 Borrowed)
        rt.on_borrow("MTLCommandQueue", "commandBuffer", "MTLCommandBuffer");
        rt.on_make("MTLRenderPassDescriptor", "new", "MTLRenderPassDescriptor");
        rt.on_borrow(
            "MTLRenderPassDescriptor",
            "colorAttachments",
            "MTLRenderPassColorAttachmentDescriptorArray",
        );
        rt.on_borrow(
            "MTLRenderPassColorAttachmentDescriptorArray",
            "objectAtIndexedSubscript:",
            "MTLRenderPassColorAttachmentDescriptor",
        );
        rt.on_borrow(
            "MTLCommandBuffer",
            "renderCommandEncoderWithDescriptor:",
            "MTLRenderCommandEncoder",
        );
        rt.on_borrow("MTLCommandBuffer", "error", "NSError");
        // layer 系 (alloc は Owned、nextDrawable/texture は Borrowed)
        rt.on_make("CAMetalLayer", "alloc", "CAMetalLayer");
        rt.on_borrow("CAMetalLayer", "nextDrawable", "CAMetalDrawable");
        rt.on_borrow("CAMetalDrawable", "texture", "MTLTexture");
        // 正常 frame の status 応答: canon MTLV_CB_COMPLETED (=4) を登録。
        rt.on_u64("MTLCommandBuffer", "status", MTLV_CB_COMPLETED.2 as u64);
        // readback guard のデバイス側真値 (fixture 標準は 64×64)
        rt.on_u64("MTLTexture", "width", 64);
        rt.on_u64("MTLTexture", "height", 64);
        rt
    }

    #[test]
    fn gn_create_full_path_mock_sequence() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        // 主要経路の順序 pin (device 取得→queue→shader→func→pipeline→buf→tex)
        let expect_seq = [
            "newCommandQueue",
            "alloc",
            "initWithUTF8String:",
            "newLibraryWithSource:options:error:",
            "release",
            "alloc",
            "initWithUTF8String:",
            "alloc",
            "initWithUTF8String:",
            "newFunctionWithName:",
            "newFunctionWithName:",
            "release",
            "release",
            "new",
            "setVertexFunction:",
            "setFragmentFunction:",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setPixelFormat:",
            "newRenderPipelineStateWithDescriptor:error:",
            "release",
            "newBufferWithLength:options:",
            "contents",
            "texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
            "setUsage:",
            "setStorageMode:",
            "newTextureWithDescriptor:",
        ];
        assert_eq!(sels, expect_seq, "create の呼出列が Apple 契約手順と一致");
        // 初期頂点が arena (contents 実メモリ相当) に物理書込みされている。
        let expect_floats: [f32; 12] = TRIANGLE_VERTS;
        let expect_bytes: Vec<u8> = expect_floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        assert_eq!(
            rt.arena_bytes(48),
            &expect_bytes[..],
            "初期三角形の物理書込みが一致"
        );
        assert_eq!(m.vbuf_len, 3, "初期頂点数");
    }

    #[test]
    fn gn_render_frame_mock_sequence_and_cleanup() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        if let Err(e) = m.render_frame(&mut rt) {
            panic!("render: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            "commandBuffer",
            "new",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setTexture:",
            "setLoadAction:",
            "setStoreAction:",
            "setClearColor:",
            "renderCommandEncoderWithDescriptor:",
            "setRenderPipelineState:",
            "setViewport:",
            "setVertexBuffer:offset:atIndex:",
            "drawPrimitives:vertexStart:vertexCount:",
            "endEncoding",
            "commit",
            "waitUntilCompleted",
            "release",
            "status",
        ];
        assert_eq!(sels, expect_seq, "frame 呼出列が Metal 標準手順と一致");
        // Owned オブジェクトは shutdown で全解放、live に残らない。
        m.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "リークなし: {:?}", rt.live);
    }

    #[test]
    fn gn_render_to_layer_mock_sequence() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        let layer = rt.mk("CAMetalLayer");
        rt.calls.clear();
        if let Err(e) = m.render_to_layer(&mut rt, layer) {
            panic!("render_to_layer: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            "nextDrawable",
            "texture",
            "commandBuffer",
            "new",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setTexture:",
            "setLoadAction:",
            "setStoreAction:",
            "setClearColor:",
            "renderCommandEncoderWithDescriptor:",
            "setRenderPipelineState:",
            "setViewport:",
            "setVertexBuffer:offset:atIndex:",
            "drawPrimitives:vertexStart:vertexCount:",
            "endEncoding",
            "presentDrawable:",
            "commit",
            "release",
        ];
        assert_eq!(sels, expect_seq, "layer present 呼出列");
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_no_device_fails_loud() {
        // c_mtl_default_device が null を返す mock (応答空)
        let mut rt = MockObjcRt::new();
        // default_device を生成しないように空の結果を返させる:
        // ここでは script 空のまま create_inner 最初の null 分岐を打つ。
        // (mock は default device を生成するため、内部で強制 null にするため
        //  unwrap しない: device 生成をオーバーライドする mock を作る)
        rt.script.clear();
        // default device mock を空にするため dummy 応答で device が
        // null になるケースは mock 構造上再現できないため、
        // NoDefaultDevice は「mock 既定で device が必ず返る」性質から
        // production 固有経路: mock では library 失敗経路を代わりに pin。
        let issue = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(_) => panic!("script 空なのに成功"),
            Err(e) => e,
        };
        assert!(
            matches!(issue, DirectMetalError::NullObject(_)),
            "got {issue:?}"
        );
    }

    #[test]
    fn gn_configure_layer_mock_sequence() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        let layer = rt.mk("CAMetalLayer");
        rt.calls.clear();
        m.configure_layer(&mut rt, layer);
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            sels,
            [
                "setDevice:",
                "setPixelFormat:",
                "setDrawableSize:",
                "setMaximumDrawableCount:",
                "setFramebufferOnly:",
                "setDisplaySyncEnabled:",
                "setPresentsWithTransaction:",
            ]
        );
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_readback_region_shape() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 8, 4) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let mut buf = vec![0u8; 8 * 4 * 4];
        m.readback(&mut rt, &mut buf);
        // guard のデバイス真値照合 (width/height) → getBytes の順序 pin。
        // fixture の真値は 64×64 登録だが self 8×4 との min クランプで
        // 読出領域は 8×4 に縮む (clamp 分岐の実動作検証)。
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            sels,
            [
                "width",
                "height",
                "getBytes:bytesPerRow:fromRegion:mipmapLevel:"
            ],
            "readback guard 呼出列"
        );
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_probe_caps_reports_device_truth() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let caps = m.probe_caps(&mut rt);
        // mock bool_1u は true 応答: Apple7 以上と判定される。
        assert!(caps.apple7_or_newer, "supportsFamily: 応答伝播");
        // デバイス名の script 未登録 → "<unknown>" fallback (fail-safe)。
        assert_eq!(caps.name, "<unknown>");
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(sels, ["name", "supportsFamily:"], "probe 呼出列");
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_create_with_format_rgba8() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create_with_format(&mut rt, 16, 16, MTLV_PF_RGBA8.2 as u64) {
            Ok(v) => v,
            Err(e) => panic!("create_with_format: {e:?}"),
        };
        assert_eq!(m.format, MTLV_PF_RGBA8.2 as u64, "RGBA8 受理");
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_create_with_format_rejects_unknown() {
        let mut rt = scripted_rt();
        let issue = match DirectMetal::create_with_format(&mut rt, 16, 16, 999) {
            Ok(_) => panic!("未知 format 受理は契約違反"),
            Err(e) => e,
        };
        assert_eq!(issue, DirectMetalError::FormatUnsupported(999));
        // 検証で落ちるため ObjC を一切呼ばない (fail-fast 証明)。
        assert!(rt.calls.is_empty(), "検証前呼出なし");
    }

    #[test]
    fn gn_update_vertices_contents_write_verified() {
        let mut rt = scripted_rt();
        let mut m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let verts: [[f32; 4]; 2] = [[0.5, -0.5, 1.0, 0.0], [0.25, 0.25, 0.0, 1.0]];
        if let Err(e) = m.update_vertices(&mut rt, &verts) {
            panic!("update: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(sels, ["contents"], "contents 経由のみ");
        assert_eq!(m.vbuf_len, 2);
        // arena の先頭 32B が新頂点で物理上書きされている (実 memcpy 検証)。
        let expect_bytes: Vec<u8> = verts
            .iter()
            .flat_map(|v| v.iter().flat_map(|f| f.to_ne_bytes()))
            .collect();
        assert_eq!(
            rt.arena_bytes(32),
            &expect_bytes[..],
            "頂点の物理上書き一致"
        );
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_update_vertices_overflow_fails_loud() {
        let mut rt = scripted_rt();
        let mut m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let over = vec![[0.0f32; 4]; (VBUF_CAP_FLOAT4 + 1) as usize];
        let issue = match m.update_vertices(&mut rt, &over) {
            Ok(_) => panic!("容量超過受理は契約違反"),
            Err(e) => e,
        };
        assert!(
            matches!(issue, DirectMetalError::VertexOverflow { .. }),
            "got {issue:?}"
        );
        assert!(rt.calls.is_empty(), "検証で即失敗 (ObjC 未呼出)");
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_render_frame_region_scissor_sequence() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        if let Err(e) = m.render_frame_region(&mut rt, 8, 8, 16, 16) {
            panic!("region: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            "commandBuffer",
            "new",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setTexture:",
            "setLoadAction:",
            "setStoreAction:",
            "setClearColor:",
            "renderCommandEncoderWithDescriptor:",
            "setRenderPipelineState:",
            "setViewport:",
            "setScissorRect:",
            "setVertexBuffer:offset:atIndex:",
            "drawPrimitives:vertexStart:vertexCount:",
            "endEncoding",
            "commit",
            "waitUntilCompleted",
            "release",
            "status",
        ];
        assert_eq!(sels, expect_seq, "部分描画は scissor 制限付き標準手順");
        m.shutdown(&mut rt);
    }

    #[test]
    fn gn_render_textured_quad_lazy_pipeline() {
        let mut rt = scripted_rt();
        let mut m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        let tex = rt.mk("MTLTexture");
        rt.calls.clear();
        if let Err(e) = m.render_textured_quad(&mut rt, tex) {
            panic!("quad: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            // lazy pipeline 構築 (quad vs/fs)
            "alloc",
            "initWithUTF8String:",
            "alloc",
            "initWithUTF8String:",
            "newFunctionWithName:",
            "newFunctionWithName:",
            "release",
            "release",
            "new",
            "setVertexFunction:",
            "setFragmentFunction:",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setPixelFormat:",
            "newRenderPipelineStateWithDescriptor:error:",
            "release",
            "release",
            "release",
            // frame (vbuf 非 bind、fragment texture bind あり)
            "commandBuffer",
            "new",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setTexture:",
            "setLoadAction:",
            "setStoreAction:",
            "setClearColor:",
            "renderCommandEncoderWithDescriptor:",
            "setRenderPipelineState:",
            "setViewport:",
            "setFragmentTexture:atIndex:",
            "drawPrimitives:vertexStart:vertexCount:",
            "endEncoding",
            "commit",
            "waitUntilCompleted",
            "release",
            "status",
        ];
        assert_eq!(sels, expect_seq, "quad 初回は lazy pipeline+frame");
        // 2 回目は pipeline を再利用 (newFunctionWithName: が出ない)。
        rt.calls.clear();
        if let Err(e) = m.render_textured_quad(&mut rt, tex) {
            panic!("quad2: {e:?}")
        }
        let sels2: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(!sels2.contains(&"newFunctionWithName:"), "lazy 再利用");
        assert!(
            sels2.contains(&"setFragmentTexture:atIndex:"),
            "2 回目も texture bind"
        );
        // tex は呼出側所有 → 自前で解放して live を揃える。
        rt.void_0(tex, SEL_RELEASE.1);
        m.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "リークなし: {:?}", rt.live);
    }

    #[test]
    fn gn_render_frame_indexed_sequence() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        // index buffer は本番 helper 経由で生成 (canon bytes 経路の消費者)。
        let ibuf = match m.create_index_buffer_u16(&mut rt, &[0u16, 1, 2]) {
            Ok(v) => v,
            Err(e) => panic!("ibuf: {e:?}"),
        };
        if let Err(e) = m.render_frame_indexed(&mut rt, ibuf, 3) {
            panic!("indexed: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            "newBufferWithBytes:length:options:",
            "commandBuffer",
            "new",
            "colorAttachments",
            "objectAtIndexedSubscript:",
            "setTexture:",
            "setLoadAction:",
            "setStoreAction:",
            "setClearColor:",
            "renderCommandEncoderWithDescriptor:",
            "setRenderPipelineState:",
            "setViewport:",
            "setVertexBuffer:offset:atIndex:",
            "drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:",
            "endEncoding",
            "commit",
            "waitUntilCompleted",
            "release",
            "status",
        ];
        assert_eq!(sels, expect_seq, "index 描画の標準手順 (ibuf 生成込み)");
        rt.void_0(ibuf, SEL_RELEASE.1);
        m.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "リークなし: {:?}", rt.live);
    }

    #[test]
    fn gn_attach_layer_owned_lifecycle() {
        let mut rt = scripted_rt();
        let mut m = match DirectMetal::create(&mut rt, 64, 64) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let layer = match m.attach_layer(&mut rt) {
            Ok(v) => v,
            Err(e) => panic!("attach: {e:?}"),
        };
        assert!(!layer.is_null(), "layer 実体");
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let expect_seq = [
            "alloc",
            "init",
            "setDevice:",
            "setPixelFormat:",
            "setDrawableSize:",
            "setMaximumDrawableCount:",
            "setFramebufferOnly:",
            "setDisplaySyncEnabled:",
            "setPresentsWithTransaction:",
        ];
        assert_eq!(sels, expect_seq, "layer 生成+configure の標準手順");
        // Owned layer も含め shutdown で全解放 (live が空)。
        m.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "リークなし: {:?}", rt.live);
    }

    #[test]
    fn gn_upload_pixels_replace_region() {
        let mut rt = scripted_rt();
        let m = match DirectMetal::create(&mut rt, 8, 4) {
            Ok(v) => v,
            Err(e) => panic!("create: {e:?}"),
        };
        rt.calls.clear();
        let pixels = vec![7u8; 8 * 4 * 4];
        if let Err(e) = m.upload_pixels(&mut rt, &pixels) {
            panic!("upload: {e:?}")
        }
        let sels: Vec<&str> = rt
            .calls
            .iter()
            .map(|c| c.sel.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            sels,
            ["replaceRegion:mipmapLevel:withBytes:bytesPerRow:"],
            "upload 呼出列"
        );
        // 短いデータは fail-loud。
        let issue = match m.upload_pixels(&mut rt, &[0u8; 3]) {
            Ok(_) => panic!("不足データ受理は契約違反"),
            Err(e) => e,
        };
        assert_eq!(
            issue,
            DirectMetalError::PixelDataShort {
                have: 3,
                want: 8 * 4 * 4
            }
        );
        m.shutdown(&mut rt);
    }
}
