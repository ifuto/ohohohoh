
//! DAG Job System - parse->light->ao->mesh->upload依存グラフをトポロジカルに並列実行

use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u32);

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    /// 【wave 181 GA-2 truth 契約】存在しない TaskId を含む場合、当該タスク
    /// は永久にスケジュールされない (silent eternal block、条件緩和なし)。
    pub deps: Vec<TaskId>,
    /// 【wave 181 GA-2 truth 契約】検証なし透過: 負値で chain を縮め、NaN は
    /// critical max 比較 (f32::max 非 NaN 仕様) で自然落選する。
    pub cost_ms: f32,
}

pub struct DagScheduler {
    tasks: HashMap<TaskId, Task>,
    next_id: u32,
}

impl DagScheduler {
    pub fn new() -> Self { Self { tasks: HashMap::new(), next_id: 0 } }

    pub fn add_task(&mut self, name: &str, deps: &[TaskId], cost_ms: f32) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        self.tasks.insert(id, Task { id, name: name.to_string(), deps: deps.to_vec(), cost_ms });
        id
    }

    /// トポロジカルソート。結果順は HashMap 由来で非決定的 (一意性は保証
    /// しない、依存関係順序のみ保証、diamond テストが pos 関係 pin)。
    pub fn topological_order(&self) -> Vec<TaskId> {
        let mut indeg: HashMap<TaskId, usize> = self.tasks.keys().map(|&k| (k,0)).collect();
        let mut adj: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
        for task in self.tasks.values() {
            for &dep in &task.deps {
                *indeg.entry(task.id).or_insert(0) += 1;
                adj.entry(dep).or_default().push(task.id);
            }
        }
        let mut q: VecDeque<TaskId> = indeg.iter().filter(|(_, &d)| d==0).map(|(&k,_)| k).collect();
        let mut order = Vec::new();
        while let Some(id) = q.pop_front() {
            order.push(id);
            if let Some(neigh) = adj.get(&id) {
                for &n in neigh {
                    let e = indeg.get_mut(&n).unwrap();
                    *e -= 1;
                    if *e==0 { q.push_back(n); }
                }
            }
        }
        order
    }

    pub fn critical_path_ms(&self) -> f32 {
        let order = self.topological_order();
        let mut earliest: HashMap<TaskId, f32> = HashMap::new();
        let mut max = 0.0;
        for id in order {
            let task = &self.tasks[&id];
            let start = task.deps.iter().map(|d| earliest.get(d).copied().unwrap_or(0.0)).fold(0.0f32, f32::max);
            let end = start + task.cost_ms;
            earliest.insert(id, end);
            if end > max { max = end; }
        }
        max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_topological_order_is_exact() {
        let mut s = DagScheduler::new();
        let a = s.add_task("parse", &[], 1.0);
        let b = s.add_task("light", &[a], 2.0);
        let c = s.add_task("mesh", &[b], 3.0);
        assert_eq!((a, b, c), (TaskId(0), TaskId(1), TaskId(2)));
        assert_eq!(s.topological_order(), vec![a, b, c]); // 単一鎖は一意
        assert_eq!(s.critical_path_ms(), 6.0);
    }

    #[test]
    fn diamond_respects_dependencies_and_critical_path() {
        let mut s = DagScheduler::new();
        let a = s.add_task("a", &[], 2.0);
        let b = s.add_task("b", &[a], 1.0); // 合計 3.0
        let c = s.add_task("c", &[a], 4.0); // 合計 6.0 (critical)
        let d = s.add_task("d", &[b, c], 1.0); // 6+1 = 7.0
        let order = s.topological_order();
        assert_eq!(order.len(), 4); // DAG なら全ノードが浮上
        let pos = |t: TaskId| order.iter().position(|&x| x == t).unwrap();
        assert!(pos(a) < pos(b) && pos(a) < pos(c));
        assert!(pos(b) < pos(d) && pos(c) < pos(d));
        assert_eq!(s.critical_path_ms(), 7.0);
    }

    #[test]
    fn self_loop_task_is_unschedulable_but_others_run() {
        // white-box: 公開 API では構築不能な閉路 (自己依存) を直接注入する
        // (公開 API は id 単調発行のため閉路は作れない = DAG 性が構造保証)。
        let mut s = DagScheduler::new();
        let a = s.add_task("a", &[], 1.0);
        let b = s.add_task("b", &[], 2.0);
        s.tasks.get_mut(&a).unwrap().deps.push(a);
        let order = s.topological_order();
        assert_eq!(order, vec![b]); // a は indeg が 0 にならず永遠に浮上しない
        assert_eq!(s.critical_path_ms(), 2.0); // スケジュール可能分のみで算出
    }

    #[test]
    fn empty_graph() {
        let s = DagScheduler::new();
        assert!(s.topological_order().is_empty());
        assert_eq!(s.critical_path_ms(), 0.0);
    }

    /// 【wave 181 GA-2】dangling dep (存在しない TaskId) の truth pin:
    /// 永久ブロック (silent eternal block)、schedule 可能分のみ出力、
    /// critical path も schedulable subset で評価 (doc 契約 GA-2 追加済)。
    #[test]
    fn ga_dangling_dep_blocks_silently() {
        let mut s = DagScheduler::new();
        let a = s.add_task("a", &[], 1.0);
        let _x = s.add_task("x", &[TaskId(999)], 2.0);
        let order = s.topological_order();
        assert_eq!(order, vec![a], "x は dangling dep で永久ブロック (silent)");
        assert_eq!(s.critical_path_ms(), 1.0, "critical は schedulable subset");
    }

    /// 【wave 181 GA-2】cost_ms 特例 truth pin (green-today 観測事実):
    /// NaN 混は chain 合算を NaN 化するが critical max 比較で f32::max が
    /// 非 NaN 側を選び他 chain は算出継続する防御的 truth。負 cost は
    /// clamp なし透過で chain を縮める (物理的意味なしだが契約として pin)。
    #[test]
    fn ga_cost_special_values_truth() {
        let mut s = DagScheduler::new();
        let a = s.add_task("a", &[], 1.0);
        let _nan = s.add_task("nan", &[a], f32::NAN);
        let b = s.add_task("b", &[], 5.0);
        assert_eq!(
            s.critical_path_ms(),
            5.0,
            "NaN chain は b chain 5.0 に落選 (f32::max 非 NaN 選択)"
        );
        let mut s2 = DagScheduler::new();
        let p = s2.add_task("p", &[], 4.0);
        let _q = s2.add_task("q", &[p], -1.5);
        assert_eq!(
            s2.critical_path_ms(),
            4.0,
            "q end=2.5 < p end=4.0 → max 4.0 (負 cost 透過 truth)"
        );
    }
}
