package com.rsift;

import java.lang.reflect.Method;

/**
 * Hooks Minecraft render flip + LWJGL draw path → Rust DX12 Agility (not OpenGL).
 */
public final class RsiftRenderHooks {
    private static volatile boolean installed;

    private RsiftRenderHooks() {}

    public static void ensureInstalled(Object minecraft, ClassLoader gameLoader) {
        if (installed || minecraft == null || gameLoader == null) {
            return;
        }
        try {
            hookFlipFrame(minecraft, gameLoader);
            installed = true;
            nativeLog("[RsiftRenderHooks] flipFrame → DX12 installed");
        } catch (Throwable t) {
            nativeLog("[RsiftRenderHooks] install failed: " + t.getMessage());
        }
    }

    /** Schedule DXGI present on the Minecraft client (render) thread. */
    public static void scheduleFlip(Object minecraft) {
        if (minecraft == null) {
            return;
        }
        try {
            Method execute = minecraft.getClass().getMethod("execute", Runnable.class);
            execute.invoke(minecraft, (Runnable) () -> onFlipFrame(minecraft));
        } catch (Throwable t) {
            onFlipFrame(minecraft);
        }
    }

    /** Called from bytecode-injected RenderSystem.flipFrame tail (or manual invoke). */
    public static void onFlipFrame(Object minecraft) {
        if (minecraft == null) {
            return;
        }
        try {
            Object window = invokeNoArg(minecraft, "getWindow");
            if (window == null) {
                return;
            }
            int w = ((Number) invokeNoArg(window, "getWidth")).intValue();
            int h = ((Number) invokeNoArg(window, "getHeight")).intValue();
            long glfw = ((Number) invokeNoArg(window, "getWindow")).longValue();
            long hwnd = glfwWin32(glfw);
            if (hwnd != 0L) {
                nativeOnFlip(hwnd, w, h);
            }
        } catch (Throwable t) {
            nativeLog("[RsiftRenderHooks] onFlipFrame: " + t.getMessage());
        }
    }

    /** LWJGL draw gate — return false to skip vanilla OpenGL. */
    public static boolean shouldGlDraw() {
        return nativeGlDraw();
    }

    public static void onGlSwap() {
        nativeGlSwap();
    }

    // ============================================================
    // Transpiler HEAD-inject targets (bytecode_transpiler.rs の実注入先)。
    // BakedModel.getQuads / LevelRenderer.renderChunkLayer の HEAD から
    // invokestatic される。ネイティブ未登録時も絶対に落とさない (1度だけ警告)。
    // ============================================================
    private static boolean hookNativeWarned;

    /** BakedModel.getQuads HEAD (vanilla bake 呼出の実測カウンタ。必ず即復帰)。 */
    public static void getQuadsHeadHook() {
        try {
            nativeGetQuadsHook();
        } catch (Throwable t) {
            warnHookOnce(t);
        }
    }

    /** LevelRenderer.renderChunkLayer HEAD (vanilla 描画ループ実測。必ず即復帰)。 */
    public static void chunkLayerHeadHook() {
        try {
            nativeChunkLayerHook();
        } catch (Throwable t) {
            warnHookOnce(t);
        }
    }

    private static void warnHookOnce(Throwable t) {
        if (!hookNativeWarned) {
            hookNativeWarned = true;
            nativeLog("[RsiftRenderHooks] transpiled hook native unavailable: " + t);
        }
    }

    private static void hookFlipFrame(Object minecraft, ClassLoader loader) throws ReflectiveOperationException {
        Class<?> rsClass = Class.forName("com.mojang.blaze3d.systems.RenderSystem", true, loader);
        // Poll hook: register static callback field if MC exposes it; flip is invoked via onFlipFrame from agent tick.
        nativeLog("[RsiftRenderHooks] RenderSystem ready class=" + rsClass.getName());
    }

    private static long glfwWin32(long glfwWindow) throws ReflectiveOperationException {
        Class<?> nativeWin = Class.forName("org.lwjgl.glfw.GLFWNativeWin32", true,
                Thread.currentThread().getContextClassLoader());
        Method m = nativeWin.getMethod("glfwGetWin32Window", long.class);
        return ((Number) m.invoke(null, glfwWindow)).longValue();
    }

    private static Object invokeNoArg(Object target, String method) throws ReflectiveOperationException {
        Method m = target.getClass().getMethod(method);
        return m.invoke(target);
    }

    private static void nativeLog(String line) {
        try {
            RsiftRuntimeHooks.class.getDeclaredMethod("nativeLog", String.class).invoke(null, line);
        } catch (Throwable ignored) {
        }
    }

    private static native void nativeOnFlip(long hwnd, int width, int height);
    private static native boolean nativeGlDraw();
    private static native void nativeGlSwap();
    private static native long nativeGetQuadsHook();
    private static native long nativeChunkLayerHook();
}
