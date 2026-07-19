//! # 24-Task Dynamic Sync/Async Policy Governor (`SyncAsyncPolicyScheduler`)
//!
//! RsCalc の処理領域を細粒度な 24 種類 (`ComputeTaskKind`) に分類し、
//! 各タスクの「スレッド同期必須性 (`ExecutionClass::StrictSync`)」または
//! 「非同期安全 (`ExecutionClass::AsyncSafe`)」の定義および負荷閾値 ($N \ge T$) に基づき、
//! メインゲームスレッドで同期実行 (`Sync`) するか、Rayon スレッドプールで
//! 非同期オーバーラップ実行 (`Async`) するかを毎フレーム動的に割り振る世界最高峰のタスクガバナー。

use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComputeTaskKind {
    // ---- Category A: Strictly Synchronous (Must finish before frame boundary / barrier) ----
    PlayerPositionTransform,
    LocalBoundingBoxUpdate,
    ActiveHotbarItemUse,
    ScreenGuiInputState,
    DirectBufferJniSync,

    // ---- Category B: Load-Adaptive (Sync for small N, Async Rayon for N >= Threshold) ----
    SweptAabbCollision,
    SemiImplicitPhysicsStep,
    BoidsFlockingAi,
    RedstoneBitboardPropagation,
    CellularHydrodynamics,
    BlockEntityTicking,
    ChunkRandomTicking,
    HopperItemTransfer,

    // ---- Category C: Strictly Asynchronous (Background worker safe, frame overlap) ----
    HpaClusterGraphRouting,
    ZstdParallelRegionCompression,
    SsaBasicBlockTranspilation,
    DdaOcclusionRaycasting,
    VirtualTextureLruEviction,
    MeshletConeCullingBuild,
    AmbientOcclusionBaking,
    SoundHrtfSpatialCalculation,
    AdvancementConditionVerification,
    WorldMirrorGarbagePurge,
    AnalyticsAndTelemetryHarvesting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionClass {
    StrictSync,
    Adaptive { async_threshold: usize },
    StrictAsync,
}

impl ComputeTaskKind {
    pub fn class(&self) -> ExecutionClass {
        match self {
            Self::PlayerPositionTransform
            | Self::LocalBoundingBoxUpdate
            | Self::ActiveHotbarItemUse
            | Self::ScreenGuiInputState
            | Self::DirectBufferJniSync => ExecutionClass::StrictSync,

            Self::SweptAabbCollision => ExecutionClass::Adaptive { async_threshold: 64 },
            Self::SemiImplicitPhysicsStep => ExecutionClass::Adaptive { async_threshold: 64 },
            Self::BoidsFlockingAi => ExecutionClass::Adaptive { async_threshold: 128 },
            Self::RedstoneBitboardPropagation => ExecutionClass::Adaptive { async_threshold: 64 },
            Self::CellularHydrodynamics => ExecutionClass::Adaptive { async_threshold: 512 },
            Self::BlockEntityTicking => ExecutionClass::Adaptive { async_threshold: 64 },
            Self::ChunkRandomTicking => ExecutionClass::Adaptive { async_threshold: 16 },
            Self::HopperItemTransfer => ExecutionClass::Adaptive { async_threshold: 64 },

            Self::HpaClusterGraphRouting
            | Self::ZstdParallelRegionCompression
            | Self::SsaBasicBlockTranspilation
            | Self::DdaOcclusionRaycasting
            | Self::VirtualTextureLruEviction
            | Self::MeshletConeCullingBuild
            | Self::AmbientOcclusionBaking
            | Self::SoundHrtfSpatialCalculation
            | Self::AdvancementConditionVerification
            | Self::WorldMirrorGarbagePurge
            | Self::AnalyticsAndTelemetryHarvesting => ExecutionClass::StrictAsync,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::PlayerPositionTransform => "PlayerPositionTransform",
            Self::LocalBoundingBoxUpdate => "LocalBoundingBoxUpdate",
            Self::ActiveHotbarItemUse => "ActiveHotbarItemUse",
            Self::ScreenGuiInputState => "ScreenGuiInputState",
            Self::DirectBufferJniSync => "DirectBufferJniSync",
            Self::SweptAabbCollision => "SweptAabbCollision",
            Self::SemiImplicitPhysicsStep => "SemiImplicitPhysicsStep",
            Self::BoidsFlockingAi => "BoidsFlockingAi",
            Self::RedstoneBitboardPropagation => "RedstoneBitboardPropagation",
            Self::CellularHydrodynamics => "CellularHydrodynamics",
            Self::BlockEntityTicking => "BlockEntityTicking",
            Self::ChunkRandomTicking => "ChunkRandomTicking",
            Self::HopperItemTransfer => "HopperItemTransfer",
            Self::HpaClusterGraphRouting => "HpaClusterGraphRouting",
            Self::ZstdParallelRegionCompression => "ZstdParallelRegionCompression",
            Self::SsaBasicBlockTranspilation => "SsaBasicBlockTranspilation",
            Self::DdaOcclusionRaycasting => "DdaOcclusionRaycasting",
            Self::VirtualTextureLruEviction => "VirtualTextureLruEviction",
            Self::MeshletConeCullingBuild => "MeshletConeCullingBuild",
            Self::AmbientOcclusionBaking => "AmbientOcclusionBaking",
            Self::SoundHrtfSpatialCalculation => "SoundHrtfSpatialCalculation",
            Self::AdvancementConditionVerification => "AdvancementConditionVerification",
            Self::WorldMirrorGarbagePurge => "WorldMirrorGarbagePurge",
            Self::AnalyticsAndTelemetryHarvesting => "AnalyticsAndTelemetryHarvesting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignedExecutionMode {
    ExecuteSyncOnMainThread,
    DispatchAsyncToRayonPool,
}

pub struct SyncAsyncPolicyScheduler {
    pub sync_dispatches: AtomicUsize,
    pub async_dispatches: AtomicUsize,
}

impl Default for SyncAsyncPolicyScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncAsyncPolicyScheduler {
    pub fn new() -> Self {
        Self {
            sync_dispatches: AtomicUsize::new(0),
            async_dispatches: AtomicUsize::new(0),
        }
    }

    /// Dynamically assign task execution mode based on item count (`workload_size`) and multi-core availability (`threads > 1`).
    pub fn assign_mode(
        &self,
        task: ComputeTaskKind,
        workload_size: usize,
        rayon_threads: usize,
    ) -> AssignedExecutionMode {
        let mode = match task.class() {
            ExecutionClass::StrictSync => AssignedExecutionMode::ExecuteSyncOnMainThread,
            ExecutionClass::StrictAsync => {
                if rayon_threads > 1 {
                    AssignedExecutionMode::DispatchAsyncToRayonPool
                } else {
                    AssignedExecutionMode::ExecuteSyncOnMainThread
                }
            }
            ExecutionClass::Adaptive { async_threshold } => {
                if rayon_threads > 1 && workload_size >= async_threshold {
                    AssignedExecutionMode::DispatchAsyncToRayonPool
                } else {
                    AssignedExecutionMode::ExecuteSyncOnMainThread
                }
            }
        };

        match mode {
            AssignedExecutionMode::ExecuteSyncOnMainThread => {
                self.sync_dispatches.fetch_add(1, Ordering::Relaxed);
            }
            AssignedExecutionMode::DispatchAsyncToRayonPool => {
                self.async_dispatches.fetch_add(1, Ordering::Relaxed);
            }
        }

        mode
    }

    pub fn log_stats(&self) {
        debug!(
            "[SyncAsyncPolicyScheduler] stats: sync_dispatches={} async_dispatches={}",
            self.sync_dispatches.load(Ordering::Relaxed),
            self.async_dispatches.load(Ordering::Relaxed)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_strict_and_adaptive() {
        let scheduler = SyncAsyncPolicyScheduler::new();
        // Strict sync always returns main thread
        assert_eq!(
            scheduler.assign_mode(ComputeTaskKind::PlayerPositionTransform, 10000, 8),
            AssignedExecutionMode::ExecuteSyncOnMainThread
        );
        // Strict async returns Rayon if threads > 1
        assert_eq!(
            scheduler.assign_mode(ComputeTaskKind::HpaClusterGraphRouting, 1, 8),
            AssignedExecutionMode::DispatchAsyncToRayonPool
        );
        // Adaptive returns Sync when N < threshold (64)
        assert_eq!(
            scheduler.assign_mode(ComputeTaskKind::SweptAabbCollision, 30, 8),
            AssignedExecutionMode::ExecuteSyncOnMainThread
        );
        // Adaptive returns Async when N >= threshold (64)
        assert_eq!(
            scheduler.assign_mode(ComputeTaskKind::SweptAabbCollision, 250, 8),
            AssignedExecutionMode::DispatchAsyncToRayonPool
        );
    }
}
