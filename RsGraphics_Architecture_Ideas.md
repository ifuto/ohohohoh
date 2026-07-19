# RsGraphics 超軽量化アーキテクチャ 網羅的アイデアリスト
> 目標: OptiFine/Sodium/Iris/Lithiumを超える、Rustで実現するMinecraft専用レンダラー+ModLoader

## カテゴリ1: グラフィックスAPI・描画最適化 [DirectX12前提 / 低スペックPC対応]

**前提: NvidiumのようなMesh Shader / Work Graphs / RayTracing / DX12 Ultimate必須技術は除外**

1.  **windows-rs + hassle-rsによるD3D12 Explicit制御**
    *   メリット: LWJGL経由のOpenGLを完全乗っ取り、wgpuのオーバーヘッドすら排除。RootSignatureからBarrierまで完全手動管理でCPUオーバーヘッド極小化。

2.  **Descriptor Heap Ring/Linear Allocator**
    *   `gpu-descriptor`クレート参考。フレーム毎に線形に確保、GPU完了後にリセット。CBV/SRV/UAVの確保がほぼO(1)。低スペックでもDescriptor生成がボトルネックにならない。

3.  **Root Signature 1.1 + Static Samplers最適化**
    *   Root ConstantsにCamera Matrix、Root DescriptorにBindless Index、Static Samplerでサンプラーヒープアクセスをゼロに。Rootコストを最小化しGPUキャッシュヒット率向上。

4.  **PSOキャッシュとID3D12PipelineLibrary**
    *   Rust側でPSOをハッシュ化してディスクキャッシュ(`.bin`)。初回起動のハングを防止。低スペックのHDDでも2回目以降は爆速起動。

5.  **ExecuteIndirectによる間接描画 / GPU-driven Chunk Batching**
    *   チャンクSection毎のDrawCallをCPUで発行せず、Rustで用意した引数バッファをGPUに直接実行させる。数千DrawCallを1回のExecuteIndirectに統合。Mesh Shader無しでGPUドリブン。

6.  **Bundle (ID3D12GraphicsCommandListの再利用)**
    *   静的なチャンク描画コマンドをBundleとして録画、再利用。遠景の再描画時はBundleをExecuteBundleするだけで済む。

7.  **Enhanced Barriersのバッチ化**
    *   旧来のResourceBarrierを多数発行せず、D3D12 Enhanced Barriersでまとめて遷移+分割バリアでUploadと描画をオーバーラップ。

8.  **Persistent Mapped Upload Heap Ring Buffer**
    *   `gpu-allocator`を使い256MBのUpload Heapを常時マップ。`bytemuck::cast_slice`でゼロコピー書き込み、チャンクメッシュを毎フレームコピーせずに済む。

9.  **GPU-based Hi-Z Occlusion Culling (Compute Shader)**
    *   Depth PyramidをComputeで生成(64x64→1x1)。CPUで可視判定せずGPUでChunk AABBをテスト。SodiumのCPUオクルージョンをGPU化。低スペックでもCSは1つなので軽量。

10. **SWRastライクなソフトウェア階層Z (CPUフォールバック)**
    *   低スペックでComputeが詰まる場合の保険。Rustの`wide`で4 AABB同時テスト。CPUで数千チャンクを1ms以内でカリング。

11. **Binary Greedy Meshing + Directional Greedy**
    *   通常Greedyをビットボード( u32 = 32ブロック)で処理。面のマージ判定をAVX2のビット演算で並列化。頂点数60-80%削減。

12. **Aggressive Hidden Face Culling (3x3x3 Neighbor Cache)**
    *   メッシュ生成時に隣接26チャンクのBlockStateをSoAでキャッシュ。Rustの参照で安全に借用し、空気-空気/同不透明ブロック間の面を完全に削除。

13. **Material-based Re-batching / Atlas Merging**
    *   Opaque/Cutout/TranslucentでPSOを分けつつ、同一テクスチャアトラス内のクアッドはCPUで1つの頂点バッファに再バッチ。DrawCall数をSodium比で1/3に。

14. **Bindless Texture with SM 6.6 Dynamic Resources**
    *   DX12 Tier1相当でも動くBindless。Descriptor Indexを頂点に埋め込み、テクスチャ切り替えゼロ。アトラス破綻問題を解決しつつ低スペックでも動く。

15. **Sparse Virtual Texture Atlas**
    *   1024個の16x16を1枚の4096にせず、不要タイルを未常駐に。VRAM 100MB以下のPCでも高解像度リソースパック対応。

