//! Checkpoint / Rewind / Reset examples.
//!
//! O(1) snapshot, O(k) rollback, zero-cost reset.

use fastarena::Arena;

fn main() {
    // --- Basic checkpoint and rewind ---
    let mut arena = Arena::new();
    arena.alloc(1u64);
    let cp = arena.checkpoint(); // O(1) — copies 3 integers
    arena.alloc(2u64);
    arena.alloc(3u64);
    println!(
        "before rewind: {} bytes, {} blocks",
        arena.stats().bytes_allocated,
        arena.block_count()
    );

    arena.rewind(cp); // 2 and 3 gone; blocks retained for reuse
    println!(
        "after rewind:  {} bytes, {} blocks",
        arena.stats().bytes_allocated,
        arena.block_count()
    );

    // --- Verify pre-checkpoint data survives ---
    let mut arena = Arena::new();
    let x_ptr = arena.alloc(0xDEAD_BEEF_u64) as *mut u64;
    let cp = arena.checkpoint();
    arena.alloc(0xCAFE_u64);
    arena.rewind(cp);
    assert_eq!(unsafe { *x_ptr }, 0xDEAD_BEEF);
    println!("pre-checkpoint value survives rewind: {:#x}", unsafe {
        *x_ptr
    });

    // --- Reset: reclaim everything ---
    let mut arena = Arena::new();
    for _ in 0..50 {
        arena.alloc(0u64);
    }
    let before = arena.stats();
    println!("before reset: {before:?}");

    arena.reset();
    let after = arena.stats();
    println!("after reset:  {after:?}");
    assert_eq!(after.bytes_allocated, 0);
    assert_eq!(after.bytes_reserved, before.bytes_reserved);

    // --- Checkpoint inside a transaction ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.alloc(1u32);
        let _cp = txn.checkpoint();
        txn.alloc(2u32);
        txn.arena_mut().alloc(3u32);
        println!("txn bytes_used: {}", txn.bytes_used());
        let _ = txn.commit();
    }

    // --- Stats after reset cycle ---
    let mut arena = Arena::new();
    for batch in 0..5 {
        for i in 0..100 {
            arena.alloc(i + batch * 100);
        }
        println!("batch {batch}: {:?}", arena.stats());
        arena.reset();
    }
    println!("final: {:?}", arena.stats());
}
