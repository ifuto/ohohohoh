package com.rsift;

/**
 * Static hooks invoked from rewritten Minecraft bytecode (invokestatic).
 * Methods must stay {@code ()V} so HEAD injection is stack-safe.
 * JNI natives are registered from Rust {@code platform_bridge}/{@code mod_bridge}.
 */
public final class RsiftHooks {
    private RsiftHooks() {}

    public static void onNetworkPacket() {
        nativeHook("network_packet");
    }

    public static void onRenderFlip() {
        nativeHook("render_flip");
    }

    public static void onClientTickHook() {
        nativeHook("client_tick");
    }

    public static void onClientRun() {
        nativeHook("client_run");
    }

    public static void onScreenInit() {
        nativeHook("screen_init");
    }

    public static void onMobAiStep() {
        nativeHook("mob_ai_step");
    }

    public static void onEntityTravel() {
        nativeHook("entity_travel");
    }

    public static void onRedstoneCalculate() {
        nativeHook("redstone");
    }

    public static void onChunkTick() {
        nativeHook("chunk_tick");
    }

    public static void onHopperTick() {
        nativeHook("hopper_tick");
    }

    public static void onServerLevelTick() {
        nativeHook("server_level_tick");
    }

    public static void onFluidTick() {
        nativeHook("fluid_tick");
    }

    public static void onGenericCompute() {
        nativeHook("generic_compute");
    }

    private static void nativeHook(String name) {
        try {
            nativeOnHook(name);
        } catch (UnsatisfiedLinkError ignored) {
            // Native not registered yet during earliest bootstrap.
        } catch (Throwable t) {
            // Never break vanilla.
        }
    }

    private static native void nativeOnHook(String name);

    // wave HR: Java → Rust obf_map 解決ブリッジ (全 CNFE/NoSuchMethod の統一根治)
    private static native String nativeResolveClass0(String mojmapDotted);
    private static native String nativeResolveMethod0(String classDotted, String mojmapMethod);

    /** mojmap クラス名 → 実行時名(難読化版では難読名)。ネイティブ未登録時は原名。 */
    public static String resolveClass(String mojmapDotted) {
        try {
            String r = nativeResolveClass0(mojmapDotted);
            return (r != null && !r.isEmpty()) ? r : mojmapDotted;
        } catch (UnsatisfiedLinkError e) {
            return mojmapDotted;
        }
    }

    /** (mojmap クラス, mojmap メソッド名) → 実行時メソッド名。ネイティブ未登録時は原名。 */
    public static String resolveMethod(String classDotted, String mojmapMethod) {
        try {
            String r = nativeResolveMethod0(classDotted, mojmapMethod);
            return (r != null && !r.isEmpty()) ? r : mojmapMethod;
        } catch (UnsatisfiedLinkError e) {
            return mojmapMethod;
        }
    }
}
