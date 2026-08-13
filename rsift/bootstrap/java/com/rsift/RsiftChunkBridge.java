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
                if (syncCounter <= 3) {
                    nativeLog("[RsiftChunkBridge] level=null (sync #" + syncCounter + ") — readLevel obf resolve failed");
                }
                return;
            }
            Object player = readPlayer(minecraft);
            if (player == null) {
                if (syncCounter <= 3) {
                    nativeLog("[RsiftChunkBridge] player=null (sync #" + syncCounter + ")");
                }
                return;
            }

            double px = asDouble(resolveInvoke(player, "net.minecraft.world.entity.Entity", "getX"), 0);
            double py = asDouble(resolveInvoke(player, "net.minecraft.world.entity.Entity", "getY"), 64);
            double pz = asDouble(resolveInvoke(player, "net.minecraft.world.entity.Entity", "getZ"), 0);
            float yaw = resolveFieldGetFloat(player, "net.minecraft.world.entity.Entity", "yRot", 0);
            float pitch = resolveFieldGetFloat(player, "net.minecraft.world.entity.Entity", "xRot", 0);
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
        Object msVal = resolveInvoke(chunk, "net.minecraft.world.level.chunk.LevelChunk", "getMinSection");
        if (msVal instanceof Number) { base = ((Number) msVal).intValue(); }

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
            Object block = resolveInvoke(state, "net.minecraft.world.level.block.state.BlockState", "getBlock");
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
        Object v = resolveInvoke(state, "net.minecraft.world.level.block.state.BlockState", "isAir");
        if (v instanceof Boolean) { return (Boolean) v; }
        return false;
    }

    private static boolean isSectionEmpty(Object section) {
        Object v = resolveInvoke(section, "net.minecraft.world.level.chunk.LevelChunkSection", "hasOnlyAir");
        if (v instanceof Boolean) { return (Boolean) v; }
        v = resolveInvoke(section, "net.minecraft.world.level.chunk.LevelChunkSection", "isEmpty");
        if (v instanceof Boolean) { return (Boolean) v; }
        return false;
    }

    private static Object getBlockState(Object section, int x, int y, int z) {
        Method m = resolveFindMethod(section.getClass(), "net.minecraft.world.level.chunk.LevelChunkSection", "getBlockState", 3);
        if (m != null) {
            try { return m.invoke(section, x, y, z); } catch (Throwable ignored) {}
        }
        return null;
    }

    private static Object[] getSections(Object chunk) {
        Object v = resolveInvoke(chunk, "net.minecraft.world.level.chunk.LevelChunk", "getSections");
        if (v instanceof Object[]) { return (Object[]) v; }
        Field f = resolveFindField(chunk.getClass(), "net.minecraft.world.level.chunk.LevelChunk", "sections");
        if (f != null) {
            try {
                f.setAccessible(true);
                Object fv = f.get(chunk);
                if (fv instanceof Object[]) { return (Object[]) fv; }
            } catch (Throwable ignored) {}
        }
        return null;
    }

    private static Object getChunk(Object level, int cx, int cz) {
        Method m = resolveFindMethod(level.getClass(), "net.minecraft.client.multiplayer.ClientLevel", "getChunk", 2);
        if (m != null) {
            try { return m.invoke(level, cx, cz); } catch (Throwable ignored) {}
        }
        m = resolveFindMethod(level.getClass(), "net.minecraft.client.multiplayer.ClientLevel", "getChunkAt", 1);
        if (m != null) {
            try {
                Object pos = blockPos(level.getClass().getClassLoader(), cx * SECTION, 0, cz * SECTION);
                if (pos != null) { return m.invoke(level, pos); }
            } catch (Throwable ignored) {}
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
        Object v = resolveInvoke(level, "net.minecraft.client.multiplayer.ClientLevel", "getMinSection");
        if (v instanceof Number) { return ((Number) v).intValue(); }
        v = resolveInvoke(level, "net.minecraft.client.multiplayer.ClientLevel", "getMinSectionY");
        if (v instanceof Number) { return ((Number) v).intValue(); }
        return -4; // 1.18+ default overworld
    }

    private static Object readLevel(Object minecraft) {
        try {
            String name = RsiftHooks.resolveMethod("net.minecraft.client.Minecraft", "level");
            Object level = minecraft.getClass().getMethod(name).invoke(minecraft);
            if (level != null) {
                return level;
            }
        } catch (Throwable ignored) {
        }
        String fname = RsiftHooks.resolveField("net.minecraft.client.Minecraft", "level");
        Field f = findField(minecraft.getClass(), fname);
        if (f == null && !fname.equals("level")) {
            f = findField(minecraft.getClass(), "level");
        }
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
            String name = RsiftHooks.resolveMethod("net.minecraft.client.Minecraft", "player");
            Object p = minecraft.getClass().getMethod(name).invoke(minecraft);
            if (p != null) {
                return p;
            }
        } catch (Throwable ignored) {
        }
        String fname = RsiftHooks.resolveField("net.minecraft.client.Minecraft", "player");
        Field f = findField(minecraft.getClass(), fname);
        if (f == null && !fname.equals("player")) {
            f = findField(minecraft.getClass(), "player");
        }
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

    /** obf解決済みフィールド名でfloatフィールドを読む。 */
    private static float resolveFieldGetFloat(Object target, String ownerMojmap, String mojmapField, float def) {
        try {
            String name = RsiftHooks.resolveField(ownerMojmap, mojmapField);
            Field f = findField(target.getClass(), name);
            if (f == null && !name.equals(mojmapField)) {
                f = findField(target.getClass(), mojmapField);
            }
            if (f != null) {
                f.setAccessible(true);
                return f.getFloat(target);
            }
        } catch (Throwable ignored) {
        }
        return def;
    }

    /** obf解決済みメソッド名で0引数メソッドを反射実行。失敗は null。 */
    private static Object resolveInvoke(Object target, String ownerMojmap, String mojmapMethod) {
        try {
            String name = RsiftHooks.resolveMethod(ownerMojmap, mojmapMethod);
            Method m = target.getClass().getMethod(name);
            return m.invoke(target);
        } catch (Throwable ignored) {
        }
        return null;
    }

    /** obf解決済みメソッド名で paramCount 引数メソッドを検索。 */
    private static Method resolveFindMethod(Class<?> type, String ownerMojmap, String mojmapMethod, int paramCount) {
        String name = RsiftHooks.resolveMethod(ownerMojmap, mojmapMethod);
        Method m = findMethod(type, name, paramCount);
        if (m == null && !name.equals(mojmapMethod)) {
            m = findMethod(type, mojmapMethod, paramCount);
        }
        return m;
    }

    /** obf解決済みフィールド名でフィールドを検索。 */
    private static Field resolveFindField(Class<?> type, String ownerMojmap, String mojmapField) {
        String name = RsiftHooks.resolveField(ownerMojmap, mojmapField);
        Field f = findField(type, name);
        if (f == null && !name.equals(mojmapField)) {
            f = findField(type, mojmapField);
        }
        return f;
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
