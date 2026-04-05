use bumpalo::Bump;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fastarena::{Arena, ArenaVec};
use std::hint::black_box;

fn bench_alloc_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc 1k items");
    g.throughput(Throughput::Elements(1000));

    g.bench_function("fastarena", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(1000 * 16);
            for i in 0u64..1000 {
                black_box(a.alloc(black_box(i)));
            }
        });
    });

    g.bench_function("bumpalo", |b| {
        b.iter(|| {
            let a = Bump::with_capacity(1000 * 16);
            for i in 0u64..1000 {
                black_box(a.alloc(black_box(i)));
            }
        });
    });

    g.bench_function("typed-arena", |b| {
        b.iter(|| {
            let a = typed_arena::Arena::new();
            for i in 0u64..1000 {
                black_box(a.alloc(black_box(i)));
            }
        });
    });

    g.finish();
}

fn bench_alloc_slice_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_slice");

    for n in [64usize, 1024] {
        g.throughput(Throughput::Elements(n as u64));

        g.bench_with_input(BenchmarkId::new("fastarena", n), &n, |b, &n| {
            b.iter(|| {
                let mut a = Arena::with_capacity(n * 8 * 4);
                black_box(a.alloc_slice(0u32..n as u32));
            });
        });

        g.bench_with_input(BenchmarkId::new("bumpalo", n), &n, |b, &n| {
            b.iter(|| {
                let a = Bump::with_capacity(n * 8 * 4);
                black_box(a.alloc_slice_fill_with(n, |_| black_box(0u32)));
            });
        });

        if n <= 64 {
            g.bench_with_input(BenchmarkId::new("typed-arena", n), &n, |b, &n| {
                b.iter(|| {
                    let a = typed_arena::Arena::new();
                    black_box((0u32..n as u32).map(|i| a.alloc(i)).collect::<Vec<_>>());
                });
            });
        }
    }

    g.finish();
}

fn bench_string_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_str x100");
    g.throughput(Throughput::Elements(100));

    // Reuse one arena / bump per sample: 100 copies then reset — matches
    // request-scoped “many short strings per cycle” without `Block::new` each iter.
    g.bench_function("fastarena", |b| {
        let mut a = Arena::with_capacity(1024);
        b.iter(|| {
            for _ in 0..100 {
                black_box(a.alloc_str("hello world this is a test string"));
            }
            a.reset();
        });
    });

    g.bench_function("bumpalo", |b| {
        let mut bump = Bump::with_capacity(1024);
        b.iter(|| {
            for _ in 0..100 {
                black_box(bump.alloc_str("hello world this is a test string"));
            }
            bump.reset();
        });
    });

    g.finish();
}

fn bench_reset_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("reset x1000 allocs");

    g.bench_function("fastarena (1 block)", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(64 * 1024);
            for _ in 0..1000 {
                let _ = a.alloc(0u64);
            }
            a.reset();
        });
    });

    g.bench_function("fastarena (4 blocks)", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(64);
            for _ in 0..4000 {
                let _ = a.alloc(0u64);
            }
            a.reset();
        });
    });

    g.bench_function("bumpalo", |b| {
        b.iter(|| {
            let mut a = Bump::with_capacity(64 * 1024);
            for _ in 0..1000 {
                let _ = a.alloc(0u64);
            }
            a.reset();
        });
    });

    g.bench_function("typed-arena (drop+new)", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let a = typed_arena::Arena::new();
                let _ = a.alloc(0u64);
            }
        });
    });

    g.finish();
}

fn bench_arena_vec_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("ArenaVec");

    for n in [16usize, 256, 4096] {
        g.throughput(Throughput::Elements(n as u64));

        g.bench_with_input(BenchmarkId::new("fastarena ArenaVec", n), &n, |b, &n| {
            b.iter(|| {
                let mut a = Arena::with_capacity(n * 16);
                let s = {
                    let mut v = ArenaVec::<u64>::with_capacity(&mut a, n);
                    for i in 0..n {
                        v.push(black_box(i as u64));
                    }
                    v.finish()
                };
                black_box(s);
            });
        });

        g.bench_with_input(BenchmarkId::new("bumpalo Vec", n), &n, |b, &n| {
            b.iter(|| {
                let a = Bump::with_capacity(n * 16);
                let v: Vec<_> = (0u64..n as u64).map(|i| black_box(a.alloc(i))).collect();
                black_box(v);
            });
        });

        g.bench_with_input(BenchmarkId::new("typed-arena Vec", n), &n, |b, &n| {
            b.iter(|| {
                let a = typed_arena::Arena::new();
                let v: Vec<_> = (0u64..n as u64).map(|i| a.alloc(black_box(i))).collect();
                black_box(v);
            });
        });
    }

    g.finish();
}

fn bench_throughput_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("throughput 10k");
    g.throughput(Throughput::Elements(10_000));

    g.bench_function("fastarena + reset", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(10_000 * 16);
            for i in 0u64..10_000 {
                let _ = a.alloc(black_box(i));
            }
            a.reset();
        });
    });

    g.bench_function("bumpalo + reset", |b| {
        b.iter(|| {
            let mut a = Bump::with_capacity(10_000 * 16);
            for i in 0u64..10_000 {
                let _ = a.alloc(black_box(i));
            }
            a.reset();
        });
    });

    g.bench_function("typed-arena + drop", |b| {
        b.iter(|| {
            for i in 0u64..10_000 {
                let a = typed_arena::Arena::new();
                let _ = a.alloc(black_box(i));
            }
        });
    });

    g.finish();
}

fn bench_large_alloc_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("large alloc 128KB");
    g.throughput(Throughput::Bytes(128 * 1024));

    g.bench_function("fastarena", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(64 * 1024);
            black_box(a.alloc_raw(128 * 1024, 1));
        });
    });

    g.bench_function("bumpalo", |b| {
        b.iter(|| {
            let a = Bump::with_capacity(64 * 1024);
            black_box(a.alloc(128 * 1024));
        });
    });

    g.finish();
}

fn bench_nested_depth_comparison(c: &mut Criterion) {
    let mut g = c.benchmark_group("nested depth 10");
    g.throughput(Throughput::Elements(1000));

    g.bench_function("fastarena", |b| {
        b.iter(|| {
            let mut a = Arena::with_capacity(64 * 1024);
            fn alloc_depth(a: &mut Arena, depth: usize) {
                if depth == 0 {
                    return;
                }
                for _ in 0..100 {
                    black_box(a.alloc(0u64));
                }
                alloc_depth(a, depth - 1);
            }
            alloc_depth(&mut a, 10);
            a.reset();
        });
    });

    g.bench_function("bumpalo", |b| {
        b.iter(|| {
            let a = Bump::with_capacity(64 * 1024);
            fn alloc_depth(a: &Bump, depth: usize) {
                if depth == 0 {
                    return;
                }
                for _ in 0..100 {
                    black_box(a.alloc(0u64));
                }
                alloc_depth(a, depth - 1);
            }
            alloc_depth(&a, 10);
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_alloc_comparison,
    bench_alloc_slice_comparison,
    bench_string_comparison,
    bench_reset_comparison,
    bench_arena_vec_comparison,
    bench_throughput_comparison,
    bench_large_alloc_comparison,
    bench_nested_depth_comparison,
);

criterion_main!(benches);
