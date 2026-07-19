package com.rsift;

/**
 * Stable JVM ↔ native mod dispatch (one ABI — mods register in Rust, never add natives here).
 */
public final class RsiftModBridge {
    public static final String OP_PACKET = "packet";
    public static final String OP_RENDER = "render";
    public static final String OP_CLIENT_TICK = "client_tick";

    private RsiftModBridge() {}

    public static void onPacket(int packetId, long bufPtr, int bufLen) {
        nativeDispatch(OP_PACKET, packetId & 0xFFFFFFFFL, bufPtr, bufLen);
    }

    public static void onRender(int width, int height, float deltaSeconds) {
        nativeDispatch(OP_RENDER, width & 0xFFFFFFFFL, height & 0xFFFFFFFFL, Float.floatToIntBits(deltaSeconds));
    }

    public static void onClientTick() {
        nativeDispatch(OP_CLIENT_TICK, 0L, 0L, 0);
    }

    private static native void nativeDispatch(String op, long a, long b, int c);
}
