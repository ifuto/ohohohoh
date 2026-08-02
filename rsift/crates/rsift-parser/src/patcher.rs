//! # Bytecode Patcher & Injection Engine
//!
//! Rewrites target Minecraft class files by injecting `invokestatic` calls to
//! `com.rsift.RsiftHooks` at method HEAD. JVMTI ClassFileLoadHook / transformer
//! consumes `PatchResult::new_bytecode`.

use crate::class_file::ClassFileView;
use crate::class_rewriter::{hook_inject_for_redirect, ClassRewriter, HeadInject};
use crate::compute_redirect::{is_compute_class, redirects_for_class};
use crate::mixin_eq::MixinInjector;
use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info};

#[derive(Debug)]
pub struct PatchResult {
    pub class_name: String,
    pub was_modified: bool,
    pub new_bytecode: Vec<u8>,
}

pub const TARGET_CONNECTION_CLASS: &str = "net/minecraft/network/Connection";
pub const TARGET_RENDER_CLASS: &str = "com/mojang/blaze3d/systems/RenderSystem";
pub const TARGET_MINECRAFT_CLIENT: &str = "net/minecraft/client/Minecraft";
pub const TARGET_SERVER_LEVEL: &str = "net/minecraft/server/level/ServerLevel";
pub const TARGET_MOB: &str = "net/minecraft/world/entity/Mob";
pub const TARGET_ENTITY: &str = "net/minecraft/world/entity/Entity";
pub const TARGET_REDSTONE_WIRE: &str = "net/minecraft/world/level/redstone/RedstoneWireBlock";
pub const TARGET_LEVEL_CHUNK: &str = "net/minecraft/world/level/chunk/LevelChunk";
pub const TARGET_HOPPER: &str = "net/minecraft/world/level/block/entity/HopperBlockEntity";
pub const TARGET_FLOWING_FLUID: &str = "net/minecraft/world/level/material/FlowingFluid";
pub const TARGET_SCREEN: &str = "net/minecraft/client/gui/screens/Screen";

fn dynamic_targets() -> &'static Mutex<HashSet<String>> {
    static TARGETS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashSet::new()))
}

static GLOBAL_MIXINS: OnceLock<Mutex<MixinInjector>> = OnceLock::new();

fn global_mixins() -> &'static Mutex<MixinInjector> {
    GLOBAL_MIXINS.get_or_init(|| Mutex::new(MixinInjector::new()))
}

pub fn register_dynamic_target(class_name: &str) {
    let name = class_name.replace('.', "/");
    dynamic_targets().lock().unwrap().insert(name);
}

pub fn register_mixin_rule(rule: crate::mixin_eq::MixinRule) {
    register_dynamic_target(&rule.target_class);
    global_mixins().lock().unwrap().register_rule(rule);
}

/// 静的ターゲット一覧 (wave 205: RetransformClasses 対象収集の消費者 = rsift-jvm)。
pub fn static_target_classes() -> [&'static str; 11] {
    [
        TARGET_CONNECTION_CLASS,
        TARGET_RENDER_CLASS,
        TARGET_MINECRAFT_CLIENT,
        TARGET_SERVER_LEVEL,
        TARGET_MOB,
        TARGET_ENTITY,
        TARGET_REDSTONE_WIRE,
        TARGET_LEVEL_CHUNK,
        TARGET_HOPPER,
        TARGET_FLOWING_FLUID,
        TARGET_SCREEN,
    ]
}

/// dynamic targets のスナップショット (wave 205: Retransform 対象収集の消費者)。
pub fn dynamic_targets_snapshot() -> Vec<String> {
    dynamic_targets()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default()
}

pub struct BytecodePatcher;

impl BytecodePatcher {
    #[inline]
    fn normalize_class_name(class_name: &str) -> String {
        class_name.replace('.', "/")
    }

    #[inline]
    pub fn is_target_class(class_name: &str) -> bool {
        let name = Self::normalize_class_name(class_name);
        matches!(
            name.as_str(),
            TARGET_CONNECTION_CLASS
                | TARGET_RENDER_CLASS
                | TARGET_MINECRAFT_CLIENT
                | TARGET_SERVER_LEVEL
                | TARGET_MOB
                | TARGET_ENTITY
                | TARGET_REDSTONE_WIRE
                | TARGET_LEVEL_CHUNK
                | TARGET_HOPPER
                | TARGET_FLOWING_FLUID
                | TARGET_SCREEN
        ) || is_compute_class(&name)
            || dynamic_targets()
                .lock()
                .map(|g| g.contains(&name))
                .unwrap_or(false)
            || global_mixins()
                .lock()
                .map(|m| {
                    m.rules
                        .iter()
                        .any(|r| r.target_class.replace('.', "/") == name)
                })
                .unwrap_or(false)
            || crate::f3_marker::is_f3_target(&name)
    }

