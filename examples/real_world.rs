//! Real-world usage patterns: compilers, LSM trees, request-scoped workloads.

use fastarena::{Arena, ArenaVec};

// --- Compiler speculative optimization pass ---
fn compiler_example() {
    #[derive(Debug, PartialEq)]
    enum Op {
        Const(i64),
        Add,
        Mul,
        Nop,
    }

    let mut arena = Arena::new();

    let _base_nodes = arena.with_transaction_infallible(|txn| {
        let a = txn.alloc(Op::Const(10)) as *mut Op;
        let b = txn.alloc(Op::Const(20)) as *mut Op;
        let c = txn.alloc(Op::Add) as *mut Op;
        (a, b, c)
    });
    let baseline = arena.stats().bytes_allocated;

    // Speculative pass — try optimizations, keep only if profitable
    {
        let mut txn = arena.transaction();
        for _ in 0..100 {
            txn.alloc(Op::Nop);
        }
        // dropped — speculative work discarded
    }
    assert_eq!(arena.stats().bytes_allocated, baseline);

    // Apply real optimization
    arena
        .with_transaction(|txn| -> Result<(), ()> {
            txn.alloc(Op::Mul);
            Ok(())
        })
        .unwrap();

    println!(
        "compiler: baseline={}, final={}",
        baseline,
        arena.stats().bytes_allocated
    );
}

// --- LSM batch insert with abort ---
fn lsm_example() {
    #[allow(dead_code)]
    #[derive(Debug)]
    struct KvEntry {
        key_ptr: *const u8,
        key_len: usize,
        seq: u64,
    }

    let mut arena = Arena::new();

    // Committed batch
    arena
        .with_transaction(|txn| -> Result<(), &str> {
            let k1 = txn.alloc_str("apple") as *const str;
            txn.alloc(KvEntry {
                key_ptr: k1 as *const u8,
                key_len: 5,
                seq: 1,
            });
            let k2 = txn.alloc_str("banana") as *const str;
            txn.alloc(KvEntry {
                key_ptr: k2 as *const u8,
                key_len: 6,
                seq: 2,
            });
            Ok(())
        })
        .unwrap();
    let committed = arena.stats().bytes_allocated;

    // Aborted batch — no trace left
    let _ = arena.with_transaction(|txn| -> Result<(), &str> {
        let k = txn.alloc_str("cherry") as *const str;
        txn.alloc(KvEntry {
            key_ptr: k as *const u8,
            key_len: 6,
            seq: 3,
        });
        Err("duplicate key")
    });

    assert_eq!(arena.stats().bytes_allocated, committed);
    println!("LSM: committed bytes preserved after abort");
}

// --- Request-scoped allocator with reset cycles ---
fn request_scoped_example() {
    let mut arena = Arena::new();
    let mut succeeded = 0u32;

    for round in 0u32..20 {
        let ok = arena.with_transaction(|txn| -> Result<u32, &str> {
            let id = *txn.alloc(round);
            let tag = txn.alloc_str("req-tag");
            let tag_len = tag.len() as u32;
            let _ = txn.alloc_slice(0u8..16u8);
            if round % 5 == 3 {
                return Err("simulated failure");
            }
            Ok(id + tag_len)
        });
        if ok.is_ok() {
            succeeded += 1;
        }
        arena.reset();
    }

    println!("request-scoped: {succeeded}/20 succeeded (4 simulated failures)");
}

// --- ArenaVec inside transactions ---
fn arenavec_in_transaction() {
    let mut arena = Arena::new();

    {
        let mut txn = arena.transaction();
        let _slice = {
            let mut v = ArenaVec::new(txn.arena_mut());
            v.extend_exact([1u32, 2, 3]);
            v.finish()
        };
        let _ = txn.commit();
    }

    println!("ArenaVec in transaction committed");
}

fn main() {
    compiler_example();
    lsm_example();
    request_scoped_example();
    arenavec_in_transaction();
    println!("\nAll real-world examples passed.");
}
