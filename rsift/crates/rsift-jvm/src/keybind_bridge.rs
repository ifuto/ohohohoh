//! # Vanilla KeyMapping Reflection Bridge (`keybind_bridge`) — wave 203
//!
//! Mod が rsift-api で宣言したキーバインドを **Minecraft 本体の入力機構に
//! 食い込ませる**ためのリフレクション層。やることは 3 つ:
//!
//! 1. **登録**: 宣言ごとに本物の `net.minecraft.client.KeyMapping` を生成し、
//!    `Minecraft.getInstance().options.keyMappings` 配列の末尾へ追記する。
//!    追記されたバインドはバニラの「コントロール (キー設定)」画面に表示され、
//!    再割当・options.txt 永続化はバニラがそのまま担う (= 我々は実装しない)。
//! 2. **既存照合**: 同名が既に居れば (options.txt から復元済み含む) それを使う。
//! 3. **状態同期**: 各 `KeyMapping.isDown()` をポーリングし、変化時だけ
//!    `rsift_api` の KeybindRegistry へ書き戻す (Mod は共有セルを読むだけ)。
//!
//! 呼出点は `nativeOnHook` の "screen_init" / "client_tick" / "render_flip"
//! (最初のタイトル画面 init で install、以降は tick/frame で状態同期)。
//! リフレクションは mojmap 1.21.x の実名を対象に、複数世代のコンストラクタ
//! シグネチャを順に試す (どれか 1 つが当たるまで例外を潰して次へ)。
//! 全失敗時は一度だけ警告し、以後静黙 (キーは繋がらないが、嘘の代替入力を
//! 読んだりはしない)。

use jni::objects::{JClass, JObject, JObjectArray, JString, JValue};
use jni::JNIEnv;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_log::agent_log;

static LOGGED_OK: AtomicBool = AtomicBool::new(false);
static LOGGED_WARN: AtomicBool = AtomicBool::new(false);

/// 試すコンストラクタ戦略 (新しい世代から順)。args は戦略ごとに組み立てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CtorStrategy {
    /// 1.21.9+ 系: (String id, Category, InputConstants$Type, int)
    IdCategoryTypeCode,
    /// (String id, InputConstants$Type, int, String category)
    IdTypeCodeCategory,
    /// 旧世代 (〜1.21.4 頃): (String id, int code, String category)
    IdCodeCategory,
    /// 稀な並び: (String id, String category, int code)
    IdCategoryCode,
}

pub const CTOR_STRATEGIES: &[CtorStrategy] = &[
    CtorStrategy::IdCategoryTypeCode,
    CtorStrategy::IdTypeCodeCategory,
    CtorStrategy::IdCodeCategory,
    CtorStrategy::IdCategoryCode,
];

const MC_CLASS: &str = "net/minecraft/client/Minecraft";
const OPTIONS_CLASS: &str = "net/minecraft/client/Options";
const KEYMAPPING_CLASS: &str = "net/minecraft/client/KeyMapping";
const KM_CATEGORY_CLASS: &str = "net/minecraft/client/KeyMapping$Category";
const INPUT_TYPE_CLASS: &str = "com/mojang/blaze3d/platform/InputConstants$Type";

// wave HR (#5 Phase 2): 難読化ランタイム向け名前解決ヘルパー (obf_map 経由)。
// Unobfuscated/未 install 時は原名へ安全落下 = 従来挙動完全保存。
#[inline]
fn rcls(mojmap_slash: &str) -> String {
    crate::obf_map::resolve_class_internal(&mojmap_slash.replace('/', "."))
        .unwrap_or_else(|| mojmap_slash.to_string())
}
#[inline]
fn rmname(mojmap_class_dotted: &str, mojmap_method: &str) -> String {
    crate::obf_map::resolve_method_by_name(mojmap_class_dotted, mojmap_method)
        .unwrap_or_else(|| mojmap_method.to_string())
}
#[inline]
fn rfield(mojmap_class_dotted: &str, mojmap_field: &str) -> String {
    crate::obf_map::resolve_field(mojmap_class_dotted, mojmap_field)
        .unwrap_or_else(|| mojmap_field.to_string())
}

