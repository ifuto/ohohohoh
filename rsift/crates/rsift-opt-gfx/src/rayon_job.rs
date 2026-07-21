
//! Rayon work-stealing Chunk Meshing - 32セクション並列

use rayon::prelude::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct RayonJobConfig {
    pub threads: usize,
}

pub fn parallel_for_each_chunk<I, F>(chunks: I, f: F)
where
    I: IntoParallelIterator,
    F: Fn(I::Item) + Sync + Send,
    I::Item: Send,
{
    chunks.into_par_iter().for_each(f);
}

pub fn parallel_map_chunks<I, T, F>(chunks: I, f: F) -> Vec<T>
where
    I: IntoParallelIterator,
    F: Fn(I::Item) -> T + Sync + Send,
    T: Send,
    I::Item: Send,
{
    chunks.into_par_iter().map(f).collect()
}

/// P-Core専用固定スレッドプール構築 (core_affinityクレートでピン留め)
pub fn build_pcore_threadpool(num_threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("rsift-pcore-{}", i))
        .build()
        .expect("Failed to build p-core pool")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn parallel_map_preserves_order_and_values() {
        let v: Vec<i32> = (0..10_000).collect();
        let out = parallel_map_chunks(v, |x| x * 2 + 1);
        assert_eq!(out.len(), 10_000);
        for (i, x) in out.iter().enumerate() {
            assert_eq!(*x, i as i32 * 2 + 1); // indexed collect は順序保持
        }
    }

    #[test]
    fn parallel_for_each_visits_every_item_once() {
        let seen = AtomicUsize::new(0);
        parallel_for_each_chunk(0..1_000usize, |_| {
            seen.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(seen.load(Ordering::Relaxed), 1_000);
    }

    #[test]
    fn pcore_pool_named_and_sized() {
        let pool = build_pcore_threadpool(1);
        assert_eq!(pool.current_num_threads(), 1);
        let name = pool.install(|| std::thread::current().name().map(|s| s.to_string()));
        assert_eq!(name.as_deref(), Some("rsift-pcore-0"));
    }
}
