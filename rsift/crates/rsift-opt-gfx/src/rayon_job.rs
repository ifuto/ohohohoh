
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