/// フック入口。env を伴うフレームで呼ぶ。失敗は内部で一度だけ警告して潰す。
pub fn poll_and_sync(env: &mut JNIEnv) {
    let Some(rt) = rsift_api::runtime::runtime() else {
        return;
    };
    let regs = rt.keybinds().registrations();
    if regs.is_empty() {
        return;
    }
    match sync_inner(env, rt, &regs) {
        Ok(installed_any) => {
            if installed_any && !LOGGED_OK.swap(true, Ordering::Relaxed) {
                let names: Vec<&str> = regs.iter().map(|r| r.name.as_str()).collect();
                agent_log(&format!(
                    "[RsiftKeys] vanilla KeyMapping 登録+同期 OK: {:?} (Options → Controls 画面で再割当可)",
                    names
                ));
            }
        }
        Err(e) => {
            if !LOGGED_WARN.swap(true, Ordering::Relaxed) {
                agent_log(&format!(
                    "[RsiftKeys] WARN vanilla KeyMapping リフレクション失敗 (キー入力は未接続): {e}"
                ));
            }
        }
    }
}

/// 登録照合 (install 欠損分のみ追加) → 状態同期。戻り値 = 今回新規追記があったか。
fn sync_inner(
    env: &mut JNIEnv,
    rt: &rsift_api::runtime::RsiftRuntime,
    regs: &[rsift_api::keybinds::KeybindRegistration],
) -> Result<bool, String> {
    let mc = minecraft_instance(env)?;
    // wave HR (#5 Phase 2): options (Minecraft) / keyMappings (Options) フィールド名 + descriptor 解決。
    let options_field = rfield("net.minecraft.client.Minecraft", "options");
    let options_sig = crate::obf_map::resolve_descriptor(&format!("L{OPTIONS_CLASS};"));
    let km_field = rfield("net.minecraft.client.Options", "keyMappings");
    let km_sig = crate::obf_map::resolve_descriptor(&format!("[L{KEYMAPPING_CLASS};"));
    let options = get_object_field(env, &mc, &options_field, &options_sig)?;
    let arr = get_object_field(env, &options, &km_field, &km_sig)?;
    let arr = JObjectArray::from(arr);
    let km_class = find_game_class(env, KEYMAPPING_CLASS)?;

    // 1) 既存照合 & 欠損分の生成→追記
    let mut newly_installed = false;
    for reg in regs {
        let arr_now = get_object_field(env, &options, &km_field, &km_sig)?;
        let arr_now = JObjectArray::from(arr_now);
        if mapping_index_of(env, &arr_now, &reg.name)?.is_some() {
            continue; // options.txt 復元済み or 前回追記済み
        }
        let mapping = new_key_mapping(env, &km_class, &reg.name, &reg.category, reg.default_code)?;
        let grown = grown_with(env, &arr_now, &km_class, &mapping)?;
        env.set_field(&options, &km_field, &km_sig, JValue::Object(&grown))
            .map_err(|e| format!("set keyMappings: {e}"))?;
        clear_pending(env);
        newly_installed = true;
    }
    let _ = &arr; // 最初の配列は install 可否判定で都度読み直すため保持不要

    // 2) 状態同期 (追記後の配列を読み直す)
    let arr = get_object_field(env, &options, &km_field, &km_sig)?;
    let arr = JObjectArray::from(arr);
    for reg in regs {
        let Some(idx) = mapping_index_of(env, &arr, &reg.name)? else {
            continue;
        };
        let elem = env
            .get_object_array_element(&arr, idx as i32)
            .map_err(|e| format!("get keyMappings[{idx}]: {e}"))?;
        let isdown = rmname("net.minecraft.client.KeyMapping", "isDown");
        let down = env
            .call_method(&elem, &isdown, "()Z", &[])
            .map_err(|e| format!("isDown: {e}"))?
            .z()
            .map_err(|e| format!("isDown ret: {e}"))?;
        clear_pending(env);
        rt.keybind_set_state(&reg.name, down);
    }
    Ok(newly_installed)
}

fn minecraft_instance<'local>(env: &mut JNIEnv<'local>) -> Result<JObject<'local>, String> {
    let cls = find_game_class(env, MC_CLASS)?;
    let m = rmname("net.minecraft.client.Minecraft", "getInstance");
    let d = crate::obf_map::resolve_descriptor(&format!("()L{MC_CLASS};"));
    env.call_static_method(cls, &m, &d, &[])
        .map_err(|e| format!("Minecraft.getInstance: {e}"))?
        .l()
        .map_err(|e| format!("getInstance ret: {e}"))
}

