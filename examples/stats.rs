//! ArenaStats and introspection examples.
//!
//! O(1) memory usage snapshot with utilization and idle calculations.

use fastarena::Arena;

fn main() {
    let mut arena = Arena::new();

    // --- Stats before any allocation ---
    let s = arena.stats();
    println!("empty arena: {s:?}");
    println!("  utilization: {:.1}%", s.utilization() * 100.0);
    println!("  idle bytes: {}", s.bytes_idle());

    // --- Stats after some allocations ---
    for i in 0u64..100 {
        arena.alloc(i);
    }
    let s = arena.stats();
    println!("100 u64s: {s:?}");
    println!("  utilization: {:.1}%", s.utilization() * 100.0);

    // --- Block count grows with large allocations ---
    let mut arena = Arena::with_capacity(64);
    for _ in 0..100 {
        arena.alloc(0u64);
    }
    let s = arena.stats();
    println!(
        "small arena, 100 u64s: blocks={}, reserved={}, allocated={}",
        s.block_count, s.bytes_reserved, s.bytes_allocated
    );

    // --- Stats after rewind ---
    let mut arena = Arena::new();
    let cp = arena.checkpoint();
    for _ in 0..10 {
        arena.alloc(0u64);
    }
    println!("10 u64s: {:?}", arena.stats());

    arena.rewind(cp);
    println!("after rewind: {:?}", arena.stats());
    assert_eq!(arena.stats().bytes_allocated, 0);

    // --- Transaction diff ---
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    txn.alloc(1u64);
    txn.alloc(2u64);
    println!(
        "txn diff: bytes={}, blocks={}",
        txn.diff().bytes_allocated,
        txn.diff().blocks_touched
    );
    txn.commit();

    // --- Pre-allocated arena ---
    let arena = Arena::with_capacity(1024 * 1024); // 1 MiB
    println!("pre-allocated 1MiB: {:?}", arena.stats());

    // --- Stats after reset (reserved retained) ---
    let mut arena = Arena::new();
    for _ in 0..50 {
        arena.alloc(0u64);
    }
    let before = arena.stats();
    arena.reset();
    let after = arena.stats();
    println!("reset: reserved kept? {}", after.bytes_reserved > 0);
    assert_eq!(after.bytes_reserved, before.bytes_reserved);
}
