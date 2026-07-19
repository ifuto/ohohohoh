package com.rsift;

/**
 * JVM-only boot state — no JNI during ClassFileTransformer (avoids 0xC0000005 / exit 1).
 */
final class RsiftAgentState {
    static volatile ClassLoader gameLoader;
    static volatile boolean transformerRegistered;

    private RsiftAgentState() {}

    static void captureLoader(ClassLoader loader) {
        if (loader != null) {
            gameLoader = loader;
        }
    }

    static ClassLoader getGameLoader() {
        return gameLoader;
    }
}
