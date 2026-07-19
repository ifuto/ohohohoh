//! Native Mixin engine — applies real HEAD `invokestatic` injections via ClassRewriter.

use crate::class_file::ClassFileView;
use crate::class_rewriter::{hook_inject_for_redirect, ClassRewriter};
use crate::simd_scan::SimdScanner;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectionPoint {
    Head,
    Return,
    Invoke { target_method: String, shift: InjectionShift },
    Field { target_field: String, opcode: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionShift {
    Before,
    After,
}

#[derive(Debug, Clone)]
pub enum MixinAction {
    Inject {
        point: InjectionPoint,
        dll_symbol: String,
        cancellable: bool,
    },
    Redirect {
        point: InjectionPoint,
        dll_symbol: String,
    },
    Overwrite {
        dll_symbol: String,
    },
    ModifyVariable {
        local_index: u16,
        dll_symbol: String,
    },
    Accessor {
        field_name: String,
        is_getter: bool,
    },
    Invoker {
        method_name: String,
    },
}

#[derive(Debug, Clone)]
pub struct MixinRule {
    pub target_class: String,
    pub target_method: String,
    pub target_descriptor: String,
    pub action: MixinAction,
}

#[derive(Default)]
pub struct MixinInjector {
    pub rules: Vec<MixinRule>,
}

impl MixinInjector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_rule(&mut self, rule: MixinRule) {
        info!(
            "Registering Native Mixin Rule: {}#{} -> {:?}",
            rule.target_class, rule.target_method, rule.action
        );
        self.rules.push(rule);
    }

    pub fn apply_mixins(&self, class_name: &str, raw_data: &[u8]) -> Option<Vec<u8>> {
        let normalized = class_name.replace('.', "/");
        let relevant: Vec<&MixinRule> = self
            .rules
            .iter()
            .filter(|r| {
                let tc = r.target_class.replace('.', "/");
                tc == normalized || tc == class_name
            })
            .collect();

        if relevant.is_empty() {
            return None;
        }

        if !SimdScanner::contains_pattern(raw_data, normalized.as_bytes())
            && !SimdScanner::contains_pattern(raw_data, class_name.as_bytes())
        {
            // Still try — CP may use different encoding
        }

        let view = ClassFileView::parse(raw_data).ok()?;
        let mut injects = Vec::new();

        for rule in relevant {
            for m in &view.methods {
                if m.name != rule.target_method {
                    continue;
                }
                if !rule.target_descriptor.is_empty() && m.descriptor != rule.target_descriptor {
                    continue;
                }
                let hook_name = match &rule.action {
                    MixinAction::Inject { dll_symbol, point, .. } => {
                        if !matches!(point, InjectionPoint::Head) {
                            debug!("Non-HEAD inject not yet supported for {}", dll_symbol);
                        }
                        sanitize_hook(dll_symbol)
                    }
                    MixinAction::Redirect { dll_symbol, .. } | MixinAction::Overwrite { dll_symbol } => {
                        sanitize_hook(dll_symbol)
                    }
                    MixinAction::ModifyVariable { dll_symbol, .. }
                    | MixinAction::Accessor { field_name: dll_symbol, .. }
                    | MixinAction::Invoker {
                        method_name: dll_symbol,
                    } => sanitize_hook(dll_symbol),
                };
                injects.push(hook_inject_for_redirect(
                    &rule.target_method,
                    &rule.target_descriptor,
                    &hook_name,
                ));
            }
        }

        if injects.is_empty() {
            return None;
        }

        let mut rewriter = ClassRewriter::from_bytes(raw_data).ok()?;
        let n = rewriter.inject_head_calls(&injects).ok()?;
        if n == 0 {
            return None;
        }
        let out = rewriter.into_bytes();
        if out.as_slice() == raw_data {
            None
        } else {
            Some(out)
        }
    }
}

fn sanitize_hook(symbol: &str) -> String {
    // Map DLL symbols to RsiftHooks method names when possible.
    if symbol.starts_with("rsift_") || symbol.starts_with("on") {
        let s = symbol.trim_start_matches("rsift_native_");
        let mut name = String::from("on");
        let mut cap = true;
        for ch in s.chars() {
            if ch == '_' {
                cap = true;
            } else if cap {
                name.push(ch.to_ascii_uppercase());
                cap = false;
            } else {
                name.push(ch);
            }
        }
        if name == "on" {
            "onGenericCompute".into()
        } else {
            name
        }
    } else {
        "onGenericCompute".into()
    }
}
