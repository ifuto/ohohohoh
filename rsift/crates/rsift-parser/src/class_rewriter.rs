//! Real Java class-file rewriter — constant-pool append + method HEAD inject.
//!
//! Injects `invokestatic` to `com/rsift/RsiftHooks` static methods so compute /
//! network / lifecycle hooks run without leaving the class file untouched.

use crate::class_file::{ClassFileView, ConstantTag, ParseError};
use std::collections::HashMap;

const ACC_NATIVE: u16 = 0x0100;
const ACC_ABSTRACT: u16 = 0x0400;

#[derive(Debug)]
pub enum RewriteError {
    Parse(ParseError),
    Truncated,
    NoCode,
    Unsupported,
    /// 検証できない形に対して「嘘の注入」をしないための保守的拒否 (理由付き)。
    Refused(&'static str),
}

impl From<ParseError> for RewriteError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

/// Spec for injecting a static call at method entry.
#[derive(Debug, Clone)]
pub struct HeadInject {
    pub method_name: String,
    pub method_descriptor: String,
    /// Internal name, e.g. `com/rsift/RsiftHooks`
    pub hook_class: String,
    /// e.g. `onMobAiStep`
    pub hook_method: String,
    /// Must be `()V` for void prepend (no args/return on stack at HEAD for instance methods
    /// before aload_0 — we inject BEFORE any code, so only `()V` static hooks are safe universally).
    pub hook_descriptor: String,
}

pub struct ClassRewriter {
    data: Vec<u8>,
    utf8: HashMap<String, u16>,
    next_cp_index: u16,
    cp_count_offset: usize,
}

impl ClassRewriter {
    pub fn from_bytes(raw: &[u8]) -> Result<Self, RewriteError> {
        let view = ClassFileView::parse(raw)?;
        let mut utf8 = HashMap::new();
        for (idx, s) in &view.utf8_constants {
            utf8.insert(s.clone(), *idx);
        }
        Ok(Self {
            data: raw.to_vec(),
            utf8,
            next_cp_index: view.constant_pool_count,
            cp_count_offset: 8, // magic(4)+minor(2)+major(2)
        })
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    fn write_u16_at(&mut self, off: usize, v: u16) {
        self.data[off] = (v >> 8) as u8;
        self.data[off + 1] = (v & 0xff) as u8;
    }

    fn read_u16_at(&self, off: usize) -> u16 {
        u16::from_be_bytes([self.data[off], self.data[off + 1]])
    }

    /// End offset of the constant pool (byte index of access_flags).
    fn cp_end(&self) -> Result<usize, RewriteError> {
        let mut offset = 10; // after cp_count
        let cp_count = self.read_u16_at(self.cp_count_offset);
        let mut idx = 1u16;
        while idx < cp_count {
            if offset >= self.data.len() {
                return Err(RewriteError::Truncated);
            }
            let tag = ConstantTag::from(self.data[offset]);
            offset += 1;
            match tag {
                ConstantTag::Utf8 => {
                    let len = self.read_u16_at(offset) as usize;
                    offset += 2 + len;
                }
                ConstantTag::Class
                | ConstantTag::String
                | ConstantTag::MethodType
                | ConstantTag::Module
                | ConstantTag::Package => offset += 2,
                ConstantTag::MethodHandle => offset += 3,
                ConstantTag::Integer
                | ConstantTag::Float
                | ConstantTag::Fieldref
                | ConstantTag::Methodref
                | ConstantTag::InterfaceMethodref
                | ConstantTag::NameAndType
                | ConstantTag::Dynamic
                | ConstantTag::InvokeDynamic => {
                    offset += 4;
                }
                ConstantTag::Long | ConstantTag::Double => {
                    offset += 8;
                    idx += 1; // takes two slots
                }
                ConstantTag::Unknown => return Err(RewriteError::Unsupported),
            }
            idx += 1;
        }
        Ok(offset)
    }

    fn ensure_utf8(&mut self, s: &str) -> Result<u16, RewriteError> {
        if let Some(&idx) = self.utf8.get(s) {
            return Ok(idx);
        }
        let insert_at = self.cp_end()?;
        let bytes = s.as_bytes();
        let mut entry = Vec::with_capacity(3 + bytes.len());
        entry.push(1u8); // Utf8
        entry.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        entry.extend_from_slice(bytes);
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        self.utf8.insert(s.to_string(), idx);
        Ok(idx)
    }

    fn append_class(&mut self, name: &str) -> Result<u16, RewriteError> {
        let name_idx = self.ensure_utf8(name)?;
        let insert_at = self.cp_end()?;
        let mut entry = vec![7u8]; // Class
        entry.extend_from_slice(&name_idx.to_be_bytes());
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        Ok(idx)
    }

    fn append_name_and_type(&mut self, name: &str, desc: &str) -> Result<u16, RewriteError> {
        let n = self.ensure_utf8(name)?;
        let d = self.ensure_utf8(desc)?;
        let insert_at = self.cp_end()?;
        let mut entry = vec![12u8];
        entry.extend_from_slice(&n.to_be_bytes());
        entry.extend_from_slice(&d.to_be_bytes());
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        Ok(idx)
    }

    /// CONSTANT_String (tag 8) を追加。F3 マーカー文字列用 (wave 205)。
    pub(crate) fn append_string(&mut self, s: &str) -> Result<u16, RewriteError> {
        let utf8_idx = self.ensure_utf8(s)?;
        let insert_at = self.cp_end()?;
        let mut entry = vec![8u8]; // String
        entry.extend_from_slice(&utf8_idx.to_be_bytes());
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        Ok(idx)
    }

    /// CONSTANT_InterfaceMethodref (tag 11) を追加。F3 マーカーの List.add 用 (wave 205)。
    pub(crate) fn append_interface_methodref(
        &mut self,
        class_name: &str,
        method: &str,
        desc: &str,
    ) -> Result<u16, RewriteError> {
        let class_idx = self.append_class(class_name)?;
        let nat = self.append_name_and_type(method, desc)?;
        let insert_at = self.cp_end()?;
        let mut entry = vec![11u8]; // InterfaceMethodref
        entry.extend_from_slice(&class_idx.to_be_bytes());
        entry.extend_from_slice(&nat.to_be_bytes());
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        Ok(idx)
    }

    fn append_methodref(
        &mut self,
        class_name: &str,
        method: &str,
        desc: &str,
    ) -> Result<u16, RewriteError> {
        let class_idx = self.append_class(class_name)?;
        let nat = self.append_name_and_type(method, desc)?;
        let insert_at = self.cp_end()?;
        let mut entry = vec![10u8]; // Methodref
        entry.extend_from_slice(&class_idx.to_be_bytes());
        entry.extend_from_slice(&nat.to_be_bytes());
        self.data.splice(insert_at..insert_at, entry);
        let idx = self.next_cp_index;
        self.next_cp_index += 1;
        self.write_u16_at(self.cp_count_offset, self.next_cp_index);
        Ok(idx)
    }

    /// Inject `invokestatic hook` at the start of matching methods' Code attributes.
    pub fn inject_head_calls(&mut self, injects: &[HeadInject]) -> Result<usize, RewriteError> {
        // Re-parse after any prior edits for method locations relative to current buffer.
        // We locate methods by scanning from CP end each time.
        let mut applied = 0usize;
        for inj in injects {
            let methodref =
                self.append_methodref(&inj.hook_class, &inj.hook_method, &inj.hook_descriptor)?;
            // Find method Code and prepend invokestatic
            if self.prepend_invokestatic_to_method(
                &inj.method_name,
                &inj.method_descriptor,
                methodref,
            )? {
                applied += 1;
            }
        }
        Ok(applied)
    }

    fn prepend_invokestatic_to_method(
        &mut self,
        method_name: &str,
        method_desc: &str,
        methodref: u16,
    ) -> Result<bool, RewriteError> {
        let view = ClassFileView::parse(&self.data)?;
        for m in &view.methods {
            if m.name != method_name {
                continue;
            }
            if !method_desc.is_empty() && m.descriptor != method_desc {
                continue;
            }
            if m.access_flags & (ACC_NATIVE | ACC_ABSTRACT) != 0 {
                continue;
            }
            let Some((code_attr_payload_off, _code_len_field)) = m.code_offset else {
                continue;
            };
            // code_offset in MethodIndex is start of bytecode array inside Code attribute.
            // Structure: max_stack u16, max_locals u16, code_length u32, code[], ...
            // So bytecode starts at code_offset; max_stack is at code_offset - 8.
            let code_start = code_attr_payload_off;
            if code_start < 12 || code_start >= self.data.len() {
                return Err(RewriteError::NoCode);
            }
            // attribute_length u32 sits just before max_stack
            let attr_len_off = code_start - 12;
            let max_stack_off = code_start - 8;
            let code_length_off = code_start - 4;
            let old_len = u32::from_be_bytes([
                self.data[code_length_off],
                self.data[code_length_off + 1],
                self.data[code_length_off + 2],
                self.data[code_length_off + 3],
            ]) as usize;

            // Already patched?
            if old_len >= 3
                && self.data[code_start] == 0xb8
                && self.read_u16_at(code_start + 1) == methodref
            {
                return Ok(false);
            }

            // wave 205 欠陥D 根治 (read-only 事前検査、破壊前に拒否):
            // offset 0 への挿入では全命令が一様に +3 シフトするため、相対分岐
            // オフセット (if/goto は opcode 位置基準の相対) は数学的に不変。
            // 実際に壊れるのは次の 2 系統のみ:
            //   (1) tableswitch/lookupswitch — オペランドが「コード先頭からの
            //       4byte アライン」に依存 → +3 で全テーブル解釈が破壊。
            //       再パディングは長さ変化を連鎖させるため v1 保守的に拒否。
            //   (2) 先頭スタックマップフレームの delta+3 が same_frame/
            //       same_locals_1_stack の 1byte 表現 (≤63) を溢れる場合、
            //       昇格 (same_frame_extended 化 = 属性内で長さ変化) しないと
            //       VerifyError。事前拒否する (旧コードは無言スキップで破壊)。
            //   (3) 先頭 append_frame (252-254) が bump 対象外だった欠陥は
            //       bump_first_stack_map_delta 側を修正済。
            {
                let code_slice = &self.data[code_start..code_start + old_len];
                if code_contains_tableswitch(code_slice) {
                    return Err(RewriteError::Refused(
                        "method contains tableswitch/lookupswitch — HEAD inject would corrupt switch padding",
                    ));
                }
                let exc_off = code_start + old_len;
                if exc_off + 2 > self.data.len() {
                    return Err(RewriteError::Truncated);
                }
                let exc_count = self.read_u16_at(exc_off) as usize;
                let attrs_off = exc_off + 2 + exc_count * 8;
                if attrs_off + 2 <= self.data.len() {
                    let attr_count = self.read_u16_at(attrs_off) as usize;
                    let mut p = attrs_off + 2;
                    for _ in 0..attr_count {
                        if p + 6 > self.data.len() {
                            break;
                        }
                        let name_idx = self.read_u16_at(p);
                        let alen = u32::from_be_bytes([
                            self.data[p + 2],
                            self.data[p + 3],
                            self.data[p + 4],
                            self.data[p + 5],
                        ]) as usize;
                        let is_smt = view
                            .utf8_constants
                            .get(&name_idx)
                            .map(|s| s == "StackMapTable")
                            .unwrap_or(false);
                        if is_smt {
                            if let Some(reason) =
                                first_frame_delta_overflow(&self.data, p + 6, alen, 3)
                            {
                                return Err(RewriteError::Refused(reason));
                            }
                        }
                        p += 6 + alen;
                    }
                }
            }

            let inject = [0xb8u8, (methodref >> 8) as u8, (methodref & 0xff) as u8];
            self.data
                .splice(code_start..code_start, inject.iter().copied());

            let new_len = (old_len + 3) as u32;
            self.data[code_length_off] = (new_len >> 24) as u8;
            self.data[code_length_off + 1] = (new_len >> 16) as u8;
            self.data[code_length_off + 2] = (new_len >> 8) as u8;
            self.data[code_length_off + 3] = (new_len & 0xff) as u8;

            let old_attr_len = u32::from_be_bytes([
                self.data[attr_len_off],
                self.data[attr_len_off + 1],
                self.data[attr_len_off + 2],
                self.data[attr_len_off + 3],
            ]);
            let new_attr_len = old_attr_len.saturating_add(3);
            self.data[attr_len_off] = (new_attr_len >> 24) as u8;
            self.data[attr_len_off + 1] = (new_attr_len >> 16) as u8;
            self.data[attr_len_off + 2] = (new_attr_len >> 8) as u8;
            self.data[attr_len_off + 3] = (new_attr_len & 0xff) as u8;

            let max_stack = self.read_u16_at(max_stack_off);
            if max_stack < 4 {
                self.write_u16_at(max_stack_off, max_stack + 1);
            }

            // Exception table
            let exc_off = code_start + new_len as usize;
            let mut attrs_off = exc_off;
            if exc_off + 2 <= self.data.len() {
                let exc_count = self.read_u16_at(exc_off) as usize;
                let mut p = exc_off + 2;
                for _ in 0..exc_count {
                    if p + 8 > self.data.len() {
                        break;
                    }
                    for rel in [0usize, 2, 4] {
                        let v = self.read_u16_at(p + rel).saturating_add(3);
                        self.write_u16_at(p + rel, v);
                    }
                    p += 8;
                }
                attrs_off = p;
            }

            // Code attributes (StackMapTable / LineNumberTable)
            if attrs_off + 2 <= self.data.len() {
                let attr_count = self.read_u16_at(attrs_off) as usize;
                let mut p = attrs_off + 2;
                for _ in 0..attr_count {
                    if p + 6 > self.data.len() {
                        break;
                    }
                    let name_idx = self.read_u16_at(p);
                    let alen = u32::from_be_bytes([
                        self.data[p + 2],
                        self.data[p + 3],
                        self.data[p + 4],
                        self.data[p + 5],
                    ]) as usize;
                    let name = {
                        // Resolve utf8 from current CP
                        ClassFileView::parse(&self.data)
                            .ok()
                            .and_then(|v| v.utf8_constants.get(&name_idx).cloned())
                            .unwrap_or_default()
                    };
                    let data_off = p + 6;
                    if name == "StackMapTable" && data_off + 2 <= self.data.len() {
                        // Bump first frame offset_delta by 3 (same_frame / same_locals_1_stack / full etc.)
                        self.bump_first_stack_map_delta(data_off, alen);
                    } else if name == "LineNumberTable" && data_off + 2 <= self.data.len() {
                        let n = self.read_u16_at(data_off) as usize;
                        let mut lp = data_off + 2;
                        for _ in 0..n {
                            if lp + 4 > self.data.len() {
                                break;
                            }
                            let start = self.read_u16_at(lp).saturating_add(3);
                            self.write_u16_at(lp, start);
                            lp += 4;
                        }
                    }
                    p = data_off + alen;
                }
            }

            return Ok(true);
        }
        Ok(false)
    }

    fn bump_first_stack_map_delta(&mut self, data_off: usize, alen: usize) {
        if alen < 3 || data_off + 3 > self.data.len() {
            return;
        }
        let _num = self.read_u16_at(data_off);
        let frame_off = data_off + 2;
        if frame_off >= self.data.len() {
            return;
        }
        let tag = self.data[frame_off];
        // same_frame: tag 0-63 is offset_delta
        if tag <= 63 {
            let new_tag = (tag as u16).saturating_add(3);
            if new_tag <= 63 {
                self.data[frame_off] = new_tag as u8;
            } else {
                // promote to same_frame_extended (251) + u16 delta
                // Would need to insert bytes — skip; verification may still pass for small methods
            }
            return;
        }
        // same_locals_1_stack_item_frame: 64-127
        if (64..=127).contains(&tag) {
            let delta = tag as u16 - 64;
            let new_delta = delta.saturating_add(3);
            if new_delta <= 63 {
                self.data[frame_off] = (64 + new_delta) as u8;
            }
            return;
        }
        // same_frame_extended (251) / full_frame (255) / append/chop (248-250,252-254):
        // u16 offset after tag。旧コードは append_frame (252-254) を抜かしており
        // 先頭フレームが append の場合に +3 されず VerifyError 相当だった (wave 205 修正)。
        if matches!(tag, 247..=255) && frame_off + 3 <= self.data.len() {
            let d = self.read_u16_at(frame_off + 1).saturating_add(3);
            self.write_u16_at(frame_off + 1, d);
        }
    }

    // --------------------------------------------------------------
    // wave 205: F3 マーカー用 TAIL 注入 (areturn 直前への List.add 追加)
    // --------------------------------------------------------------

    /// `java.lang.String` 要素の List にマーカー文字列を追加する TAIL 注入。
    /// 条件 (v1 保守 — 一つでも不合なら Err(Refused) で「嘘の注入」禁止):
    ///   * 対象メソッドが abstract/native でなく Code を持つ
    ///   * descriptor が `)Ljava/util/List;` 終端
    ///   * 最終命令が areturn (0xB0)
    ///   * exception_table_length == 0 (handler/range の pc 再配置を不要化)
    ///   * StackMapTable 全フレームの絶対オフセットが挿入点 (= old_len-1) 未満
    ///     (挿入点以降へのフレーム存在 = その位置への分岐存在を意味し破壊不可避)
    ///   * code_length + 注入長 ≤ 65535
    /// Stack の頂上は List 参照 (return 値) のみ、max_stack += 2 が厳密十分量。
    pub fn inject_tail_list_marker(&mut self, inj: &TailInject) -> Result<bool, RewriteError> {
        let site = match self.locate_tail_site(inj)? {
            None => return Ok(false),
            Some(s) => s,
        };
        if site.already {
            return Ok(false);
        }
        // constant pool には読み取り検査の「後」に追加する (拒否時の CP 汚染回避)。
        let str_idx = self.append_string(&inj.marker_text)?;
        let list_add_imref =
            self.append_interface_methodref("java/util/List", "add", "(Ljava/lang/Object;)Z")?;
        let component_literal_mref = match inj.element {
            ListElement::String => None,
            ListElement::Component => Some(self.append_methodref(
                "net/minecraft/network/chat/Component",
                "literal",
                "(Ljava/lang/String;)Lnet/minecraft/network/chat/MutableComponent;",
            )?),
        };
        // CP 追加でコード側のオフセットが全シフトするため再配置する。
        let site = self.locate_tail_site(inj)?.ok_or(RewriteError::NoCode)?;
        if site.already {
            return Ok(false);
        }

        let mut payload: Vec<u8> = Vec::with_capacity(tail_inject_len(&inj.element));
        payload.push(0x59); // dup — List を複製
        payload.push(0x13); // ldc_w <marker>
        payload.extend_from_slice(&str_idx.to_be_bytes());
        if let Some(mref) = component_literal_mref {
            payload.push(0xb8); // invokestatic Component.literal(String)
            payload.extend_from_slice(&mref.to_be_bytes());
        }
        payload.push(0xb9); // invokeinterface List.add(Object)
        payload.extend_from_slice(&list_add_imref.to_be_bytes());
        payload.push(2); // count = 2 (objectref + 1 arg)
        payload.push(0);
        payload.push(0x57); // pop — add の boolean 返り値を破棄
        let n = payload.len();
        debug_assert_eq!(n, tail_inject_len(&inj.element));

        let p = site.insert_at;
        self.data.splice(p..p, payload);

        let new_len = (site.old_len + n) as u32;
        self.data[site.code_length_off] = (new_len >> 24) as u8;
        self.data[site.code_length_off + 1] = (new_len >> 16) as u8;
        self.data[site.code_length_off + 2] = (new_len >> 8) as u8;
        self.data[site.code_length_off + 3] = (new_len & 0xff) as u8;

        let old_attr_len = u32::from_be_bytes([
            self.data[site.attr_len_off],
            self.data[site.attr_len_off + 1],
            self.data[site.attr_len_off + 2],
            self.data[site.attr_len_off + 3],
        ]);
        let new_attr_len = old_attr_len.saturating_add(n as u32);
        self.data[site.attr_len_off] = (new_attr_len >> 24) as u8;
        self.data[site.attr_len_off + 1] = (new_attr_len >> 16) as u8;
        self.data[site.attr_len_off + 2] = (new_attr_len >> 8) as u8;
        self.data[site.attr_len_off + 3] = (new_attr_len & 0xff) as u8;

        let max_stack = self.read_u16_at(site.max_stack_off);
        self.write_u16_at(site.max_stack_off, max_stack.saturating_add(2));

        // LineNumberTable (デバッグ情報、検証非対象): 挿入点以降の行の
        // start_pc だけ +n して areturn の行に追随させる。
        // 注意: 上の splice でコードより後方の絶対オフセットは +n シフト済み。
        if let Some((lnt_data_off, lnt_alen)) = site.lnt {
            let lnt_data_off = if lnt_data_off >= p {
                lnt_data_off + n
            } else {
                lnt_data_off
            };
            if lnt_data_off + lnt_alen <= self.data.len() && lnt_alen >= 2 {
                let count = self.read_u16_at(lnt_data_off) as usize;
                let mut lp = lnt_data_off + 2;
                for _ in 0..count {
                    if lp + 4 > lnt_data_off + lnt_alen {
                        break;
                    }
                    let start_pc = self.read_u16_at(lp) as usize;
                    if start_pc >= site.insert_bc_off {
                        self.write_u16_at(lp, (start_pc + n).min(65535) as u16);
                    }
                    lp += 4;
                }
            }
        }
        Ok(true)
    }

    /// 読み取り専用の TAIL 注入可否判定 + 位置特定。
    /// 全ガードをここに集約 (mutate 前に二度評価しても同じ結論になる)。
    fn locate_tail_site(&self, inj: &TailInject) -> Result<Option<TailSite>, RewriteError> {
        let view = ClassFileView::parse(&self.data)?;
        let n = tail_inject_len(&inj.element);
        for m in &view.methods {
            if m.name != inj.method_name {
                continue;
            }
            if !inj.method_descriptor.is_empty() && m.descriptor != inj.method_descriptor {
                continue;
            }
            if m.access_flags & (ACC_NATIVE | ACC_ABSTRACT) != 0 {
                continue;
            }
            if !m.descriptor.ends_with(")Ljava/util/List;") {
                return Err(RewriteError::Refused(
                    "tail marker requires a method returning java.util.List",
                ));
            }
            let Some((code_start, _)) = m.code_offset else {
                continue;
            };
            if code_start < 12 || code_start >= self.data.len() {
                return Err(RewriteError::NoCode);
            }
            let attr_len_off = code_start - 12;
            let max_stack_off = code_start - 8;
            let code_length_off = code_start - 4;
            let old_len = u32::from_be_bytes([
                self.data[code_length_off],
                self.data[code_length_off + 1],
                self.data[code_length_off + 2],
                self.data[code_length_off + 3],
            ]) as usize;
            if old_len < 1 || code_start + old_len + 2 > self.data.len() {
                return Err(RewriteError::Truncated);
            }
            if self.data[code_start + old_len - 1] != 0xb0 {
                return Err(RewriteError::Refused(
                    "last instruction is not areturn — cannot place tail marker safely",
                ));
            }
            if old_len + n > 65535 {
                return Err(RewriteError::Refused("code would exceed 65535-byte limit"));
            }
            let insert_bc_off = old_len - 1; // areturn の位置 (この直前に差し込む)
            if tail_already_marked(&self.data, code_start, insert_bc_off, &inj.element) {
                return Ok(Some(TailSite {
                    attr_len_off,
                    max_stack_off,
                    code_length_off,
                    insert_at: code_start + insert_bc_off,
                    insert_bc_off,
                    old_len,
                    lnt: None,
                    already: true,
                }));
            }
            // exception table 空条件
            let exc_off = code_start + old_len;
            if exc_off + 2 > self.data.len() {
                return Err(RewriteError::Truncated);
            }
            if self.read_u16_at(exc_off) != 0 {
                return Err(RewriteError::Refused(
                    "exception table not empty — refusing tail inject (pc remap required)",
                ));
            }
            // Code サブ属性走査: StackMapTable は全フレーム < 挿入点を要求、
            // LNT は後続調整のため位置だけ保存。
            let attrs_off = exc_off + 2;
            if attrs_off + 2 > self.data.len() {
                return Err(RewriteError::Truncated);
            }
            let attr_count = self.read_u16_at(attrs_off) as usize;
            let mut q = attrs_off + 2;
            let mut lnt: Option<(usize, usize)> = None;
            for _ in 0..attr_count {
                if q + 6 > self.data.len() {
                    return Err(RewriteError::Truncated);
                }
                let name_idx = self.read_u16_at(q);
                let alen = u32::from_be_bytes([
                    self.data[q + 2],
                    self.data[q + 3],
                    self.data[q + 4],
                    self.data[q + 5],
                ]) as usize;
                let data_off = q + 6;
                if data_off + alen > self.data.len() {
                    return Err(RewriteError::Truncated);
                }
                let name = view
                    .utf8_constants
                    .get(&name_idx)
                    .cloned()
                    .unwrap_or_default();
                if name == "StackMapTable" {
                    match stackmap_max_abs_offset(&self.data, data_off, alen)? {
                        Some(max_abs) if max_abs >= insert_bc_off => {
                            return Err(RewriteError::Refused(
                                "stack map frame exists at/after insertion point",
                            ));
                        }
                        _ => {}
                    }
                } else if name == "LineNumberTable" {
                    lnt = Some((data_off, alen));
                }
                q = data_off + alen;
            }
            return Ok(Some(TailSite {
                attr_len_off,
                max_stack_off,
                code_length_off,
                insert_at: code_start + insert_bc_off,
                insert_bc_off,
                old_len,
                lnt,
                already: false,
            }));
        }
        Ok(None)
    }
}

// ------------------------------------------------------------------
// wave 205: 共有ユーティリティ (TAIL 注入 / HEAD 注入ガード)
// ------------------------------------------------------------------

/// List 要素の注入戦略 (F3 行リストの実要素型)。
/// 実行時リフレクション picker が generic return type から判別して決める。
/// String 以外の List<Component> 等に String を足すと消費側キャストで
/// ClassCastException になるため、要素型が不明な場合は注入自体を行わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListElement {
    /// `List<String>` — 文字列を直接 add (10 byte 注入)。
    String,
    /// `List<Component>` — Component.literal(marker) を add (13 byte 注入)。
    Component,
}

/// F3 マーカー用 TAIL 注入仕様。
#[derive(Debug, Clone)]
pub struct TailInject {
    pub method_name: String,
    pub method_descriptor: String,
    pub marker_text: String,
    pub element: ListElement,
}

struct TailSite {
    attr_len_off: usize,
    max_stack_off: usize,
    code_length_off: usize,
    /// バイト列上の挿入位置 (areturn の先頭)。
    insert_at: usize,
    /// バイトコードオフセットとしての挿入位置 (old_len - 1)。
    insert_bc_off: usize,
    old_len: usize,
    lnt: Option<(usize, usize)>,
    already: bool,
}

/// 注入バイト数の正本 (dup + ldc_w + [invokestatic] + invokeinterface(5) + pop)。
pub fn tail_inject_len(element: &ListElement) -> usize {
    match element {
        // dup(1) ldc_w(3) invokeinterface(5) pop(1) = 10
        ListElement::String => 10,
        // dup(1) ldc_w(3) invokestatic(3) invokeinterface(5) pop(1) = 13
        ListElement::Component => 13,
    }
}

/// areturn 直前の n バイトが既に注入パターンなら二重注入抑止 (冪等)。
fn tail_already_marked(
    data: &[u8],
    code_start: usize,
    insert_bc_off: usize,
    element: &ListElement,
) -> bool {
    let n = tail_inject_len(element);
    if insert_bc_off < n {
        return false;
    }
    let base = code_start + insert_bc_off - n;
    let pat: &[u8] = &data[base..base + n];
    let matches_wild = |fixed: &[(usize, u8)]| fixed.iter().all(|(i, b)| pat[*i] == *b);
    match element {
        ListElement::String => matches_wild(&[
            (0, 0x59),
            (1, 0x13),
            (4, 0xb9),
            (7, 0x02),
            (8, 0x00),
            (9, 0x57),
        ]),
        ListElement::Component => matches_wild(&[
            (0, 0x59),
            (1, 0x13),
            (4, 0xb8),
            (7, 0xb9),
            (10, 0x02),
            (11, 0x00),
            (12, 0x57),
        ]),
    }
}

/// 命令長ウォーカーで tableswitch/lookupswitch (0xAA/0xAB) 含有を判定。
/// 単純バイト走査だと ldc オペランド等の 0xAA を誤検するため命令境界を辿る。
/// 解析不能 (途中で範囲外) は保守的に true (= 拒否側) を返す。
pub fn code_contains_tableswitch(code: &[u8]) -> bool {
    let len = code.len();
    let mut pos = 0usize;
    while pos < len {
        let op = code[pos];
        match op {
            0xaa | 0xab => return true,
            0xc4 => {
                // wide: 次が iinc なら 6byte、それ以外 4byte。
                if pos + 1 >= len {
                    return true;
                }
                pos += if code[pos + 1] == 0x84 { 6 } else { 4 };
                continue;
            }
            _ => {}
        }
        pos += match op {
            0x00..=0x0f
            | 0x1a..=0x35
            | 0x3b..=0x83
            | 0x85..=0x98
            | 0xac..=0xb1
            | 0xbe..=0xbf
            | 0xc2..=0xc3
            | 0xca
            | 0xfe..=0xff => 1,
            0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => 2,
            0x11
            | 0x13..=0x14
            | 0x99..=0xa8
            | 0xb2..=0xb8
            | 0xbb
            | 0xbd
            | 0xc0..=0xc1
            | 0xc6..=0xc7 => 3,
            0xc5 => 4,
            0x84 => 3,
            0xb9..=0xba | 0xc8..=0xc9 => 5,
            // 到達しない (上で return / continue 済) が網羅のため
            _ => return true,
        };
    }
    false
}

/// StackMapTable 属性データの全フレーム絶対オフセットの最大値。
/// フレーム 0 件なら None。解析不能なら Err(Unsupported)。
/// 絶対値: 先頭フレーム = offset_delta、以降 = 前回 + offset_delta + 1 (JVMS §4.7.4)。
pub fn stackmap_max_abs_offset(
    data: &[u8],
    data_off: usize,
    alen: usize,
) -> Result<Option<usize>, RewriteError> {
    if data_off + alen > data.len() || alen < 2 {
        return Err(RewriteError::Truncated);
    }
    let end = data_off + alen;
    let read_u16 = |off: usize| -> u16 { u16::from_be_bytes([data[off], data[off + 1]]) };
    let num = read_u16(data_off) as usize;
    let mut off = data_off + 2;
    let mut max_abs: Option<usize> = None;
    let mut prev: usize = 0;
    for i in 0..num {
        if off >= end {
            return Err(RewriteError::Unsupported);
        }
        let tag = data[off];
        off += 1;
        let delta: usize;
        if tag <= 63 {
            delta = tag as usize; // same_frame
        } else if tag <= 127 {
            delta = (tag - 64) as usize; // same_locals_1_stack_item_frame
            off = skip_verification_type_info(data, off, end, 1)?;
        } else if tag == 247 {
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            delta = read_u16(off) as usize;
            off += 2;
            off = skip_verification_type_info(data, off, end, 1)?;
        } else if (248..=250).contains(&tag) {
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            delta = read_u16(off) as usize; // chop_frame
            off += 2;
        } else if tag == 251 {
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            delta = read_u16(off) as usize; // same_frame_extended
            off += 2;
        } else if (252..=254).contains(&tag) {
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            delta = read_u16(off) as usize; // append_frame
            off += 2;
            off = skip_verification_type_info(data, off, end, (tag - 251) as usize)?;
        } else if tag == 255 {
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            delta = read_u16(off) as usize; // full_frame
            off += 2;
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            let nlocals = read_u16(off) as usize;
            off += 2;
            off = skip_verification_type_info(data, off, end, nlocals)?;
            if off + 2 > end {
                return Err(RewriteError::Truncated);
            }
            let nstack = read_u16(off) as usize;
            off += 2;
            off = skip_verification_type_info(data, off, end, nstack)?;
        } else {
            // 128-246 reserved — 妥当なクラスでは出現しない。保守拒否。
            return Err(RewriteError::Unsupported);
        }
        let abs = if i == 0 { delta } else { prev + delta + 1 };
        prev = abs;
        max_abs = Some(max_abs.map(|m: usize| m.max(abs)).unwrap_or(abs));
    }
    Ok(max_abs)
}

fn skip_verification_type_info(
    data: &[u8],
    mut off: usize,
    end: usize,
    count: usize,
) -> Result<usize, RewriteError> {
    for _ in 0..count {
        if off >= end {
            return Err(RewriteError::Truncated);
        }
        let tag = data[off];
        // Object_variable_info (7) / Uninitialized_variable_info (8) は u16 追従。
        off += 1;
        match tag {
            0..=6 => {}
            7 | 8 => {
                if off + 2 > end {
                    return Err(RewriteError::Truncated);
                }
                off += 2;
            }
            _ => return Err(RewriteError::Unsupported),
        }
    }
    Ok(off)
}

/// 先頭スタックマップフレームの delta+`add` が現行エンコードで表現不能か。
/// 表現不能/未知タグなら拒否理由を返す。フレーム 0 件は問題なし (None)。
fn first_frame_delta_overflow(
    data: &[u8],
    smt_data_off: usize,
    alen: usize,
    add: u16,
) -> Option<&'static str> {
    if alen < 3 || smt_data_off + 3 > data.len() {
        return None;
    }
    let num = u16::from_be_bytes([data[smt_data_off], data[smt_data_off + 1]]);
    if num == 0 {
        return None;
    }
    let frame_off = smt_data_off + 2;
    let tag = data[frame_off];
    let read_u16 = |off: usize| -> u16 { u16::from_be_bytes([data[off], data[off + 1]]) };
    if tag <= 63 {
        if tag as u32 + add as u32 > 63 {
            return Some(
                "first stack map frame same_frame delta would overflow (>63) — needs extended promotion, refusing HEAD inject",
            );
        }
        return None;
    }
    if (64..=127).contains(&tag) {
        if (tag - 64) as u32 + add as u32 > 63 {
            return Some(
                "first stack map frame same_locals_1_stack delta would overflow (>63) — refusing HEAD inject",
            );
        }
        return None;
    }
    if tag >= 247 {
        if frame_off + 3 > data.len() {
            return None;
        }
        let d = read_u16(frame_off + 1) as u32;
        if d + add as u32 > 65535 {
            return Some("first stack map frame extended delta overflow — refusing HEAD inject");
        }
        return None;
    }
    // 128-246 reserved
    Some("reserved first stack map frame tag — refusing HEAD inject")
}

