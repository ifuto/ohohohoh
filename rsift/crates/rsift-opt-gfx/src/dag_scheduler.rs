
//! DAG Job System - parse->light->ao->mesh->upload依存グラフをトポロジカルに並列実行

use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u32);

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub deps: Vec<TaskId>,
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