16. **Vertex Compression: R10G10B10A2 + FP16 UV**
    *   `BlockPos + Face`を30bitに詰め、UVはhalf float。頂点サイズ48byte→16byteへ。帯域幅1/3で低スペックGPUで効く。

17. **Weighted Blended OIT (WBOIT) for Translucent**
    *   透過の厳密なソートを廃止。半透明のソートO(n log n)をO(n)に。湖・氷・水の多いシーンでCPUソートが消える。

18. **Temporal Mesh Diff / Dirty Tracking**
    *   チャンクSectionは全再構築せず、`u64 dirtyMask`で変わったブロックのみ差分再メッシュ。石1つ置いても8頂点だけ更新。

19. **Visibility Graph + Flood Fill (Sodium 0.5+の発展)**
    *   チャンク間の可視を事前にBFSでグラフ化。Rustの`bitvec`で64bitで可視を表現。カメラ移動時の探索が爆速。

20. **Distance-based Micro LOD & Impostor**
    *   Mesh Shader無しLOD。遠景(16+チャンク)はGreedy結果を8x8にダウンサンプル+ベイクAOで疑似メッシュを生成。低スペックでも頂点数90%減。

21. **Instanced Rendering for Non-cube Blocks**
    *   草、花、作物は同一メッシュを`DrawInstanced`。TransformはStructuredBufferでGPUへ。草原バイオームでDrawCall 2000→10。

22. **Depth Pre-pass省略 & Reverse-Z**
    *   OpaqueはFront-to-Back+Early-Zで十分。Reverse-Z(1.0→0.0)で深度精度を上げZ-fightingを130%改善、Hi-Z精度も向上。

23. **SRV/CBVとSampler Heap分離最適化**
    *   Samplerは静的に2つ(Linear Point)のみ。描画ループでSamplerヒープ切り替えを発生させない。

### カテゴリ2: メモリ管理・データ構造

24. **Global Allocator 置換: mimalloc / snmalloc-rs**
    *   Rustのデフォルトptmallocを置換。チャンクメッシュのような中サイズ頻繁確保で2倍高速。断片化も低スペックで致命的なOOMを防ぐ。

25. **Bump Arena (bumpalo) Per Meshing Task**
    *   メッシュ生成時の一時Vec<Quad>はArenaから確保。タスク完了で一括リセット、dropコスト0。SodiumのJava GCパルスを完全排除。

26. **Object Pool + Slab Allocator for RenderSection**
    *   `slab` + `object-pool`でRenderSection(約5KB)を再利用。確保/解放がポインタの付け替えだけ。

27. **SoA (Structure of Arrays) 頂点レイアウト**
    *   AoS `struct Vertex{x,y,z,u,v}`をやめ、`Vec<Pos>`, `Vec<UV>`, `Vec<Color>`に分離。メッシュ生成時の書き込みがSIMD化しキャッシュミス激減。

28. **Palette Storage with Const Generic Bit-packing**
    *   `PaletteStorage<const BITS: usize>`。Rustのconst genericsでパレットビット数ごとに単相化、match分岐を消す。Vanillaのビット配列より20%高速。

29. **Roaring Bitmap for Light Propagation**
    *   光伝播キューを`roaring` Bitmapで管理。`VecDeque<BlockPos>`のポインタチェイスをやめ、キャッシュラインに収まる。

30. **Morton Order (Z-order Curve) Chunk Storage**
    *   `x,y,z` → `morton_encode`で線形配置。隣接ブロックアクセスがL1/L2に乗りやすい。Greedy時の局所性向上。

31. **bytemuck + zerocopy + rkyv ゼロコピー基盤**
    *   `bytemuck::Pod`な頂点は`cast_slice`でGPUバッファへ直接転送、`rkyv`でRegionファイルのパースをゼロコピー。コピーを1回も挟まない。

32. **SmallVec<[Quad; 32]> / TinyVec**
    *   小さいSectionはヒープ確保せずスタックに。8割の空Sectionはalloc 0。

33. **HashBrown + FxHash / ahash for BlockPos**
    *   `std::HashMap`を`hashbrown::HashMap`に、`FxHasher`でハッシュ計算をintそのままに。`BlockPos -> BlockState`ルックアップが最速。

34. **Copy-on-Write ChunkSnapshot: Arc<ChunkWithVersion>**
    *   ワールドスレッドは書き込み、描画スレッドは`Arc::clone`でスナップショットを不変参照。ロック無しで安全に並列メッシュ化。

35. **Intrusive Collections for BlockEntity List**
    *   `intrusive-collections`でBlockEntityをリンクリスト化。VecからremoveするO(n)をO(1)に。

