//! Compute class redirect registry — maps vanilla hot methods to native C ABI symbols.
//! Bytecode redirects call these; on PARITY_JVM_FALLBACK vanilla method runs unchanged.

/// Vanilla class → method → native DLL symbol (zero spec change: fallback always available)
pub struct ComputeRedirect {
    pub class_name: &'static str,
    pub method_name: &'static str,
    pub descriptor: &'static str,
    pub native_symbol: &'static str,
}

pub const COMPUTE_REDIRECTS: &[ComputeRedirect] = &[
    ComputeRedirect {
        class_name: "net/minecraft/world/entity/Mob",
        method_name: "aiStep",
        descriptor: "()V",
        native_symbol: "rsift_native_mob_ai_step",
    },
    ComputeRedirect {
        class_name: "net/minecraft/world/entity/Entity",
        method_name: "travel",
        descriptor: "(Lnet/minecraft/world/phys/Vec3;)Lnet/minecraft/world/phys/Vec3;",
        native_symbol: "rsift_native_entity_travel",
    },
    ComputeRedirect {
        class_name: "net/minecraft/world/level/redstone/RedstoneWireBlock",
        method_name: "calculateTargetStrength",
        descriptor: "(Lnet/minecraft/world/level/LevelReader;Lnet/minecraft/core/BlockPos;)I",
        native_symbol: "rsift_native_redstone_calculate",
    },
    ComputeRedirect {
        class_name: "net/minecraft/world/level/chunk/LevelChunk",
        method_name: "tick",
        descriptor: "()V",
        native_symbol: "rsift_native_chunk_tick",
    },
    ComputeRedirect {
        class_name: "net/minecraft/world/level/block/entity/HopperBlockEntity",
        method_name: "tick",
        descriptor: "()V",
        native_symbol: "rsift_native_server_tick",
    },
    ComputeRedirect {
        class_name: "net/minecraft/server/level/ServerLevel",
        method_name: "tick",
        descriptor: "(Ljava/util/function/BooleanSupplier;)V",
        native_symbol: "rsift_native_server_tick",
    },
    ComputeRedirect {
        class_name: "net/minecraft/world/level/material/FlowingFluid",
        method_name: "tick",
        descriptor: "(Lnet/minecraft/world/level/Level;Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/level/material/FluidState;)V",
        native_symbol: "rsift_native_server_tick",
    },
    ComputeRedirect {
        class_name: "net/minecraft/network/Connection",
        method_name: "channelRead0",
        descriptor: "(Lio/netty/channel/ChannelHandlerContext;Ljava/lang/Object;)V",
        native_symbol: "rsift_native_on_packet",
    },
];

pub fn is_compute_class(class_name: &str) -> bool {
    COMPUTE_REDIRECTS.iter().any(|r| r.class_name == class_name)
}

pub fn redirects_for_class(class_name: &str) -> impl Iterator<Item = &ComputeRedirect> {
    COMPUTE_REDIRECTS.iter().filter(move |r| r.class_name == class_name)
}
