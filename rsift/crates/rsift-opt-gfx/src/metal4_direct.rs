//! Metal 4 (MTL4) 直 binding 完全実装 — MTL4 コマンド系を objc runtime
//! dispatcher 経由で実駆動する。【wave 196 GO (2026-07-31)】
//!
//! ユーザー要求「METAL、GL 完全実装・直 binding・Mac 実機なしで Apple
//! 一次情報の独自静的解析マシンで動作保証して」の MTL4 拡張。
//!
//! ## 保証体系 (wave 194 GN の 3 層を MTL4 へ拡張)
//! 1. **静的解析マシン** `apple_ffi_audit`: 本ファイルの全 selector/enum/
//!    extern/呼出形状を canon 一次情報と機械照合 (canon は CANON_SDK26_* =
//!    Apple SDK 26.5 Metal.framework 全 98 ヘッダ機械転記、Metal4=5002・
//!    MTLStages・MTL4CommandQueue 系全 selector を含む)。
//! 2. **Mock 動作保証**: `objc_rt::MockObjcRt` が ObjC メモリ規則を模倣し
//!    本番コード全行を Linux/CI で実走行 (呼出列・引数実値・リーク 0)。
//! 3. **実機経路**: `#[cfg(target_os = "macos")]` `NativeObjcRt` の
//!    objc_msgSend typed transmute 正規呼出経路に完全対応。
//!
//! ## 一次情報 (仕様の出典、全て Apple 公式)
//! - MTL4* 全型の selector 表: MacOSX26.5.sdk Metal.framework Headers
//!   (alexey-lysiuk/macos-sdk verbatim ミラー) → CANON_SDK26_SELS 機械転記。
//! - frame 進行 (in-flight 3・allocator ring・event 待機・drawable 同期):
//!   Apple 公式サンプル「Drawing a triangle with Metal 4」(DrawingContent)。
//!   `frameNumber >= kMaxFramesInFlight(3)` なら
//!   `[sharedEvent waitUntilSignaledValue:(frameNumber-3) timeoutMS:10]` →
//!   `[frameAllocator reset]` → begin/encode/end →
//!   `[commandQueue waitForDrawable:]` → `commit:count:` →
//!   `[commandQueue signalDrawable:]` → `[currentDrawable present]` →
//!   `[commandQueue signalEvent:value:frameNumber]` の順序を厳密踏襲。
//! - residency: `MTLResidencySetDescriptor` alloc/init → device
//!   `newResidencySetWithDescriptor:error:` → `addAllocation:` ×リソース →
//!   `commit` → queue `addResidencySet:` (Hello Triangle 正規形)。
//! - argument table: `MTL4ArgumentTableDescriptor` (maxBufferBindCount≤31 /
//!   maxTextureBindCount≤128 — ヘッダ doc 一次情報) → `setAddress:atIndex:`
//!   (buffer(GPUAddress)) / `setTexture:atIndex:` (MTLResourceID) →
//!   encoder `setArgumentTable:atStages:` (MTLRenderStages)。texture の
//!   sampler は MSL の `constexpr sampler` に静的直結 (metal_direct の
//!   TRIANGLE_MSL 一次形継承) のため sampler スロット不要。
//!
//! ## selector 宣言規約 (監査機が構文抽出する固定形式、wave 194 踏襲)
//! - `pub const SEL_MTL4_*: (&str, &CStr) = ("<Class>", c"<sel>");`
//! - `pub const MTLV_MTL4 系 (MTLV_ 接頭辞準拠): (&str, &str, i64) = ("<Enum>", "<Variant>", <val>);`
//! - 呼出は `rt.<disp>(obj, SEL_MTL4_*.1, args...)`、disp 名の引数種別と
//!   selector コロン数・呼出引数個数を監査機 R3 が全件機械照合。

use crate::metal_direct::{
    err_string, ns_str, DirectMetal, DirectMetalError, MTLV_LOAD_CLEAR, MTLV_PF_BGRA8,
    MTLV_PRIM_TRIANGLE, MTLV_STORAGE_SHARED, MTLV_STORE_STORE, SEL_ALLOC, SEL_CONTENTS,
    SEL_DRAWABLE_TEXTURE, SEL_INIT, SEL_NEW_BUFFER_LEN_OPTS, SEL_NEXT_DRAWABLE,
    SEL_OBJECT_AT_INDEXED_SUBSCRIPT, SEL_RELEASE, SEL_SET_CLEAR_COLOR, SEL_SET_LOAD_ACTION,
    SEL_SET_STORE_ACTION, SEL_SET_TEXTURE, SEL_SUPPORTS_FAMILY,
};
use crate::objc_rt::{MtlClearColor, MtlViewport, ObjcId, ObjcRt};
use core::ffi::CStr;

// ---------------------------------------------------------------------------
// SEL: MTL4 生成系 (canon 一次情報: MTLDevice.h MTL4 section、全て Owned)
// ---------------------------------------------------------------------------
/// MTL4 コマンドキュー生成 (canon Owned)。
pub const SEL_MTL4_NEW_COMMAND_QUEUE: (&str, &CStr) = ("MTLDevice", c"newMTL4CommandQueue");
/// MTL4 コマンドバッファ生成 (canon Owned)。
pub const SEL_MTL4_NEW_COMMAND_BUFFER: (&str, &CStr) = ("MTLDevice", c"newCommandBuffer");
/// コマンドアロケータ生成 (descriptor+error、canon Owned)。
pub const SEL_MTL4_NEW_COMMAND_ALLOCATOR: (&str, &CStr) =
    ("MTLDevice", c"newCommandAllocatorWithDescriptor:error:");
/// argument table 生成 (descriptor+error、canon Owned)。
pub const SEL_MTL4_NEW_ARGUMENT_TABLE: (&str, &CStr) =
    ("MTLDevice", c"newArgumentTableWithDescriptor:error:");
/// MTL4Compiler 生成 (descriptor+error、canon Owned)。
pub const SEL_MTL4_NEW_COMPILER: (&str, &CStr) = ("MTLDevice", c"newCompilerWithDescriptor:error:");
/// residency set 生成 (descriptor+error、canon Owned)。
pub const SEL_MTL4_NEW_RESIDENCY_SET: (&str, &CStr) =
    ("MTLDevice", c"newResidencySetWithDescriptor:error:");
/// shared event 生成 (canon Owned、frame 同期用)。
pub const SEL_MTL4_NEW_SHARED_EVENT: (&str, &CStr) = ("MTLDevice", c"newSharedEvent");
/// MTL4 render pipeline 生成 (canonical 一次情報:
/// MTL4Compiler.h `newRenderPipelineStateWithDescriptor:compilerTaskOptions:
/// error:`、canon Owned)。
pub const SEL_MTL4_NEW_PIPELINE: (&str, &CStr) = (
    "MTL4Compiler",
    c"newRenderPipelineStateWithDescriptor:compilerTaskOptions:error:",
);

// ---------------------------------------------------------------------------
// SEL: MTL4CommandQueue 操作 (canon MTL4CommandQueue.h、全て Borrowed)
// ---------------------------------------------------------------------------
/// residency set 登録 (GPU 共有リソースを frame 基底で常駐させる正規経路)。
pub const SEL_MTL4_ADD_RESIDENCY_SET: (&str, &CStr) = ("MTL4CommandQueue", c"addResidencySet:");
/// コマンドバッファ配列コミット (引数1= const id<MTL4CommandBuffer>*)。
pub const SEL_MTL4_COMMIT_COUNT: (&str, &CStr) = ("MTL4CommandQueue", c"commit:count:");
/// イベントシグナル (commit 完了時点で frame 番号を刻む)。
pub const SEL_MTL4_SIGNAL_EVENT_VALUE: (&str, &CStr) = ("MTL4CommandQueue", c"signalEvent:value:");
/// drawable 利用開始の GPU 待機挿入。
pub const SEL_MTL4_WAIT_DRAWABLE: (&str, &CStr) = ("MTL4CommandQueue", c"waitForDrawable:");
/// drawable 利用完了のシグナル (present 許可発行)。
pub const SEL_MTL4_SIGNAL_DRAWABLE: (&str, &CStr) = ("MTL4CommandQueue", c"signalDrawable:");