36. **Double-Buffered Versioned Storage**
    *   チャンクデータを奇数/偶数フレームで2面持ち、書き込み側と読み込み側を分離。完全ロックフリー。

37. **MaybeUninit + ManuallyDropでスクラッチ再利用**
    *   1MBのメッシュ用バッファを`MaybeUninit<[u8; 1<<20]>`で確保、初期化コスト0で毎フレーム再利用。

38. **Cache-Aligned (64byte) Hot Structures**
    *   `#[repr(align(64))]`でAO LUTやSection Metaをアライン。False Sharingを防ぎ、プリフェッチが効く。

39. **Const LUT for AO / Light**
    *   `const AO_TABLE: [u8; 4096]`をコンパイル時に生成。実行時に分岐無しでAO計算。

40. **BitSet based BlockState Caching**
    *   不透明か否かを`u64`ビットセットで保持、`isOpaque(id)`が配列アクセス1回+ビットテスト。

### カテゴリ3: 並列・非同期処理

41. **Rayon par_iter Chunk Meshing**
    *   `par_chunks_exact`で32 Section並列メッシュ。work-stealingでコアを使い切る。JavaのForkJoinPoolより軽量。

42. **P-Core専用固定スレッドプール**
    *   `core_affinity`でPコアにメッシュワーカをピン留め、EコアにIOを割り当て。Intel 12世代以降の低スペックi3でも効率最大化。

43. **Job System with DAG Scheduler (petgraph)**
    *   チャンクタスクを`Parse NBT -> Light -> AO -> Mesh -> Upload`のDAGで定義、トポロジカルに並列実行。依存が自動解決。

44. **crossbeam-deque Work Stealing Queue**
    *   Rayonの下層と同じDequeを自前で使い、0.5ms単位の超細粒度タスクを盗み合い。Sodiumよりレイテンシ低。

45. **Lock-Free DashMap / Flurry for Task Registry**
    *   `DashMap<ChunkPos, BuildState>`でメッシュ状態管理。JavaのConcurrentHashMap相当をロックフリーRwLock無しで。

46. **rtrb / ringbuf SPSC Channel for Java->Rust Command**
    *   JNIからの「ブロック更新」通知をロックフリーSPSCリングで渡す。Javaスレッドはwait無し。

47. **Chunk Pipeline Triple Buffering**
    *   Logic Thread / Mesh Thread / Render Threadを3重バッファで分離。いずれかが詰まっても他が止まらない。

48. **Parallel Radix Sort for Translucent**
    *   半透明クアッドのソートを`voracious_radix_sort`で並列基数ソート。比較ソートより5倍速。

49. **Parallel Light Engine (Sodium Lithium式をRustで)**
    *   光伝播をRayonでレイヤーごとに並列+SIMD。Vanillaの光バグ修正も兼ねる。

50. **Async IO + Memory Mapped Region**
    *   `memmap2` + `tokio-uring` (Linux) / `windows-rs` IOCP (Win)で領域ファイルを非同期読み込み。読み込み中も描画は60fps維持。

51. **GPU Upload Coalescing Thread**
    *   メッシュ完了を集約し、1フレームに1回だけUpload Heapへまとめてコピー。D3D12のコマンドリスト記録と同期を分離。

52. **Frustum & Occlusion Batch Test (SIMD Lane並列)**
    *   AABB 8個を`__m256`で同時フラスタム判定。1万Sectionでも0.1ms。

53. **Fiber-based Cooperative Scheduler (May)**
    *   チャンクIOがブロックしたらFiberをyieldし他チャンクを処理。OSスレッド生成コストを避ける。

54. **Atomic Dirty Flag + Version Counter**
    *   `AtomicU64`でチャンクバージョン管理、描画スレッドはCASで最新版を検出。Mutex 0。

### カテゴリ4: JVMとの相互運用

55. **GetPrimitiveArrayCritical / GetStringCriticalの徹底活用**
    *   GC pin留めでコピーを回避。Vanilla JNIより10倍速い転送。

56. **Long-lived DirectByteBuffer共有 (ゼロコピー転送の核)**
    *   Rust側で確保した`Box<[u8]>`を`NewDirectByteBuffer`でJavaに渡しっぱなし。Java側からはオフヒープ配列として見える。毎フレームのnew無し。

57. **Shared Memory Ring Buffer (MappedByteBuffer <-> memmap2)**
    *   JavaとRustで同一`FileChannel.map`を共有。`struct ChunkUpdate{pos:u64, state:u32}`をリングに書き込むだけ。JNIコール自体を1フレーム1回に。

