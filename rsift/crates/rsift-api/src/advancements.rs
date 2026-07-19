//! # Rsift Advancement System (Fabric `fabric-api-base` / NeoForge `AdvancementEvent` parity)
//!
//! アドバンスメント（実績）の本物実装です。**Fabric も NeoForge も一切不要**で、
//! Rsift 単体のネイティブ Rust で動作します。`rsift_api::resources::ResourceLoader`
//! （ルートテーブル修正）と同様に、バニラ Minecraft のアドバンスメント概念を
//! Rust で再現しています。
//!
//! - `AdvancementRegistry` がアドバンスメント定義を保持（Builder で宣言）。
//! - `PlayerAdvancementState` が各プレイヤーの進捗（どの基準を達成したか）を管理。
//! - `PlayerAdvancementState::fire_trigger` がゲームプレイイベント（kill / 採掘 /
//!   インベントリ変化 など）を受けて進捗を加算し、要件を満たせば付与します。

use crate::registry::RegistryKey;
use std::collections::{HashMap, HashSet};
use tracing::info;

/// アドバンスメントの枠種別（表示アイコンの周りの枠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvancementFrame {
    Task,
    Goal,
    Challenge,
}

/// アドバンスメントの表示情報（タイトル・説明・アイコン・枠など）。
#[derive(Debug, Clone)]
pub struct AdvancementDisplay {
    pub title: String,
    pub description: String,
    pub icon: RegistryKey,
    pub frame: AdvancementFrame,
    pub background: Option<String>,
    pub show_toast: bool,
    pub announce_to_chat: bool,
    pub hidden: bool,
}

impl AdvancementDisplay {
    pub fn new(title: &str, description: &str, icon: RegistryKey, frame: AdvancementFrame) -> Self {
        Self {
            title: title.to_string(),
            description: description.to_string(),
            icon,
            frame,
            background: None,
            show_toast: true,
            announce_to_chat: true,
            hidden: false,
        }
    }
    pub fn background(mut self, bg: &str) -> Self {
        self.background = Some(bg.to_string());
        self
    }
    pub fn hidden(mut self, h: bool) -> Self {
        self.hidden = h;
        self
    }
    pub fn show_toast(mut self, v: bool) -> Self {
        self.show_toast = v;
        self
    }
    pub fn announce_to_chat(mut self, v: bool) -> Self {
        self.announce_to_chat = v;
        self
    }
}

/// アドバンスメント達成時の報酬。
#[derive(Debug, Clone, Default)]
pub struct AdvancementRewards {
    pub experience: u32,
    pub loot_tables: Vec<RegistryKey>,
    pub items: Vec<RegistryKey>,
    pub function: Option<String>,
}

impl AdvancementRewards {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn experience(mut self, xp: u32) -> Self {
        self.experience = xp;
        self
    }
    pub fn loot(mut self, table: RegistryKey) -> Self {
        self.loot_tables.push(table);
        self
    }
    pub fn item(mut self, item: RegistryKey) -> Self {
        self.items.push(item);
        self
    }
    pub fn function(mut self, f: &str) -> Self {
        self.function = Some(f.to_string());
        self
    }
}

/// 単一の基準（criteria）。名前・トリガー ID・必要進捗回数を持つ。
#[derive(Debug, Clone)]
pub struct AdvancementCriterion {
    pub name: String,
    pub trigger: String,
    pub required_progress: u32,
}

impl AdvancementCriterion {
    pub fn new(name: &str, trigger: &str) -> Self {
        Self {
            name: name.to_string(),
            trigger: trigger.to_string(),
            required_progress: 1,
        }
    }
    /// この基準を満たすためにトリガーが何回発火する必要があるか（最小 1）。
    pub fn with_required_progress(mut self, n: u32) -> Self {
        self.required_progress = n.max(1);
        self
    }
}

/// アドバンスメント定義本体。
#[derive(Debug, Clone)]
pub struct Advancement {
    pub id: RegistryKey,
    pub parent: Option<RegistryKey>,
    pub display: Option<AdvancementDisplay>,
    pub criteria: Vec<AdvancementCriterion>,
    /// 要件（AND-of-OR）。各内側 Vec が 1 つの基準名集合で、その集合は
    /// いずれか 1 つが満たされればよい。全集合が満たされて初めて付与される。
    pub requirements: Vec<Vec<String>>,
    pub rewards: AdvancementRewards,
}

