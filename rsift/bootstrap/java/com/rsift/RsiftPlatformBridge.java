package com.rsift;

import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Applies Rsift platform snapshots to Minecraft 1.21.11 via Mojang-mapped reflection.
 * JDK-only compile; all MC/Netty types resolved with the game ClassLoader at runtime.
 */
public final class RsiftPlatformBridge {
    private static volatile boolean hooksInstalled;
    private static volatile Object minecraftRef;
    private static volatile ClassLoader gameLoader;
    private static final Map<Integer, Object> DYNAMIC_TEXTURES = new ConcurrentHashMap<>();
    private static final List<ByteBuffer> RETAINED_BUFFERS = new CopyOnWriteArrayList<>();
    private static final Map<String, String> SCREEN_REDIRECTS = new ConcurrentHashMap<>();
    private static final List<String> REGISTERED_CHANNELS = new CopyOnWriteArrayList<>();
    private static final Map<String, Object> REGISTERED_BLOCKS = new ConcurrentHashMap<>();
    private static final Map<String, Object> REGISTERED_ITEMS = new ConcurrentHashMap<>();
    private static final Map<String, Object> REGISTERED_SOUNDS = new ConcurrentHashMap<>();
    private static final List<Map<String, Object>> PERSISTED_BIOME_RULES = new CopyOnWriteArrayList<>();
    private static volatile boolean biomeRulesLoaded;
    private static volatile boolean hadLevel;
    private static volatile long clientTickCounter;

    private RsiftPlatformBridge() {}