fn find_game_class<'local>(env: &mut JNIEnv<'local>, name: &str) -> Result<JClass<'local>, String> {
    // wave HR (#5 Phase 2): 実行時内部名 (難読化版では難読名) へ解決して検索。
    let resolved = rcls(name);
    if let Ok(c) = env.find_class(&resolved) {
        clear_pending(env);
        return Ok(c);
    }
    clear_pending(env);
    let Some(loader) = super::screen_inject::game_class_loader(env) else {
        return Err(format!("game classloader unavailable for {name}"));
    };
    let dotted = resolved.replace('/', ".");
    let jname: JString = env
        .new_string(&dotted)
        .map_err(|e| format!("new_string: {e}"))?;
    let obj = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&jname)],
        )
        .map_err(|e| format!("loadClass {name}: {e}"))?
        .l()
        .map_err(|e| format!("loadClass ret: {e}"))?;
    clear_pending(env);
    Ok(JClass::from(obj))
}

fn get_object_field<'local>(
    env: &mut JNIEnv<'local>,
    obj: &JObject,
    field: &str,
    sig: &str,
) -> Result<JObject<'local>, String> {
    env.get_field(obj, field, sig)
        .map_err(|e| format!("field {field}: {e}"))?
        .l()
        .map_err(|e| format!("field {field} ret: {e}"))
}

fn clear_pending(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

/// 配列内で name (= KeyMapping.getName()) に一致する index を探す。
fn mapping_index_of(
    env: &mut JNIEnv,
    arr: &JObjectArray,
    name: &str,
) -> Result<Option<usize>, String> {
    let len = env
        .get_array_length(arr)
        .map_err(|e| format!("array length: {e}"))? as usize;
    for i in 0..len {
        let elem = env
            .get_object_array_element(arr, i as i32)
            .map_err(|e| format!("keyMappings[{i}]: {e}"))?;
        let getname = rmname("net.minecraft.client.KeyMapping", "getName");
        let jname = env
            .call_method(&elem, &getname, "()Ljava/lang/String;", &[])
            .map_err(|e| format!("getName: {e}"))?
            .l()
            .map_err(|e| format!("getName ret: {e}"))?;
        clear_pending(env);
        let s: String = env
            .get_string(&JString::from(jname))
            .map(|s| s.into())
            .unwrap_or_default();
        if s == name {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// 1 要素大きい新配列へコピーして末尾に mapping を入れて返す。
fn grown_with<'local>(
    env: &mut JNIEnv<'local>,
    arr: &JObjectArray<'local>,
    km_class: &JClass<'local>,
    mapping: &JObject<'local>,
) -> Result<JObjectArray<'local>, String> {
    let len = env
        .get_array_length(arr)
        .map_err(|e| format!("array length: {e}"))?;
    let out = env
        .new_object_array(len + 1, km_class, JObject::null())
        .map_err(|e| format!("new keyMappings[{len}+1]: {e}"))?;
    for i in 0..len {
        let elem = env
            .get_object_array_element(arr, i)
            .map_err(|e| format!("copy[{i}]: {e}"))?;
        env.set_object_array_element(&out, i, elem)
            .map_err(|e| format!("set[{i}]: {e}"))?;
    }
    env.set_object_array_element(&out, len, mapping)
        .map_err(|e| format!("set[{len}]: {e}"))?;
    Ok(out)
}

/// 複数世代のシグネチャを順に試して KeyMapping を生成する。
fn new_key_mapping<'local>(
    env: &mut JNIEnv<'local>,
    km_class: &JClass<'local>,
    name: &str,
    category: &str,
    code: i32,
) -> Result<JObject<'local>, String> {
    let jname: JString = env.new_string(name).map_err(|e| e.to_string())?;
    let jcat: JString = env.new_string(category).map_err(|e| e.to_string())?;
    let mut errors = Vec::new();
    for strat in CTOR_STRATEGIES {
        match try_ctor(env, km_class, *strat, &jname, &jcat, code) {
            Ok(obj) => return Ok(obj),
            Err(e) => {
                errors.push(format!("{strat:?}: {e}"));
                clear_pending(env);
            }
        }
    }
    Err(errors.join(" | "))
}

