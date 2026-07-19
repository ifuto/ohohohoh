package com.rsift;

import java.lang.reflect.Method;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * UI injection on the render thread (via {@code Minecraft.execute}).
 * Avoids bytecode-patching {@code Screen.init()} which can crash the JVM (0xC0000005).
 */
public final class RsiftScreenHooks {
    private static final Set<Integer> INJECTED = ConcurrentHashMap.newKeySet();

    private RsiftScreenHooks() {}

    /** Schedule button injection on the client/render thread. Safe to call from any thread. */
    public static void injectCurrentScreen(Object minecraft) {
        if (minecraft == null) {
            return;
        }
        try {
            Method execute = minecraft.getClass().getMethod("execute", Runnable.class);
            execute.invoke(minecraft, (Runnable) () -> injectScreenNow(minecraft));
        } catch (ReflectiveOperationException e) {
            injectScreenNow(minecraft);
        }
    }

    private static void injectScreenNow(Object minecraft) {
        try {
            Method screenMethod = minecraft.getClass().getMethod("screen");
            Object screen = screenMethod.invoke(minecraft);
            if (screen == null) {
                Method alt = minecraft.getClass().getMethod("getScreen");
                screen = alt.invoke(minecraft);
            }
            if (screen != null) {
                onScreenInit(screen);
            }
        } catch (Throwable t) {
            nativeLog("[RsiftScreenHooks] injectCurrentScreen failed: " + t.getMessage());
        }
    }

    static void onScreenInit(Object screen) {
        if (screen == null) {
            return;
        }
        int key = System.identityHashCode(screen);
        if (!INJECTED.add(key)) {
            return;
        }
        String name = screen.getClass().getName();
        ClassLoader loader = screen.getClass().getClassLoader();
        if (loader == null) {
            nativeLog("[RsiftScreenHooks] FAIL — ClassLoader null for " + name);
            INJECTED.remove(key);
            return;
        }
        try {
            RsiftUiBridge.injectButtons(screen, name, loader);
            nativeLog("[RsiftScreenHooks] injected into " + name);
        } catch (Throwable t) {
            INJECTED.remove(key);
            nativeLog("[RsiftScreenHooks] FAIL " + name + ": " + t.getMessage());
        }
    }

    private static native void nativeLog(String line);
}
