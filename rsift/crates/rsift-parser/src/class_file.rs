//! # Ultra-Fast ClassFile Parser
//!
//! Javaクラスファイルをゼロコピー（または最小限のメモリ割り当て）でパースします。
//! 定数プールとメソッドのバイトコードテーブルを高速にインデックス化します。

use crate::simd_scan::SimdScanner;
use thiserror::Error;
use tracing::{debug, trace};

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Invalid magic number: expected 0xCAFEBABE")]
    InvalidMagic,
    #[error("Unexpected EOF during class parsing at offset {0}")]
    UnexpectedEof(usize),
    #[error("Unsupported class file version: {0}")]
    UnsupportedVersion(u16),
}

/// 定数プールのタグ定数
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConstantTag {
    Utf8 = 1,
    Integer = 3,
    Float = 4,
    Long = 5,
    Double = 6,
    Class = 7,
    String = 8,
    Fieldref = 9,
    Methodref = 10,
    InterfaceMethodref = 11,
    NameAndType = 12,
    MethodHandle = 15,
    MethodType = 16,
    Dynamic = 17,
    InvokeDynamic = 18,
    Module = 19,
    Package = 20,
    Unknown = 255,
}

impl From<u8> for ConstantTag {
    fn from(val: u8) -> Self {
        match val {
            1 => Self::Utf8,
            3 => Self::Integer,
            4 => Self::Float,
            5 => Self::Long,
            6 => Self::Double,
            7 => Self::Class,
            8 => Self::String,
            9 => Self::Fieldref,
            10 => Self::Methodref,
            11 => Self::InterfaceMethodref,
            12 => Self::NameAndType,
            15 => Self::MethodHandle,
            16 => Self::MethodType,
            17 => Self::Dynamic,
            18 => Self::InvokeDynamic,
            19 => Self::Module,
            20 => Self::Package,
            _ => Self::Unknown,
        }
    }
}

/// メソッド情報の軽量インデックス
#[derive(Debug, Clone)]
pub struct MethodIndex {
    pub name_index: u16,
    pub descriptor_index: u16,
    pub access_flags: u16,
    pub name: String,
    pub descriptor: String,
    /// Code属性内のバイトコードのデータ内オフセットと長さ
    pub code_offset: Option<(usize, usize)>,
}

/// クラスファイルの解析結果ビュー
pub struct ClassFileView<'a> {
    pub raw_data: &'a [u8],
    pub minor_version: u16,
    pub major_version: u16,
    pub constant_pool_count: u16,
    pub this_class_name: String,
    pub super_class_name: String,
    pub methods: Vec<MethodIndex>,
    /// 定数プール内の文字列 (index -> string)
    pub utf8_constants: std::collections::HashMap<u16, String>,
}

