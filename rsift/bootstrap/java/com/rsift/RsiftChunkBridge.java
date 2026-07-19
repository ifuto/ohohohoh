package com.rsift;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.IdentityHashMap;
import java.util.Map;

/**
 * Production chunk ingest: ClientLevel → flat section palettes → native mesher.
 * JDK-only compile; Mojang-mapped Minecraft types resolved at runtime.
 */
public final class RsiftChunkBridge {
    private static final int SECTION = 16;
    private static final int SECTION_VOLUME = SECTION * SECTION * SECTION;
    /** Chunks around the player to poll each sync (fits 64-block pull window). */
    private static final int SYNC_RADIUS = 1;
    private static volatile boolean nativesReady;
    private static final Map<Object, Integer> STATE_IDS = new IdentityHashMap<>();
    private static int nextStateId = 1;
    private static long syncCounter;

    private RsiftChunkBridge() {}

    public static void markNativesReady() {
        nativesReady = true;
    }

    /** Called from {@link RsiftPlatformBridge#onClientTick}. */
    public static void syncFromMinecraft(Object minecraft, ClassLoader loader) {
        if (!nativesReady || minecraft == null) {
            return;
        }
        syncCounter++;
        try {
            Object level = readLevel(minecraft);
            if (level == null) {
                return;
            }
            Object player = readPlayer(minecraft);
            if (player == null) {
                return;
            }

            double px = asDouble(invokeNoArg(player, "getX"), 0);
            double py = asDouble(invokeNoArg(player, "getY"), 64);
            double pz = asDouble(invokeNoArg(player, "getZ"), 0);
            float yaw = asFloat(invokeNoArg(player, "getYRot"), 0);
            float pitch = asFloat(invokeNoArg(player, "getXRot"), 0);
            nativeSetCamera((float) px, (float) py, (float) pz, yaw, pitch);

            int cx = floorDiv((int) Math.floor(px), SECTION);
            int cz = floorDiv((int) Math.floor(pz), SECTION);
            int minSection = readMinSection(level);

            for (int dx = -SYNC_RADIUS; dx <= SYNC_RADIUS; dx++) {
                for (int dz = -SYNC_RADIUS; dz <= SYNC_RADIUS; dz++) {
                    ingestChunk(level, cx + dx, cz + dz, minSection);
                }
            }
            nativePrune(cx, cz, SYNC_RADIUS + 1);

            if (syncCounter == 1 || syncCounter % 200 == 0) {
                nativeLog("[RsiftChunkBridge] synced around (" + cx + "," + cz + ") minSection=" + minSection);
            }
        } catch (Throwable t) {
            if (syncCounter < 5 || syncCounter % 200 == 0) {
                nativeLog("[RsiftChunkBridge] sync failed: " + t.getMessage());
            }
        }
    }

    private static void ingestChunk(Object level, int cx, int cz, int minSection) {
        Object chunk = getChunk(level, cx, cz);
        if (chunk == null) {
            return;
        }
        Object[] sections = getSections(chunk);
        if (sections == null || sections.length == 0) {
            return;
        }
        int base = minSection;
        try {
            Object v = invokeNoArg(chunk, "getMinSection");
            if (v instanceof Number) {
                base = ((Number) v).intValue();
            }
        } catch (Throwable ignored) {
        }

        short[] flat = new short[sections.length * SECTION_VOLUME];
        int opaque = 0;
        for (int si = 0; si < sections.length; si++) {
            Object section = sections[si];
            int offset = si * SECTION_VOLUME;
            if (section == null || isSectionEmpty(section)) {
                continue;
            }
            for (int y = 0; y < SECTION; y++) {
                for (int z = 0; z < SECTION; z++) {
                    for (int x = 0; x < SECTION; x++) {
                        Object state = getBlockState(section, x, y, z);
                        int id = stateId(state);
                        // Match rsift-opt-gfx idx: x + y*16 + z*256
                        flat[offset + (x + y * SECTION + z * SECTION * SECTION)] = (short) id;
                        if (id != 0) {
                            opaque++;
                        }
                    }
                }
            }
        }
        if (opaque == 0) {
            // Still ingest empty columns so mesher can cull correctly.
            nativeIngestColumn(cx, cz, base, flat, sections.length);
            return;
        }
        nativeIngestColumn(cx, cz, base, flat, sections.length);
    }

    private static int stateId(Object state) {
        if (state == null || isAir(state)) {
            return 0;
        }
        Integer cached = STATE_IDS.get(state);
        if (cached != null) {
            return cached;
        }
        int id = nextStateId++;
        if (id > 4095) {
            id = 1 + (id % 4095);
        }
        // Prefer registry id when available (stable across reloads).
        try {
            Object block = invokeNoArg(state, "getBlock");
            if (block != null) {
                int reg = registryBlockId(block);
                if (reg >= 0) {
                    id = (reg % 4095) + 1;
                }
            }
        } catch (Throwable ignored) {
        }
        STATE_IDS.put(state, id);
        return id;
    }