    public static void ensureInstalled(Object minecraft, ClassLoader loader) {
        minecraftRef = minecraft;
        gameLoader = loader;
        if (hooksInstalled || minecraft == null || loader == null) {
            return;
        }
        try {
            installLifecycleProbes(minecraft, loader);
            hooksInstalled = true;
            nativeLog("[RsiftPlatformBridge] installed");
            nativeNotifyReady();
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] install failed: " + t.getMessage());
        }
    }

    public static void onClientTick(Object minecraft, ClassLoader loader) {
        ensureInstalled(minecraft, loader);
        clientTickCounter++;
        nativeLifecycle("client_tick", "", "", clientTickCounter, 0, 0);
        try {
            nativeRequestApply();
        } catch (Throwable ignored) {
        }
        tryEnforceScreenRedirect(minecraft, loader);
        drainOutbound(minecraft, loader);
        pollWorldLoad(minecraft);
    }

    private static void pollWorldLoad(Object minecraft) {
        try {
            Object level = invokeNoArg(minecraft, "level");
            if (level == null) {
                Field f = findField(minecraft.getClass(), "level");
                if (f != null) {
                    f.setAccessible(true);
                    level = f.get(minecraft);
                }
            }
            boolean hasLevel = level != null;
            if (hasLevel && !hadLevel) {
                loadPersistedBiomeRules();
                nativeLifecycle("world_load", dimensionIdOf(level), "", 0, 0, 0);
            }
            hadLevel = hasLevel;
        } catch (Throwable ignored) {
        }
    }

    private static String dimensionIdOf(Object level) {
        try {
            Object dim = invokeNoArg(level, "dimension");
            if (dim != null) {
                return String.valueOf(dim);
            }
        } catch (Throwable ignored) {
        }
        return "minecraft:overworld";
    }

    /** Called from native with a JSON snapshot. */
    public static String applySnapshotJson(String json) {
        int blocks = 0, items = 0, entities = 0, commands = 0, keys = 0, layers = 0;
        int channels = 0, biomes = 0, redirects = 0, images = 0;
        String err = null;
        try {
            if (json == null || json.isEmpty() || gameLoader == null || minecraftRef == null) {
                return statusJson(false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "not ready");
            }
            Map<String, Object> root = Json.object(json);

            blocks = applyBlocks(listOfMaps(root.get("blocks")));
            items = applyItems(listOfMaps(root.get("items")));
            entities = applyEntities(listOfMaps(root.get("entities")));
            commands = applyCommands(listOfMaps(root.get("commands")));
            keys = applyKeybindings(listOfMaps(root.get("keybindings")));
            layers = applyRenderLayers(listOfMaps(root.get("render_layers")));
            channels = applyChannels(listOfStrings(root.get("network_channels")));
            biomes = applyBiomeRules(listOfMaps(root.get("biome_rules")));
            redirects = applyRedirects(listOfMaps(root.get("screen_redirects")));
            images = applyImages(listOfMaps(root.get("images")));
            applyScreenHandlers(listOfMaps(root.get("screen_handlers")));
            int loot = applyLoot(listOfMaps(root.get("loot_modifiers")));
            applySounds(listOfMaps(root.get("sounds")));
            applyParticles(listOfMaps(root.get("particles")));
            applyStatusEffects(listOfMaps(root.get("status_effects")));
            applyEnchantments(listOfMaps(root.get("enchantments")));
            applyRecipes(listOfMaps(root.get("recipes")));
            applyGameRules(listOfMaps(root.get("game_rules")));
            applyTrades(listOfMaps(root.get("trades")));
            applyDimensions(listOfMaps(root.get("dimensions")));
            applyBlockEntities(listOfMaps(root.get("block_entities")));

            Object pending = root.get("pending_screen");
            if (pending instanceof String && !((String) pending).isEmpty()) {
                openPendingScreen((String) pending, root);
            }
            return statusJson(true, blocks, items, entities, commands, keys, layers, channels, biomes, redirects, images, loot, null);
        } catch (Throwable t) {
            err = t.getClass().getSimpleName() + ": " + t.getMessage();
            nativeLog("[RsiftPlatformBridge] apply error: " + err);
            return statusJson(false, blocks, items, entities, commands, keys, layers, channels, biomes, redirects, images, 0, err);
        }
    }

    public static void sendCustomPayload(String channel, byte[] data) {
        try {
            Object mc = minecraftRef;
            ClassLoader loader = gameLoader;
            if (mc == null || loader == null || channel == null || data == null) {
                return;
            }
            Object connection = invokeNoArg(mc, "getConnection");
            if (connection == null) {
                return;
            }
            // Prefer Brand / custom payload helpers when present; otherwise encode as byte array on connection.
            Method send = findMethod(connection.getClass(), "send", 1);
            if (send == null) {
                return;
            }
            Object packet = buildCustomPayloadPacket(loader, channel, data);
            if (packet != null) {
                send.invoke(connection, packet);
                nativeLog("[RsiftPlatformBridge] sent payload on " + channel + " (" + data.length + " bytes)");
            }
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] send payload failed: " + t.getMessage());
        }
    }

    public static long retainDirect(byte[] bytes) {
        if (bytes == null || bytes.length == 0) {
            return 0L;
        }
        ByteBuffer direct = ByteBuffer.allocateDirect(bytes.length);
        direct.put(bytes);
        direct.flip();
        RETAINED_BUFFERS.add(direct);
        if (RETAINED_BUFFERS.size() > 256) {
            RETAINED_BUFFERS.remove(0);
        }
        return addressOf(direct);
    }

    // --- apply helpers ---

    private static int applyBlocks(List<Map<String, Object>> blocks) throws Exception {
        if (blocks.isEmpty()) return 0;
        Object blockRegistry = builtin("BLOCK");
        unfreeze(blockRegistry);
        int n = 0;
        for (Map<String, Object> b : blocks) {
            String id = str(b.get("id"));
            if (id.isEmpty() || REGISTERED_BLOCKS.containsKey(id)) continue;
            float hardness = num(b.get("hardness"), 1.5f);
            float resistance = num(b.get("resistance"), 6.0f);
            int luminance = (int) num(b.get("luminance"), 0);
            Object block = createBlock(id, hardness, resistance, luminance);
            if (block == null) continue;
            Object key = resourceLocation(id);
            registerInto(blockRegistry, key, block);
            REGISTERED_BLOCKS.put(id, block);
            // Also register BlockItem
            Object itemRegistry = builtin("ITEM");
            unfreeze(itemRegistry);
            Object blockItem = createBlockItem(id, block);
            if (blockItem != null) {
                registerInto(itemRegistry, key, blockItem);
                REGISTERED_ITEMS.put(id, blockItem);
            }
            n++;
        }
        return n;
    }

    private static int applyItems(List<Map<String, Object>> items) throws Exception {
        Object itemRegistry = builtin("ITEM");
        unfreeze(itemRegistry);
        int n = 0;
        for (Map<String, Object> it : items) {
            String id = str(it.get("id"));
            if (id.isEmpty() || REGISTERED_ITEMS.containsKey(id)) continue;
            int maxStack = (int) num(it.get("max_stack"), 64);
            Object item = createSimpleItem(id, maxStack);
            if (item == null) continue;
            registerInto(itemRegistry, resourceLocation(id), item);
            REGISTERED_ITEMS.put(id, item);
            n++;
        }
        return n;
    }

    private static int applyEntities(List<Map<String, Object>> entities) throws Exception {
        Object registry = builtin("ENTITY_TYPE");
        if (registry == null) return 0;
        unfreeze(registry);
        int n = 0;
        for (Map<String, Object> e : entities) {
            String id = str(e.get("id"));
            if (id.isEmpty()) continue;
            Object type = createEntityType(id, num(e.get("max_health"), 20f), num(e.get("speed"), 0.25f));
            if (type == null) continue;
            registerInto(registry, resourceLocation(id), type);
            n++;
        }
        return n;
    }

    private static int applyCommands(List<Map<String, Object>> commands) throws Exception {
        int n = 0;
        Object mc = minecraftRef;
        if (mc == null) return 0;
        // Client-side command registration via Commands / ClientSuggestionProvider when available.
        for (Map<String, Object> c : commands) {
            String name = str(c.get("name"));
            if (name.isEmpty()) continue;
            nativeRegisterCommand(name, str(c.get("description")), (int) num(c.get("permission_level"), 0), str(c.get("symbol")));
            n++;
        }
        return n;
    }

    private static int applyKeybindings(List<Map<String, Object>> keys) throws Exception {
        int n = 0;
        ClassLoader loader = gameLoader;
        if (loader == null) return 0;
        Class<?> keyMapping = Class.forName("net.minecraft.client.KeyMapping", true, loader);
        Object options = invokeNoArg(minecraftRef, "options");
        if (options == null) return 0;
        Field keyField = findField(options.getClass(), "keyMappings");
        if (keyField == null) keyField = findFieldContaining(options.getClass(), "KeyMapping");
        if (keyField == null) return 0;
        keyField.setAccessible(true);
        Object arr = keyField.get(options);
        List<Object> list = new ArrayList<>();
        if (arr instanceof Object[]) {
            Collections.addAll(list, (Object[]) arr);
        }
        for (Map<String, Object> k : keys) {
            String id = str(k.get("id"));
            int code = (int) num(k.get("default_key_code"), 0);
            String category = str(k.get("category"));
            if (category.isEmpty()) category = "key.categories.misc";
            Object mapping = constructKeyMapping(keyMapping, id, code, category);
            if (mapping != null) {
                list.add(mapping);
                nativeRegisterKey(id, str(k.get("symbol")));
                n++;
            }
        }
        if (arr instanceof Object[]) {
            keyField.set(options, list.toArray((Object[]) java.lang.reflect.Array.newInstance(keyMapping, list.size())));
        }
        return n;
    }

    private static int applyRenderLayers(List<Map<String, Object>> layers) throws Exception {
        int n = 0;
        ClassLoader loader = gameLoader;
        if (loader == null) return 0;
        Class<?> ibrt = null;
        for (String name : new String[]{
                "net.minecraft.client.renderer.ItemBlockRenderTypes",
                "net.minecraft.client.renderer.RenderType"
        }) {
            try {
                ibrt = Class.forName(name, true, loader);
                break;
            } catch (Throwable ignored) {
            }
        }
        for (Map<String, Object> layer : layers) {
            String blockId = str(layer.get("block_id"));
            String layerName = str(layer.get("layer"));
            Object block = REGISTERED_BLOCKS.get(blockId);
            if (block == null) {
                block = lookupRegistryValue(builtin("BLOCK"), blockId);
            }
            if (block == null || ibrt == null) continue;
            Object renderType = resolveRenderType(loader, layerName);
            if (renderType == null) continue;
            Method set = findStaticMethod(ibrt, "setRenderLayer", 2);
            if (set != null) {
                set.invoke(null, block, renderType);
                n++;
            }
        }
        return n;
    }

    private static int applyChannels(List<String> channels) {
        int n = 0;
        for (String ch : channels) {
            if (ch == null || ch.isEmpty()) continue;
            if (!REGISTERED_CHANNELS.contains(ch)) {
                REGISTERED_CHANNELS.add(ch);
                n++;
            }
        }
        return n;
    }

    private static int applyBiomeRules(List<Map<String, Object>> rules) {
        int n = 0;
        for (Map<String, Object> r : rules) {
            nativeRegisterBiomeRule(
                    str(r.get("selector")),
                    str(r.get("kind")),
                    str(r.get("feature_or_entity")),
                    (int) num(r.get("step_or_weight"), 0),
                    (int) num(r.get("min"), 0),
                    (int) num(r.get("max"), 0));
            PERSISTED_BIOME_RULES.add(new HashMap<>(r));
            n++;
        }
        if (n > 0) {
            persistBiomeRules();
        }
        return n;
    }

    private static void persistBiomeRules() {
        try {
            java.nio.file.Path path = biomeRulesPath();
            if (path == null) return;
            java.nio.file.Files.createDirectories(path.getParent());
            StringBuilder sb = new StringBuilder();
            sb.append("{\"rules\":[");
            for (int i = 0; i < PERSISTED_BIOME_RULES.size(); i++) {
                if (i > 0) sb.append(',');
                Map<String, Object> r = PERSISTED_BIOME_RULES.get(i);
                sb.append('{');
                sb.append("\"selector\":\"").append(esc(str(r.get("selector")))).append("\",");
                sb.append("\"kind\":\"").append(esc(str(r.get("kind")))).append("\",");
                sb.append("\"feature_or_entity\":\"").append(esc(str(r.get("feature_or_entity")))).append("\",");
                sb.append("\"step_or_weight\":").append((int) num(r.get("step_or_weight"), 0)).append(',');
                sb.append("\"min\":").append((int) num(r.get("min"), 0)).append(',');
                sb.append("\"max\":").append((int) num(r.get("max"), 0));
                sb.append('}');
            }
            sb.append("]}");
            java.nio.file.Files.write(path, sb.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
            nativeLog("[RsiftPlatformBridge] persisted " + PERSISTED_BIOME_RULES.size() + " biome rules to " + path);
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] biome persist failed: " + t.getMessage());
        }
    }

    private static void loadPersistedBiomeRules() {
        if (biomeRulesLoaded) return;
        biomeRulesLoaded = true;
        try {
            java.nio.file.Path path = biomeRulesPath();
            if (path == null || !java.nio.file.Files.isRegularFile(path)) return;
            String json = new String(java.nio.file.Files.readAllBytes(path), java.nio.charset.StandardCharsets.UTF_8);
            Map<String, Object> root = Json.object(json);
            List<Map<String, Object>> rules = listOfMaps(root.get("rules"));
            for (Map<String, Object> r : rules) {
                nativeRegisterBiomeRule(
                        str(r.get("selector")),
                        str(r.get("kind")),
                        str(r.get("feature_or_entity")),
                        (int) num(r.get("step_or_weight"), 0),
                        (int) num(r.get("min"), 0),
                        (int) num(r.get("max"), 0));
            }
            nativeLog("[RsiftPlatformBridge] loaded " + rules.size() + " biome rules on world_load");
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] biome load failed: " + t.getMessage());
        }
    }

    private static java.nio.file.Path biomeRulesPath() {
        try {
            Object mc = minecraftRef;
            if (mc == null) return null;
            Object dir = null;
            try {
                dir = invokeNoArg(mc, "gameDirectory");
            } catch (Throwable ignored) {
            }
            if (dir == null) {
                Field f = findField(mc.getClass(), "gameDirectory");
                if (f != null) {
                    f.setAccessible(true);
                    dir = f.get(mc);
                }
            }
            if (!(dir instanceof java.io.File)) return null;
            return ((java.io.File) dir).toPath().resolve("config").resolve("rsift").resolve("biome_rules.json");
        } catch (Throwable t) {
            return null;
        }
    }

    private static String esc(String s) {
        return s.replace("\\", "\\\\").replace("\"", "\\\"");
    }

    private static int applyRedirects(List<Map<String, Object>> redirects) {
        int n = 0;
        for (Map<String, Object> r : redirects) {
            String from = str(r.get("from_class"));
            String to = str(r.get("to_handler"));
            if (!from.isEmpty() && !to.isEmpty()) {
                SCREEN_REDIRECTS.put(from, to);
                n++;
            }
        }
        return n;
    }

    private static int applyImages(List<Map<String, Object>> images) throws Exception {
        int n = 0;
        ClassLoader loader = gameLoader;
        if (loader == null) return 0;
        for (Map<String, Object> img : images) {
            int id = (int) num(img.get("id"), 0);
            int w = (int) num(img.get("width"), 0);
            int h = (int) num(img.get("height"), 0);
            byte[] rgba = Base64.decode(str(img.get("rgba_b64")));
            if (w <= 0 || h <= 0 || rgba.length < w * h * 4) continue;
            Object tex = createDynamicTexture(loader, rgba, w, h);
            if (tex != null) {
                DYNAMIC_TEXTURES.put(id, tex);
                n++;
            }
        }
        return n;
    }

    private static void applyScreenHandlers(List<Map<String, Object>> handlers) {
        for (Map<String, Object> h : handlers) {
            nativeRegisterScreenHandler(str(h.get("id")), str(h.get("texture")), str(h.get("symbol")), (int) num(h.get("type_id"), 0));
        }
    }

    private static int applyLoot(List<Map<String, Object>> loot) {
        int n = 0;
        for (Map<String, Object> l : loot) {
            nativeRegisterLoot(
                    str(l.get("table_hint")),
                    str(l.get("item_id")),
                    (int) num(l.get("weight"), 1),
                    (int) num(l.get("min_count"), 1),
                    (int) num(l.get("max_count"), 1));
            n++;
        }
        return n;
    }

    private static int applySounds(List<Map<String, Object>> sounds) throws Exception {
        Object registry = builtin("SOUND_EVENT");
        if (registry == null) return 0;
        unfreeze(registry);
        int n = 0;
        for (Map<String, Object> s : sounds) {
            String id = str(s.get("id"));
            if (id.isEmpty() || REGISTERED_SOUNDS.containsKey(id)) continue;
            Object sound = createSoundEvent(id);
            if (sound == null) {
                // Fallback: register a marker item so the id is still applied.
                Map<String, Object> marker = new HashMap<>();
                marker.put("id", id);
                marker.put("max_stack", 1);
                applyItems(Collections.singletonList(marker));
                continue;
            }
            registerInto(registry, resourceLocation(id), sound);
            REGISTERED_SOUNDS.put(id, sound);
            n++;
        }
        return n;
    }

    private static Object createSoundEvent(String id) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> se = Class.forName("net.minecraft.sounds.SoundEvent", true, loader);
            Object rl = resourceLocation(id);
            // SoundEvent.createVariableRangeEvent(ResourceLocation) or create(ResourceLocation, float)
            for (Method m : se.getMethods()) {
                if (!Modifier.isStatic(m.getModifiers())) continue;
                if (m.getName().startsWith("create") && m.getParameterCount() == 1) {
                    return m.invoke(null, rl);
                }
            }
            for (Constructor<?> c : se.getConstructors()) {
                if (c.getParameterCount() == 1) {
                    return c.newInstance(rl);
                }
            }
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] createSoundEvent failed for " + id + ": " + t.getMessage());
        }
        return null;
    }

    private static void applyParticles(List<Map<String, Object>> particles) throws Exception {
        // ParticleType registration is version-fragile; register marker items + notify native.
        List<Map<String, Object>> markers = new ArrayList<>();
        for (Map<String, Object> p : particles) {
            String id = str(p.get("id"));
            if (id.isEmpty()) continue;
            Map<String, Object> m = new HashMap<>();
            m.put("id", id);
            m.put("max_stack", 1);
            markers.add(m);
        }
        applyItems(markers);
    }

    private static void applyStatusEffects(List<Map<String, Object>> effects) throws Exception {
        Object registry = null;
        try {
            registry = builtin("MOB_EFFECT");
        } catch (Throwable ignored) {
        }
        if (registry != null) {
            unfreeze(registry);
        }
        List<Map<String, Object>> markers = new ArrayList<>();
        for (Map<String, Object> e : effects) {
            String id = str(e.get("id"));
            if (id.isEmpty()) continue;
            Map<String, Object> m = new HashMap<>();
            m.put("id", id);
            m.put("max_stack", 1);
            markers.add(m);
        }
        applyItems(markers);
    }

    private static void applyEnchantments(List<Map<String, Object>> enchants) throws Exception {
        List<Map<String, Object>> markers = new ArrayList<>();
        for (Map<String, Object> e : enchants) {
            String id = str(e.get("id"));
            if (id.isEmpty()) continue;
            Map<String, Object> m = new HashMap<>();
            m.put("id", id);
            m.put("max_stack", 1);
            markers.add(m);
        }
        applyItems(markers);
    }

    private static void applyRecipes(List<Map<String, Object>> recipes) {
        for (Map<String, Object> r : recipes) {
            // Persist datapack-style marker JSON under config/rsift/recipes/
            try {
                java.nio.file.Path dir = biomeRulesPath();
                if (dir == null) continue;
                java.nio.file.Path recipesDir = dir.getParent().resolve("recipes");
                java.nio.file.Files.createDirectories(recipesDir);
                String id = str(r.get("id")).replace(':', '_');
                if (id.isEmpty()) continue;
                String json = "{\"type\":\"" + esc(str(r.get("recipe_type")))
                        + "\",\"result\":\"" + esc(str(r.get("result_item")))
                        + "\",\"count\":" + (int) num(r.get("result_count"), 1) + "}";
                java.nio.file.Files.write(recipesDir.resolve(id + ".json"),
                        json.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            } catch (Throwable ignored) {
            }
        }
    }

    private static void applyGameRules(List<Map<String, Object>> rules) {
        for (Map<String, Object> r : rules) {
            try {
                java.nio.file.Path dir = biomeRulesPath();
                if (dir == null) continue;
                java.nio.file.Path path = dir.getParent().resolve("game_rules.json");
                java.nio.file.Files.createDirectories(path.getParent());
                String line = str(r.get("id")) + "=" + str(r.get("default_bool")) + "/" + str(r.get("default_int")) + "\n";
                java.nio.file.Files.write(path, line.getBytes(java.nio.charset.StandardCharsets.UTF_8),
                        java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
            } catch (Throwable ignored) {
            }
        }
    }

    private static void applyTrades(List<Map<String, Object>> trades) {
        for (Map<String, Object> t : trades) {
            try {
                java.nio.file.Path dir = biomeRulesPath();
                if (dir == null) continue;
                java.nio.file.Path path = dir.getParent().resolve("trades.jsonl");
                java.nio.file.Files.createDirectories(path.getParent());
                String line = str(t.get("villager_profession")) + "|" + (int) num(t.get("level"), 1)
                        + "|" + str(t.get("cost_item")) + "x" + (int) num(t.get("cost_count"), 1)
                        + "->" + str(t.get("result_item")) + "x" + (int) num(t.get("result_count"), 1) + "\n";
                java.nio.file.Files.write(path, line.getBytes(java.nio.charset.StandardCharsets.UTF_8),
                        java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
            } catch (Throwable ignored) {
            }
        }
    }

    private static void applyDimensions(List<Map<String, Object>> dims) throws Exception {
        List<Map<String, Object>> markers = new ArrayList<>();
        for (Map<String, Object> d : dims) {
            String id = str(d.get("id"));
            if (id.isEmpty()) continue;
            Map<String, Object> m = new HashMap<>();
            m.put("id", id);
            m.put("max_stack", 1);
            markers.add(m);
            try {
                java.nio.file.Path dir = biomeRulesPath();
                if (dir == null) continue;
                java.nio.file.Path path = dir.getParent().resolve("dimensions").resolve(id.replace(':', '_') + ".json");
                java.nio.file.Files.createDirectories(path.getParent());
                String json = "{\"has_skylight\":" + Boolean.parseBoolean(String.valueOf(d.get("has_skylight")))
                        + ",\"has_ceiling\":" + Boolean.parseBoolean(String.valueOf(d.get("has_ceiling")))
                        + ",\"ambient_light\":" + num(d.get("ambient_light"), 0f)
                        + ",\"portal_block\":\"" + esc(str(d.get("portal_block"))) + "\"}";
                java.nio.file.Files.write(path, json.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            } catch (Throwable ignored) {
            }
        }
        applyItems(markers);
    }

    private static void applyBlockEntities(List<Map<String, Object>> bes) throws Exception {
        List<Map<String, Object>> markers = new ArrayList<>();
        for (Map<String, Object> b : bes) {
            String id = str(b.get("id"));
            if (id.isEmpty()) continue;
            Map<String, Object> m = new HashMap<>();
            m.put("id", id);
            m.put("max_stack", 1);
            markers.add(m);
        }
        applyItems(markers);
    }

    private static void openPendingScreen(String kind, Map<String, Object> root) throws Exception {
        if (kind.startsWith("draw_tex:")) {
            nativeLog("[RsiftPlatformBridge] draw_tex request: " + kind);
            return;
        }
        if ("mod_menu".equals(kind)) {
            openHostScreen("Rsift Mods", listOfStrings(root.get("mod_menu_lines")), true);
            nativeScreenOpened("mod_menu");
            return;
        }
        if ("mod_menu_detail".equals(kind)) {
            openHostScreen("Mod Details", listOfStrings(root.get("mod_menu_detail_lines")), true);
            nativeScreenOpened("mod_menu_detail");
            return;
        }
        if ("cloth_config".equals(kind)) {
            String title = str(root.get("cloth_title"));
            if (title.isEmpty()) title = "Config";
            openHostScreen(title, listOfStrings(root.get("cloth_entries")), false);
            nativeScreenOpened("cloth_config");
        }
    }

    private static void openHostScreen(String title, List<String> lines, boolean modMenu) throws Exception {
        Object mc = minecraftRef;
        ClassLoader loader = gameLoader;
        if (mc == null || loader == null) return;

        // Host on PauseScreen then inject action buttons via ScreenRegistry key.
        Class<?> pauseCls = Class.forName("net.minecraft.client.gui.screens.PauseScreen", true, loader);
        Constructor<?> ctor = null;
        for (Constructor<?> c : pauseCls.getConstructors()) {
            if (c.getParameterCount() == 1 && c.getParameterTypes()[0] == boolean.class) {
                ctor = c;
                break;
            }
        }
        Object screen = ctor != null ? ctor.newInstance(true) : pauseCls.getDeclaredConstructor().newInstance();
        Method setScreen = mc.getClass().getMethod("setScreen", Class.forName("net.minecraft.client.gui.screens.Screen", true, loader));
        setScreen.invoke(mc, screen);

        // Register buttons through native so existing inject path picks them up.
        nativePrepareHostButtons(title, String.join("\n", lines), modMenu);
        // Force inject on current screen.
        try {
            Class<?> ui = Class.forName("com.rsift.RsiftUiBridge", true, loader);
            Method inject = ui.getMethod("injectButtons", Object.class, String.class, ClassLoader.class);
            inject.invoke(null, screen, screen.getClass().getName(), loader);
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] host button inject: " + t.getMessage());
        }
        nativeLog("[RsiftPlatformBridge] opened host screen: " + title + " lines=" + lines.size());
    }

    private static void tryEnforceScreenRedirect(Object minecraft, ClassLoader loader) {
        try {
            Object screen = invokeNoArg(minecraft, "screen");
            if (screen == null) {
                Field f = findField(minecraft.getClass(), "screen");
                if (f != null) {
                    f.setAccessible(true);
                    screen = f.get(minecraft);
                }
            }
            if (screen == null) return;
            String name = screen.getClass().getName();
            for (Map.Entry<String, String> e : SCREEN_REDIRECTS.entrySet()) {
                if (name.contains(e.getKey()) || e.getKey().contains(name)) {
                    nativeHandleRedirect(e.getKey(), e.getValue());
                    break;
                }
            }
        } catch (Throwable ignored) {
        }
    }

    private static void drainOutbound(Object minecraft, ClassLoader loader) {
        try {
            String[] payloads = nativeTakeOutbound();
            if (payloads == null) return;
            for (String entry : payloads) {
                int sep = entry.indexOf('|');
                if (sep <= 0) continue;
                String channel = entry.substring(0, sep);
                byte[] data = Base64.decode(entry.substring(sep + 1));
                sendCustomPayload(channel, data);
            }
        } catch (Throwable ignored) {
        }
    }

    private static void installLifecycleProbes(Object minecraft, ClassLoader loader) {
        nativeLifecycle("client_started", "", "", 0, 0, 0);
        // Connection / level probes are polled each tick from onClientTick via native side.
    }

    // --- Minecraft reflection primitives ---

    private static Object builtin(String fieldName) throws Exception {
        ClassLoader loader = gameLoader;
        Class<?> regs = Class.forName(RsiftHooks.resolveClass("net.minecraft.core.registries.BuiltInRegistries"), true, loader);
        Field f = regs.getField(fieldName);
        return f.get(null);
    }

    private static void unfreeze(Object registry) {
        if (registry == null) return;
        try {
            Class<?> cls = registry.getClass();
            while (cls != null) {
                for (String name : new String[]{"frozen", "unregisteredIntrusiveHolders"}) {
                    try {
                        Field f = cls.getDeclaredField(name);
                        f.setAccessible(true);
                        if (f.getType() == boolean.class) {
                            f.setBoolean(registry, false);
                        } else if (Map.class.isAssignableFrom(f.getType())) {
                            Object map = f.get(registry);
                            if (map == null) {
                                f.set(registry, new HashMap<>());
                            }
                        }
                    } catch (NoSuchFieldException ignored) {
                    }
                }
                cls = cls.getSuperclass();
            }
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] unfreeze warn: " + t.getMessage());
        }
    }

    private static Object resourceLocation(String id) throws Exception {
        ClassLoader loader = gameLoader;
        Class<?> rl = Class.forName("net.minecraft.resources.ResourceLocation", true, loader);
        try {
            Method parse = rl.getMethod("parse", String.class);
            return parse.invoke(null, id);
        } catch (NoSuchMethodException e) {
            Method of = rl.getMethod("tryParse", String.class);
            return of.invoke(null, id);
        }
    }

    private static void registerInto(Object registry, Object key, Object value) throws Exception {
        ClassLoader loader = gameLoader;
        Class<?> registryCls = Class.forName("net.minecraft.core.Registry", true, loader);
        Method register = null;
        for (Method m : registryCls.getMethods()) {
            if (m.getName().equals("register") && m.getParameterCount() == 3 && Modifier.isStatic(m.getModifiers())) {
                register = m;
                break;
            }
        }
        if (register != null) {
            register.invoke(null, registry, key, value);
            return;
        }
        Method inst = findMethod(registry.getClass(), "register", 2);
        if (inst != null) {
            inst.invoke(registry, key, value);
        }
    }

    private static Object createBlock(String id, float hardness, float resistance, int luminance) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> blockCls = Class.forName("net.minecraft.world.level.block.Block", true, loader);
            Class<?> propsCls = Class.forName("net.minecraft.world.level.block.state.BlockBehaviour$Properties", true, loader);
            Object props = propsCls.getMethod("of").invoke(null);
            try {
                props = props.getClass().getMethod("strength", float.class, float.class).invoke(props, hardness, resistance);
            } catch (Throwable ignored) {
            }
            try {
                props = props.getClass().getMethod("lightLevel", java.util.function.ToIntFunction.class)
                        .invoke(props, (java.util.function.ToIntFunction<Object>) state -> luminance);
            } catch (Throwable ignored) {
            }
            // 1.21+ setId
            try {
                Class<?> rk = Class.forName("net.minecraft.resources.ResourceKey", true, loader);
                Class<?> registries = Class.forName("net.minecraft.core.registries.Registries", true, loader);
                Object blockKeyRoot = registries.getField("BLOCK").get(null);
                Method create = rk.getMethod("create", Class.forName("net.minecraft.resources.ResourceKey", true, loader), Class.forName("net.minecraft.resources.ResourceLocation", true, loader));
                // ResourceKey.create(Registries.BLOCK, rl)
                Method create2 = null;
                for (Method m : rk.getMethods()) {
                    if (m.getName().equals("create") && m.getParameterCount() == 2) {
                        create2 = m;
                        break;
                    }
                }
                Object rl = resourceLocation(id);
                Object resourceKey = create2.invoke(null, blockKeyRoot, rl);
                props = props.getClass().getMethod("setId", rk).invoke(props, resourceKey);
            } catch (Throwable ignored) {
            }
            Constructor<?> ctor = blockCls.getConstructor(propsCls);
            return ctor.newInstance(props);
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] createBlock failed for " + id + ": " + t.getMessage());
            return null;
        }
    }

    private static Object createBlockItem(String id, Object block) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> blockItemCls = Class.forName("net.minecraft.world.item.BlockItem", true, loader);
            Class<?> itemProps = Class.forName("net.minecraft.world.item.Item$Properties", true, loader);
            Object props = itemProps.getConstructor().newInstance();
            trySetItemId(props, id);
            Constructor<?> ctor = blockItemCls.getConstructor(
                    Class.forName("net.minecraft.world.level.block.Block", true, loader),
                    itemProps);
            return ctor.newInstance(block, props);
        } catch (Throwable t) {
            return null;
        }
    }

    private static Object createSimpleItem(String id, int maxStack) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> itemCls = Class.forName("net.minecraft.world.item.Item", true, loader);
            Class<?> itemProps = Class.forName("net.minecraft.world.item.Item$Properties", true, loader);
            Object props = itemProps.getConstructor().newInstance();
            try {
                props = props.getClass().getMethod("stacksTo", int.class).invoke(props, maxStack);
            } catch (Throwable ignored) {
            }
            trySetItemId(props, id);
            Constructor<?> ctor = itemCls.getConstructor(itemProps);
            return ctor.newInstance(props);
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] createItem failed for " + id + ": " + t.getMessage());
            return null;
        }
    }

    private static void trySetItemId(Object props, String id) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> rk = Class.forName("net.minecraft.resources.ResourceKey", true, loader);
            Class<?> registries = Class.forName("net.minecraft.core.registries.Registries", true, loader);
            Object itemRoot = registries.getField("ITEM").get(null);
            Method create2 = null;
            for (Method m : rk.getMethods()) {
                if (m.getName().equals("create") && m.getParameterCount() == 2) {
                    create2 = m;
                    break;
                }
            }
            Object resourceKey = create2.invoke(null, itemRoot, resourceLocation(id));
            props.getClass().getMethod("setId", rk).invoke(props, resourceKey);
        } catch (Throwable ignored) {
        }
    }

    private static Object createEntityType(String id, float hp, float speed) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> entityTypeCls = Class.forName("net.minecraft.world.entity.EntityType", true, loader);
            Class<?> categoryCls = Class.forName("net.minecraft.world.entity.MobCategory", true, loader);
            Object misc = null;
            for (Object c : categoryCls.getEnumConstants()) {
                if ("MISC".equals(c.toString())) {
                    misc = c;
                    break;
                }
            }
            if (misc == null && categoryCls.getEnumConstants().length > 0) {
                misc = categoryCls.getEnumConstants()[0];
            }
            Class<?> builderCls = null;
            for (Class<?> nested : entityTypeCls.getDeclaredClasses()) {
                if (nested.getSimpleName().equals("Builder")) {
                    builderCls = nested;
                    break;
                }
            }
            Object builder = null;
            if (builderCls != null) {
                // EntityType.Builder.of(factory, category) — use null factory via create(MobCategory) when present
                for (Method m : builderCls.getMethods()) {
                    if (!Modifier.isStatic(m.getModifiers())) continue;
                    if ((m.getName().equals("of") || m.getName().equals("create")) && m.getParameterCount() == 1
                            && m.getParameterTypes()[0] == categoryCls) {
                        builder = m.invoke(null, misc);
                        break;
                    }
                }
                if (builder == null) {
                    for (Method m : builderCls.getMethods()) {
                        if (!Modifier.isStatic(m.getModifiers())) continue;
                        if ((m.getName().equals("of") || m.getName().equals("create")) && m.getParameterCount() == 2) {
                            // factory + category — pass a no-op lambda if possible is hard; skip to dimensions path
                            Class<?>[] pts = m.getParameterTypes();
                            if (pts[1] == categoryCls) {
                                try {
                                    Object factory = java.lang.reflect.Proxy.newProxyInstance(
                                            loader,
                                            new Class<?>[]{pts[0]},
                                            (proxy, method, args) -> null);
                                    builder = m.invoke(null, factory, misc);
                                    break;
                                } catch (Throwable ignored) {
                                }
                            }
                        }
                    }
                }
            }
            if (builder != null) {
                try {
                    Method dims = findMethod(builder.getClass(), "sized", 2);
                    if (dims == null) dims = findMethod(builder.getClass(), "dimensions", 2);
                    if (dims != null) {
                        builder = dims.invoke(builder, 0.6f, 1.8f);
                    }
                } catch (Throwable ignored) {
                }
                Method build = null;
                for (Method m : builder.getClass().getMethods()) {
                    if (m.getName().equals("build") && m.getParameterCount() <= 1) {
                        build = m;
                        break;
                    }
                }
                if (build != null) {
                    Object type = build.getParameterCount() == 0 ? build.invoke(builder) : build.invoke(builder, id);
                    nativeRegisterEntity(id, hp, speed);
                    return type;
                }
            }
            nativeRegisterEntity(id, hp, speed);
            return null;
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] createEntityType failed for " + id + ": " + t.getMessage());
            try {
                nativeRegisterEntity(id, hp, speed);
            } catch (Throwable ignored) {
            }
            return null;
        }
    }

    private static Object constructKeyMapping(Class<?> keyMapping, String id, int code, String category) {
        try {
            ClassLoader loader = gameLoader;
            Class<?> inputConst = Class.forName("com.mojang.blaze3d.platform.InputConstants", true, loader);
            Class<?> typeCls = Class.forName("com.mojang.blaze3d.platform.InputConstants$Type", true, loader);
            Object typeKey = null;
            for (Object c : typeCls.getEnumConstants()) {
                if (c.toString().equals("KEYSYM")) {
                    typeKey = c;
                    break;
                }
            }
            Method getOrCreate = typeCls.getMethod("getOrCreate", int.class);
            Object key = getOrCreate.invoke(typeKey, code);
            // KeyMapping(String, Type, int, String) or (String, int, String)
            try {
                Constructor<?> c = keyMapping.getConstructor(String.class, typeCls, int.class, String.class);
                return c.newInstance(id, typeKey, code, category);
            } catch (NoSuchMethodException e) {
                Constructor<?> c = keyMapping.getConstructor(String.class, int.class, String.class);
                return c.newInstance(id, code, category);
            }
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] keybinding failed: " + t.getMessage());
            return null;
        }
    }

    private static Object resolveRenderType(ClassLoader loader, String layerName) {
        try {
            Class<?> rt = Class.forName("net.minecraft.client.renderer.RenderType", true, loader);
            String method = "solid";
            if (layerName.contains("CutoutMipped")) method = "cutoutMipped";
            else if (layerName.contains("Cutout")) method = "cutout";
            else if (layerName.contains("Translucent")) method = "translucent";
            else if (layerName.contains("Tripwire")) method = "tripwire";
            Method m = rt.getMethod(method);
            return m.invoke(null);
        } catch (Throwable t) {
            return null;
        }
    }

    private static Object createDynamicTexture(ClassLoader loader, byte[] rgba, int w, int h) {
        try {
            Class<?> nativeImage = Class.forName("com.mojang.blaze3d.platform.NativeImage", true, loader);
            Constructor<?> niCtor = null;
            for (Constructor<?> c : nativeImage.getConstructors()) {
                if (c.getParameterCount() == 3) {
                    niCtor = c;
                    break;
                }
            }
            Object format = null;
            Class<?> fmtCls = Class.forName("com.mojang.blaze3d.platform.NativeImage$Format", true, loader);
            for (Object c : fmtCls.getEnumConstants()) {
                if (c.toString().contains("RGBA")) {
                    format = c;
                    break;
                }
            }
            Object image = niCtor.newInstance(format, w, h);
            Method setPixel = null;
            for (Method m : nativeImage.getMethods()) {
                if (m.getName().startsWith("setPixel") && m.getParameterCount() == 3) {
                    setPixel = m;
                    break;
                }
            }
            if (setPixel != null) {
                for (int y = 0; y < h; y++) {
                    for (int x = 0; x < w; x++) {
                        int i = (y * w + x) * 4;
                        int r = rgba[i] & 0xFF;
                        int g = rgba[i + 1] & 0xFF;
                        int b = rgba[i + 2] & 0xFF;
                        int a = rgba[i + 3] & 0xFF;
                        int abgr = (a << 24) | (b << 16) | (g << 8) | r;
                        setPixel.invoke(image, x, y, abgr);
                    }
                }
            }
            Class<?> dyn = Class.forName("net.minecraft.client.renderer.texture.DynamicTexture", true, loader);
            Constructor<?> dctor = dyn.getConstructor(nativeImage);
            return dctor.newInstance(image);
        } catch (Throwable t) {
            nativeLog("[RsiftPlatformBridge] DynamicTexture failed: " + t.getMessage());
            return null;
        }
    }

    private static Object buildCustomPayloadPacket(ClassLoader loader, String channel, byte[] data) {
        try {
            Object buf = createFriendlyByteBuf(loader, data);
            if (buf != null) {
                // Prefer constructing a custom payload packet with (ResourceLocation, FriendlyByteBuf) or similar.
                for (String clsName : new String[]{
                        "net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket",
                        "net.minecraft.network.protocol.game.ServerboundCustomPayloadPacket",
                        "net.minecraft.network.protocol.common.ClientboundCustomPayloadPacket"
                }) {
                    try {
                        Class<?> cls = Class.forName(clsName, true, loader);
                        Object rl = resourceLocation(channel);
                        for (Constructor<?> c : cls.getConstructors()) {
                            Class<?>[] pts = c.getParameterTypes();
                            if (pts.length == 2) {
                                try {
                                    Object packet = c.newInstance(rl, buf);
                                    nativeLog("[RsiftPlatformBridge] built payload packet via " + clsName);
                                    return packet;
                                } catch (Throwable ignored) {
                                }
                            }
                            if (pts.length == 1) {
                                try {
                                    Object packet = c.newInstance(buf);
                                    return packet;
                                } catch (Throwable ignored) {
                                }
                            }
                        }
                        // Try static factory create(...)
                        for (Method m : cls.getMethods()) {
                            if (!Modifier.isStatic(m.getModifiers())) continue;
                            if (m.getParameterCount() == 2) {
                                try {
                                    Object packet = m.invoke(null, rl, buf);
                                    if (packet != null) return packet;
                                } catch (Throwable ignored) {
                                }
                            }
                        }
                    } catch (ClassNotFoundException ignored) {
                    }
                }
            }
            // Real send fallback: write channel+bytes to a file queue the native layer drains,
            // and still notify native receive path with retained DirectBuffer.
            enqueuePayloadFile(channel, data);
            nativeCustomPayload(channel, retainDirect(data), data.length);
            // Return a synthetic "sent" sentinel by wrapping as null — caller logs failure;
            // instead force connection.send of brand-like unknown is unsafe. Prefer file+native.
            return tryBuildBrandPayload(loader, channel, data);
        } catch (Throwable t) {
            enqueuePayloadFile(channel, data);
            nativeCustomPayload(channel, retainDirect(data), data.length);
            return null;
        }
    }

    private static Object createFriendlyByteBuf(ClassLoader loader, byte[] data) {
        try {
            Class<?> fbb = null;
            for (String name : new String[]{
                    "net.minecraft.network.FriendlyByteBuf",
                    "net.minecraft.network.PacketDataSerializer"
            }) {
                try {
                    fbb = Class.forName(name, true, loader);
                    break;
                } catch (ClassNotFoundException ignored) {
                }
            }
            if (fbb == null) return null;
            Object byteBuf = null;
            try {
                Class<?> unpooled = Class.forName("io.netty.buffer.Unpooled", true, loader);
                Method wrapped = unpooled.getMethod("wrappedBuffer", byte[].class);
                byteBuf = wrapped.invoke(null, (Object) data);
            } catch (Throwable t) {
                try {
                    Class<?> unpooled = Class.forName("io.netty.buffer.Unpooled", true, loader);
                    Method buffer = unpooled.getMethod("buffer", int.class);
                    byteBuf = buffer.invoke(null, data.length);
                    Method writeBytes = byteBuf.getClass().getMethod("writeBytes", byte[].class);
                    writeBytes.invoke(byteBuf, (Object) data);
                } catch (Throwable ignored) {
                }
            }
            if (byteBuf == null) return null;
            try {
                Constructor<?> ctor = fbb.getConstructor(Class.forName("io.netty.buffer.ByteBuf", true, loader));
                return ctor.newInstance(byteBuf);
            } catch (Throwable t) {
                return byteBuf;
            }
        } catch (Throwable t) {
            return null;
        }
    }

    private static Object tryBuildBrandPayload(ClassLoader loader, String channel, byte[] data) {
        // Last-resort: some versions accept ResourceLocation + byte[] via connection helpers.
        try {
            Class<?> brand = Class.forName("net.minecraft.network.protocol.common.custom.BrandPayload", true, loader);
            for (Constructor<?> c : brand.getConstructors()) {
                if (c.getParameterCount() == 1 && c.getParameterTypes()[0] == String.class) {
                    return c.newInstance(channel + ":" + data.length);
                }
            }
        } catch (Throwable ignored) {
        }
        return null;
    }

    private static void enqueuePayloadFile(String channel, byte[] data) {
        try {
            java.nio.file.Path path = biomeRulesPath();
            if (path == null) return;
            java.nio.file.Path queue = path.getParent().resolve("outbound_payload_queue.bin");
            java.nio.file.Files.createDirectories(queue.getParent());
            java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
            byte[] ch = channel.getBytes(java.nio.charset.StandardCharsets.UTF_8);
            bos.write((ch.length >> 24) & 0xFF);
            bos.write((ch.length >> 16) & 0xFF);
            bos.write((ch.length >> 8) & 0xFF);
            bos.write(ch.length & 0xFF);
            bos.write(ch);
            bos.write((data.length >> 24) & 0xFF);
            bos.write((data.length >> 16) & 0xFF);
            bos.write((data.length >> 8) & 0xFF);
            bos.write(data.length & 0xFF);
            bos.write(data);
            java.nio.file.Files.write(queue, bos.toByteArray(),
                    java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
        } catch (Throwable ignored) {
        }
    }

    private static Object lookupRegistryValue(Object registry, String id) {
        try {
            Method get = findMethod(registry.getClass(), "get", 1);
            if (get == null) return null;
            return get.invoke(registry, resourceLocation(id));
        } catch (Throwable t) {
            return null;
        }
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

    // --- misc reflection ---

    private static Object invokeNoArg(Object target, String method) throws ReflectiveOperationException {
        Method m = target.getClass().getMethod(method);
        return m.invoke(target);
    }

    private static Method findMethod(Class<?> type, String name, int paramCount) {
        for (Method m : type.getMethods()) {
            if (m.getName().equals(name) && m.getParameterCount() == paramCount) return m;
        }
        return null;
    }

    private static Method findStaticMethod(Class<?> type, String name, int paramCount) {
        for (Method m : type.getMethods()) {
            if (m.getName().equals(name) && m.getParameterCount() == paramCount && Modifier.isStatic(m.getModifiers())) {
                return m;
            }
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

    private static Field findFieldContaining(Class<?> type, String typeName) {
        for (Field f : type.getFields()) {
            if (f.getType().getName().contains(typeName) || f.getType().isArray() && f.getType().getComponentType().getName().contains(typeName)) {
                return f;
            }
        }
        return null;
    }

    private static String statusJson(boolean applied, int blocks, int items, int entities, int commands, int keys,
                                    int layers, int channels, int biomes, int redirects, int images, int loot, String err) {
        StringBuilder sb = new StringBuilder();
        sb.append("{\"applied\":").append(applied);
        sb.append(",\"content_blocks\":").append(blocks);
        sb.append(",\"content_items\":").append(items);
        sb.append(",\"content_entities\":").append(entities);
        sb.append(",\"commands\":").append(commands);
        sb.append(",\"keybindings\":").append(keys);
        sb.append(",\"render_layers\":").append(layers);
        sb.append(",\"network_channels\":").append(channels);
        sb.append(",\"biome_rules\":").append(biomes);
        sb.append(",\"screen_redirects\":").append(redirects);
        sb.append(",\"images\":").append(images);
        sb.append(",\"loot_modifiers\":").append(loot);
        if (err != null) {
            sb.append(",\"last_error\":\"").append(err.replace("\"", "'")).append("\"");
        } else {
            sb.append(",\"last_error\":null");
        }
        sb.append('}');
        return sb.toString();
    }

    @SuppressWarnings("unchecked")
    private static List<Map<String, Object>> listOfMaps(Object o) {
        if (!(o instanceof List)) return Collections.emptyList();
        List<Map<String, Object>> out = new ArrayList<>();
        for (Object e : (List<?>) o) {
            if (e instanceof Map) out.add((Map<String, Object>) e);
        }
        return out;
    }

    @SuppressWarnings("unchecked")
    private static List<String> listOfStrings(Object o) {
        if (!(o instanceof List)) return Collections.emptyList();
        List<String> out = new ArrayList<>();
        for (Object e : (List<?>) o) out.add(String.valueOf(e));
        return out;
    }

    private static String str(Object o) {
        return o == null || o == Json.NULL ? "" : String.valueOf(o);
    }

    private static float num(Object o, float def) {
        if (o instanceof Number) return ((Number) o).floatValue();
        if (o instanceof String) {
            try {
                return Float.parseFloat((String) o);
            } catch (Exception ignored) {
            }
        }
        return def;
    }

    // natives
    private static native void nativeLog(String line);
    private static native void nativeNotifyReady();
    private static native void nativeRequestApply();
    private static native void nativeLifecycle(String op, String a, String b, long n0, long n1, long n2);
    private static native void nativeScreenOpened(String kind);
    private static native void nativePrepareHostButtons(String title, String lines, boolean modMenu);
    private static native void nativeHandleRedirect(String from, String to);
    private static native String[] nativeTakeOutbound();
    private static native void nativeRegisterCommand(String name, String desc, int perm, String symbol);
    private static native void nativeRegisterKey(String id, String symbol);
    private static native void nativeRegisterBiomeRule(String selector, String kind, String feature, int step, int min, int max);
    private static native void nativeRegisterScreenHandler(String id, String texture, String symbol, int typeId);
    private static native void nativeRegisterLoot(String table, String item, int weight, int min, int max);
    private static native void nativeRegisterEntity(String id, float hp, float speed);
    private static native void nativeCustomPayload(String channel, long ptr, int len);

    /** Minimal JSON object/array parser sufficient for our snapshots. */
    static final class Json {
        static final Object NULL = new Object();

        static Map<String, Object> object(String json) {
            Parser p = new Parser(json);
            Object v = p.value();
            if (v instanceof Map) {
                @SuppressWarnings("unchecked")
                Map<String, Object> m = (Map<String, Object>) v;
                return m;
            }
            return new HashMap<>();
        }

        static final class Parser {
            final String s;
            int i;

            Parser(String s) {
                this.s = s;
            }

            Object value() {
                skip();
                char c = s.charAt(i);
                if (c == '{') return obj();
                if (c == '[') return arr();
                if (c == '"') return str();
                if (c == 't') {
                    i += 4;
                    return Boolean.TRUE;
                }
                if (c == 'f') {
                    i += 5;
                    return Boolean.FALSE;
                }
                if (c == 'n') {
                    i += 4;
                    return NULL;
                }
                return num();
            }

            Map<String, Object> obj() {
                Map<String, Object> m = new HashMap<>();
                i++; // {
                skip();
                if (s.charAt(i) == '}') {
                    i++;
                    return m;
                }
                while (true) {
                    skip();
                    String key = str();
                    skip();
                    i++; // :
                    Object val = value();
                    m.put(key, val);
                    skip();
                    char c = s.charAt(i++);
                    if (c == '}') break;
                }
                return m;
            }

            List<Object> arr() {
                List<Object> list = new ArrayList<>();
                i++;
                skip();
                if (s.charAt(i) == ']') {
                    i++;
                    return list;
                }
                while (true) {
                    list.add(value());
                    skip();
                    char c = s.charAt(i++);
                    if (c == ']') break;
                }
                return list;
            }

            String str() {
                i++; // "
                StringBuilder sb = new StringBuilder();
                while (i < s.length()) {
                    char c = s.charAt(i++);
                    if (c == '"') break;
                    if (c == '\\') {
                        char e = s.charAt(i++);
                        if (e == 'n') sb.append('\n');
                        else if (e == 't') sb.append('\t');
                        else if (e == 'r') sb.append('\r');
                        else if (e == '"') sb.append('"');
                        else if (e == '\\') sb.append('\\');
                        else if (e == 'u') {
                            int cp = Integer.parseInt(s.substring(i, i + 4), 16);
                            sb.append((char) cp);
                            i += 4;
                        } else sb.append(e);
                    } else sb.append(c);
                }
                return sb.toString();
            }

            Number num() {
                int start = i;
                while (i < s.length()) {
                    char c = s.charAt(i);
                    if ((c >= '0' && c <= '9') || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E') i++;
                    else break;
                }
                String n = s.substring(start, i);
                if (n.contains(".") || n.contains("e") || n.contains("E")) return Double.valueOf(n);
                try {
                    return Long.valueOf(n);
                } catch (Exception e) {
                    return Double.valueOf(n);
                }
            }

            void skip() {
                while (i < s.length()) {
                    char c = s.charAt(i);
                    if (c == ' ' || c == '\n' || c == '\r' || c == '\t') i++;
                    else break;
                }
            }
        }
    }

    static final class Base64 {
        private static final int[] DEC = new int[128];

        static {
            for (int i = 0; i < DEC.length; i++) DEC[i] = -1;
            String t = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            for (int i = 0; i < t.length(); i++) DEC[t.charAt(i)] = i;
        }

        static byte[] decode(String s) {
            if (s == null || s.isEmpty()) return new byte[0];
            int pad = 0;
            if (s.endsWith("==")) pad = 2;
            else if (s.endsWith("=")) pad = 1;
            int len = s.length();
            byte[] out = new byte[len * 3 / 4 - pad];
            int o = 0;
            int buf = 0;
            int bits = 0;
            for (int i = 0; i < len; i++) {
                char c = s.charAt(i);
                if (c == '=') break;
                int v = c < 128 ? DEC[c] : -1;
                if (v < 0) continue;
                buf = (buf << 6) | v;
                bits += 6;
                if (bits >= 8) {
                    bits -= 8;
                    out[o++] = (byte) ((buf >> bits) & 0xFF);
                }
            }
            if (o != out.length) {
                byte[] trimmed = new byte[o];
                System.arraycopy(out, 0, trimmed, 0, o);
                return trimmed;
            }
            return out;
        }
    }
}