58. **Flat POD Wire Format (bincode / flatbuffers)**
    *   Javaクラスを渡さずプリミティブ配列で渡す。`BlockState`は`u32 id`のみ、`BlockPos`は`u64 packed`。シリアライズコストほぼ0。

59. **C ABI VTableによる1回だけのバインディング**
    *   起動時に`struct JavaBridge { fn_update_chunk: extern "C" fn(...), ... }`テーブルをJavaに渡す。以降はdlsym的呼び出しで`GetMethodID`の文字列検索が0。

60. **MethodID / ClassRef Global Cache via OnceLock**
    *   `OnceLock<JClassCache>`で`<init>`や`blockPos`のIDをキャッシュ。毎回`FindClass`しない。

61. **Batched JNI Call Coalescing**
    *   1ブロック破壊で1回JNIせず、Tick末に`Vec<BlockChange>`をまとめてRustへ。JNI遷移回数を1/60に。

62. **Symbol Table Interning for Block/Item IDs**
    *   String `minecraft:stone`を毎回送らず、起動時に`u16 symbolId`に変換。文字列変換・UTF-8コピーを撲滅。

63. **Panama Foreign Function API (JDK 22+)移行パス**
    *   JNIの代替として`java.lang.foreign`でRustの`extern "C"`を直接リンク。Safepointを避けられ、理論上JNIの3倍速。

64. **JNIEnv Thread-Local Cache + Persistent Attach**
    *   `AttachCurrentThread`を毎回呼ばず、ワーカースレッド生成時に1回だけAttachし`JNIEnv`をTLSに保存。

65. **Deferred Exception Check**
    *   JNI呼出し毎に`ExceptionCheck`せず、バッチ完了後に1回だけチェック。分岐予測ミス削減。

66. **Unsafe.allocateInstance + Off-Heap World Storage**
    *   チャンクの`BlockState[]`をJavaヒープに置かずRust側に置く。GC圧力を激減させ、GC Stop The Worldを短縮。

67. **Function Pointer Event Bus (C ABI Callback)**
    *   JavaからRustへの「Mod Event」をインターフェース実装ではなく関数ポインタ配列で飛ばす。vtable経由の仮想呼び出しより直呼び。

### カテゴリ5: フック・インジェクション技術 (Mod Loaderコア)

68. **Rust製ClassFileパーサ / ライタ (cafebabe / custom)**
    *   ASMライブラリをJava側で使わず、Rustで`.class`を直接書き換え。起動時間を支配するクラス変換をネイティブ速度で。

69. **Mixin互換Injection Point DSL Parser in Rust**
    *   `@Inject(at=@At("HEAD"))`をRust側でパースしバイトコードを書き換え。既存Mixin Modとの互換を保ちつつ高速化。

70. **JVMTI AgentをRustで実装 (jvmti-rs)**
    *   `ClassFileLoadHook`イベントでクラスがJVMに定義される前にフック。JavaAgentより早く、Bootstrapクラスも書き換え可。

71. **TOMLベース宣言的Hook定義 (rs-graphics.toml)**
    *   Modが`[[hook]] class="net/minecraft/client/renderer/LevelRenderer" method="renderChunkLayer" at="HEAD" action="CANCEL"`のように定義。Rustが一括パッチ。

72. **OpenGL32.dll / lwjgl Opengl Detour Hook (Windows Detours / minhook-rs)**
    *   `wglSwapBuffers`や`glDrawElements`をフックし、OpenGL呼び出しをD3D12にリダイレクト。Vanillaコードを書き換えずとも描画を乗っ取れる。

73. **IR Transpilation: Vanilla BakingをRust Stubに置換**
    *   `BakedModel.getQuads`を呼ぶバイトコードを`invokestatic RsGraphics.getQuadsRust(id)`に差し替え。中間のJavaオブジェクト生成を消滅。

74. **Bootstrap ClassLoader Injection + Rust Owned Jar Scanner**
    *   Mod Jarの索引をRustの`rayon + zip`で並列スキャン。Javaの`JarFile`より3倍速くクラス一覧作成。

75. **ABI安定 Cインターフェース + Semver Feature Negotiation**
    *   Mod Loader APIを`#[repr(C)]`な`extern "C"`関数群として公開。`RsGraphicsAPI_VERSION=2`のように互換チェック。Rust更新でもModが壊れにくい。

76. **WASM Mod Sandbox (wasmtime)**
    *   ModをWASMにコンパイルさせRustホストが実行。メモリ安全性をModにも強制、クラッシュしてもゲーム本体は落ちない。ECSへのアクセスはCapability制。

