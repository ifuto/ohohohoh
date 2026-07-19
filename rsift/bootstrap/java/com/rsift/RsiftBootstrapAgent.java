package com.rsift;

import java.lang.instrument.Instrumentation;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * Javaagent companion to {@code -agentpath}: registers ClassFileTransformer only.
 * Premain intentionally avoids JNI — native bridge is already up via agentpath.
 */
public final class RsiftBootstrapAgent {
    private static volatile boolean initialized;
    private static volatile Instrumentation instrumentation;

    public static void premain(String agentArgs, Instrumentation inst) {
        if (initialized) {
            return;
        }
        initialized = true;
        instrumentation = inst;

        String gameDir = readArg(agentArgs, "gameDir");
        boolean agentpathBoot = agentpathMarkerPresent(gameDir);

        try {
            RsiftClassTransformer.setInstrumentation(inst);
            inst.addTransformer(new RsiftClassTransformer(), true);
            RsiftAgentState.transformerRegistered = true;
            if (gameDir != null) {
                Files.writeString(Paths.get(gameDir, ".rsift-javaagent-ok"), "transformer-registered\n");
            }
        } catch (Throwable t) {
            System.err.println("[Rsift] premain: addTransformer FAILED: " + t.getMessage());
            throw new RuntimeException(t);
        }

        if (!agentpathBoot) {
            System.err.println("[Rsift] premain: javaagent-only mode — native DLL must load via agentpath");
        }
    }

    /** Called from native after RegisterNatives for the transformer. */
    public static void notifyNativeReady() {
        RsiftClassTransformer.markNativeReady();
    }

    public static Instrumentation instrumentation() {
        return instrumentation;
    }

    private static boolean agentpathMarkerPresent(String gameDir) {
        if (gameDir == null || gameDir.isEmpty()) {
            return false;
        }
        Path marker = Paths.get(gameDir, ".rsift-agentpath-active");
        return Files.isRegularFile(marker);
    }

    static String readArg(String args, String key) {
        if (args == null || args.isEmpty()) {
            return null;
        }
        String needle = key + "=";
        for (String part : args.split(",")) {
            String trimmed = part.trim();
            if (trimmed.startsWith(needle)) {
                return trimmed.substring(needle.length());
            }
        }
        return null;
    }
}
