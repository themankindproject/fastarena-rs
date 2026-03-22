//! Drop-tracking examples (requires `--features drop-tracking`).
//!
//! By default, fastarena never runs destructors (zero overhead).
//! Enable `drop-tracking` to run them in LIFO order on `reset()` / `rewind()`.
//!
//! Run with: cargo run --example drop_tracking --features drop-tracking

use fastarena::Arena;

fn main() {
    let mut arena = Arena::new();

    // --- Reset runs destructors ---
    let cp = arena.checkpoint();
    arena.alloc(String::from("hello"));
    arena.alloc(String::from("world"));
    // With drop-tracking: drops fire in LIFO order ("world", then "hello")
    // Without drop-tracking: no destructors, memory reclaimed instantly
    arena.rewind(cp);
    println!("rewind after 2 Strings => destructors ran (LIFO)");

    // --- Reset runs all destructors ---
    arena.alloc(String::from("one"));
    arena.alloc(String::from("two"));
    arena.alloc(String::from("three"));
    arena.reset();
    println!("reset after 3 Strings => all destructors ran");

    // --- Transaction rollback runs destructors ---
    {
        let mut txn = arena.transaction();
        txn.alloc(String::from("rolled-back"));
        txn.alloc(String::from("also-rolled-back"));
        // drop => rollback => destructors run
    }
    println!("transaction rollback => destructors ran");

    // --- Transaction commit defers drops to reset ---
    {
        let mut txn = arena.transaction();
        txn.alloc(String::from("committed-string"));
        txn.commit();
    }
    println!("committed txn => destructors NOT yet run");
    arena.reset();
    println!("reset after commit => NOW destructors run");

    // --- alloc_slice registers each element ---
    let cp = arena.checkpoint();
    {
        let items = (0..5).map(|i| format!("item-{i}"));
        arena.alloc_slice(items);
    }
    arena.rewind(cp);
    println!("rewind after alloc_slice(5 Strings) => all 5 destructors ran");
}
