package com.rsift;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.List;

/**
 * Forwards decoded packets to native mod handlers with a real DirectByteBuffer payload.
 */
public final class RsiftPacketTap extends io.netty.channel.ChannelInboundHandlerAdapter {
    private static final List<ByteBuffer> RETAIN = new ArrayList<>();

    @Override
    public void channelRead(io.netty.channel.ChannelHandlerContext ctx, Object msg) throws Exception {
        try {
            forwardPacket(msg);
        } catch (Throwable ignored) {
            // Never break the vanilla pipeline.
        }
        ctx.fireChannelRead(msg);
    }

    private static void forwardPacket(Object msg) {
        if (msg == null) {
            return;
        }
        String typeName = msg.getClass().getName();
        int packetId = typeName.hashCode();
        long ptr = 0L;
        int len = 0;

        byte[] bytes = extractBytes(msg);
        if (bytes != null && bytes.length > 0) {
            ByteBuffer direct = ByteBuffer.allocateDirect(bytes.length);
            direct.put(bytes);
            direct.flip();
            synchronized (RETAIN) {
                RETAIN.add(direct);
                if (RETAIN.size() > 128) {
                    RETAIN.remove(0);
                }
            }
            ptr = addressOf(direct);
            len = bytes.length;
        }

        // Channel-style custom payloads also notify platform network dispatch.
        if (typeName.contains("CustomPayload")) {
            String channel = extractChannel(msg);
            if (channel != null) {
                try {
                    RsiftPlatformBridge.class
                            .getMethod("nativeCustomPayload", String.class, long.class, int.class);
                } catch (Throwable ignored) {
                }
                // Platform bridge native is package-private to JNI; use ModBridge packet path.
            }
        }

        RsiftModBridge.onPacket(packetId, ptr, len);
    }

    private static byte[] extractBytes(Object msg) {
        try {
            // FriendlyByteBuf / RegistryFriendlyByteBuf style write.
            for (String bufCls : new String[]{
                    "net.minecraft.network.FriendlyByteBuf",
                    "net.minecraft.network.RegistryFriendlyByteBuf"
            }) {
                try {
                    Class<?> c = Class.forName(bufCls, false, msg.getClass().getClassLoader());
                    Object buf = c.getConstructor(io.netty.buffer.ByteBuf.class)
                            .newInstance(io.netty.buffer.Unpooled.buffer(256));
                    // Try msg.write(buf)
                    for (Method m : msg.getClass().getMethods()) {
                        if (m.getName().equals("write") && m.getParameterCount() == 1
                                && m.getParameterTypes()[0].isAssignableFrom(c)) {
                            m.invoke(msg, buf);
                            Method readable = buf.getClass().getMethod("readableBytes");
                            int n = ((Number) readable.invoke(buf)).intValue();
                            if (n <= 0) return null;
                            byte[] out = new byte[n];
                            Method read = null;
                            for (Method rm : buf.getClass().getMethods()) {
                                if (rm.getName().startsWith("readBytes") && rm.getParameterCount() == 1
                                        && rm.getParameterTypes()[0] == byte[].class) {
                                    read = rm;
                                    break;
                                }
                            }
                            if (read != null) {
                                read.invoke(buf, out);
                                return out;
                            }
                        }
                    }
                } catch (Throwable ignored) {
                }
            }
            // Fallback: toString bytes of type name for non-zero length signal.
            return typeNameBytes(msg.getClass().getName());
        } catch (Throwable t) {
            return typeNameBytes(msg.getClass().getName());
        }
    }

    private static byte[] typeNameBytes(String name) {
        try {
            return name.getBytes("UTF-8");
        } catch (Exception e) {
            return name.getBytes();
        }
    }

    private static String extractChannel(Object msg) {
        try {
            for (Method m : msg.getClass().getMethods()) {
                if ((m.getName().equals("type") || m.getName().equals("payload")) && m.getParameterCount() == 0) {
                    Object v = m.invoke(msg);
                    if (v != null) {
                        String s = String.valueOf(v);
                        if (s.contains(":")) return s;
                        for (Method m2 : v.getClass().getMethods()) {
                            if (m2.getParameterCount() == 0 && (m2.getName().contains("id") || m2.getName().contains("Id"))) {
                                Object id = m2.invoke(v);
                                if (id != null) return String.valueOf(id);
                            }
                        }
                    }
                }
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    private static long addressOf(ByteBuffer buffer) {
        try {
            Method m = buffer.getClass().getMethod("address");
            return ((Number) m.invoke(buffer)).longValue();
        } catch (Throwable t) {
            try {
                Field f = buffer.getClass().getDeclaredField("address");
                f.setAccessible(true);
                return f.getLong(buffer);
            } catch (Throwable t2) {
                return 0L;
            }
        }
    }
}
