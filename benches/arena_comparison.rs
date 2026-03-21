use bumpalo::Bump;
use fastarena::{Arena, ArenaVec};
use std::hint::black_box;
use std::time::Instant;

fn measure(label: &str, iters: u64, mut f: impl FnMut()) {
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ns = start.elapsed().as_nanos() as f64 / iters as f64;
    println!("  {label:<40} {ns:>8.1} ns/iter");
}

fn main() {
    const N: u64 = 100_000;

    println!("\n=== alloc<T> (1k allocs) ===");
    measure("fastarena", N, || {
        let mut a = Arena::with_capacity(1_000 * 16);
        for i in 0u64..1000 {
            black_box(a.alloc(black_box(i)));
        }
    });
    measure("bumpalo", N, || {
        let a = Bump::with_capacity(1_000 * 16);
        for i in 0u64..1000 {
            black_box(a.alloc(black_box(i)));
        }
    });
    measure("typed-arena", N, || {
        let a = typed_arena::Arena::new();
        for i in 0u64..1000 {
            black_box(a.alloc(black_box(i)));
        }
    });

    println!("\n=== alloc_slice n=64 ===");
    measure("fastarena", N, || {
        let mut a = Arena::with_capacity(1024);
        black_box(a.alloc_slice(0u32..64));
    });
    measure("bumpalo", N, || {
        let a = Bump::with_capacity(1024);
        black_box(a.alloc_slice_fill_with(64, |_| black_box(0u32)));
    });
    measure("typed-arena", N, || {
        let a = typed_arena::Arena::new();
        black_box((0u32..64).map(|i| a.alloc(i)).collect::<Vec<_>>());
    });

    println!("\n=== alloc_slice n=1024 ===");
    measure("fastarena", N / 4, || {
        let mut a = Arena::with_capacity(8 * 1024);
        black_box(a.alloc_slice(0u32..1024));
    });
    measure("bumpalo", N / 4, || {
        let a = Bump::with_capacity(8 * 1024);
        black_box(a.alloc_slice_fill_with(1024, |_| black_box(0u32)));
    });

    println!("\n=== string allocation ===");
    measure("fastarena", N, || {
        let mut a = Arena::with_capacity(1024);
        for _ in 0..100 {
            black_box(a.alloc_str("hello world this is a test string"));
        }
    });
    measure("bumpalo", N, || {
        let a = Bump::with_capacity(1024);
        for _ in 0..100 {
            black_box(a.alloc_str("hello world this is a test string"));
        }
    });

    println!("\n=== reset/clear ===");
    measure("fastarena reset (1 block)", N / 10, || {
        let mut a = Arena::with_capacity(64 * 1024);
        for _ in 0..1000 {
            let _ = a.alloc(0u64);
        }
        a.reset();
    });
    measure("fastarena reset (4 blocks)", N / 10, || {
        let mut a = Arena::with_capacity(64);
        for _ in 0..4000 {
            let _ = a.alloc(0u64);
        }
        a.reset();
    });
    measure("bumpalo reset", N / 10, || {
        let mut a = Bump::with_capacity(64 * 1024);
        for _ in 0..1000 {
            let _ = a.alloc(0u64);
        }
        a.reset();
    });
    measure("typed-arena drop", N / 10, || {
        for _ in 0..1000 {
            let a = typed_arena::Arena::new();
            let _ = a.alloc(0u64);
        }
    });

    println!("\n=== ArenaVec vs Vec ===");
    for n in [16usize, 256, 4096] {
        measure(
            &format!("fastarena ArenaVec n={n}"),
            N / (n as u64 / 16 + 1),
            || {
                let mut a = Arena::with_capacity(n * 16);
                let s = {
                    let mut v = ArenaVec::<u64>::with_capacity(&mut a, n);
                    for i in 0..n {
                        v.push(black_box(i as u64));
                    }
                    v.finish()
                };
                black_box(s);
            },
        );
        measure(
            &format!("bumpalo Vec              n={n}"),
            N / (n as u64 / 16 + 1),
            || {
                let a = Bump::with_capacity(n * 16);
                let v: Vec<_> = (0u64..n as u64).map(|i| black_box(a.alloc(i))).collect();
                black_box(v);
            },
        );
        measure(
            &format!("typed-arena Vec          n={n}"),
            N / (n as u64 / 16 + 1),
            || {
                let a = typed_arena::Arena::new();
                let v: Vec<_> = (0u64..n as u64).map(|i| a.alloc(black_box(i))).collect();
                black_box(v);
            },
        );
    }

    println!("\n=== throughput 10k allocs ===");
    measure("fastarena + reset", N / 100, || {
        let mut a = Arena::with_capacity(10_000 * 16);
        for i in 0u64..10_000 {
            let _ = a.alloc(black_box(i));
        }
        a.reset();
    });
    measure("bumpalo + reset", N / 100, || {
        let mut a = Bump::with_capacity(10_000 * 16);
        for i in 0u64..10_000 {
            let _ = a.alloc(black_box(i));
        }
        a.reset();
    });
    measure("typed-arena + drop", N / 100, || {
        for i in 0u64..10_000 {
            let a = typed_arena::Arena::new();
            let _ = a.alloc(black_box(i));
        }
    });

    println!("\n=== large allocation (> 64KB block) ===");
    measure("fastarena 128KB alloc", N / 10, || {
        let mut a = Arena::with_capacity(64 * 1024);
        black_box(a.alloc_raw(128 * 1024, 1));
    });
    measure("bumpalo 128KB alloc", N / 10, || {
        let a = Bump::with_capacity(64 * 1024);
        black_box(a.alloc(128 * 1024));
    });

    println!("\n=== nested/allocation depth ===");
    measure("fastarena depth 10", N / 10, || {
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
    measure("bumpalo depth 10", N / 10, || {
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

    println!();
}