// ---------------------------------------------------------------------------
// SEL: allocator / command buffer / encoder
// ---------------------------------------------------------------------------
/// allocator heap 再利用マーク (前 frame 完了後のみ許容 — event 待機が前提)。
pub const SEL_MTL4_ALLOCATOR_RESET: (&str, &CStr) = ("MTL4CommandAllocator", c"reset");
/// コマンドバッファの encode 開始 (allocator を attach)。
pub const SEL_MTL4_BEGIN_WITH_ALLOCATOR: (&str, &CStr) =
    ("MTL4CommandBuffer", c"beginCommandBufferWithAllocator:");
/// encode 終了 (commit 可能状態へ)。
pub const SEL_MTL4_END_COMMAND_BUFFER: (&str, &CStr) = ("MTL4CommandBuffer", c"endCommandBuffer");
/// render encoder 生成 (MTL4RenderPassDescriptor 駆動、戻り値は
/// nullable Borrowed — 失敗 null を fail-loud 検査)。
pub const SEL_MTL4_RENDER_ENCODER: (&str, &CStr) =
    ("MTL4CommandBuffer", c"renderCommandEncoderWithDescriptor:");
/// 本コマンドバッファに residency set を適用。
pub const SEL_MTL4_USE_RESIDENCY_SET: (&str, &CStr) = ("MTL4CommandBuffer", c"useResidencySet:");
/// encoder 終了 (canon は親 MTL4CommandEncoder 帰属 → 継承解決で監査)。
pub const SEL_MTL4_END_ENCODING: (&str, &CStr) = ("MTL4CommandEncoder", c"endEncoding");

// ---------------------------------------------------------------------------
// SEL: argument table バインド
// ---------------------------------------------------------------------------
/// buffer を GPU アドレスで index バインド (MTLGPUAddress=u64)。
pub const SEL_MTL4_SET_ADDRESS_AT_INDEX: (&str, &CStr) =
    ("MTL4ArgumentTable", c"setAddress:atIndex:");
/// texture を MTLResourceID で index バインド。
pub const SEL_MTL4_SET_TEXTURE_AT_INDEX: (&str, &CStr) =
    ("MTL4ArgumentTable", c"setTexture:atIndex:");
/// argument table を stage 集合へ関連付け (MTLRenderStages bitmask)。
pub const SEL_MTL4_SET_ARGUMENT_TABLE_AT_STAGES: (&str, &CStr) =
    ("MTL4RenderCommandEncoder", c"setArgumentTable:atStages:");

// ---------------------------------------------------------------------------
// SEL: MTL4 render encoder 状態 / draw
// ---------------------------------------------------------------------------
/// pipeline state bind (MTLRenderPipelineState、classic と同型 protocol)。
pub const SEL_MTL4_SET_PIPELINE_STATE: (&str, &CStr) =
    ("MTL4RenderCommandEncoder", c"setRenderPipelineState:");
/// viewport 設定 (MTLViewport struct 渡し)。
pub const SEL_MTL4_SET_VIEWPORT: (&str, &CStr) = ("MTL4RenderCommandEncoder", c"setViewport:");
/// 非 index draw (vertex_id 駆動、則ち vertices([[buffer(0)]]) も可)。
pub const SEL_MTL4_DRAW_PRIMITIVES: (&str, &CStr) = (
    "MTL4RenderCommandEncoder",
    c"drawPrimitives:vertexStart:vertexCount:",
);

// ---------------------------------------------------------------------------
// SEL: 同期 / GPU リソース識別子 / residency
// ---------------------------------------------------------------------------
/// frame in-flight 上限超過時の完了待機 (一次情報: Hello Triangle
/// `[sharedEvent waitUntilSignaledValue:(frameNumber-3) timeoutMS:10]`)。
pub const SEL_MTL4_WAIT_SIGNALED: (&str, &CStr) =
    ("MTLSharedEvent", c"waitUntilSignaledValue:timeoutMS:");
/// MTLBuffer の GPU アドレス (macOS 13+、argument table 経路の正規キー)。
pub const SEL_MTL4_GPU_ADDRESS: (&str, &CStr) = ("MTLBuffer", c"gpuAddress");
/// MTLTexture の GPU resource ID (MTLResourceID、argument table 経路)。
pub const SEL_MTL4_GPU_RESOURCE_ID: (&str, &CStr) = ("MTLTexture", c"gpuResourceID");
/// residency set へリソース追加 (MTLBuffer/MTLTexture は MTLAllocation 適合)。
pub const SEL_MTL4_RESIDENCY_ADD_ALLOCATION: (&str, &CStr) = ("MTLResidencySet", c"addAllocation:");
/// residency set の内容確定 (queue 登録前に必須)。
pub const SEL_MTL4_RESIDENCY_COMMIT: (&str, &CStr) = ("MTLResidencySet", c"commit");
/// drawable の画面提示 (MTLDrawable 一次契約、Hello Triangle の提示呼出)。
pub const SEL_MTL4_PRESENT: (&str, &CStr) = ("MTLDrawable", c"present");

// ---------------------------------------------------------------------------
// SEL: descriptor 構成 (property getter/setter、canon SDK26 一次情報)
// ---------------------------------------------------------------------------
/// MTL4RenderPassDescriptor の添付配列 (readonly、旧型
/// MTLRenderPassColorAttachmentDescriptorArray — classic objectAtIndexedSubscript
/// 系 SEL がそのまま有効、DocC 一次情報)。
pub const SEL_MTL4_COLOR_ATTACHMENTS: (&str, &CStr) =
    ("MTL4RenderPassDescriptor", c"colorAttachments");
/// MTL4RenderPipelineDescriptor の添付配列 (readonly)。
pub const SEL_MTL4_PIPE_COLOR_ATTACHMENTS: (&str, &CStr) =
    ("MTL4RenderPipelineDescriptor", c"colorAttachments");
/// vertex stage の関数 descriptor 設定 (copy property)。
pub const SEL_MTL4_SET_VERTEX_FUNCTION_DESCRIPTOR: (&str, &CStr) = (
    "MTL4RenderPipelineDescriptor",
    c"setVertexFunctionDescriptor:",
);
/// fragment stage の関数 descriptor 設定 (copy property)。
pub const SEL_MTL4_SET_FRAGMENT_FUNCTION_DESCRIPTOR: (&str, &CStr) = (
    "MTL4RenderPipelineDescriptor",
    c"setFragmentFunctionDescriptor:",
);
/// MTL4LibraryFunctionDescriptor: 対象 library (retain property)。
pub const SEL_MTL4_FD_SET_LIBRARY: (&str, &CStr) =
    ("MTL4LibraryFunctionDescriptor", c"setLibrary:");
/// MTL4LibraryFunctionDescriptor: 関数名 (copy property)。
pub const SEL_MTL4_FD_SET_NAME: (&str, &CStr) = ("MTL4LibraryFunctionDescriptor", c"setName:");
/// argument table descriptor: buffer バインド上限 (≤31、ヘッダ一次情報)。
pub const SEL_MTL4_ATD_SET_MAX_BUFFER_BIND_COUNT: (&str, &CStr) =
    ("MTL4ArgumentTableDescriptor", c"setMaxBufferBindCount:");
/// argument table descriptor: texture バインド上限 (≤128、ヘッダ一次情報)。
pub const SEL_MTL4_ATD_SET_MAX_TEXTURE_BIND_COUNT: (&str, &CStr) =
    ("MTL4ArgumentTableDescriptor", c"setMaxTextureBindCount:");
