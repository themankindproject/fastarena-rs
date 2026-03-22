//! Transaction examples.
//!
//! Demonstrates `with_transaction`, `with_transaction_infallible`,
//! manual `transaction()` + `commit()` / `rollback()`, and
//! transaction introspection (`diff`, `bytes_used`).

use fastarena::Arena;

fn main() {
    // --- Closure API: Ok commits, Err rolls back ---
    let mut arena = Arena::new();

    let result: Result<u32, &str> = arena.with_transaction(|txn| {
        let score = txn.alloc(100u32);
        Ok(*score)
    });
    assert_eq!(result, Ok(100));
    println!("with_transaction Ok => committed: {:?}", arena.stats());

    // Failed transaction — everything allocated inside is gone
    let before = arena.stats().bytes_allocated;
    let _result: Result<(), &str> = arena.with_transaction(|txn| {
        txn.alloc(1u32);
        txn.alloc(2u32);
        Err("abort")
    });
    assert_eq!(arena.stats().bytes_allocated, before);
    println!("with_transaction Err => rolled back, bytes unchanged");

    // --- Infallible variant — commits even through panic ---
    let val = arena.with_transaction_infallible(|txn| {
        let x = txn.alloc(7u32);
        *x * 6
    });
    assert_eq!(val, 42);
    println!("with_transaction_infallible => {val}");

    // --- Manual transaction API ---
    {
        let mut txn = arena.transaction();
        txn.alloc(100u64);
        txn.alloc(200u64);
        println!("manual txn: 2 allocs");
        println!("  bytes_used={}", txn.bytes_used());
        println!("  diff={:?}", txn.diff());
        txn.commit();
    }

    // Manual rollback (drop without commit)
    let bytes_before_rollback = arena.stats().bytes_allocated;
    {
        let mut txn = arena.transaction();
        txn.alloc(999u64);
        // drop without commit => rollback
    }
    assert_eq!(arena.stats().bytes_allocated, bytes_before_rollback);
    println!("manual rollback => bytes unchanged");

    // Explicit rollback
    {
        let mut txn = arena.transaction();
        txn.alloc(1u32);
        let status = txn.rollback();
        println!("explicit rollback => status={status:?}");
    }

    // --- Transaction introspection ---
    {
        let mut txn = arena.transaction();
        txn.alloc(1u64);
        txn.alloc(2u64);
        println!(
            "txn diff: bytes={}, blocks={}",
            txn.diff().bytes_allocated,
            txn.diff().blocks_touched
        );
        txn.commit();
    }
}
