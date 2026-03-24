use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fastarena::{Arena, ArenaVec};
use std::hint::black_box;

fn bench_alloc(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc");

    g.bench_function("alloc u64 x1000", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(1000 * 16);
            for i in 0u64..1000 {
                black_box(a.alloc(black_box(i)));
            }
            a.reset();
        });
    });

    g.bench_function("Box::new u64 x1000", |b| {
        b.iter(|| {
            let v: Vec<Box<u64>> = (0u64..1000).map(|i| Box::new(black_box(i))).collect();
            black_box(v);
        });
    });

    g.finish();
}

fn bench_alloc_slice(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_slice");

    for n in [8usize, 64, 512, 4096] {
        g.throughput(Throughput::Elements(n as u64));

        g.bench_with_input(BenchmarkId::new("arena alloc_slice", n), &n, |b, &n| {
            b.iter(|| {
                let mut a = Arena::with_capacity(n * 8 * 4);
                black_box(a.alloc_slice(0u32..n as u32));
            });
        });

        g.bench_with_input(BenchmarkId::new("Vec collect", n), &n, |b, &n| {
            b.iter(|| {
                black_box((0u32..n as u32).collect::<Vec<_>>());
            });
        });
    }

    g.finish();
}

fn bench_alloc_slice_copy(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_slice_copy");

    for n in [8usize, 64, 512, 4096] {
        g.throughput(Throughput::Elements(n as u64));
        let src: Vec<u32> = (0..n as u32).collect();

        g.bench_with_input(BenchmarkId::new("arena alloc_slice_copy", n), &n, |b, _| {
            b.iter(|| {
                let mut a = Arena::with_capacity(n * 8 * 4);
                black_box(a.alloc_slice_copy(black_box(&src)));
            });
        });
    }

    g.finish();
}

fn bench_checkpoint_rewind(c: &mut Criterion) {
    let mut g = c.benchmark_group("checkpoint_rewind");

    g.bench_function("checkpoint x100", |b| {
        b.iter(|| {
            let mut a = Arena::new();
            let _ = a.alloc(0u64);
            for _ in 0..100 {
                let _ = black_box(a.checkpoint());
            }
        });
    });

    g.bench_function("rewind same-block x100", |b| {
        b.iter(|| {
            let mut a = Arena::new();
            for _ in 0..100 {
                let cp = a.checkpoint();
                let _ = a.alloc(black_box(1u64));
                a.rewind(cp);
            }
        });
    });

    g.bench_function("rewind multi-block x100", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(64);
            for _ in 0..100 {
                let cp = a.checkpoint();
                for _ in 0..32 {
                    let _ = a.alloc(black_box(0u64));
                }
                a.rewind(cp);
            }
        });
    });

    g.finish();
}

fn bench_reset(c: &mut Criterion) {
    let mut g = c.benchmark_group("reset");

    for blocks in [1usize, 4, 8] {
        g.bench_with_input(
            BenchmarkId::new("reset", format!("{blocks} blocks")),
            &blocks,
            |b, &blocks| {
                b.iter(|| {
                    let mut a = Arena::with_capacity(64);
                    for _ in 0..(blocks * 8) {
                        let _ = a.alloc(0u64);
                    }
                    a.reset();
                    for _ in 0..(blocks * 8) {
                        let _ = a.alloc(black_box(0u64));
                    }
                    a.reset();
                });
            },
        );
    }

    g.finish();
}

fn bench_transaction(c: &mut Criterion) {
    let mut g = c.benchmark_group("transaction");

    g.bench_function("commit empty x100", |b| {
        b.iter(|| {
            let mut a = Arena::new();
            for _ in 0..100 {
                let _ = black_box(a.transaction().commit());
            }
        });
    });

    g.bench_function("commit 16 allocs x100", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(100 * 16 * 16);
            for _ in 0..100 {
                let mut t = a.transaction();
                for _ in 0..16 {
                    let _ = t.alloc(black_box(0u64));
                }
                let _ = black_box(t.commit());
            }
            a.reset();
        });
    });

    g.bench_function("rollback 16 allocs x100", |b| {
        b.iter(|| {
            let mut a = Arena::new();
            for _ in 0..100 {
                let mut t = a.transaction();
                for _ in 0..16 {
                    let _ = t.alloc(black_box(0u64));
                }
                drop(t);
            }
        });
    });

    g.bench_function("with_transaction Ok x100", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(100 * 16 * 16);
            for _ in 0..100 {
                let _ = black_box(a.with_transaction(|t| -> Result<u64, ()> {
                    Ok(*t.alloc(black_box(21u64)) * 2)
                }));
            }
            a.reset();
        });
    });

    g.bench_function("with_transaction Err x100", |b| {
        b.iter(|| {
            let mut a = Arena::new();
            for _ in 0..100 {
                let _ = black_box(a.with_transaction(|t| -> Result<(), &str> {
                    let _ = t.alloc(black_box(0u64));
                    Err("fail")
                }));
            }
        });
    });

    g.finish();
}

fn bench_arena_vec(c: &mut Criterion) {
    let mut g = c.benchmark_group("ArenaVec");

    for n in [16usize, 256, 4096] {
        g.throughput(Throughput::Elements(n as u64));

        g.bench_with_input(BenchmarkId::new("ArenaVec push+finish", n), &n, |b, &n| {
            b.iter(|| {
                let mut a = Arena::with_capacity(n * 8 * 4);
                let s = {
                    let mut v = ArenaVec::<u64>::with_capacity(&mut a, n);
                    for i in 0..n {
                        v.push(i as u64);
                    }
                    v.finish()
                };
                black_box(s);
            });
        });

        g.bench_with_input(BenchmarkId::new("Vec with_cap push", n), &n, |b, &n| {
            b.iter(|| {
                let mut v: Vec<u64> = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(black_box(i as u64));
                }
                black_box(v);
            });
        });
    }

    g.finish();
}

fn bench_throughput(c: &mut Criterion) {
    let mut g = c.benchmark_group("throughput");

    g.throughput(Throughput::Elements(10_000));

    g.bench_function("arena 10k u64 + reset", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(10_000 * 16);
            for i in 0u64..10_000 {
                let _ = a.alloc(black_box(i));
            }
            a.reset();
        });
    });

    g.bench_function("Box 10k u64 + drop", |b| {
        b.iter(|| {
            black_box(
                (0u64..10_000)
                    .map(|i| Box::new(black_box(i)))
                    .collect::<Vec<_>>(),
            );
        });
    });

    g.finish();
}

fn bench_new(c: &mut Criterion) {
    c.bench_function("Arena::new", |b| {
        b.iter(|| {
            black_box(Arena::new());
        });
    });
}

criterion_group!(
    benches,
    bench_alloc,
    bench_alloc_slice,
    bench_alloc_slice_copy,
    bench_checkpoint_rewind,
    bench_reset,
    bench_transaction,
    bench_arena_vec,
    bench_throughput,
    bench_new,
);

criterion_main!(benches);