/// MTL4 pipeline 添付配列の index アクセス。
pub const SEL_MTL4_PIPE_ATT_OBJECT_AT: (&str, &CStr) = (
    "MTL4RenderPipelineColorAttachmentDescriptorArray",
    c"objectAtIndexedSubscript:",
);
/// MTL4 pipeline 添付の pixel format 設定。
pub const SEL_MTL4_PIPE_ATT_SET_PIXEL_FORMAT: (&str, &CStr) = (
    "MTL4RenderPipelineColorAttachmentDescriptor",
    c"setPixelFormat:",
);

// ---------------------------------------------------------------------------
// MTLV4: SDK26 正典 enum 値 (classic canon 未収録分、全件 CANON_SDK26_ENUMS
// と監査機 R2 照合)
// ---------------------------------------------------------------------------
/// Metal 4 GPU family (macOS 26、MTLDevice.h:255 `= 5002` 逐語)。
pub const MTLV_FAMILY_METAL4: (&str, &str, i64) = ("MTLGPUFamily", "Metal4", 5002);
/// MTLRenderStages vertex (MTLRenderCommandEncoder.h:120 `(1UL << 0)`)。
pub const MTLV_RSTAGE_VERTEX: (&str, &str, i64) = ("MTLRenderStages", "Vertex", 1);
/// MTLRenderStages fragment (同:121 `(1UL << 1)`)。
pub const MTLV_RSTAGE_FRAGMENT: (&str, &str, i64) = ("MTLRenderStages", "Fragment", 2);

// ---------------------------------------------------------------------------
// CLASS: descriptor 生成用の ObjC クラス名
// ---------------------------------------------------------------------------
/// MTL4 レンダーパス descriptor クラス。
pub const CLASS_MTL4_PASS_DESC: &CStr = c"MTL4RenderPassDescriptor";
/// argument table descriptor クラス。
pub const CLASS_MTL4_ARG_TABLE_DESC: &CStr = c"MTL4ArgumentTableDescriptor";
/// MTL4 render pipeline descriptor クラス。
pub const CLASS_MTL4_RENDER_PIPELINE_DESC: &CStr = c"MTL4RenderPipelineDescriptor";
/// MTL4 library 関数 descriptor クラス。
pub const CLASS_MTL4_LIBRARY_FUNCTION_DESC: &CStr = c"MTL4LibraryFunctionDescriptor";
/// MTL4 compiler descriptor クラス (newCompilerWithDescriptor: は
/// NS_ASSUME_NONNULL 区域内のため null 不可 — 実インスタンス必須)。
pub const CLASS_MTL4_COMPILER_DESC: &CStr = c"MTL4CompilerDescriptor";
/// residency set descriptor クラス。
pub const CLASS_MTL4_RESIDENCY_SET_DESC: &CStr = c"MTLResidencySetDescriptor";
/// コマンドアロケータ descriptor クラス。
pub const CLASS_MTL4_COMMAND_ALLOCATOR_DESC: &CStr = c"MTL4CommandAllocatorDescriptor";

/// frame in-flight 数 (Hello Triangle `kMaxFramesInFlight = 3` 一次情報)。
pub const MTL4_FRAMES_IN_FLIGHT: u64 = 3;

/// event 待機の timeout ミリ秒 (Hello Triangle `timeoutMS: 10` 一次情報)。
pub const MTL4_WAIT_TIMEOUT_MS: u64 = 10;

/// MTL4 頂点バッファ 1 個の容量 (float4 個数。UI バッチ現実上限)。
pub const VBUF4_CAP_FLOAT4: u64 = 1024;

// ---------------------------------------------------------------------------
// 小物ヘルパー
// ---------------------------------------------------------------------------

/// descriptor インスタンス生成 (class alloc→init、canon Owned 規則)。
fn make_obj(rt: &mut dyn ObjcRt, class_name: &'static CStr) -> Option<ObjcId> {
    let cls = rt.get_class(class_name);
    if cls.is_null() {
        return None;
    }
    let allocd = rt.id_0(cls, SEL_ALLOC.1);
    if allocd.is_null() {
        return None;
    }
    // init family: alloc の +1 を init が absorb (canon 規則、mock も同一返却)。
    let obj = rt.id_0(allocd, SEL_INIT.1);
    if obj.is_null() {
        return None;
    }
    Some(obj)
}

/// MTL4 関数 descriptor (library + name を set)。
fn make_function_descriptor(
    rt: &mut dyn ObjcRt,
    library: ObjcId,
    fn_name: &str,
) -> Result<ObjcId, DirectMetalError> {
    let fd = make_obj(rt, CLASS_MTL4_LIBRARY_FUNCTION_DESC).ok_or(
        DirectMetalError::ClassMissing("MTL4LibraryFunctionDescriptor"),
    )?;
    rt.void_1p(fd, SEL_MTL4_FD_SET_LIBRARY.1, library);
    let name = ns_str(rt, fn_name).ok_or(DirectMetalError::ClassMissing("NSString"))?;
    rt.void_1p(fd, SEL_MTL4_FD_SET_NAME.1, name);
    rt.void_0(name, SEL_RELEASE.1);
    Ok(fd)
}

// ---------------------------------------------------------------------------
// 完全 MTL4 描画コンテキスト
// ---------------------------------------------------------------------------

/// Metal 4 直 binding セッション (classic 基部 + MTL4 コマンド系全構築)。
/// 所有規則は canon/SDK26 と一致: queue4/cmd/allocators/shared_event/
/// pipelines/arg_tables/vbufs/residency = Owned (shutdown で release)、
/// base 側は metal_direct の規則そのまま。
pub struct DirectMetal4 {
    /// classic 基部 (device/library/target 共有、offscreen 検証の readback 用)。
    pub base: DirectMetal,
    /// MTL4 コマンドキュー (Owned)。
    pub queue4: ObjcId,
    /// MTL4 コマンドバッファ (Owned、begin/end を frame 毎に反復)。
    pub cmd: ObjcId,
    /// frame in-flight 分のコマンドアロケータ (Owned × MTL4_FRAMES_IN_FLIGHT)。
    pub allocators: [ObjcId; 3],
    /// frame 完了同期 shared event (Owned)。
    pub shared_event: ObjcId,
    /// MTL4 render pipeline (Owned、三角形/頂点バッファ経路)。
    pub pipeline4: ObjcId,
    /// MTL4 textured pipeline (Owned、UI quad 経路)。
    pub tex_pipeline4: ObjcId,
    /// buffer 専用 argument table (Owned)。
    pub arg_table: ObjcId,
    /// texture 専用 argument table (Owned)。
    pub tex_arg_table: ObjcId,
    /// frame 専用頂点バッファリング (Owned ×3 — 一次情報 Hello Triangle:
    /// 「各フレーム専用バッファ」で CPU 書込 × GPU 読込競合を構造排除)。
    pub vbufs: [ObjcId; 3],
    /// vbuf 各個の gpuAddress キャッシュ (生成時 1 回問合せ、frame で再利用)。
    pub vbuf_addrs: [u64; 3],
    /// GPU 共有リソース residency set (Owned、queue 登録済)。
    pub residency: ObjcId,
    /// 現在 frame 番号 (signalEvent:value: に刻む単調増加値、0 開始)。
    pub frame_value: u64,
}

impl DirectMetal4 {
    /// オフスクリーン MTL4 完全構築。BGRA8 target を MTL4 で駆動する。
    /// 全失敗は DirectMetalError で詳細化 (スタブ/黙殺なし)。
    pub fn create4(rt: &mut dyn ObjcRt, width: u32, height: u32) -> Result<Self, DirectMetalError> {
        let pool = rt.pool_push();
        let result = Self::create4_inner(rt, width, height);
        rt.pool_pop(pool);
        result
    }

