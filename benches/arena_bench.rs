use fastarena::{Arena, ArenaVec};
use std::hint::black_box;
use std::time::Instant;

fn measure(label: &str, iters: u64, mut f: impl FnMut()) {
    for _ in 0..iters / 10 {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ns = start.elapsed().as_nanos() as f64 / iters as f64;
    println!("  {label:<48} {ns:>8.1} ns/iter");
}

fn main() {
    const N: u64 = 200_000;

    println!("\n=== alloc<T> ===");
    measure("arena alloc u64", N, || {
        let mut a = Arena::with_capacity(N as usize * 16);
        for i in 0u64..1000 {
            black_box(a.alloc(black_box(i)));
        }
        a.reset();
    });
    measure("Box::new u64 x1000", N / 10, || {
        let v: Vec<Box<u64>> = (0u64..1000).map(|i| Box::new(black_box(i))).collect();
        black_box(v);
    });

    println!("\n=== alloc_slice ===");
    for n in [8usize, 64, 512, 4096] {
        measure(
            &format!("arena alloc_slice n={n}"),
            N / (n as u64 / 8 + 1),
            || {
                let mut a = Arena::with_capacity(n * 8 * 4);
                black_box(a.alloc_slice(0u32..n as u32));
            },
        );
        measure(
            &format!("Vec collect       n={n}"),
            N / (n as u64 / 8 + 1),
            || {
                black_box((0u32..n as u32).collect::<Vec<_>>());
            },
        );
    }

    println!("\n=== checkpoint / rewind ===");
    measure("checkpoint()", N, || {
        let mut a = Arena::new();
        let _ = a.alloc(0u64);
        for _ in 0..100 {
            black_box(a.checkpoint());
        }
    });
    measure("rewind same-block", N, || {
        let mut a = Arena::new();
        for _ in 0..100 {
            let cp = a.checkpoint();
            let _ = a.alloc(black_box(1u64));
            a.rewind(cp);
        }
    });
    measure("rewind multi-block (4)", N / 10, || {
        let mut a = Arena::with_capacity(64);
        for _ in 0..100 {
            let cp = a.checkpoint();
            for _ in 0..32 {
                let _ = a.alloc(black_box(0u64));
            }
            a.rewind(cp);
        }
    });

    println!("\n=== reset ===");
    for blocks in [1usize, 4, 8] {
        measure(&format!("reset {blocks} block(s)"), N / 100, || {
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
    }

    println!("\n=== transaction ===");
    measure("commit empty", N, || {
        let mut a = Arena::new();
        for _ in 0..100 {
            black_box(a.transaction().commit());
        }
    });
    measure("commit 16 allocs", N / 10, || {
        let mut a = Arena::with_capacity(N as usize * 16);
        for _ in 0..100 {
            let mut t = a.transaction();
            for _ in 0..16 {
                let _ = t.alloc(black_box(0u64));
            }
            black_box(t.commit());
        }
        a.reset();
    });
    measure("rollback 16 allocs", N / 10, || {
        let mut a = Arena::new();
        for _ in 0..100 {
            let mut t = a.transaction();
            for _ in 0..16 {
                let _ = t.alloc(black_box(0u64));
            }
            drop(t);
        }
    });
    measure("with_transaction Ok", N, || {
        let mut a = Arena::with_capacity(N as usize * 16);
        for _ in 0..100 {
            let _ =
                black_box(a.with_transaction(|t| -> Result<u64, ()> {
                    Ok(*t.alloc(black_box(21u64)) * 2)
                }));
        }
        a.reset();
    });
    measure("with_transaction Err (rollback)", N, || {
        let mut a = Arena::new();
        for _ in 0..100 {
            let _ = black_box(a.with_transaction(|t| -> Result<(), &str> {
                let _ = t.alloc(black_box(0u64));
                Err("fail")
            }));
        }
    });

    println!("\n=== ArenaVec ===");
    for n in [16usize, 256, 4096] {
        measure(
            &format!("ArenaVec push+finish n={n}"),
            N / (n as u64 / 16 + 1),
            || {
                let mut a = Arena::with_capacity(n * 8 * 4);
                let s = {
                    let mut v = ArenaVec::<u64>::with_capacity(&mut a, n);
                    for i in 0..n {
                        v.push(i as u64);
                    }
                    v.finish()
                };
                black_box(s);
            },
        );
        measure(
            &format!("Vec with_cap push    n={n}"),
            N / (n as u64 / 16 + 1),
            || {
                let mut v: Vec<u64> = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(black_box(i as u64));
                }
                black_box(v);
            },
        );
    }

    println!("\n=== Arena::new ===");
    measure("Arena::new (no malloc for blocks)", N, || {
        black_box(Arena::new());
    });

    println!("\n=== throughput 10k allocs ===");
    measure("arena 10k u64 + reset", N / 100, || {
        let mut a = Arena::with_capacity(10_000 * 16);
        for i in 0u64..10_000 {
            let _ = a.alloc(black_box(i));
        }
        a.reset();
    });
    measure("Box  10k u64 + drop ", N / 1000, || {
        black_box(
            (0u64..10_000)
                .map(|i| Box::new(black_box(i)))
                .collect::<Vec<_>>(),
        );
    });

    println!();
}