impl Advancement {
    pub fn builder(id: RegistryKey) -> AdvancementBuilder {
        AdvancementBuilder::new(id)
    }
    pub fn criterion_names(&self) -> Vec<String> {
        self.criteria.iter().map(|c| c.name.clone()).collect()
    }
}

/// 流れるような Builder。`Advancement::builder(id).criterion(...).build()` で構築。
pub struct AdvancementBuilder {
    adv: Advancement,
}

impl AdvancementBuilder {
    pub fn new(id: RegistryKey) -> Self {
        Self {
            adv: Advancement {
                id,
                parent: None,
                display: None,
                criteria: Vec::new(),
                requirements: Vec::new(),
                rewards: AdvancementRewards::new(),
            },
        }
    }
    pub fn parent(mut self, p: RegistryKey) -> Self {
        self.adv.parent = Some(p);
        self
    }
    pub fn display(mut self, d: AdvancementDisplay) -> Self {
        self.adv.display = Some(d);
        self
    }
    pub fn criterion(mut self, c: AdvancementCriterion) -> Self {
        self.adv.criteria.push(c);
        self
    }
    pub fn requirements(mut self, sets: &[&[&str]]) -> Self {
        self.adv.requirements = sets
            .iter()
            .map(|s| s.iter().map(|n| n.to_string()).collect())
            .collect();
        self
    }
    pub fn rewards(mut self, r: AdvancementRewards) -> Self {
        self.adv.rewards = r;
        self
    }
    pub fn build(self) -> Advancement {
        self.adv
    }
}

/// 登録済みアドバンスメントを保持するレジストリ（Rsift 単体・ネイティブ）。
#[derive(Default)]
pub struct AdvancementRegistry {
    advancements: HashMap<RegistryKey, Advancement>,
}

impl AdvancementRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// アドバンスメントを登録する。要件が存在しない基準を参照している場合は Err。
    pub fn register(&mut self, adv: Advancement) -> Result<(), String> {
        if adv.criteria.is_empty() {
            return Err(format!("advancement '{}' has no criteria", adv.id.as_str()));
        }
        let names: HashSet<String> = adv.criteria.iter().map(|c| c.name.clone()).collect();
        for set in &adv.requirements {
            for n in set {
                if !names.contains(n) {
                    return Err(format!(
                        "advancement '{}' requires unknown criterion '{}'",
                        adv.id.as_str(),
                        n
                    ));
                }
            }
        }
        info!("Registering Advancement: {}", adv.id.as_str());
        self.advancements.insert(adv.id.clone(), adv);
        crate::platform::mark_dirty();
        Ok(())
    }

    pub fn get(&self, id: &RegistryKey) -> Option<&Advancement> {
        self.advancements.get(id)
    }

    pub fn contains(&self, id: &RegistryKey) -> bool {
        self.advancements.contains_key(id)
    }

    pub fn all_ids(&self) -> Vec<RegistryKey> {
        self.advancements.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.advancements.len()
    }
}

/// 1 プレイヤー分のアドバンスメント進捗状態。
#[derive(Default, Clone)]
pub struct PlayerAdvancementState {
    /// adv_id -> (criterion 名 -> 累積進捗)
    progress: HashMap<RegistryKey, HashMap<String, u32>>,
    /// adv_id -> 付与済みか
    granted: HashMap<RegistryKey, bool>,
}

