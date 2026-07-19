package com.rsift;

import java.lang.reflect.Method;

/**
 * Installs runtime hooks (packet tap + client tick) using mapped MC class names at runtime.
 * JDK-only compile; Netty/MC types resolved via game ClassLoader.
 */
public final class RsiftRuntimeHooks {
    private static volatile boolean installed;

    private RsiftRuntimeHooks() {}

    public static void ensureInstalled(Object minecraft, ClassLoader gameLoader) {
        if (installed || minecraft == null || gameLoader == null) {
            return;
        }
        try {
            tryInstallPacketTap(minecraft, gameLoader);
            tryInstallPlatform(minecraft, gameLoader);
            installed = true;
            nativeLog("[RsiftRuntimeHooks] installed");
        } catch (Throwable t) {
            nativeLog("[RsiftRuntimeHooks] install failed: " + t.getMessage());
        }
    }

    private static void tryInstallPlatform(Object minecraft, ClassLoader loader) {
        try {
            Class<?> cls = Class.forName("com.rsift.RsiftPlatformBridge", true, loader);
            Method m = cls.getMethod("ensureInstalled", Object.class, ClassLoader.class);
            m.invoke(null, minecraft, loader);
            nativeLog("[RsiftRuntimeHooks] platform bridge ensureInstalled");
        } catch (Throwable t) {
            nativeLog("[RsiftRuntimeHooks] platform bridge skipped: " + t.getMessage());
        }
    }

    private static void tryInstallPacketTap(Object minecraft, ClassLoader loader) throws ReflectiveOperationException {
        if (!classExists("io.netty.channel.ChannelInboundHandlerAdapter", loader)) {
            nativeLog("[RsiftRuntimeHooks] Netty not visible — packet tap skipped");
            return;
        }
        Object connection = invokeNoArg(minecraft, "getConnection");
        if (connection == null) {
            return;
        }
        Object channel = invokeNoArg(connection, "channel");
        if (channel == null) {
            return;
        }
        Object pipeline = invokeNoArg(channel, "pipeline");
        if (pipeline == null) {
            return;
        }
        Method names = pipeline.getClass().getMethod("names");
        @SuppressWarnings("unchecked")
        java.util.List<String> list = (java.util.List<String>) names.invoke(pipeline);
        if (list.contains("rsift_packet_tap")) {
            return;
        }
        Class<?> handlerClass = Class.forName("com.rsift.RsiftPacketTap", true, loader);
        Object handler = handlerClass.getConstructor().newInstance();
        Method addBefore = findMethod(pipeline.getClass(), "addBefore", 3);
        if (addBefore == null) {
            Method addLast = pipeline.getClass().getMethod("addLast", String.class, findNettyHandlerClass(loader));
            addLast.invoke(pipeline, "rsift_packet_tap", handler);
        } else {
            String anchor = list.contains("decoder") ? "decoder" : list.isEmpty() ? "rsift_packet_tap" : list.get(0);
            addBefore.invoke(pipeline, anchor, "rsift_packet_tap", handler);
        }
        nativeLog("[RsiftRuntimeHooks] packet tap added to pipeline");
    }

    private static Class<?> findNettyHandlerClass(ClassLoader loader) throws ClassNotFoundException {
        return Class.forName("io.netty.channel.ChannelHandler", true, loader);
    }

    private static boolean classExists(String name, ClassLoader loader) {
        try {
            Class.forName(name, false, loader);
            return true;
        } catch (Throwable ignored) {
            return false;
        }
    }

    private static Object invokeNoArg(Object target, String method) throws ReflectiveOperationException {
        Method m = target.getClass().getMethod(method);
        return m.invoke(target);
    }

    private static Method findMethod(Class<?> type, String name, int paramCount) {
        for (Method m : type.getMethods()) {
            if (m.getName().equals(name) && m.getParameterCount() == paramCount) {
                return m;
            }
        }
        return null;
    }

    private static native void nativeLog(String line);
}
