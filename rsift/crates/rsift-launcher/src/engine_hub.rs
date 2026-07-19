//! Engine hub — selects DLL or builtin RsGraphics/RsCalc

use crate::builtin_engines::{BuiltinRsCalc, BuiltinRsGraphics, DllComputeDelegate, DllGraphicsDelegate};
use crate::mod_loader::NativeModLoader;
use rsift_api::migration_hub::MigrationRouter;
use std::sync::Arc;

pub struct EngineHub {
    pub router: MigrationRouter,
    pub has_rsgraphics_dll: bool,
    pub has_rscalc_dll: bool,
}

impl EngineHub {
    pub fn from_mod_loader(mod_loader: Arc<NativeModLoader>) -> Self {
        let has_rsgraphics = mod_loader.has_mod("rsgraphics");
        let has_rscalc = mod_loader.has_mod("rscalc");

        let graphics: Box<dyn rsift_api::migration_hub::GraphicsMigrationTarget> = if has_rsgraphics {
            Box::new(DllGraphicsDelegate::new(mod_loader.clone()))
        } else {
            Box::new(BuiltinRsGraphics::new())
        };

        let compute: Box<dyn rsift_api::migration_hub::ComputeMigrationTarget> = if has_rscalc {
            Box::new(DllComputeDelegate::new(mod_loader))
        } else {
            Box::new(BuiltinRsCalc::new())
        };

        Self {
            router: MigrationRouter::new(graphics, compute, true),
            has_rsgraphics_dll: has_rsgraphics,
            has_rscalc_dll: has_rscalc,
        }
    }

    /// Fallback when no mods at all
    pub fn standalone() -> Self {
        Self {
            router: MigrationRouter::new(
                Box::new(BuiltinRsGraphics::new()),
                Box::new(BuiltinRsCalc::new()),
                true,
            ),
            has_rsgraphics_dll: false,
            has_rscalc_dll: false,
        }
    }
}