impl PlayerAdvancementState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn criterion_progress(&self, adv_id: &RegistryKey, criterion: &str) -> u32 {
        self.progress
            .get(adv_id)
            .and_then(|m| m.get(criterion).copied())
            .unwrap_or(0)
    }

    pub fn is_granted(&self, adv_id: &RegistryKey) -> bool {
        self.granted.get(adv_id).copied().unwrap_or(false)
    }

    /// このアドバンスメントで現在満たされている基準名の一覧。
    pub fn satisfied_criteria(&self, reg: &AdvancementRegistry, adv_id: &RegistryKey) -> Vec<String> {
        let Some(adv) = reg.get(adv_id) else {
            return Vec::new();
        };
        let per = self.progress.get(adv_id);
        adv.criteria
            .iter()
            .filter(|c| {
                let p = per.and_then(|m| m.get(&c.name).copied()).unwrap_or(0);
                p >= c.required_progress
            })
            .map(|c| c.name.clone())
            .collect()
    }

    /// `criterion` に `amount` 進捗を加算する。
    /// 戻り値: (その基準が今回新たに完了したか, アドバンスメントが今回新たに付与されたか)
    pub fn grant_progress(
        &mut self,
        reg: &AdvancementRegistry,
        adv_id: &RegistryKey,
        criterion: &str,
        amount: u32,
    ) -> (bool, bool) {
        let Some(adv) = reg.get(adv_id) else {
            return (false, false);
        };
        let Some(crit) = adv.criteria.iter().find(|c| c.name == criterion) else {
            return (false, false);
        };
        if self.is_granted(adv_id) {
            return (false, false);
        }
        let per = self.progress.entry(adv_id.clone()).or_default();
        let cur = per.entry(criterion.to_string()).or_insert(0);
        let done_before = *cur >= crit.required_progress;
        *cur = (*cur + amount).min(crit.required_progress);
        let done_now = *cur >= crit.required_progress;
        let criterion_newly_done = done_now && !done_before;

        // 要件の解決: 明示的 requirements がなければ「全基準の AND」とする（Minecraft 互換）。
        let reqs: Vec<Vec<String>> = if adv.requirements.is_empty() {
            vec![adv.criteria.iter().map(|c| c.name.clone()).collect()]
        } else {
            adv.requirements.clone()
        };
        let all_satisfied = reqs.iter().all(|set| {
            set.iter().any(|n| {
                let p = per.get(n).copied().unwrap_or(0);
                let need = adv
                    .criteria
                    .iter()
                    .find(|c| &c.name == n)
                    .map(|c| c.required_progress)
                    .unwrap_or(1);
                p >= need
            })
        });
        let newly_granted = if all_satisfied {
            !self.granted.contains_key(adv_id)
        } else {
            false
        };
        if all_satisfied {
            self.granted.insert(adv_id.clone(), true);
        }
        (criterion_newly_done, newly_granted)
    }

    /// ゲームプレイトリガーを発火させる。`trigger` に一致し `cond` を通過する
    /// すべての基準に `amount` 進捗を加算し、変化した
    /// (アドバンスメント id, 基準名, アドバンスメント新規付与フラグ) のリストを返す。
    pub fn fire_trigger<F>(
        &mut self,
        reg: &AdvancementRegistry,
        trigger: &str,
        amount: u32,
        cond: F,
    ) -> Vec<(RegistryKey, String, bool)>
    where
        F: Fn(&AdvancementCriterion) -> bool,
    {
        let mut out = Vec::new();
        for adv_id in reg.all_ids() {
            let Some(adv) = reg.get(&adv_id) else {
                continue;
            };
            for crit in &adv.criteria {
                if crit.trigger == trigger && cond(crit) {
                    let (_, granted) = self.grant_progress(reg, &adv_id, &crit.name, amount);
                    out.push((adv_id.clone(), crit.name.clone(), granted));
                }
            }
        }
        out
    }

    /// 進捗・付与状態をリセットする。
    pub fn reset(&mut self, adv_id: &RegistryKey) {
        self.progress.remove(adv_id);
        self.granted.remove(adv_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> AdvancementRegistry {
        AdvancementRegistry::new()
    }

    #[test]
    fn register_and_lookup() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "first"))
            .display(AdvancementDisplay::new(
                "First",
                "Do it",
                RegistryKey::new("minecraft", "diamond"),
                AdvancementFrame::Task,
            ))
            .criterion(AdvancementCriterion::new("get_diamond", "minecraft:inventory_changed"))
            .build();
        assert!(r.register(adv).is_ok());
        assert!(r.contains(&RegistryKey::new("mymod", "first")));
        let got = r.get(&RegistryKey::new("mymod", "first")).unwrap();
        assert_eq!(got.criteria.len(), 1);
        assert_eq!(got.criterion_names(), vec!["get_diamond".to_string()]);
    }

    #[test]
    fn rejects_unknown_criterion_in_requirement() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "bad"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .requirements(&[&["a", "ghost"]])
            .build();
        let err = r.register(adv);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("ghost"));
    }

    #[test]
    fn rejects_empty_criteria() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "empty")).build();
        assert!(r.register(adv).is_err());
    }

    #[test]
    fn single_criterion_grants() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "one"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "one");
        let mut st = PlayerAdvancementState::new();
        let (done, granted) = st.grant_progress(&r, &id, "a", 1);
        assert!(done);
        assert!(granted);
        assert!(st.is_granted(&id));
    }

    #[test]
    fn required_progress_needs_multiple() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "multi"))
            .criterion(AdvancementCriterion::new("a", "t:a").with_required_progress(3))
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "multi");
        let mut st = PlayerAdvancementState::new();
        let (_, g1) = st.grant_progress(&r, &id, "a", 1);
        assert!(!g1);
        assert_eq!(st.criterion_progress(&id, "a"), 1);
        let (_, g2) = st.grant_progress(&r, &id, "a", 1);
        assert!(!g2);
        let (done, g3) = st.grant_progress(&r, &id, "a", 1);
        assert!(done);
        assert!(g3);
        assert!(st.is_granted(&id));
        // 上限でクランプされる
        st.grant_progress(&r, &id, "a", 100);
        assert_eq!(st.criterion_progress(&id, "a"), 3);
    }

    #[test]
    fn and_requirements_need_all() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "and"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .criterion(AdvancementCriterion::new("b", "t:b"))
            .requirements(&[&["a"], &["b"]]) // AND: 両方必要
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "and");
        let mut st = PlayerAdvancementState::new();
        st.grant_progress(&r, &id, "a", 1);
        assert!(!st.is_granted(&id));
        let (_, g) = st.grant_progress(&r, &id, "b", 1);
        assert!(g);
        assert!(st.is_granted(&id));
    }

    #[test]
    fn or_requirements_need_any() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "or"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .criterion(AdvancementCriterion::new("b", "t:b"))
            .requirements(&[&["a", "b"]]) // OR: どちらか 1 つでよい
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "or");
        let mut st = PlayerAdvancementState::new();
        let (_, g) = st.grant_progress(&r, &id, "a", 1);
        assert!(g); // a だけで単一 OR 集合を満たす
        assert!(st.is_granted(&id));
    }

    #[test]
    fn fire_trigger_applies_to_matching_criteria() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "kill"))
            .criterion(AdvancementCriterion::new("zombie", "minecraft:killed_entity"))
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "kill");
        let mut st = PlayerAdvancementState::new();
        let changes = st.fire_trigger(&r, "minecraft:killed_entity", 1, |_| true);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].2); // 新規付与
        assert!(st.is_granted(&id));
    }

    #[test]
    fn fire_trigger_respects_condition() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "zombie_or_skeleton"))
            .criterion(
                AdvancementCriterion::new("mob", "minecraft:killed_entity")
                    .with_required_progress(2),
            )
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "zombie_or_skeleton");
        let mut st = PlayerAdvancementState::new();
        // 条件が false なら進捗しない
        let none = st.fire_trigger(&r, "minecraft:killed_entity", 1, |_| false);
        assert!(none.is_empty());
        assert!(!st.is_granted(&id));
        // 2 回 true で達成
        st.fire_trigger(&r, "minecraft:killed_entity", 1, |_| true);
        let second = st.fire_trigger(&r, "minecraft:killed_entity", 1, |_| true);
        assert!(second.iter().any(|c| c.2));
        assert!(st.is_granted(&id));
    }

    #[test]
    fn already_granted_no_double_progress() {
        let mut r = reg();
        let adv = Advancement::builder(RegistryKey::new("mymod", "once"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .build();
        r.register(adv).unwrap();
        let id = RegistryKey::new("mymod", "once");
        let mut st = PlayerAdvancementState::new();
        st.grant_progress(&r, &id, "a", 1);
        let (done, granted) = st.grant_progress(&r, &id, "a", 1);
        assert!(!done);
        assert!(!granted);
        assert_eq!(st.criterion_progress(&id, "a"), 1);
    }

    #[test]
    fn parent_and_rewards_stored() {
        let mut r = reg();
        let child = Advancement::builder(RegistryKey::new("mymod", "child"))
            .parent(RegistryKey::new("mymod", "root"))
            .criterion(AdvancementCriterion::new("a", "t:a"))
            .rewards(
                AdvancementRewards::new()
                    .experience(100)
                    .loot(RegistryKey::new("mymod", "chest")),
            )
            .build();
        r.register(child).unwrap();
        let got = r.get(&RegistryKey::new("mymod", "child")).unwrap();
        assert_eq!(got.parent, Some(RegistryKey::new("mymod", "root")));
        assert_eq!(got.rewards.experience, 100);
        assert_eq!(
            got.rewards.loot_tables,
            vec![RegistryKey::new("mymod", "chest")]
        );
    }
}