fn try_ctor<'local>(
    env: &mut JNIEnv<'local>,
    km_class: &JClass<'local>,
    strat: CtorStrategy,
    jname: &JString,
    jcat: &JString,
    code: i32,
) -> Result<JObject<'local>, String> {
    match strat {
        CtorStrategy::IdCodeCategory => env
            .new_object(
                km_class,
                "(Ljava/lang/String;ILjava/lang/String;)V",
                &[
                    JValue::Object(jname),
                    JValue::Int(code),
                    JValue::Object(jcat),
                ],
            )
            .map_err(|e| e.to_string()),
        CtorStrategy::IdCategoryCode => env
            .new_object(
                km_class,
                "(Ljava/lang/String;Ljava/lang/String;I)V",
                &[
                    JValue::Object(jname),
                    JValue::Object(jcat),
                    JValue::Int(code),
                ],
            )
            .map_err(|e| e.to_string()),
        CtorStrategy::IdTypeCodeCategory => {
            let t = first_enum_constant(env, INPUT_TYPE_CLASS)?;
            env.new_object(
                km_class,
                &crate::obf_map::resolve_descriptor(&format!(
                    "(Ljava/lang/String;L{INPUT_TYPE_CLASS};ILjava/lang/String;)V"
                )),
                &[
                    JValue::Object(jname),
                    JValue::Object(&t),
                    JValue::Int(code),
                    JValue::Object(jcat),
                ],
            )
            .map_err(|e| e.to_string())
        }
        CtorStrategy::IdCategoryTypeCode => {
            let c = first_enum_constant(env, KM_CATEGORY_CLASS)?;
            let t = first_enum_constant(env, INPUT_TYPE_CLASS)?;
            env.new_object(
                km_class,
                &crate::obf_map::resolve_descriptor(&format!(
                    "(Ljava/lang/String;L{KM_CATEGORY_CLASS};L{INPUT_TYPE_CLASS};I)V"
                )),
                &[
                    JValue::Object(jname),
                    JValue::Object(&c),
                    JValue::Object(&t),
                    JValue::Int(code),
                ],
            )
            .map_err(|e| e.to_string())
        }
    }
}

/// enum クラスの全定数から任意 (index 0) の実値を取る。定数名 (KEYSYM 等) を
/// 世代横断で知らなくて済む唯一の頑健経路。
fn first_enum_constant<'local>(
    env: &mut JNIEnv<'local>,
    class_name: &str,
) -> Result<JObject<'local>, String> {
    let cls = find_game_class(env, class_name)?;
    let arr = env
        .call_method(
            JObject::from(cls),
            "getEnumConstants",
            &crate::obf_map::resolve_descriptor(&format!("()[L{class_name};")),
            &[],
        )
        .map_err(|e| format!("getEnumConstants {class_name}: {e}"))?
        .l()
        .map_err(|e| format!("getEnumConstants ret: {e}"))?;
    clear_pending(env);
    if arr.is_null() {
        return Err(format!("{class_name} は enum 定数を返さなかった"));
    }
    let arr = JObjectArray::from(arr);
    let len = env
        .get_array_length(&arr)
        .map_err(|e| format!("constants length: {e}"))?;
    if len == 0 {
        return Err(format!("{class_name} に enum 定数が無い"));
    }
    env.get_object_array_element(&arr, 0)
        .map_err(|e| format!("constants[0]: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctor_strategies_are_newest_first_and_cover_legacy() {
        // 新世代 (Category 実体型) を先頭に、旧来 (String,int,String) も網羅。
        assert_eq!(CTOR_STRATEGIES[0], CtorStrategy::IdCategoryTypeCode);
        assert!(CTOR_STRATEGIES.contains(&CtorStrategy::IdCodeCategory));
        assert!(CTOR_STRATEGIES.contains(&CtorStrategy::IdCategoryCode));
        assert_eq!(CTOR_STRATEGIES.len(), 4, "戦略数の退化防止");
        let mut seen = std::collections::HashSet::new();
        for s in CTOR_STRATEGIES {
            assert!(seen.insert(s), "重複戦略");
        }
    }

    #[test]
    fn class_names_are_mojmap_real_paths() {
        // バニラ実クラス名の退行防止ピン (JNIEnv.find_class 形式の '/' 区切り)。
        assert!(MC_CLASS.starts_with("net/minecraft/client"));
        assert!(KEYMAPPING_CLASS.ends_with("KeyMapping"));
        assert!(KM_CATEGORY_CLASS.contains('$'));
        assert!(INPUT_TYPE_CLASS.contains('$'));
        // getEnumConstants 配列シグ形状
        assert_eq!(
            format!("()[L{KM_CATEGORY_CLASS};"),
            "()[Lnet/minecraft/client/KeyMapping$Category;"
        );
    }
}