    fn create4_inner(
        rt: &mut dyn ObjcRt,
        width: u32,
        height: u32,
    ) -> Result<Self, DirectMetalError> {
        let base = DirectMetal::create_with_format(rt, width, height, MTLV_PF_BGRA8.2 as u64)?;
        let device = base.device;
        // Metal 4 硬ゲート: 実機 MTLGPUFamilyMetal4 (5002) 必須
        // (apple_backend Metal4Surface「Silicon && macOS≥26」policy と一致、
        // 実機側の最終真理は device 応答にある一次情報設計)。
        let ok4 = rt.bool_1u(device, SEL_SUPPORTS_FAMILY.1, MTLV_FAMILY_METAL4.2 as u64);
        if !ok4 {
            return Err(DirectMetalError::Metal4Unsupported);
        }
        // queue / command buffer (Owned)
        let queue4 = rt.id_0(device, SEL_MTL4_NEW_COMMAND_QUEUE.1);
        if queue4.is_null() {
            return Err(DirectMetalError::NullObject("newMTL4CommandQueue"));
        }
        let cmd = rt.id_0(device, SEL_MTL4_NEW_COMMAND_BUFFER.1);
        if cmd.is_null() {
            return Err(DirectMetalError::NullObject("newCommandBuffer"));
        }
        // allocators ×3 (descriptor alloc→init → device factory、Owned)
        let mut allocators: [ObjcId; 3] = [core::ptr::null_mut(); 3];
        for slot in allocators.iter_mut() {
            let desc = make_obj(rt, CLASS_MTL4_COMMAND_ALLOCATOR_DESC).ok_or(
                DirectMetalError::ClassMissing("MTL4CommandAllocatorDescriptor"),
            )?;
            let mut err: ObjcId = core::ptr::null_mut();
            let a = rt.id_2pp(
                device,
                SEL_MTL4_NEW_COMMAND_ALLOCATOR.1,
                desc,
                &mut err as *mut ObjcId as ObjcId,
            );
            rt.void_0(desc, SEL_RELEASE.1);
            if a.is_null() {
                let msg = err_string(rt, err, "newCommandAllocatorWithDescriptor failed");
                return Err(DirectMetalError::Pipeline(msg));
            }
            *slot = a;
        }
        // shared event (Owned)
        let shared_event = rt.id_0(device, SEL_MTL4_NEW_SHARED_EVENT.1);
        if shared_event.is_null() {
            return Err(DirectMetalError::NullObject("newSharedEvent"));
        }
        // compiler (descriptor 必須 — NS_ASSUME_NONNULL 一次情報)
        let cdesc = make_obj(rt, CLASS_MTL4_COMPILER_DESC)
            .ok_or(DirectMetalError::ClassMissing("MTL4CompilerDescriptor"))?;
        let mut cerr: ObjcId = core::ptr::null_mut();
        let compiler = rt.id_2pp(
            device,
            SEL_MTL4_NEW_COMPILER.1,
            cdesc,
            &mut cerr as *mut ObjcId as ObjcId,
        );
        rt.void_0(cdesc, SEL_RELEASE.1);
        if compiler.is_null() {
            let msg = err_string(rt, cerr, "newCompilerWithDescriptor failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        // pipeline ×2 (三角形 + textured quad)
        let pipeline4 = Self::make_pipeline4(
            rt,
            compiler,
            base.library,
            "rsift_direct_vs",
            "rsift_direct_fs",
            base.format,
        )?;
        let tex_pipeline4 = Self::make_pipeline4(
            rt,
            compiler,
            base.library,
            "rsift_quad_vs",
            "rsift_quad_fs",
            base.format,
        )?;
        // compiler は pipeline 生成が仕事の全て (factory 参照は ARC で
        // pipeline 側が保持) — 本 struct では以後使わないため release。
        rt.void_0(compiler, SEL_RELEASE.1);
        // argument tables (buffer 1 slot / texture 1 slot)
        let arg_table = Self::make_arg_table(rt, device, 1, 0)?;
        let tex_arg_table = Self::make_arg_table(rt, device, 0, 1)?;
        // frame 専用頂点バッファ ×3 + gpuAddress キャッシュ
        let mut vbufs: [ObjcId; 3] = [core::ptr::null_mut(); 3];
        let mut vbuf_addrs = [0u64; 3];
        let len_bytes = VBUF4_CAP_FLOAT4 * 16;
        for i in 0..3 {
            let b = rt.id_2uu(
                device,
                SEL_NEW_BUFFER_LEN_OPTS.1,
                len_bytes,
                MTLV_STORAGE_SHARED.2 as u64,
            );
            if b.is_null() {
                return Err(DirectMetalError::NullObject("newBufferWithLength:options:"));
            }
            vbuf_addrs[i] = rt.u64_0(b, SEL_MTL4_GPU_ADDRESS.1);
            vbufs[i] = b;
        }
        // residency set: 全 vbuf + target を常駐化して queue 登録
        let rsd = make_obj(rt, CLASS_MTL4_RESIDENCY_SET_DESC)
            .ok_or(DirectMetalError::ClassMissing("MTLResidencySetDescriptor"))?;
        let mut rerr: ObjcId = core::ptr::null_mut();
        let residency = rt.id_2pp(
            device,
            SEL_MTL4_NEW_RESIDENCY_SET.1,
            rsd,
            &mut rerr as *mut ObjcId as ObjcId,
        );
        rt.void_0(rsd, SEL_RELEASE.1);
        if residency.is_null() {
            let msg = err_string(rt, rerr, "newResidencySetWithDescriptor failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        for b in vbufs {
            rt.void_1p(residency, SEL_MTL4_RESIDENCY_ADD_ALLOCATION.1, b);
        }
        rt.void_1p(residency, SEL_MTL4_RESIDENCY_ADD_ALLOCATION.1, base.target);
        rt.void_0(residency, SEL_MTL4_RESIDENCY_COMMIT.1);
        rt.void_1p(queue4, SEL_MTL4_ADD_RESIDENCY_SET.1, residency);
        Ok(Self {
            base,
            queue4,
            cmd,
            allocators,
            shared_event,
            pipeline4,
            tex_pipeline4,
            arg_table,
            tex_arg_table,
            vbufs,
            vbuf_addrs,
            residency,
            frame_value: 0,
        })
    }

    /// MTL4 pipeline 1 本の生成 (descriptor alloc→compose→compiler build)。
    fn make_pipeline4(
        rt: &mut dyn ObjcRt,
        compiler: ObjcId,
        library: ObjcId,
        vs_name: &str,
        fs_name: &str,
        format: u64,
    ) -> Result<ObjcId, DirectMetalError> {
        let fdv = make_function_descriptor(rt, library, vs_name)?;
        let fdf = make_function_descriptor(rt, library, fs_name)?;
        let desc = make_obj(rt, CLASS_MTL4_RENDER_PIPELINE_DESC).ok_or(
            DirectMetalError::ClassMissing("MTL4RenderPipelineDescriptor"),
        )?;
        rt.void_1p(desc, SEL_MTL4_SET_VERTEX_FUNCTION_DESCRIPTOR.1, fdv);
        rt.void_1p(desc, SEL_MTL4_SET_FRAGMENT_FUNCTION_DESCRIPTOR.1, fdf);
        let atts = rt.id_0(desc, SEL_MTL4_PIPE_COLOR_ATTACHMENTS.1);
        if atts.is_null() {
            return Err(DirectMetalError::NullObject(
                "MTL4 pipeline colorAttachments",
            ));
        }
        let att0 = rt.id_1u(atts, SEL_MTL4_PIPE_ATT_OBJECT_AT.1, 0);
        rt.void_1u(att0, SEL_MTL4_PIPE_ATT_SET_PIXEL_FORMAT.1, format);
        let mut err: ObjcId = core::ptr::null_mut();
        let pipe = rt.id_3ppp(
            compiler,
            SEL_MTL4_NEW_PIPELINE.1,
            desc,
            core::ptr::null_mut(),
            &mut err as *mut ObjcId as ObjcId,
        );
        rt.void_0(desc, SEL_RELEASE.1);
        rt.void_0(fdv, SEL_RELEASE.1);
        rt.void_0(fdf, SEL_RELEASE.1);
        if pipe.is_null() {
            let msg = err_string(rt, err, "MTL4 pipeline creation failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        Ok(pipe)
    }

    /// argument table 1 本の生成 (descriptor 上限設定 → device factory)。
    fn make_arg_table(
        rt: &mut dyn ObjcRt,
        device: ObjcId,
        max_buffers: u64,
        max_textures: u64,
    ) -> Result<ObjcId, DirectMetalError> {
        let atd = make_obj(rt, CLASS_MTL4_ARG_TABLE_DESC).ok_or(DirectMetalError::ClassMissing(
            "MTL4ArgumentTableDescriptor",
        ))?;
        rt.void_1u(atd, SEL_MTL4_ATD_SET_MAX_BUFFER_BIND_COUNT.1, max_buffers);
        rt.void_1u(atd, SEL_MTL4_ATD_SET_MAX_TEXTURE_BIND_COUNT.1, max_textures);
        let mut err: ObjcId = core::ptr::null_mut();
        let table = rt.id_2pp(
            device,
            SEL_MTL4_NEW_ARGUMENT_TABLE.1,
            atd,
            &mut err as *mut ObjcId as ObjcId,
        );
        rt.void_0(atd, SEL_RELEASE.1);
        if table.is_null() {
            let msg = err_string(rt, err, "newArgumentTableWithDescriptor failed");
            return Err(DirectMetalError::Pipeline(msg));
        }
        Ok(table)
    }

    /// frame 先頭処理 (Hello Triangle 一次情報の厳密形): frame_value を進め
    /// ring index 確定、in-flight 上限超過なら event 待機、allocator reset、
    /// command buffer begin + residency 適用。戻り値は ring index。
    fn begin_frame4(&mut self, rt: &mut dyn ObjcRt) -> Result<usize, DirectMetalError> {
        self.frame_value += 1;
        let idx = (self.frame_value % MTL4_FRAMES_IN_FLIGHT) as usize;
        if self.frame_value >= MTL4_FRAMES_IN_FLIGHT {
            let awaited = self.frame_value - MTL4_FRAMES_IN_FLIGHT;
            let ok = rt.bool_2uu(
                self.shared_event,
                SEL_MTL4_WAIT_SIGNALED.1,
                awaited,
                MTL4_WAIT_TIMEOUT_MS,
            );
            if !ok {
                return Err(DirectMetalError::FrameSyncTimeout {
                    frame: self.frame_value,
                    awaited,
                });
            }
        }
        rt.void_0(self.allocators[idx], SEL_MTL4_ALLOCATOR_RESET.1);
        rt.void_1p(
            self.cmd,
            SEL_MTL4_BEGIN_WITH_ALLOCATOR.1,
            self.allocators[idx],
        );
        rt.void_1p(self.cmd, SEL_MTL4_USE_RESIDENCY_SET.1, self.residency);
        Ok(idx)
    }

    /// frame 完了処理: endCommandBuffer → commit[1] (Hello Triangle
    /// `[commandQueue commit:&commandBuffer count:1]`) →
    /// signalEvent:value:frame_value。
    fn finish_frame4(&self, rt: &mut dyn ObjcRt) {
        rt.void_0(self.cmd, SEL_MTL4_END_COMMAND_BUFFER.1);
        let arr: [ObjcId; 1] = [self.cmd];
        rt.void_2pu(
            self.queue4,
            SEL_MTL4_COMMIT_COUNT.1,
            arr.as_ptr() as ObjcId,
            1,
        );
        rt.void_2pu(
            self.queue4,
            SEL_MTL4_SIGNAL_EVENT_VALUE.1,
            self.shared_event,
            self.frame_value,
        );
    }

    /// MTL4 レンダーパス descriptor 組立 (target attach → Clear/Store)。
    fn make_pass4(rt: &mut dyn ObjcRt, target_tex: ObjcId) -> Result<ObjcId, DirectMetalError> {
        let pass = make_obj(rt, CLASS_MTL4_PASS_DESC)
            .ok_or(DirectMetalError::ClassMissing("MTL4RenderPassDescriptor"))?;
        let atts = rt.id_0(pass, SEL_MTL4_COLOR_ATTACHMENTS.1);
        if atts.is_null() {
            return Err(DirectMetalError::NullObject("MTL4 pass colorAttachments"));
        }
        let att0 = rt.id_1u(atts, SEL_OBJECT_AT_INDEXED_SUBSCRIPT.1, 0);
        rt.void_1p(att0, SEL_SET_TEXTURE.1, target_tex);
        rt.void_1u(att0, SEL_SET_LOAD_ACTION.1, MTLV_LOAD_CLEAR.2 as u64);
        rt.void_1u(att0, SEL_SET_STORE_ACTION.1, MTLV_STORE_STORE.2 as u64);
        rt.void_clearcolor(
            att0,
            SEL_SET_CLEAR_COLOR.1,
            MtlClearColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 1.0,
            },
        );
        Ok(pass)
    }

    /// encoder 生成 + pipeline/viewport 共通セット。
    fn begin_encoder4(
        &self,
        rt: &mut dyn ObjcRt,
        pass: ObjcId,
        pipeline: ObjcId,
    ) -> Result<ObjcId, DirectMetalError> {
        let enc = rt.id_1p(self.cmd, SEL_MTL4_RENDER_ENCODER.1, pass);
        if enc.is_null() {
            return Err(DirectMetalError::NullObject(
                "renderCommandEncoderWithDescriptor:",
            ));
        }
        rt.void_1p(enc, SEL_MTL4_SET_PIPELINE_STATE.1, pipeline);
        rt.void_viewport(
            enc,
            SEL_MTL4_SET_VIEWPORT.1,
            MtlViewport {
                origin_x: 0.0,
                origin_y: 0.0,
                width: self.base.width as f64,
                height: self.base.height as f64,
                znear: 0.0,
                zfar: 1.0,
            },
        );
        Ok(enc)
    }

    /// vbuf ring slot へ頂点書込 (contents memcpy、容量固定)。
    fn write_vertices4(
        &self,
        rt: &mut dyn ObjcRt,
        idx: usize,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        if verts.len() as u64 > VBUF4_CAP_FLOAT4 {
            return Err(DirectMetalError::VertexOverflow {
                have_float4: verts.len() as u64,
                capacity_float4: VBUF4_CAP_FLOAT4,
            });
        }
        let p = rt.ptr_0(self.vbufs[idx], SEL_CONTENTS.1);
        if p.is_null() {
            return Err(DirectMetalError::ContentsUnavailable);
        }
        let bytes = core::mem::size_of_val(verts);
        unsafe {
            core::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, p as *mut u8, bytes);
        }
        Ok(())
    }

    /// オフスクリーン MTL4 で頂点列 1 フレーム描画 (三角形群)。
    /// Apple Hello Triangle (Metal 4) の frame 進行を厳密踏襲。
    pub fn render_frame4(
        &mut self,
        rt: &mut dyn ObjcRt,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = self.render_frame4_inner(rt, verts);
        rt.pool_pop(pool);
        result
    }

    fn render_frame4_inner(
        &mut self,
        rt: &mut dyn ObjcRt,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        let idx = self.begin_frame4(rt)?;
        self.write_vertices4(rt, idx, verts)?;
        let pass = Self::make_pass4(rt, self.base.target)?;
        let enc = self.begin_encoder4(rt, pass, self.pipeline4)?;
        rt.void_0(pass, SEL_RELEASE.1);
        // buffer(0) = 当該 frame 専用 vbuf の GPU アドレス (ring idx)。
        rt.void_2uu(
            self.arg_table,
            SEL_MTL4_SET_ADDRESS_AT_INDEX.1,
            self.vbuf_addrs[idx],
            0,
        );
        rt.void_2pu(
            enc,
            SEL_MTL4_SET_ARGUMENT_TABLE_AT_STAGES.1,
            self.arg_table,
            MTLV_RSTAGE_VERTEX.2 as u64,
        );
        rt.void_3uuu(
            enc,
            SEL_MTL4_DRAW_PRIMITIVES.1,
            MTLV_PRIM_TRIANGLE.2 as u64,
            0,
            verts.len() as u64,
        );
        rt.void_0(enc, SEL_MTL4_END_ENCODING.1);
        self.finish_frame4(rt);
        Ok(())
    }

    /// オフスクリーン MTL4 で fullscreen textured quad 1 フレーム描画
    /// (texture(0) は tex_arg_table の MTLResourceID バインド、stage
    /// Vertex|Fragment、MSL の constexpr sampler に静的直結)。
    pub fn render_textured_quad4(
        &mut self,
        rt: &mut dyn ObjcRt,
        tex: ObjcId,
    ) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = self.render_textured_quad4_inner(rt, tex);
        rt.pool_pop(pool);
        result
    }

    fn render_textured_quad4_inner(
        &mut self,
        rt: &mut dyn ObjcRt,
        tex: ObjcId,
    ) -> Result<(), DirectMetalError> {
        let _idx = self.begin_frame4(rt)?;
        let pass = Self::make_pass4(rt, self.base.target)?;
        let enc = self.begin_encoder4(rt, pass, self.tex_pipeline4)?;
        rt.void_0(pass, SEL_RELEASE.1);
        let rid = rt.u64_0(tex, SEL_MTL4_GPU_RESOURCE_ID.1);
        rt.void_2uu(self.tex_arg_table, SEL_MTL4_SET_TEXTURE_AT_INDEX.1, rid, 0);
        rt.void_2pu(
            enc,
            SEL_MTL4_SET_ARGUMENT_TABLE_AT_STAGES.1,
            self.tex_arg_table,
            (MTLV_RSTAGE_VERTEX.2 | MTLV_RSTAGE_FRAGMENT.2) as u64,
        );
        rt.void_3uuu(
            enc,
            SEL_MTL4_DRAW_PRIMITIVES.1,
            MTLV_PRIM_TRIANGLE.2 as u64,
            0,
            3,
        );
        rt.void_0(enc, SEL_MTL4_END_ENCODING.1);
        self.finish_frame4(rt);
        Ok(())
    }

    /// CAMetalLayer への MTL4 present 経路 (Hello Triangle present 列の厳密形):
    /// drawable 取得 → waitForDrawable (encode 前 GPU 待機) → pass/encode →
    /// endCommandBuffer → commit → signalDrawable → present → signalEvent。
    /// 三角形描画 (verts ∈ [[f32;4]])。
    pub fn render_to_layer4(
        &mut self,
        rt: &mut dyn ObjcRt,
        layer: ObjcId,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        let pool = rt.pool_push();
        let result = self.render_to_layer4_inner(rt, layer, verts);
        rt.pool_pop(pool);
        result
    }

    fn render_to_layer4_inner(
        &mut self,
        rt: &mut dyn ObjcRt,
        layer: ObjcId,
        verts: &[[f32; 4]],
    ) -> Result<(), DirectMetalError> {
        let idx = self.begin_frame4(rt)?;
        self.write_vertices4(rt, idx, verts)?;
        let drawable = rt.id_0(layer, SEL_NEXT_DRAWABLE.1);
        if drawable.is_null() {
            return Err(DirectMetalError::NoDrawable);
        }
        // GPU 側 attach 前に drawable 利用完了を queue 待機 (一次情報厳密順序)。
        rt.void_1p(self.queue4, SEL_MTL4_WAIT_DRAWABLE.1, drawable);
        let target = rt.id_0(drawable, SEL_DRAWABLE_TEXTURE.1);
        if target.is_null() {
            return Err(DirectMetalError::NullObject("drawable texture"));
        }
        let pass = Self::make_pass4(rt, target)?;
        let enc = self.begin_encoder4(rt, pass, self.pipeline4)?;
        rt.void_0(pass, SEL_RELEASE.1);
        rt.void_2uu(
            self.arg_table,
            SEL_MTL4_SET_ADDRESS_AT_INDEX.1,
            self.vbuf_addrs[idx],
            0,
        );
        rt.void_2pu(
            enc,
            SEL_MTL4_SET_ARGUMENT_TABLE_AT_STAGES.1,
            self.arg_table,
            MTLV_RSTAGE_VERTEX.2 as u64,
        );
        rt.void_3uuu(
            enc,
            SEL_MTL4_DRAW_PRIMITIVES.1,
            MTLV_PRIM_TRIANGLE.2 as u64,
            0,
            verts.len() as u64,
        );
        rt.void_0(enc, SEL_MTL4_END_ENCODING.1);
        rt.void_0(self.cmd, SEL_MTL4_END_COMMAND_BUFFER.1);
        let arr: [ObjcId; 1] = [self.cmd];
        rt.void_2pu(
            self.queue4,
            SEL_MTL4_COMMIT_COUNT.1,
            arr.as_ptr() as ObjcId,
            1,
        );
        // commit 後に drawable usable 完了をシグナルし present (一次情報厳密形)。
        rt.void_1p(self.queue4, SEL_MTL4_SIGNAL_DRAWABLE.1, drawable);
        rt.void_0(drawable, SEL_MTL4_PRESENT.1);
        rt.void_2pu(
            self.queue4,
            SEL_MTL4_SIGNAL_EVENT_VALUE.1,
            self.shared_event,
            self.frame_value,
        );
        Ok(())
    }

    /// 全 Owned オブジェクトを canon 規則で release (base 側は
    /// metal_direct::shutdown が処理)。device = Borrowed のため対象外。
    pub fn shutdown(self, rt: &mut dyn ObjcRt) {
        for obj in [
            self.residency,
            self.vbufs[2],
            self.vbufs[1],
            self.vbufs[0],
            self.tex_arg_table,
            self.arg_table,
            self.tex_pipeline4,
            self.pipeline4,
            self.shared_event,
            self.allocators[2],
            self.allocators[1],
            self.allocators[0],
            self.cmd,
            self.queue4,
        ] {
            if !obj.is_null() {
                rt.void_0(obj, SEL_RELEASE.1);
            }
        }
        self.base.shutdown(rt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objc_rt::{CallRec, MockObjcRt};

    /// v3 三角形 (render_frame4 系の検証入力、TRIANGLE 色つき)。
    const TRI3: [[f32; 4]; 3] = [
        [0.0, 0.8, 1.0, 0.2],
        [-0.8, -0.8, 0.2, 1.0],
        [0.8, -0.8, 0.2, 0.2],
    ];

    /// classic 基部 + MTL4 全応答の fixture (canon 所有規則と 1:1 一致 —
    /// 監査機 R5 が on_make=Owned/on_borrow=Borrowed を機械照合)。
    fn fixture4() -> MockObjcRt {
        let mut rt = MockObjcRt::new();
        // ---- classic 基部 (metal_direct::create_with_format の全応答) ----
        rt.on_make("MTLDevice", "newCommandQueue", "MTLCommandQueue");
        rt.on_make("NSString", "alloc", "NSString");
        rt.on_make(
            "MTLDevice",
            "newLibraryWithSource:options:error:",
            "MTLLibrary",
        );
        rt.on_make("MTLLibrary", "newFunctionWithName:", "MTLFunction");
        rt.on_make(
            "MTLDevice",
            "newRenderPipelineStateWithDescriptor:error:",
            "MTLRenderPipelineState",
        );
        rt.on_borrow(
            "MTLRenderPipelineDescriptor",
            "colorAttachments",
            "MTLRenderPassColorAttachmentDescriptorArray",
        );
        rt.on_borrow(
            "MTLRenderPassColorAttachmentDescriptorArray",
            "objectAtIndexedSubscript:",
            "MTLRenderPassColorAttachmentDescriptor",
        );
        rt.on_make(
            "MTLRenderPipelineDescriptor",
            "new",
            "MTLRenderPipelineDescriptor",
        );
        rt.on_make("MTLDevice", "newBufferWithLength:options:", "MTLBuffer");
        rt.on_make(
            "MTLDevice",
            "newBufferWithBytes:length:options:",
            "MTLBuffer",
        );
        rt.on_ptr("MTLBuffer", "contents");
        rt.on_borrow(
            "MTLTextureDescriptor",
            "texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
            "MTLTextureDescriptor",
        );
        rt.on_make("MTLDevice", "newTextureWithDescriptor:", "MTLTexture");
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
        rt.on_borrow("NSError", "localizedDescription", "NSString");
        rt.on_borrow("MTLCommandBuffer", "error", "NSError");
        rt.on_make("CAMetalLayer", "alloc", "CAMetalLayer");
        rt.on_borrow("CAMetalLayer", "nextDrawable", "CAMetalDrawable");
        rt.on_borrow("CAMetalDrawable", "texture", "MTLTexture");
        rt.on_u64(
            "MTLCommandBuffer",
            "status",
            crate::metal_direct::MTLV_CB_COMPLETED.2 as u64,
        );
        rt.on_u64("MTLTexture", "width", 64);
        rt.on_u64("MTLTexture", "height", 64);
        // ---- MTL4 (canon SDK26: 生成系は全て Owned) ----
        rt.on_make("MTLDevice", "newMTL4CommandQueue", "MTL4CommandQueue");
        rt.on_make("MTLDevice", "newCommandBuffer", "MTL4CommandBuffer");
        rt.on_make(
            "MTLDevice",
            "newCommandAllocatorWithDescriptor:error:",
            "MTL4CommandAllocator",
        );
        rt.on_make(
            "MTLDevice",
            "newArgumentTableWithDescriptor:error:",
            "MTL4ArgumentTable",
        );
        rt.on_make(
            "MTLDevice",
            "newCompilerWithDescriptor:error:",
            "MTL4Compiler",
        );
        rt.on_make(
            "MTLDevice",
            "newResidencySetWithDescriptor:error:",
            "MTLResidencySet",
        );
        rt.on_make("MTLDevice", "newSharedEvent", "MTLSharedEvent");
        rt.on_make(
            "MTL4Compiler",
            "newRenderPipelineStateWithDescriptor:compilerTaskOptions:error:",
            "MTLRenderPipelineState",
        );
        // descriptor class objects (alloc→init)。spawn の帰属 class は
        // 自身の class 名 (= 後続 property getter の fixture 解決と整合)。
        for c in [
            "MTL4CommandAllocatorDescriptor",
            "MTL4CompilerDescriptor",
            "MTL4RenderPipelineDescriptor",
            "MTL4LibraryFunctionDescriptor",
            "MTL4ArgumentTableDescriptor",
            "MTLResidencySetDescriptor",
            "MTL4RenderPassDescriptor",
        ] {
            rt.on_make(c, "alloc", c);
        }
        rt.on_borrow(
            "MTL4RenderPipelineDescriptor",
            "colorAttachments",
            "MTL4RenderPipelineColorAttachmentDescriptorArray",
        );
        rt.on_borrow(
            "MTL4RenderPipelineColorAttachmentDescriptorArray",
            "objectAtIndexedSubscript:",
            "MTL4RenderPipelineColorAttachmentDescriptor",
        );
        rt.on_borrow(
            "MTL4RenderPassDescriptor",
            "colorAttachments",
            "MTLRenderPassColorAttachmentDescriptorArray",
        );
        // MTL4 encoder 生成応答 (canon Borrowed — nullable id 戻り)。
        rt.on_borrow(
            "MTL4CommandBuffer",
            "renderCommandEncoderWithDescriptor:",
            "MTL4RenderCommandEncoder",
        );
        // MTL4 数値応答: vbuf gpuAddress / texture gpuResourceID。
        rt.on_u64("MTLBuffer", "gpuAddress", 0x4000_0000);
        rt.on_u64("MTLTexture", "gpuResourceID", 0x77);
        // supportsFamily(Metal4) = true (実機 Apple Silicon 相当応答)。
        rt.on_u64("MTLDevice", "supportsFamily:", 1);
        rt
    }

    /// create4 の完全呼出 (OBJC メモリ規則: live は去る) を物理検証。
    #[test]
    fn go_create4_full_sequence_and_live_free() {
        let mut rt = fixture4();
        let m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        assert!(!m4.queue4.is_null());
        assert_eq!(m4.frame_value, 0);
        assert_ne!(m4.allocators[0], m4.allocators[1]);
        assert_ne!(m4.allocators[1], m4.allocators[2]);
        assert_ne!(m4.vbufs[0], m4.vbufs[1]);
        assert_eq!(m4.vbuf_addrs, [0x4000_0000; 3]);
        // MTL4 必須呼出列 (順序不動点 — Apple 定石形の物理 pin)。
        let calls: Vec<(String, String)> = rt
            .calls
            .iter()
            .map(|c| (c.method.to_string(), c.sel.clone()))
            .collect();
        let joined: Vec<String> = calls.iter().map(|(a, b)| format!("{a} {b}")).collect();
        for must in [
            "id_0 newMTL4CommandQueue",
            "id_0 newCommandBuffer",
            "id_2pp newCommandAllocatorWithDescriptor:error:",
            "id_0 newSharedEvent",
            "id_2pp newCompilerWithDescriptor:error:",
            "id_3ppp newRenderPipelineStateWithDescriptor:compilerTaskOptions:error:",
            "id_2pp newArgumentTableWithDescriptor:error:",
            "id_2pp newResidencySetWithDescriptor:error:",
            "void_1u setMaxBufferBindCount:",
            "void_1u setMaxTextureBindCount:",
            "u64_0 gpuAddress",
            "void_1p addAllocation:",
            "void_0 commit",
            "void_1p addResidencySet:",
        ] {
            assert!(
                joined.iter().any(|x| x == must),
                "create4 呼出列に `{must}` が必要: {joined:?}"
            );
        }
        // residency: vbuf 3 + target 1 の 4 allocation。
        let adds = rt
            .calls
            .iter()
            .filter(|c| c.sel == "addAllocation:")
            .count();
        assert_eq!(adds, 4, "vbuf×3+target の residency 登録");
        // Metal4 gate: supportsFamily が値 5002 で呼ばれた実証 (値ログ)。
        assert!(
            rt.u64_value_log
                .iter()
                .any(|(m, s, v)| *m == "bool_1u" && s == "supportsFamily:" && v[0] == 5002),
            "Metal4=5002 の gate 呼出が値ログに必要: {:?}",
            rt.u64_value_log
        );
        // descriptor 上限値の実値 pin (buffer 1 / texture 1)。
        let maxb: Vec<&Vec<u64>> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_1u" && s == "setMaxBufferBindCount:")
            .map(|(_, _, v)| v)
            .collect();
        assert!(maxb.iter().any(|v| v[0] == 0) && maxb.iter().any(|v| v[0] == 1));
        m4.shutdown(&mut rt);
        assert!(
            rt.live.is_empty(),
            "shutdown 後 live は空 (リーク 0): {:?}",
            rt.live
        );
    }

    /// render_frame4 ×4: in-flight ring と frame 3 以降の event 待機機構を
    /// 実値で pin (Hello Triangle 厳密形の動作保証)。
    #[test]
    fn go_frame4_rotation_wait_mechanism_golden() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        rt.u64_value_log.clear();
        for _ in 0..4 {
            m4.render_frame4(&mut rt, &TRI3).expect("frame4");
        }
        assert_eq!(m4.frame_value, 4);
        // waitUntilSignaledValue は frame 3,4 で発火 (>=3) — awaited = f-3。
        let waits: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "bool_2uu" && s == "waitUntilSignaledValue:timeoutMS:")
            .map(|(_, _, v)| v[0])
            .collect();
        assert_eq!(waits, vec![0, 1], "frame3→0/frame4→1 待機 (f-3)");
        // timeout は一次情報 10ms 固定。
        let timeouts: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "bool_2uu" && s == "waitUntilSignaledValue:timeoutMS:")
            .map(|(_, _, v)| v[1])
            .collect();
        assert_eq!(timeouts, vec![10, 10]);
        // signalEvent:value: は frame 毎に厳密値。
        let sig: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_2pu" && s == "signalEvent:value:")
            .map(|(_, _, v)| v[0])
            .collect();
        assert_eq!(sig, vec![1, 2, 3, 4]);
        // commit:count: は毎 frame count=1。
        let commits: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_2pu" && s == "commit:count:")
            .map(|(_, _, v)| v[0])
            .collect();
        assert_eq!(commits, vec![1, 1, 1, 1]);
        // allocator reset は 4 回 (ring 全員 1 度以上)。
        let resets = rt
            .calls
            .iter()
            .filter(|c| c.sel == "reset" && c.method == "void_0")
            .count();
        assert_eq!(
            resets,
            4,
            "reset not recorded; sels={:?}",
            rt.calls.iter().map(|c| c.sel.as_str()).collect::<Vec<_>>()
        );
        // setAddress:atIndex: の index=0・address は ring 依らず cache 値。
        let addrs: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_2uu" && s == "setAddress:atIndex:")
            .map(|(_, _, v)| v[1])
            .collect();
        assert_eq!(addrs, vec![0, 0, 0, 0], "buffer slot は 0 (Hello Triangle)");
        // draw は毎回 Triangle/0/3。
        let draws: Vec<&Vec<u64>> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_3uuu" && s == "drawPrimitives:vertexStart:vertexCount:")
            .map(|(_, _, v)| v)
            .collect();
        assert_eq!(draws.len(), 4);
        for d in draws {
            assert_eq!(d.as_slice(), &[3, 0, 3]);
        }
        m4.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "leak 0: {:?}", rt.live);
    }

