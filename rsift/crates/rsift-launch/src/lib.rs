//! Local offline launch — resolves classpath from installed `.minecraft` (no GitHub).

mod classpath;
mod java;
mod offline;
mod subprocess;
mod version_json;

pub use classpath::{build_classpath, resolve_natives_dir};
pub use java::{detect_java_home, ensure_java_home};
pub use offline::{launch_offline, list_rsift_versions, offline_uuid, LaunchProfile, OfflineLaunchConfig};
pub use subprocess::{list_launchable_versions, prepare_vanilla_launch, LaunchPlan};
pub use version_json::{load_merged_version, MergedVersion};
