package com.rsift;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;

/**
 * ClassFileTransformer — patches Screen via ASM when available, otherwise asks
 * native {@code BytecodePatcher} after the agent marks transform-ready.
 */
final class RsiftClassTransformer implements ClassFileTransformer {
    private static volatile boolean nativeReady;
    private static volatile Instrumentation instrumentation;

    static void setInstrumentation(Instrumentation inst) {
        instrumentation = inst;
    }

    static void markNativeReady() {
        nativeReady = true;
        Instrumentation inst = instrumentation;
        if (inst == null) {
            return;
        }
        try {
            // Retransform already-loaded targets when possible
            for (Class<?> c : inst.getAllLoadedClasses()) {
                if (!inst.isModifiableClass(c)) {
                    continue;
                }
                String name = c.getName().replace('.', '/');
                if (isTargetName(name)) {
                    try {
                        inst.retransformClasses(c);
                    } catch (Throwable ignored) {
                    }
                }
            }
        } catch (Throwable ignored) {
        }
    }

    static void markNativeReadyFromNative() {
        nativeReady = true;
    }

    private static boolean isTargetName(String internal) {
        return internal.startsWith("net/minecraft/")
                || internal.startsWith("com/mojang/blaze3d/")
                || "com/rsift/RsiftHooks".equals(internal);
    }

    @Override
    public byte[] transform(
            ClassLoader loader,
            String className,
            Class<?> classBeingRedefined,
            ProtectionDomain protectionDomain,
            byte[] classfileBuffer
    ) {
        if (className == null || classfileBuffer == null) {
            return null;
        }
        if ("net/minecraft/client/Minecraft".equals(className) && loader != null) {
            RsiftAgentState.captureLoader(loader);
        }

        // Pure-Java Screen patch (ASM) — no JNI
        if (className.startsWith("net/minecraft/client/gui/screens/")
                || "net/minecraft/client/gui/screens/Screen".equals(className)) {
            try {
                Class<?> patcher = Class.forName("com.rsift.ScreenInitPatcher", true,
                        RsiftClassTransformer.class.getClassLoader());
                java.lang.reflect.Method m = patcher.getMethod("patch", byte[].class);
                Object out = m.invoke(null, (Object) classfileBuffer);
                if (out instanceof byte[]) {
                    byte[] patched = (byte[]) out;
                    if (patched != classfileBuffer && patched.length > 0) {
                        return patched;
                    }
                }
            } catch (Throwable ignored) {
                // ASM not on classpath — fall through to native rewriter
            }
        }

        if (!nativeReady) {
            return null;
        }
        if (!isTargetName(className) && !nativeIsTarget(className)) {
            return null;
        }
        try {
            byte[] out = nativeTransform(className, classfileBuffer);
            if (out != null && out.length > 0 && out != classfileBuffer) {
                return out;
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    private static boolean nativeIsTarget(String className) {
        try {
            return nativeIsTargetClass(className);
        } catch (Throwable t) {
            return false;
        }
    }

    private static native byte[] nativeTransform(String className, byte[] classfileBuffer);
    private static native boolean nativeIsTargetClass(String className);
}
