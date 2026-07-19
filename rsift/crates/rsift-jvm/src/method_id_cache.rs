
//! MethodID/ClassRef Global Cache via OnceLock - FindClassを毎回呼ばない

use std::sync::OnceLock;
use std::collections::HashMap;

pub struct CachedIds {
    pub class_cache: HashMap<String, u64>,
    pub method_cache: HashMap<String, u64>,
    pub field_cache: HashMap<String, u64>,
}

static CACHE: OnceLock<std::sync::Mutex<CachedIds>> = OnceLock::new();

fn get_cache() -> &'static std::sync::Mutex<CachedIds> {
    CACHE.get_or_init(|| std::sync::Mutex::new(CachedIds {
        class_cache: HashMap::new(),
        method_cache: HashMap::new(),
        field_cache: HashMap::new(),
    }))
}

pub fn cache_class(name: &str, id: u64) {
    get_cache().lock().unwrap().class_cache.insert(name.to_string(), id);
}

pub fn get_class_id(name: &str) -> Option<u64> {
    get_cache().lock().unwrap().class_cache.get(name).copied()
}

pub fn cache_method(sig: &str, id: u64) {
    get_cache().lock().unwrap().method_cache.insert(sig.to_string(), id);
}

pub fn get_method_id(sig: &str) -> Option<u64> {
    get_cache().lock().unwrap().method_cache.get(sig).copied()
}