    pub fn patch_if_needed(class_name: &str, raw_data: &[u8]) -> Result<PatchResult, String> {
        let class_name = Self::normalize_class_name(class_name);
        if !Self::is_target_class(&class_name) {
            return Ok(PatchResult {
                class_name,
                was_modified: false,
                new_bytecode: Vec::new(),
            });
        }

        let view = match ClassFileView::parse(raw_data) {
            Ok(v) => v,
            Err(_) => {
                return Ok(PatchResult {
                    class_name,
                    was_modified: false,
                    new_bytecode: Vec::new(),
                });
            }
        };

        if view.this_class_name != class_name && !class_name.ends_with(&view.this_class_name) {
            // Allow subclass names for Screen hierarchy via dynamic targets
            if !dynamic_targets()
                .lock()
                .map(|g| g.contains(&class_name))
                .unwrap_or(false)
            {
                return Ok(PatchResult {
                    class_name,
                    was_modified: false,
                    new_bytecode: Vec::new(),
                });
            }
        }

        debug!("Rsift-Parser patching {}", class_name);

        let mut injects: Vec<HeadInject> = Vec::new();

        match class_name.as_str() {
            TARGET_CONNECTION_CLASS => {
                injects.push(hook_inject_for_redirect(
                    "channelRead0",
                    "",
                    "onNetworkPacket",
                ));
            }
            TARGET_RENDER_CLASS => {
                injects.push(hook_inject_for_redirect("flipFrame", "()V", "onRenderFlip"));
            }
            TARGET_MINECRAFT_CLIENT => {
                injects.push(hook_inject_for_redirect("tick", "", "onClientTickHook"));
                injects.push(hook_inject_for_redirect("run", "", "onClientRun"));
            }
            TARGET_SCREEN => {
                injects.push(hook_inject_for_redirect("init", "()V", "onScreenInit"));
            }
            _ => {}
        }

        for redirect in redirects_for_class(&class_name) {
            let hook = match redirect.method_name {
                "aiStep" => "onMobAiStep",
                "travel" => "onEntityTravel",
                "calculateTargetStrength" => "onRedstoneCalculate",
                "tick" if class_name.contains("LevelChunk") => "onChunkTick",
                "tick" if class_name.contains("Hopper") => "onHopperTick",
                "tick" if class_name.contains("ServerLevel") => "onServerLevelTick",
                "tick" if class_name.contains("FlowingFluid") => "onFluidTick",
                "channelRead0" => "onNetworkPacket",
                other => {
                    // Generic hook name from symbol
                    let _ = other;
                    "onGenericCompute"
                }
            };
            injects.push(hook_inject_for_redirect(
                redirect.method_name,
                redirect.descriptor,
                hook,
            ));
        }

        let mut rewriter = ClassRewriter::from_bytes(raw_data).map_err(|e| format!("{:?}", e))?;
        let mut modified = false;

        if !injects.is_empty() {
            match rewriter.inject_head_calls(&injects) {
                Ok(n) if n > 0 => {
                    modified = true;
                    info!("Injected {} HEAD hook(s) into {}", n, class_name);
                }
                Ok(_) => {}
                Err(e) => debug!("HEAD inject skipped for {}: {:?}", class_name, e),
            }
        }

        let mut bytes = rewriter.into_bytes();

        // Apply registered Mixin rules (real bytecode inject)
        if let Ok(mixins) = global_mixins().lock() {
            if let Some(out) = mixins.apply_mixins(&class_name, &bytes) {
                bytes = out;
                modified = true;
            }
            // Also try dotted name
            let dotted = class_name.replace('/', ".");
            if let Some(out) = mixins.apply_mixins(&dotted, &bytes) {
                bytes = out;
                modified = true;
            }
        }

        // F3 マーカー (wave 205): 実行時 picker が登録したクラスのみ。
        // HEAD 注入系とは独立した TAIL 注入 (保守条件不合なら内部で拒否)。
        if crate::f3_marker::is_f3_target(&class_name) {
            if let Some(out) = crate::f3_marker::apply_registered_markers(&class_name, &bytes) {
                bytes = out;
                modified = true;
                info!("Injected F3 marker into {}", class_name);
            }
        }

        // Only report modified when bytes actually differ
        if modified && bytes.as_slice() == raw_data {
            modified = false;
        }

        Ok(PatchResult {
            class_name,
            was_modified: modified,
            new_bytecode: if modified { bytes } else { Vec::new() },
        })
    }

    pub fn batch_process_classes(classes: &[(String, Vec<u8>)]) -> Vec<PatchResult> {
        classes
            .par_iter()
            .map(|(name, data)| {
                Self::patch_if_needed(name, data).unwrap_or_else(|_| PatchResult {
                    class_name: name.clone(),
                    was_modified: false,
                    new_bytecode: Vec::new(),
                })
            })
            .collect()
    }
}