    private static int registryBlockId(Object block) {
        try {
            Class<?> builtIn = Class.forName("net.minecraft.core.registries.BuiltInRegistries", true, block.getClass().getClassLoader());
            Field blockReg = builtIn.getField("BLOCK");
            Object registry = blockReg.get(null);
            Method getId = findMethod(registry.getClass(), "getId", 1);
            if (getId == null) {
                return -1;
            }
            Object v = getId.invoke(registry, block);
            if (v instanceof Number) {
                return ((Number) v).intValue();
            }
        } catch (Throwable ignored) {
        }
        return -1;
    }

    private static boolean isAir(Object state) {
        try {
            Object v = invokeNoArg(state, "isAir");
            if (v instanceof Boolean) {
                return (Boolean) v;
            }
        } catch (Throwable ignored) {
        }
        return false;
    }

    private static boolean isSectionEmpty(Object section) {
        try {
            Object v = invokeNoArg(section, "hasOnlyAir");
            if (v instanceof Boolean) {
                return (Boolean) v;
            }
        } catch (Throwable ignored) {
        }
        try {
            Object v = invokeNoArg(section, "isEmpty");
            if (v instanceof Boolean) {
                return (Boolean) v;
            }
        } catch (Throwable ignored) {
        }
        return false;
    }

    private static Object getBlockState(Object section, int x, int y, int z) {
        try {
            Method m = findMethod(section.getClass(), "getBlockState", 3);
            if (m != null) {
                return m.invoke(section, x, y, z);
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    private static Object[] getSections(Object chunk) {
        try {
            Object v = invokeNoArg(chunk, "getSections");
            if (v instanceof Object[]) {
                return (Object[]) v;
            }
        } catch (Throwable ignored) {
        }
        Field f = findField(chunk.getClass(), "sections");
        if (f != null) {
            try {
                f.setAccessible(true);
                Object v = f.get(chunk);
                if (v instanceof Object[]) {
                    return (Object[]) v;
                }
            } catch (Throwable ignored) {
            }
        }
        return null;
    }

    private static Object getChunk(Object level, int cx, int cz) {
        try {
            Method m = findMethod(level.getClass(), "getChunk", 2);
            if (m != null) {
                return m.invoke(level, cx, cz);
            }
        } catch (Throwable ignored) {
        }
        try {
            Method m = findMethod(level.getClass(), "getChunkAt", 1);
            if (m != null) {
                Object pos = blockPos(level.getClass().getClassLoader(), cx * SECTION, 0, cz * SECTION);
                if (pos != null) {
                    return m.invoke(level, pos);
                }
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    private static Object blockPos(ClassLoader loader, int x, int y, int z) {
        try {
            Class<?> cls = Class.forName("net.minecraft.core.BlockPos", true, loader);
            return cls.getConstructor(int.class, int.class, int.class).newInstance(x, y, z);
        } catch (Throwable t) {
            return null;
        }
    }

    private static int readMinSection(Object level) {
        try {
            Object v = invokeNoArg(level, "getMinSection");
            if (v instanceof Number) {
                return ((Number) v).intValue();
            }
        } catch (Throwable ignored) {
        }
        try {
            Object v = invokeNoArg(level, "getMinSectionY");
            if (v instanceof Number) {
                return ((Number) v).intValue();
            }
        } catch (Throwable ignored) {
        }
        return -4; // 1.18+ default overworld
    }

    private static Object readLevel(Object minecraft) {
        try {
            Object level = invokeNoArg(minecraft, "level");
            if (level != null) {
                return level;
            }
        } catch (Throwable ignored) {
        }
        Field f = findField(minecraft.getClass(), "level");
        if (f != null) {
            try {
                f.setAccessible(true);
                return f.get(minecraft);
            } catch (Throwable ignored) {
            }
        }
        return null;
    }

    private static Object readPlayer(Object minecraft) {
        try {
            Object p = invokeNoArg(minecraft, "player");
            if (p != null) {
                return p;
            }
        } catch (Throwable ignored) {
        }
        Field f = findField(minecraft.getClass(), "player");
        if (f != null) {
            try {
                f.setAccessible(true);
                return f.get(minecraft);
            } catch (Throwable ignored) {
            }
        }
        return null;
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
        Class<?> c = type;
        while (c != null) {
            for (Method m : c.getDeclaredMethods()) {
                if (m.getName().equals(name) && m.getParameterCount() == paramCount) {
                    m.setAccessible(true);
                    return m;
                }
            }
            c = c.getSuperclass();
        }
        return null;
    }

    private static Field findField(Class<?> type, String name) {
        Class<?> c = type;
        while (c != null) {
            try {
                return c.getDeclaredField(name);
            } catch (NoSuchFieldException e) {
                c = c.getSuperclass();
            }
        }
        return null;
    }

    private static double asDouble(Object v, double def) {
        if (v instanceof Number) {
            return ((Number) v).doubleValue();
        }
        return def;
    }

    private static float asFloat(Object v, float def) {
        if (v instanceof Number) {
            return ((Number) v).floatValue();
        }
        return def;
    }

    private static int floorDiv(int a, int b) {
        int r = a / b;
        if ((a ^ b) < 0 && r * b != a) {
            r--;
        }
        return r;
    }

    private static native void nativeLog(String line);
    private static native void nativeSetCamera(float x, float y, float z, float yaw, float pitch);
    private static native void nativeIngestColumn(int cx, int cz, int baseSectionY, short[] blocks, int sectionCount);
    private static native void nativePrune(int cx, int cz, int radius);
}