    /// textured quad: texture(0) ResourceID バインド + stage Vertex|Fragment。
    #[test]
    fn go_textured_quad4_resource_id_binding() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        rt.u64_value_log.clear();
        // tex 入力: mock の base.target 相当 (MTLTexture id)。
        let tex = rt.mk("MTLTexture");
        m4.render_textured_quad4(&mut rt, tex).expect("quad4");
        let rids: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_2uu" && s == "setTexture:atIndex:")
            .map(|(_, _, v)| v[0])
            .collect();
        assert_eq!(rids, vec![0x77], "gpuResourceID 応答値がそのまま流れる");
        let stages: Vec<u64> = rt
            .u64_value_log
            .iter()
            .filter(|(m, s, _)| *m == "void_2pu" && s == "setArgumentTable:atStages:")
            .map(|(_, _, v)| v[0])
            .collect();
        assert_eq!(stages, vec![3], "Vertex|Fragment mask");
        // tex を mk した分は手放す (本番では caller 所有: textured 描画は
        // 借用規則 — ObjC ARC 慣行で bind が参照を複製保持しない設計)。
        rt.void_0(tex, SEL_RELEASE.1);
        m4.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "leak 0: {:?}", rt.live);
    }

    /// present 経路: waitForDrawable→encode→commit→signalDrawable→present の
    /// 厳密順序 (Hello Triangle DrawingContent 一次情報) を呼出列で pin。
    #[test]
    fn go_render_to_layer4_present_sequence() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        // layer attach (classic 経路で layer を所有化)。
        let layer = m4.base.attach_layer(&mut rt).expect("attach_layer");
        rt.calls.clear();
        rt.u64_value_log.clear();
        m4.render_to_layer4(&mut rt, layer, &TRI3).expect("layer4");
        let sels: Vec<&str> = rt.calls.iter().map(|c| c.sel.as_str()).collect();
        // 厳密順序不変条件: waitForDrawable < commit < signalDrawable < present
        // < signalEvent:value:。
        let pos = |s: &str| {
            sels.iter()
                .position(|x| *x == s)
                .unwrap_or_else(|| panic!("`{s}` が呼出列にない: {sels:?}"))
        };
        assert!(
            pos("waitForDrawable:") < pos("commit:count:"),
            "wait は commit 前: {sels:?}"
        );
        assert!(pos("commit:count:") < pos("signalDrawable:"));
        assert!(pos("signalDrawable:") < pos("present"));
        assert!(pos("present") < pos("signalEvent:value:"));
        // drawable texture を pass target に使った証跡 (setTexture: 呼出)。
        assert!(sels.contains(&"texture"));
        m4.shutdown(&mut rt);
        assert!(rt.live.is_empty(), "leak 0: {:?}", rt.live);
    }

    /// Metal4 非対応デバイスは fail-loud (gate 命令形)。
    #[test]
    fn go_create4_metal4_unsupported_fails_loud() {
        let mut rt = fixture4();
        // gate 応答を 0 へ (実機 Intel 世代相当)。
        rt.on_u64("MTLDevice", "supportsFamily:", 0);
        let err = DirectMetal4::create4(&mut rt, 320, 240).err().unwrap();
        assert_eq!(err, DirectMetalError::Metal4Unsupported);
    }

    /// event 待機 timeout は FrameSyncTimeout で詳細化 (失敗の黙殺なし)。
    #[test]
    fn go_frame_sync_timeout_details() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        // shared_event wait 応答を 0 (timeout) へ。
        rt.on_u64("MTLSharedEvent", "waitUntilSignaledValue:timeoutMS:", 0);
        m4.render_frame4(&mut rt, &TRI3).expect("f1");
        m4.render_frame4(&mut rt, &TRI3).expect("f2");
        let err = m4.render_frame4(&mut rt, &TRI3).unwrap_err();
        assert_eq!(
            err,
            DirectMetalError::FrameSyncTimeout {
                frame: 3,
                awaited: 0
            }
        );
    }

    /// vbuf 容量超過は VertexOverflow (overflow 時描画しない安全側)。
    #[test]
    fn go_vertex_overflow_guard() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        let too_many = vec![[0.0f32; 4]; (VBUF4_CAP_FLOAT4 + 1) as usize];
        let err = m4.render_frame4(&mut rt, &too_many).unwrap_err();
        assert_eq!(
            err,
            DirectMetalError::VertexOverflow {
                have_float4: VBUF4_CAP_FLOAT4 + 1,
                capacity_float4: VBUF4_CAP_FLOAT4,
            }
        );
    }

    /// contents が使えない異常環境は ContentsUnavailable で fail-loud。
    #[test]
    fn go_contents_unavailable_fails_loud() {
        let mut rt = fixture4();
        // contents override を消して null 応答化。
        rt.on_ptr_off("MTLBuffer", "contents");
        let err = DirectMetal4::create4(&mut rt, 320, 240).err().unwrap();
        // classic 基部の vbuf 確保が先に失敗 (create_with_format 内で検知)。
        assert_eq!(err, DirectMetalError::ContentsUnavailable);
    }

    /// 呼出 DSL レコード経路: CallRec 形状が壊れていないことの pin
    /// (mock 拡張による既存記録系の非破壊立証)。
    #[test]
    fn go_callrec_shapes_unbroken() {
        let mut rt = fixture4();
        let mut m4 = DirectMetal4::create4(&mut rt, 320, 240).expect("create4");
        rt.calls.clear();
        m4.render_frame4(&mut rt, &TRI3).expect("frame4");
        assert!(rt.calls.contains(&CallRec {
            method: "void_2uu",
            sel: "setAddress:atIndex:".to_string(),
            args_shape: "u,u",
        }));
        assert!(rt.calls.contains(&CallRec {
            method: "void_2pu",
            sel: "commit:count:".to_string(),
            args_shape: "p,u",
        }));
        m4.shutdown(&mut rt);
    }
}