impl<'a> ClassFileView<'a> {
    /// クラスファイルをパースしてビューを構築する
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError> {
        if !SimdScanner::verify_magic(data) {
            return Err(ParseError::InvalidMagic);
        }

        let mut offset = 4;
        let read_u16 = |off: &mut usize| -> Result<u16, ParseError> {
            if *off + 2 > data.len() {
                return Err(ParseError::UnexpectedEof(*off));
            }
            let val = u16::from_be_bytes([data[*off], data[*off + 1]]);
            *off += 2;
            Ok(val)
        };

        let read_u32 = |off: &mut usize| -> Result<u32, ParseError> {
            if *off + 4 > data.len() {
                return Err(ParseError::UnexpectedEof(*off));
            }
            let val = u32::from_be_bytes([data[*off], data[*off + 1], data[*off + 2], data[*off + 3]]);
            *off += 4;
            Ok(val)
        };

        let minor_version = read_u16(&mut offset)?;
        let major_version = read_u16(&mut offset)?;
        let cp_count = read_u16(&mut offset)?;

        let mut utf8_constants = std::collections::HashMap::new();
        let mut class_indices = std::collections::HashMap::new();

        let mut idx = 1;
        while idx < cp_count {
            if offset >= data.len() {
                return Err(ParseError::UnexpectedEof(offset));
            }
            let tag = ConstantTag::from(data[offset]);
            offset += 1;

            match tag {
                ConstantTag::Utf8 => {
                    let len = read_u16(&mut offset)? as usize;
                    if offset + len > data.len() {
                        return Err(ParseError::UnexpectedEof(offset));
                    }
                    if let Ok(s) = std::str::from_utf8(&data[offset..offset + len]) {
                        utf8_constants.insert(idx, s.to_string());
                    }
                    offset += len;
                }
                ConstantTag::Class => {
                    let name_idx = read_u16(&mut offset)?;
                    class_indices.insert(idx, name_idx);
                }
                ConstantTag::String | ConstantTag::MethodType | ConstantTag::Module | ConstantTag::Package => {
                    offset += 2;
                }
                ConstantTag::Fieldref | ConstantTag::Methodref | ConstantTag::InterfaceMethodref | ConstantTag::NameAndType | ConstantTag::Dynamic | ConstantTag::InvokeDynamic => {
                    offset += 4;
                }
                ConstantTag::MethodHandle => {
                    offset += 3;
                }
                ConstantTag::Integer | ConstantTag::Float => {
                    offset += 4;
                }
                ConstantTag::Long | ConstantTag::Double => {
                    offset += 8;
                    idx += 1; // Long/Double は定数プールで2つのスロットを占有する仕様
                }
                ConstantTag::Unknown => {
                    trace!("Encountered unknown constant tag at index {}, offset {}", idx, offset - 1);
                    break;
                }
            }
            idx += 1;
        }

        let access_flags = read_u16(&mut offset)?;
        let this_class_idx = read_u16(&mut offset)?;
        let super_class_idx = read_u16(&mut offset)?;

        let get_class_name = |c_idx: u16| -> String {
            if let Some(&name_idx) = class_indices.get(&c_idx) {
                if let Some(name) = utf8_constants.get(&name_idx) {
                    return name.clone();
                }
            }
            format!("UnknownClass#{}", c_idx)
        };

        let this_class_name = get_class_name(this_class_idx);
        let super_class_name = get_class_name(super_class_idx);

        // インターフェースの読み飛ばし
        let interfaces_count = read_u16(&mut offset)?;
        offset += (interfaces_count as usize) * 2;

        // フィールドの読み飛ばし
        let fields_count = read_u16(&mut offset)?;
        for _ in 0..fields_count {
            offset += 6; // access_flags + name_index + descriptor_index
            let attr_count = read_u16(&mut offset)?;
            for _ in 0..attr_count {
                offset += 2; // attr_name_idx
                let attr_len = read_u32(&mut offset)? as usize;
                offset += attr_len;
            }
        }

        // メソッドの解析
        let methods_count = read_u16(&mut offset)?;
        let mut methods = Vec::with_capacity(methods_count as usize);

        for _ in 0..methods_count {
            let access_flags = read_u16(&mut offset)?;
            let name_index = read_u16(&mut offset)?;
            let descriptor_index = read_u16(&mut offset)?;
            let attr_count = read_u16(&mut offset)?;

            let name = utf8_constants.get(&name_index).cloned().unwrap_or_default();
            let descriptor = utf8_constants.get(&descriptor_index).cloned().unwrap_or_default();
            let mut code_offset = None;

            for _ in 0..attr_count {
                let attr_name_idx = read_u16(&mut offset)?;
                let attr_len = read_u32(&mut offset)? as usize;

                let attr_name = utf8_constants.get(&attr_name_idx).map(|s| s.as_str()).unwrap_or("");
                if attr_name == "Code" {
                    // Code属性: max_stack(2) + max_locals(2) + code_length(4) + code[code_length]
                    if offset + 8 <= data.len() {
                        let code_len = u32::from_be_bytes([data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]]) as usize;
                        let code_start = offset + 8;
                        code_offset = Some((code_start, code_len));
                    }
                }
                offset += attr_len;
            }

            methods.push(MethodIndex {
                name_index,
                descriptor_index,
                access_flags,
                name,
                descriptor,
                code_offset,
            });
        }

        Ok(Self {
            raw_data: data,
            minor_version,
            major_version,
            constant_pool_count: cp_count,
            this_class_name,
            super_class_name,
            methods,
            utf8_constants,
        })
    }
}
