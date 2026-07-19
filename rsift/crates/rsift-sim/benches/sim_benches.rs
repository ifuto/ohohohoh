use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rsift_sim::block_tick::{RngLite, Xoroshiro64};
use rsift_sim::lighting::{NibbleArray, StarlightEngine, SEC_VOL};
use rsift_sim::network::{read_varint, write_varint};
use rsift_sim::nbt::RegistryU32;
use rsift_sim::worldgen::value_noise_2d;
use rsift_sim::entity::bfs_path;

fn bench_varint(c: &mut Criterion) {
    c.bench_function("varint_roundtrip", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            write_varint(black_box(30000), &mut buf);
            let (v, _) = read_varint(&buf).unwrap();
            black_box(v);
        })
    });
}

fn bench_noise(c: &mut Criterion) {
    c.bench_function("value_noise_2d", |b| {
        b.iter(|| black_box(value_noise_2d(12.3, 45.6, 99)))
    });
}

fn bench_light(c: &mut Criterion) {
    c.bench_function("starlight_propagate", |b| {
        b.iter(|| {
            let mut eng = StarlightEngine::new();
            eng.set_block_light(0, 0, 0, 15);
            let opaque = [false; SEC_VOL];
            black_box(eng.propagate_dirty(0, 0, 0, &opaque, 4096));
        })
    });
}

fn bench_path(c: &mut Criterion) {
    c.bench_function("bfs_path_small", |b| {
        b.iter(|| {
            let path = bfs_path(|x, y, z| y == 0 && x.abs() < 20 && z.abs() < 20, (0, 0, 0), (10, 0, 10), 5000);
            black_box(path);
        })
    });
}

fn bench_registry(c: &mut Criterion) {
    c.bench_function("registry_u32", |b| {
        b.iter(|| {
            let mut r = RegistryU32::default();
            for i in 0..256 {
                black_box(r.register(format!("minecraft:block_{i}")));
            }
        })
    });
}

fn bench_nibble(c: &mut Criterion) {
    c.bench_function("nibble_set_get", |b| {
        b.iter(|| {
            let mut n = NibbleArray::new_zero();
            for i in 0..4096 {
                n.set(i, (i % 16) as u8);
                black_box(n.get(i));
            }
        })
    });
}

fn bench_rng(c: &mut Criterion) {
    c.bench_function("xoroshiro", |b| {
        let mut rng = Xoroshiro64::new(1);
        b.iter(|| black_box(rng.next_u32()))
    });
}

criterion_group!(
    benches,
    bench_varint,
    bench_noise,
    bench_light,
    bench_path,
    bench_registry,
    bench_nibble,
    bench_rng
);
criterion_main!(benches);
