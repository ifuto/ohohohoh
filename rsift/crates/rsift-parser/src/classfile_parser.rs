
//! Rust製ClassFileパーサ / ライタ - ASMをJavaで使わずRustで直接書き換え
//! JVMTI ClassFileLoadHookより前にクラスを変換

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ClassFile {
    pub magic: u32,
    pub minor: u16,
    pub major: u16,
    pub constant_pool: Vec<Constant>,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone)]
pub enum Constant {
    Utf8(String),
    Class(u16),
    MethodRef(u16,u16),
    FieldRef(u16,u16),
    String(u16),
}

#[derive(Debug, Clone)]
pub struct Method {
    pub access: u16,
    pub name_index: u16,
    pub desc_index: u16,
    pub code: Vec<u8>,
}

impl ClassFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 10 { return Err("too small".into()); }
        let magic = u32::from_be_bytes([bytes[0],bytes[1],bytes[2],bytes[3]]);
        if magic != 0xCAFEBABE { return Err("bad magic".into()); }
        Ok(Self {
            magic,
            minor: u16::from_be_bytes([bytes[4],bytes[5]]),
            major: u16::from_be_bytes([bytes[6],bytes[7]]),
            constant_pool: Vec::new(),
            methods: Vec::new(),
        })
    }

    pub fn inject_invokestatic(&mut self, target_class: &str, target_method: &str) {
        // 実際には定数プールにMethodRefを追加し、code先頭にinvokestatic(0xB8)を注入
        // ここでは簡易ログ
        let _ = (target_class, target_method);
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.magic.to_be_bytes());
        out.extend_from_slice(&self.minor.to_be_bytes());
        out.extend_from_slice(&self.major.to_be_bytes());
        out
    }
}

pub struct ParserEngine {
    cache: HashMap<String, ClassFile>,
}

impl ParserEngine {
    pub fn new() -> Self { Self { cache: HashMap::new() } }
    pub fn transform(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut cf = ClassFile::parse(bytes)?;
        if name.contains("LevelRenderer") {
            cf.inject_invokestatic("com/rsift/RsiftRenderHooks", "onRender");
        }
        self.cache.insert(name.to_string(), cf.clone());
        Ok(cf.to_bytes())
    }
}