/// Build HEAD injects for a compute redirect into RsiftHooks.
pub fn hook_inject_for_redirect(
    method_name: &str,
    method_desc: &str,
    hook_method: &str,
) -> HeadInject {
    HeadInject {
        method_name: method_name.to_string(),
        method_descriptor: method_desc.to_string(),
        hook_class: "com/rsift/RsiftHooks".into(),
        hook_method: hook_method.to_string(),
        hook_descriptor: "()V".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        assert!(ClassRewriter::from_bytes(&[0, 1, 2, 3]).is_err());
    }

    // --------------------------------------------------------------
    // 最小クラス fixture ビルダ (1 メソッド + Code 属性)
    // --------------------------------------------------------------
    struct Fixture {
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn push_u16(&mut self, v: u16) {
            self.bytes.extend_from_slice(&v.to_be_bytes());
        }
        fn push_u32(&mut self, v: u32) {
            self.bytes.extend_from_slice(&v.to_be_bytes());
        }
    }

    /// CP:
    /// 1 Utf8 "test/Test" / 2 Class#1 / 3 Utf8 "java/lang/Object" / 4 Class#3
    /// 5 Utf8 "Code" / 6 Utf8 <method> / 7 Utf8 <desc>
    /// 8 Utf8 "StackMapTable" / 9 Utf8 "LineNumberTable"
    #[allow(clippy::too_many_arguments)]
    fn build_fixture(
        method_name: &str,
        desc: &str,
        max_stack: u16,
        code: &[u8],
        exc_entries: u16,
        smt: Option<&[u8]>,
        lnt: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut f = Fixture { bytes: Vec::new() };
        let utf8 = |f: &mut Fixture, s: &str| {
            f.bytes.push(1u8);
            f.push_u16(s.len() as u16);
            f.bytes.extend_from_slice(s.as_bytes());
        };
        f.push_u32(0xCAFEBABE);
        f.push_u16(0); // minor
        f.push_u16(52); // major
        f.push_u16(10); // cp_count (entries 1..=9)
        utf8(&mut f, "test/Test");
        f.bytes.push(7u8);
        f.push_u16(1);
        utf8(&mut f, "java/lang/Object");
        f.bytes.push(7u8);
        f.push_u16(3);
        utf8(&mut f, "Code");
        utf8(&mut f, method_name);
        utf8(&mut f, desc);
        utf8(&mut f, "StackMapTable");
        utf8(&mut f, "LineNumberTable");
        f.push_u16(0x0021); // access_flags (public|super)
        f.push_u16(2); // this_class
        f.push_u16(4); // super_class
        f.push_u16(0); // interfaces
        f.push_u16(0); // fields
        f.push_u16(1); // methods_count
        f.push_u16(0x0001); // method access public
        f.push_u16(6); // name
        f.push_u16(7); // desc
        f.push_u16(1); // attributes_count (Code)

        // Code attribute
        let sub_attr_count = smt.iter().count() + lnt.iter().count();
        let code_attr_len: u32 = 2
            + 2
            + 4
            + code.len() as u32
            + 2
            + (exc_entries as u32) * 8
            + 2
            + smt.map(|s| 6 + s.len() as u32).unwrap_or(0)
            + lnt.map(|s| 6 + s.len() as u32).unwrap_or(0);
        f.push_u16(5); // "Code"
        f.push_u32(code_attr_len);
        f.push_u16(max_stack);
        f.push_u16(1); // max_locals
        f.push_u32(code.len() as u32);
        f.bytes.extend_from_slice(code);
        f.push_u16(exc_entries); // exception_table_length
        for _ in 0..exc_entries {
            for _ in 0..8 {
                f.bytes.push(0);
            }
        }
        f.push_u16(sub_attr_count as u16);
        if let Some(s) = smt {
            f.push_u16(8); // "StackMapTable"
            f.push_u32(s.len() as u32);
            f.bytes.extend_from_slice(s);
        }
        if let Some(s) = lnt {
            f.push_u16(9); // "LineNumberTable"
            f.push_u32(s.len() as u32);
            f.bytes.extend_from_slice(s);
        }
        f.push_u16(0); // class attributes
        f.bytes
    }

    fn tail_inject(element: ListElement) -> TailInject {
        TailInject {
            method_name: "lines".into(),
            method_descriptor: "()Ljava/util/List;".into(),
            marker_text: "RsGraphics Render (Rsift)".into(),
            element,
        }
    }

    #[test]
    fn walker_finds_real_tableswitch_and_ignores_ldc_false_positive() {
        // 実 fixtureswitch: iload_1; tableswitch(pad2, default=30, low=0, high=1, 10, 20); areturn
        let switch_code: &[u8] = &[
            0x1b, 0xaa, 0x00, 0x00, // iload_1; tableswitch + pad×2
            0x00, 0x00, 0x00, 0x1e, // default +30
            0x00, 0x00, 0x00, 0x00, // low 0
            0x00, 0x00, 0x00, 0x01, // high 1
            0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x14, // offsets
            0xb0, // areturn
        ];
        assert!(code_contains_tableswitch(switch_code));
        // ldc #170 — オペランドの 0xAA を誤検しない
        let ldc_code: &[u8] = &[0x12, 0xaa, 0xb1];
        assert!(!code_contains_tableswitch(ldc_code));
    }

    #[test]
    fn head_inject_refuses_tableswitch_method() {
        let code: &[u8] = &[
            0x1b, 0xaa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x14, 0xb0,
        ];
        let fixture = build_fixture("run", "()V", 1, code, 0, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let err = rw
            .inject_head_calls(&[hook_inject_for_redirect("run", "()V", "onClientRun")])
            .unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("tableswitch")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn head_inject_happy_path_bumps_first_frame() {
        let code: &[u8] = &[0x01, 0xb0]; // aconst_null; areturn
                                         // SMT: 1 frame, same_frame(10)
        let smt: &[u8] = &[0x00, 0x01, 10];
        let fixture = build_fixture("run", "()V", 1, code, 0, Some(smt), None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let n = rw
            .inject_head_calls(&[hook_inject_for_redirect("run", "()V", "onClientRun")])
            .unwrap();
        assert_eq!(n, 1);
        let out = rw.into_bytes();
        // 再パースして code_length=5 & SMT delta 13 (10+3) を確認
        let view = ClassFileView::parse(&out).unwrap();
        let m = view.methods.iter().find(|m| m.name == "run").unwrap();
        let (cs, len) = m.code_offset.unwrap();
        assert_eq!(len, 5);
        assert_eq!(&out[cs..cs + 3], &[0xb8, out[cs + 1], out[cs + 2]]);
        // SMT attr: code_start+len → exc(2) → attrs_count(2) → attr(name2+len4+data)
        let smt_off = cs + len + 2 + 2 + 6;
        assert_eq!(out[smt_off + 2], 13, "first same_frame delta bumped +3");
    }

    #[test]
    fn head_inject_refuses_first_frame_delta_overflow() {
        let code: &[u8] = &[0x01, 0xb0];
        let smt: &[u8] = &[0x00, 0x01, 62]; // same_frame 62 → +3 = 65 > 63 表現不能
        let fixture = build_fixture("run", "()V", 1, code, 0, Some(smt), None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let err = rw
            .inject_head_calls(&[hook_inject_for_redirect("run", "()V", "onClientRun")])
            .unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("overflow")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn tail_inject_string_marker_happy() {
        let code: &[u8] = &[0x01, 0xb0];
        let smt: &[u8] = &[0x00, 0x01, 0]; // same_frame(0) → abs 0 < 挿入点 1
                                           // LNT: 2 entries (0→7), (1→8)
        let lnt: &[u8] = &[0x00, 0x02, 0x00, 0x00, 0x00, 0x07, 0x00, 0x01, 0x00, 0x08];
        let fixture = build_fixture(
            "lines",
            "()Ljava/util/List;",
            1,
            code,
            0,
            Some(smt),
            Some(lnt),
        );
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        assert!(rw
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap());
        let out = rw.into_bytes();
        let view = ClassFileView::parse(&out).unwrap();
        let m = view.methods.iter().find(|m| m.name == "lines").unwrap();
        let (cs, len) = m.code_offset.unwrap();
        assert_eq!(len, 12, "2 + 10 bytes");
        // 注入列: [01] 59 13 s_hi s_lo B9 i_hi i_lo 02 00 57 B0
        assert_eq!(out[cs], 0x01);
        assert_eq!(out[cs + 1], 0x59);
        assert_eq!(out[cs + 2], 0x13);
        assert_eq!(out[cs + 5], 0xb9);
        assert_eq!(out[cs + 8], 0x02);
        assert_eq!(out[cs + 9], 0x00);
        assert_eq!(out[cs + 10], 0x57);
        assert_eq!(out[cs + 11], 0xb0);
        // マーカー utf8 が CP に存在
        assert!(view
            .utf8_constants
            .values()
            .any(|s| s == "RsGraphics Render (Rsift)"));
        // LNT: 挿入点 1 以降の start_pc=1 が 11 に追随
        let lnt_off = cs + len + 2 + 2; // code 後 exc(2) attr_count(2)
                                        // SMT attr を読み飛ばす
        let smt_len = u32::from_be_bytes([
            out[lnt_off + 2],
            out[lnt_off + 3],
            out[lnt_off + 4],
            out[lnt_off + 5],
        ]) as usize;
        let lnt_attr = lnt_off + 6 + smt_len; // 次の attr = LNT
        let lnt_data = lnt_attr + 6;
        // entry2 start_pc: data+2 (count) + 4 (entry1) → offset data+6
        assert_eq!(
            u16::from_be_bytes([out[lnt_data + 6], out[lnt_data + 7]]),
            11
        );
        // max_stack 1 → 3
        let max_stack = u16::from_be_bytes([out[cs - 8], out[cs - 7]]);
        assert_eq!(max_stack, 3);
    }

    #[test]
    fn tail_inject_component_marker_happy() {
        let code: &[u8] = &[0x01, 0xb0];
        let fixture = build_fixture("lines", "()Ljava/util/List;", 1, code, 0, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        assert!(rw
            .inject_tail_list_marker(&tail_inject(ListElement::Component))
            .unwrap());
        let out = rw.into_bytes();
        let view = ClassFileView::parse(&out).unwrap();
        let m = view.methods.iter().find(|m| m.name == "lines").unwrap();
        let (cs, len) = m.code_offset.unwrap();
        assert_eq!(len, 15, "2 + 13 bytes");
        // 59 13 _ _ B8 _ _ B9 _ _ 02 00 57 B0
        assert_eq!(out[cs + 1], 0x59);
        assert_eq!(out[cs + 2], 0x13);
        assert_eq!(out[cs + 5], 0xb8);
        assert_eq!(out[cs + 8], 0xb9);
        assert_eq!(out[cs + 11], 0x02);
        assert_eq!(out[cs + 12], 0x00);
        assert_eq!(out[cs + 13], 0x57);
        assert_eq!(out[cs + 14], 0xb0);
        assert!(view
            .utf8_constants
            .values()
            .any(|s| s == "net/minecraft/network/chat/Component"));
    }

    #[test]
    fn tail_inject_is_idempotent() {
        let code: &[u8] = &[0x01, 0xb0];
        let fixture = build_fixture("lines", "()Ljava/util/List;", 1, code, 0, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        assert!(rw
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap());
        let once = rw.into_bytes();
        let mut rw2 = ClassRewriter::from_bytes(&once).unwrap();
        // 2 度目 → Ok(false) でバイト列不変
        assert!(!rw2
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap());
        let twice = rw2.into_bytes();
        assert_eq!(once, twice);
    }

    #[test]
    fn tail_inject_refuses_exception_table() {
        let code: &[u8] = &[0x01, 0xb0];
        let fixture = build_fixture("lines", "()Ljava/util/List;", 1, code, 1, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let err = rw
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("exception table")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn tail_inject_refuses_frame_at_insertion_point() {
        let code: &[u8] = &[0x01, 0xb0];
        // same_frame(1): 絶対 offset 1 = 挿入点 → 分岐到達を意味 → 拒否
        let smt: &[u8] = &[0x00, 0x01, 1];
        let fixture = build_fixture("lines", "()Ljava/util/List;", 1, code, 0, Some(smt), None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let err = rw
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("stack map frame")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn tail_inject_refuses_non_areturn_ending() {
        let code: &[u8] = &[0x01, 0xb1]; // return (void) — List desc だが終端が違う
        let fixture = build_fixture("lines", "()Ljava/util/List;", 1, code, 0, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let err = rw
            .inject_tail_list_marker(&tail_inject(ListElement::String))
            .unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("areturn")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn tail_inject_refuses_non_list_return() {
        let code: &[u8] = &[0x01, 0xb0];
        let fixture = build_fixture("tick", "()V", 1, code, 0, None, None);
        let mut rw = ClassRewriter::from_bytes(&fixture).unwrap();
        let inj = TailInject {
            method_name: "tick".into(),
            method_descriptor: "()V".into(),
            marker_text: "x".into(),
            element: ListElement::String,
        };
        let err = rw.inject_tail_list_marker(&inj).unwrap_err();
        match err {
            RewriteError::Refused(msg) => assert!(msg.contains("java.util.List")),
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn stackmap_walker_computes_absolute_offsets() {
        // frames: same_frame(5) → abs5; append_frame(252, delta=2, 1 vti(Integer)) → abs 5+2+1=8;
        // full_frame(255, delta=3, locals 0, stack 0) → abs 8+3+1=12
        let smt: &[u8] = &[
            0x00, 0x03, // 3 frames
            5,    // same_frame(5)
            252, 0x00, 0x02, 0, // append_frame delta=2, vti Top(0)
            255, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, // full_frame delta=3
        ];
        let max = stackmap_max_abs_offset(smt, 0, smt.len()).unwrap();
        assert_eq!(max, Some(12));
        // 0 frames → None
        let empty: &[u8] = &[0x00, 0x00];
        assert_eq!(stackmap_max_abs_offset(empty, 0, 2).unwrap(), None);
        // 切断 → Err
        let broken: &[u8] = &[0x00, 0x01, 255, 0x00];
        assert!(stackmap_max_abs_offset(broken, 0, broken.len()).is_err());
    }
}
