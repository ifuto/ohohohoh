//! # NeoForge CoreMod & TransformationService Parity
//!
//! Rules are collected into a global queue consumed by `rsift_parser::BytecodePatcher`
//! during JVMTI ClassFileLoadHook.

use std::sync::{Mutex, OnceLock};
use tracing::{info, debug};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreModInjectionPoint {
    Head,
    Return,
    Invoke(String),
}

#[derive(Debug, Clone)]
pub enum CoreModAction {
    Inject { point: CoreModInjectionPoint, dll_symbol: String },
    Redirect { point: CoreModInjectionPoint, dll_symbol: String },
    Overwrite { dll_symbol: String },
}

#[derive(Debug, Clone)]
pub struct CoreModRule {
    pub target_class: String,
    pub target_method: String,
    pub target_descriptor: String,
    pub action: CoreModAction,
}

pub trait TransformationService: Send + Sync {
    fn get_name(&self) -> &str;
    fn initialize(&self, env: &crate::lifecycle::ModLoaderEnvironment) -> Result<(), String>;
    fn transformers(&self) -> Vec<CoreModRule>;
}

static GLOBAL_RULES: OnceLock<Mutex<Vec<CoreModRule>>> = OnceLock::new();

fn global_rules() -> &'static Mutex<Vec<CoreModRule>> {
    GLOBAL_RULES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn all_coremod_rules() -> Vec<CoreModRule> {
    global_rules().lock().unwrap().clone()
}

pub fn push_coremod_rule(rule: CoreModRule) {
    debug!(
        "[CoreMod] rule {}::{}{}",
        rule.target_class, rule.target_method, rule.target_descriptor
    );
    global_rules().lock().unwrap().push(rule);
    crate::platform::mark_dirty();
}

#[derive(Default)]
pub struct CoreModManager {
    pub services: Vec<Box<dyn TransformationService>>,
    pub rules: Vec<CoreModRule>,
}

impl CoreModManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_service(&mut self, service: Box<dyn TransformationService>) {
        info!(
            "Registering NeoForge TransformationService: [{}]",
            service.get_name()
        );
        for rule in service.transformers() {
            self.rules.push(rule.clone());
            push_coremod_rule(rule);
        }
        self.services.push(service);
    }

    pub fn add_rule(&mut self, rule: CoreModRule) {
        self.rules.push(rule.clone());
        push_coremod_rule(rule);
    }
}