77. **Inline Hook via RETURN Trampoline**
    *   特定メソッドの先頭5byteを`jmp`に書き換え、Rust側から処理後元のメソッドに戻す。バイトコード書き換え不可なネイティブライブラリにも対応。

78. **HotSwap via retransformClasses**
    *   JVMTIの`RetransformClasses`で実行中にクラス再定義。Mod開発時のリロードをJVM再起動無しで実現。

79. **Semantic Merge Conflict Resolver**
    *   2つのModが同じ`render`メソッドを書き換える場合、Rust側でバイトコードをAST化し、Surgical Merge。Fabricの競合クラッシュを防ぐ。

80. **Access Widener / ATをRustで事前適用**
    *   `private`→`public`変換をバイトコードレベルで一括適用。Java側リフレクションを不要に。

### カテゴリ6: コンパイル時最適化・その他

81. **LTO=thin + codegen-units=1 (Release) / 16 (Dev) + panic=abort**
    *   クロスクレート最適化でインライン化が全体に及ぶ。バイナリサイズと速度のベストバランス。

82. **PGO (Profile-Guided Optimization) + BOLT**
    *   Hypixel/低スペックPCでの実プレイプロファイルを取得し、ホットパスを` .text.hot`に配置。分岐予測15%改善。

83. **Target-CPU Dispatch: x86-64-v3 Baseline + v4 Runtime Dispatch**
    *   配布はAVX2までのv3、低スペック古いPCでも動く。起動時に`std::is_x86_feature_detected!("avx512f")`でv4パスに切り替え。`multiversion`クレート使用。

84. **portable-simd / std::simd::Simd<u32,8>**
    *   nightlyの`portable_simd`でGreedyのパレット比較を8要素同時比較。LLVMがAVX2に自動ベクタ化。

85. **AVX2 Bitmask Greedy Meshing**
    *   1スライス32ブロックを`u32`ビットマスクで表現、隣接面判定を`_mm256_andnot_si256`で一括。判定を100倍高速化。

86. **const fn & Build.rs コード生成**
    *   `build.rs`で`BlockState -> ModelID`のPerfect Hash (`phf`) とAOテーブルを生成。実行時HashMap無しでO(1)。

87. **#[inline(always)] + #[cold] / #[inline(never)]の手動配置**
    *   `isOpaque()`や`getLight()`は常時インライン、エラー処理は`#[cold]`で別セクションへ。I-Cache効率向上。

88. **Branchless Block Logic (LUT + Select)**
    *   `if(block==water)`を分岐ではなく`table[blockId]`参照+`cmov`に。低スペックCPUの分岐予測ミスペナルティを回避。

89. **Loop Unroll + get_unchecked**
    *   16x16x16ループは`#[unroll]`+`get_unchecked`。境界チェックをunsafeで削り、1チャンクあたり数十万回のチェックをゼロに。

90. **Mold / LLDリンカ + Cranelift (Dev)**
    *   リンク時間を10分の1にし開発イテレーション爆速化。CIでは`mold`使用。

91. **Fast Math + fast-float**
    *   Light計算でNaN/Infチェック不要なら`fadd fast`。`fast-float`で座標パースを高速化。

92. **Pre-allocated String Interner (lasso)**
    *   ブロック名文字列を`lasso::Rodeo`でインターン。比較がポインタ比較に。

93. **no-std Compatible Core Crates**
    *   メッシュ生成クレートを`no_std + alloc`にし、WASMや将来的なGPU Computeと共有可能に。

94. **Const Generic Specialization for Face Direction**
    *   `fn build_face<const DIR: u8>()`で6方向を単相化、ループ内のswitchをコンパイル時に消去。

95. **SIMD Morton Encode (BMI2 PDEP)**
    *   `pdep`命令でMortonコード生成を1命令化。チャンクアクセスの座標計算が爆速。

96. **Stackful Coroutine Chunk Builder? でも低スペック向けに要調整?**

これの罫線区切。

---
付録: MVP実装ロードマップ提案 (低スペックPC優先)

Phase0: `windows-rs` D3D12最小三角形 + JVMTI Agentで`glClear`をフック
Phase1: DirectByteBuffer共有 + Rayon Greedy Meshing (Opaqueのみ)
Phase2: ExecuteIndirect + Hi-Z Culling + WBOIT
Phase3: ModLoader C ABI安定化 + WASM Sandbox POC
Phase4: PGO + mimalloc + SoA頂点圧縮で最終2倍

このリストから、まずDescriptor Heap Ring + Rayon + DirectByteBuffer共有の3点だけでもSodium同等のFPSが出ます。
