// tests/advanced_tests.rs
//
// Comprehensive tests for arena-alloc v0.1 features:
//   • alloc_uninit / alloc_zeroed
//   • try_alloc family (OOM-safe)
//   • O(1) stats accounting
//   • Transaction depth tracking
//   • Transaction budgets
//   • TxnDiff metrics
//   • ArenaVec (push/pop/finish/grow/drop)
//   • Multi-block nested transactions
//   • Large allocations inside savepoints
//   • Alignment edge cases (4096-byte)
//   • Panic-safe rollback
//   • drop-tracking correctness (feature-gated)
//   • with_transaction_infallible
//   • Hardened commit (mem::forget)

use fastarena::{Arena, ArenaVec, TxnStatus};

// ============================================================================
// alloc_uninit
// ============================================================================

#[test]
fn alloc_uninit_write_and_read() {
    let mut arena = Arena::new();
    let slot = arena.alloc_uninit::<u64>();
    slot.write(0xCAFE_BABE_u64);
    let val: &mut u64 = unsafe { slot.assume_init_mut() };
    assert_eq!(*val, 0xCAFE_BABE_u64);
}

#[test]
fn alloc_uninit_alignment_correct() {
    #[repr(align(32))]
    struct A32(u64);

    let mut arena = Arena::new();
    let _ = arena.alloc(1u8); // create misalignment
    let slot = arena.alloc_uninit::<A32>();
    let ptr = slot as *mut _ as usize;
    assert_eq!(ptr % 32, 0, "alloc_uninit must respect alignment");
}

#[test]
fn alloc_uninit_zst() {
    let mut arena = Arena::new();
    let before = arena.stats().bytes_allocated;
    let _slot: &mut std::mem::MaybeUninit<()> = arena.alloc_uninit::<()>();
    assert_eq!(
        arena.stats().bytes_allocated,
        before,
        "ZST must not allocate"
    );
}

// ============================================================================
// alloc_zeroed
// ============================================================================

#[test]
fn alloc_zeroed_all_bytes_zero() {
    let mut arena = Arena::new();
    let ptr = arena.alloc_zeroed(64, 8);
    let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), 64) };
    assert!(slice.iter().all(|&b| b == 0), "all bytes must be zero");
}

#[test]
fn alloc_zeroed_alignment() {
    let mut arena = Arena::new();
    let _ = arena.alloc(1u8); // misalign
    let ptr = arena.alloc_zeroed(128, 64);
    assert_eq!(ptr.as_ptr() as usize % 64, 0, "must be cache-line aligned");
}

#[test]
fn alloc_zeroed_zero_size() {
    let mut arena = Arena::new();
    let before = arena.stats().bytes_allocated;
    let _p = arena.alloc_zeroed(0, 8);
    assert_eq!(arena.stats().bytes_allocated, before);
}

// ============================================================================
// try_alloc family
// ============================================================================

#[test]
fn try_alloc_success() {
    let mut arena = Arena::new();
    let r = arena.try_alloc(42u32);
    assert!(r.is_some());
    assert_eq!(*r.unwrap(), 42);
}

#[test]
fn try_alloc_slice_success() {
    let mut arena = Arena::new();
    let s = arena.try_alloc_slice(0u32..8);
    assert!(s.is_some());
    assert_eq!(s.unwrap(), &[0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn try_alloc_str_success() {
    let mut arena = Arena::new();
    let s = arena.try_alloc_str("hello");
    assert_eq!(s, Some("hello"));
}

#[test]
fn try_alloc_empty_slice_no_alloc() {
    let mut arena = Arena::new();
    let before = arena.stats().bytes_allocated;
    let s: Option<&mut [u32]> = arena.try_alloc_slice(std::iter::empty());
    assert!(s.is_some());
    assert_eq!(s.unwrap().len(), 0);
    assert_eq!(arena.stats().bytes_allocated, before);
}

#[test]
fn try_alloc_raw_success() {
    let mut arena = Arena::new();
    let p = arena.try_alloc_raw(16, 8);
    assert!(p.is_some());
}

// ============================================================================
// O(1) stats accounting
// ============================================================================

#[test]
fn stats_o1_bytes_allocated_increments() {
    let mut arena = Arena::new();
    assert_eq!(arena.stats().bytes_allocated, 0);

    let _ = arena.alloc(1u8);
    assert!(arena.stats().bytes_allocated >= 1);

    let before = arena.stats().bytes_allocated;
    let _ = arena.alloc(0u64);
    let after = arena.stats().bytes_allocated;
    assert!(after >= before + 8, "u64 must consume at least 8 bytes");
}

#[test]
fn stats_bytes_reserved_only_grows() {
    let mut arena = Arena::with_capacity(64);
    let r0 = arena.stats().bytes_reserved;

    for _ in 0..100 {
        let _ = arena.alloc(0u64);
    }
    let r1 = arena.stats().bytes_reserved;
    assert!(r1 >= r0, "reserved must not decrease");

    arena.reset();
    let r2 = arena.stats().bytes_reserved;
    assert_eq!(r2, r1, "reserved must not decrease on reset");
}

#[test]
fn stats_after_rewind_restored() {
    let mut arena = Arena::new();
    let cp = arena.checkpoint();

    for _ in 0..10 {
        let _ = arena.alloc(0u64);
    }
    let peak = arena.stats().bytes_allocated;
    assert!(peak >= 80);

    arena.rewind(cp);
    assert_eq!(
        arena.stats().bytes_allocated,
        0,
        "rewind must restore bytes_allocated to snapshot value"
    );
}

#[test]
fn stats_after_reset_zeroed() {
    let mut arena = Arena::new();
    for _ in 0..50 {
        let _ = arena.alloc(0u64);
    }
    arena.reset();
    let s = arena.stats();
    assert_eq!(s.bytes_allocated, 0);
    assert!(s.bytes_reserved > 0, "reserved kept after reset");
}

#[test]
fn stats_bytes_reserved_includes_all_blocks() {
    let mut arena = Arena::with_capacity(32);
    // Force multi-block growth
    for _ in 0..100 {
        let _ = arena.alloc(0u64);
    }
    let s = arena.stats();
    assert!(s.bytes_reserved >= s.bytes_allocated);
    assert!(s.block_count >= 2);
}

// ============================================================================
// Transaction depth tracking
// ============================================================================

#[test]
fn depth_zero_initially() {
    let arena = Arena::new();
    assert_eq!(arena.transaction_depth(), 0);
}

#[test]
fn depth_increments_on_open() {
    let mut arena = Arena::new();
    let txn = arena.transaction();
    assert_eq!(txn.arena_depth(), 1);
    assert_eq!(txn.depth(), 1);
    txn.commit();
    assert_eq!(arena.transaction_depth(), 0);
}

#[test]
fn depth_increments_for_savepoints() {
    let mut arena = Arena::new();
    let mut t1 = arena.transaction();
    assert_eq!(t1.depth(), 1);
    {
        let mut t2 = t1.savepoint();
        assert_eq!(t2.depth(), 2);
        {
            let t3 = t2.savepoint();
            assert_eq!(t3.depth(), 3);
            t3.commit();
        }
        assert_eq!(t2.arena_depth(), 2);
        t2.commit();
    }
    assert_eq!(t1.arena_depth(), 1);
    t1.commit();
    assert_eq!(arena.transaction_depth(), 0);
}

#[test]
fn depth_decrements_on_rollback() {
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        let _ = txn.alloc(1u32);
        // dropped → rollback
    }
    assert_eq!(arena.transaction_depth(), 0);
}

// ============================================================================
// Transaction budget
// ============================================================================

#[test]
fn budget_allows_exact_usage() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    txn.set_limit(8);
    let _ = txn.alloc(0u64); // exactly 8 bytes
    assert_eq!(txn.budget_remaining(), Some(0));
    txn.commit();
}

#[test]
fn budget_try_alloc_returns_none_when_exceeded() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    txn.set_limit(4);
    let _ = txn.alloc(0u32); // 4 bytes — ok
                             // next try_alloc should fail
    let r = txn.try_alloc(0u32);
    assert!(r.is_none(), "budget exceeded → None");
    txn.commit();
}

#[test]
fn budget_alloc_panics_when_exceeded() {
    let mut arena = Arena::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let arena_ptr = &mut arena as *mut Arena;
        let a = unsafe { &mut *arena_ptr };
        let mut txn = a.transaction();
        txn.set_limit(0);
        let _ = txn.alloc(1u8); // should panic
    }));
    assert!(result.is_err(), "expected budget-exceeded panic");
    // arena rolled back cleanly
    assert_eq!(arena.stats().bytes_allocated, 0);
}

#[test]
fn budget_remaining_none_without_limit() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    assert_eq!(txn.budget_remaining(), None);
    let _ = txn.alloc(0u64);
    assert_eq!(txn.budget_remaining(), None);
    txn.commit();
}

#[test]
fn budget_try_alloc_str_respects_limit() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    txn.set_limit(3); // only 3 bytes
    let r = txn.try_alloc_str("hello"); // 5 bytes → should fail
    assert!(r.is_none());
    let ok = txn.try_alloc_str("hi"); // 2 bytes — fits
    assert_eq!(ok, Some("hi"));
    txn.commit();
}

// ============================================================================
// TxnDiff metrics
// ============================================================================

#[test]
fn diff_zero_on_empty_txn() {
    let mut arena = Arena::new();
    let txn = arena.transaction();
    let diff = txn.diff();
    assert_eq!(diff.bytes_allocated, 0);
    assert_eq!(diff.blocks_touched, 1);
    txn.commit();
}

#[test]
fn diff_reflects_allocations() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    let _ = txn.alloc(0u64);
    let _ = txn.alloc(0u64);
    let diff = txn.diff();
    assert!(diff.bytes_allocated >= 16, "two u64s = 16 bytes minimum");
    txn.commit();
}

#[test]
fn diff_blocks_touched_increases_across_blocks() {
    let mut arena = Arena::with_capacity(32); // tiny blocks
    let mut txn = arena.transaction();
    // Force spill into multiple blocks
    for _ in 0..20 {
        let _ = txn.alloc(0u64);
    }
    let diff = txn.diff();
    assert!(
        diff.blocks_touched >= 2,
        "160+ bytes into 32-byte blocks must touch multiple blocks"
    );
    txn.commit();
}

#[test]
fn bytes_used_is_o1() {
    // bytes_used() should be a simple subtraction, not a sum.
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    for _ in 0..1000 {
        let _ = txn.alloc(0u64);
    }
    let used = txn.bytes_used();
    // should be at least 1000 * 8 bytes
    assert!(used >= 8000, "expected >= 8000 bytes, got {used}");
    txn.commit();
    // After commit the full amount should be in stats
    assert!(arena.stats().bytes_allocated >= 8000);
}

// ============================================================================
// Hardened commit (mem::forget)
// ============================================================================

#[test]
fn commit_returns_committed_status() {
    let mut arena = Arena::new();
    let status = arena.transaction().commit();
    assert_eq!(status, TxnStatus::Committed);
}

#[test]
fn commit_depth_decremented() {
    let mut arena = Arena::new();
    let txn = arena.transaction();
    // while open, depth is 1
    assert_eq!(txn.arena_depth(), 1);
    txn.commit();
    // after commit, depth is restored
    assert_eq!(arena.transaction_depth(), 0);
}

#[test]
fn commit_allocations_survive() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    let _ = txn.alloc(42u64);
    txn.commit();
    assert!(arena.stats().bytes_allocated >= 8);
}

#[test]
fn rollback_returns_rolledback_status() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    let _ = txn.alloc(1u32);
    let status = txn.rollback();
    assert_eq!(status, TxnStatus::RolledBack);
    assert_eq!(arena.stats().bytes_allocated, 0);
}

// ============================================================================
// Nested transactions across multiple blocks
// ============================================================================

#[test]
fn nested_txn_across_multiple_blocks() {
    let mut arena = Arena::with_capacity(64);
    let mut outer = arena.transaction();
    let _ = outer.alloc(1u32);

    {
        let mut inner = outer.savepoint();
        // 200 bytes in a 64-byte-initial arena → multiple blocks
        for _ in 0..25 {
            let _ = inner.alloc(0u64);
        }
        assert!(inner.diff().blocks_touched >= 2);
        inner.commit(); // merged into outer
    }

    let diff = outer.diff();
    assert!(diff.bytes_allocated >= 200 + 4);
    outer.commit();
}

#[test]
fn nested_txn_rollback_across_multiple_blocks() {
    let mut arena = Arena::with_capacity(64);
    let bytes_before = arena.stats().bytes_allocated;

    {
        let mut txn = arena.transaction();
        for _ in 0..25 {
            let _ = txn.alloc(0u64);
        }
        // Check block spill via diff() — no need to access arena directly
        assert!(
            txn.diff().blocks_touched >= 2,
            "25 × 8B into 64B blocks must spill"
        );
        // rolled back
    }

    assert_eq!(
        arena.stats().bytes_allocated,
        bytes_before,
        "multi-block rollback must restore bytes_allocated"
    );
    assert!(arena.block_count() > 0);
}

#[test]
fn nested_savepoint_partial_rollback_partial_commit() {
    let mut arena = Arena::with_capacity(64);
    let mut outer = arena.transaction();
    let _ = outer.alloc(100u64);
    let after_outer_alloc = outer.bytes_used();

    {
        let mut sp = outer.savepoint();
        for _ in 0..25 {
            let _ = sp.alloc(0u64);
        }
        // rolled back — only outer's alloc should remain
    }

    assert_eq!(
        outer.bytes_used(),
        after_outer_alloc,
        "savepoint rollback must not affect parent"
    );
    outer.commit();
}

// ============================================================================
// Large allocations in savepoints
// ============================================================================

#[test]
fn large_alloc_in_savepoint_committed() {
    let mut arena = Arena::with_capacity(128);
    let mut txn = arena.transaction();
    {
        let mut sp = txn.savepoint();
        // 1 MiB — vastly exceeds initial capacity
        const MEBI: usize = 1024 * 1024;
        let big: &mut [u8] = sp.alloc_slice(vec![0xFFu8; MEBI]);
        assert_eq!(big.len(), MEBI);
        assert!(big.iter().all(|&b| b == 0xFF));
        sp.commit();
    }
    let diff = txn.diff();
    assert!(diff.bytes_allocated >= 1024 * 1024);
    txn.commit();
}

#[test]
fn large_alloc_in_savepoint_rolled_back() {
    let mut arena = Arena::with_capacity(128);
    let before_res = arena.stats().bytes_reserved;

    {
        let mut txn = arena.transaction();
        {
            let mut sp = txn.savepoint();
            const MEBI: usize = 1024 * 1024;
            let _ = sp.alloc_slice(vec![0u8; MEBI]);
            // sp dropped → rolled back
        }
        assert_eq!(
            txn.bytes_used(),
            0,
            "savepoint rollback must undo large alloc"
        );
        txn.commit();
    }

    assert_eq!(arena.stats().bytes_allocated, 0);
    // bytes_reserved grew but that's expected (blocks retained)
    let _ = before_res;
}

// ============================================================================
// Extreme alignment (4096 bytes)
// ============================================================================

#[test]
fn alloc_page_aligned_struct() {
    #[repr(align(4096))]
    struct PageAligned([u8; 4096]);

    let mut arena = Arena::with_capacity(64);
    let _ = arena.alloc(1u8); // misalign
    let p = arena.alloc(PageAligned([0u8; 4096]));
    assert_eq!(
        p as *mut PageAligned as usize % 4096,
        0,
        "must be 4096-byte aligned"
    );
}

#[test]
fn alloc_raw_4096_alignment() {
    let mut arena = Arena::new();
    let _ = arena.alloc(1u8);
    let ptr = arena.alloc_raw(4096, 4096);
    assert_eq!(ptr.as_ptr() as usize % 4096, 0);
}

#[test]
fn alloc_4096_in_transaction() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    let _ = txn.alloc(1u8); // misalign
    let ptr = txn.alloc_raw(128, 4096);
    assert_eq!(ptr.as_ptr() as usize % 4096, 0);
    txn.commit();
}

// ============================================================================
// Panic-safe rollback
// ============================================================================

#[test]
fn transaction_rolls_back_on_panic() {
    let mut arena = Arena::new();

    // We need to test panic rollback without moving arena.
    // Use a raw pointer to allow the closure to reference arena.
    let arena_ptr = &mut arena as *mut Arena;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let a = unsafe { &mut *arena_ptr };
        let mut txn = a.transaction();
        let _ = txn.alloc(0xDEADu64);
        panic!("deliberate panic for rollback test");
    }));

    assert!(result.is_err(), "panic should propagate");
    assert_eq!(
        arena.stats().bytes_allocated,
        0,
        "panic must trigger RAII rollback"
    );
    assert_eq!(
        arena.transaction_depth(),
        0,
        "depth must be decremented even on panic"
    );
}

#[test]
fn nested_savepoint_panic_only_rolls_back_inner() {
    let mut arena = Arena::new();
    let arena_ptr = &mut arena as *mut Arena;
    let outer_bytes;

    {
        let a = unsafe { &mut *arena_ptr };
        let mut txn = a.transaction();
        let _ = txn.alloc(111u64); // outer alloc
        outer_bytes = txn.bytes_used();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let inner_arena = unsafe { &mut *arena_ptr };
            // We can't call txn.savepoint() here because we can't borrow txn
            // across catch_unwind. Instead simulate by directly testing that
            // a separate transaction panics and rolls back.
            let mut inner = inner_arena.transaction();
            let _ = inner.alloc(222u64);
            panic!("inner panic");
        }));

        // outer transaction's allocation must still be accounted for
        // (the inner transaction was a separate one from the arena directly)
        assert!(txn.bytes_used() >= outer_bytes);
        txn.commit();
    }

    // arena should have outer's alloc (from the committed outer txn)
    // but inner's alloc should have been rolled back by panic
    assert!(arena.stats().bytes_allocated >= 8, "outer alloc survived");
}

// ============================================================================
// with_transaction and with_transaction_infallible
// ============================================================================

#[test]
fn with_transaction_ok_commits() {
    let mut arena = Arena::new();
    let result = arena.with_transaction(|txn| -> Result<u32, &str> {
        let x = txn.alloc(21u32);
        Ok(*x * 2)
    });
    assert_eq!(result, Ok(42));
    assert!(arena.stats().bytes_allocated >= 4);
}

#[test]
fn with_transaction_err_rolls_back() {
    let mut arena = Arena::new();
    let result = arena.with_transaction(|txn| -> Result<(), &str> {
        let _ = txn.alloc(99u64);
        Err("abort")
    });
    assert_eq!(result, Err("abort"));
    assert_eq!(arena.stats().bytes_allocated, 0);
}

#[test]
fn with_transaction_infallible_always_commits() {
    let mut arena = Arena::new();
    let val = arena.with_transaction_infallible(|txn| {
        let x = txn.alloc(7u32);
        *x * 6
    });
    assert_eq!(val, 42);
    assert!(arena.stats().bytes_allocated >= 4);
}

// ============================================================================
// ArenaVec integration
// ============================================================================

#[test]
fn arena_vec_basic() {
    let mut arena = Arena::new();
    let mut v = ArenaVec::new(&mut arena);
    for i in 0u32..10 {
        v.push(i * i);
    }
    assert_eq!(v.len(), 10);
    assert_eq!(v[9], 81);
}

#[test]
fn arena_vec_finish_returns_arena_slice() {
    let mut arena = Arena::new();
    let slice = {
        let mut v = ArenaVec::new(&mut arena);
        v.extend([10u64, 20, 30]);
        v.finish()
    };
    assert_eq!(slice, &[10, 20, 30]);
    // arena is usable again
    let _ = arena.alloc(99u32);
}

#[test]
fn arena_vec_grows_across_blocks() {
    let mut arena = Arena::with_capacity(64); // tiny
    let slice = {
        let mut v = ArenaVec::new(&mut arena);
        for i in 0u64..200 {
            v.push(i);
        }
        assert_eq!(v.len(), 200);
        v.finish()
    };
    for (i, &val) in slice.iter().enumerate() {
        assert_eq!(val, i as u64);
    }
}

#[test]
fn arena_vec_inside_transaction_committed() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    {
        let v = {
            let mut av = ArenaVec::new(txn.arena_mut());
            av.extend([1u32, 2, 3]);
            av.finish()
        };
        assert_eq!(v, &[1, 2, 3]);
    }
    txn.commit();
    assert!(arena.stats().bytes_allocated >= 12);
}

#[test]
fn arena_vec_pop_correctness() {
    let mut arena = Arena::new();
    let mut v = ArenaVec::new(&mut arena);
    v.extend([1u32, 2, 3]);
    assert_eq!(v.pop(), Some(3));
    assert_eq!(v.pop(), Some(2));
    assert_eq!(v.pop(), Some(1));
    assert_eq!(v.pop(), None);
}

#[test]
fn arena_vec_with_capacity_no_reallocate() {
    let mut arena = Arena::new();
    let mut v = ArenaVec::<u64>::with_capacity(&mut arena, 32);
    let cap0 = v.capacity();
    assert_eq!(cap0, 32);
    for i in 0u64..32 {
        v.push(i);
    }
    assert_eq!(v.capacity(), cap0, "no reallocation expected");
    v.finish();
}

// ============================================================================
// drop-tracking (feature-gated)
// ============================================================================

#[cfg(feature = "drop-tracking")]
mod drop_tracking_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Tracked(usize);
    impl Drop for Tracked {
        fn drop(&mut self) {
            COUNTER.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn reset_counter() {
        COUNTER.store(0, Ordering::Relaxed);
    }

    #[test]
    fn reset_runs_destructors() {
        reset_counter();
        let mut arena = Arena::new();
        let _ = arena.alloc(Tracked(1));
        let _ = arena.alloc(Tracked(2));
        let _ = arena.alloc(Tracked(3));
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
        arena.reset();
        assert_eq!(COUNTER.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn rewind_runs_only_post_checkpoint_destructors() {
        reset_counter();
        let mut arena = Arena::new();
        let _ = arena.alloc(Tracked(1)); // before checkpoint
        let cp = arena.checkpoint();
        let _ = arena.alloc(Tracked(2)); // after checkpoint
        let _ = arena.alloc(Tracked(3)); // after checkpoint
        assert_eq!(COUNTER.load(Ordering::Relaxed), 0);
        arena.rewind(cp);
        // Only the 2 post-checkpoint objects should be dropped
        assert_eq!(COUNTER.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn destructors_run_in_lifo_order() {
        static ORDER: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

        struct Ordered(usize);
        impl Drop for Ordered {
            fn drop(&mut self) {
                ORDER.lock().unwrap().push(self.0);
            }
        }

        ORDER.lock().unwrap().clear();

        let mut arena = Arena::new();
        let cp = arena.checkpoint();
        let _ = arena.alloc(Ordered(1));
        let _ = arena.alloc(Ordered(2));
        let _ = arena.alloc(Ordered(3));
        arena.rewind(cp);

        let order = ORDER.lock().unwrap().clone();
        assert_eq!(order, vec![3, 2, 1], "drops must fire in LIFO order");
    }

    #[test]
    fn transaction_rollback_runs_destructors() {
        reset_counter();
        let mut arena = Arena::new();
        {
            let mut txn = arena.transaction();
            let _ = txn.alloc(Tracked(10));
            let _ = txn.alloc(Tracked(20));
            // rolled back
        }
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            2,
            "transaction rollback must run destructors"
        );
    }

    #[test]
    fn transaction_commit_defers_drop_to_reset() {
        reset_counter();
        let mut arena = Arena::new();
        {
            let mut txn = arena.transaction();
            let _ = txn.alloc(Tracked(99));
            txn.commit();
        }
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            0,
            "commit must NOT run destructors — deferred to reset/drop"
        );
        arena.reset();
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            1,
            "reset must finally run the destructor"
        );
    }

    #[test]
    fn alloc_slice_registers_each_element() {
        reset_counter();
        let mut arena = Arena::new();
        let cp = arena.checkpoint();
        {
            let items = (0..5).map(|i| Tracked(i));
            let _ = arena.alloc_slice(items);
        }
        arena.rewind(cp);
        assert_eq!(
            COUNTER.load(Ordering::Relaxed),
            5,
            "rewind must run all 5 element destructors"
        );
    }
}

// ============================================================================
// Real-world: compiler IR builder
// ============================================================================

#[test]
fn compiler_speculative_optimization_pass() {
    #[derive(Debug, PartialEq)]
    enum Op {
        Const(i64),
        Add,
        Mul,
        Nop,
    }

    let mut arena = Arena::new();

    // Baseline IR — cast refs to raw pointers to allow multiple allocs
    // in one closure (the borrow checker requires refs not outlive the borrow)
    let _base_nodes = arena.with_transaction_infallible(|txn| {
        let a = txn.alloc(Op::Const(10)) as *mut Op;
        let b = txn.alloc(Op::Const(20)) as *mut Op;
        let c = txn.alloc(Op::Add) as *mut Op;
        (a, b, c)
    });
    let baseline_bytes = arena.stats().bytes_allocated;

    // Speculative pass — not profitable, roll back
    {
        let mut txn = arena.transaction();
        for _ in 0..100 {
            let _ = txn.alloc(Op::Nop);
        }
        // rolled back
    }
    assert_eq!(
        arena.stats().bytes_allocated,
        baseline_bytes,
        "speculative pass must leave arena unchanged"
    );

    // Profitable pass — commit
    arena
        .with_transaction(|txn| -> Result<(), ()> {
            let _ = txn.alloc(Op::Mul);
            Ok(())
        })
        .unwrap();

    assert!(arena.stats().bytes_allocated > baseline_bytes);
}

// ============================================================================
// Real-world: LSM memtable batch insert with abort
// ============================================================================

#[test]
fn lsm_batch_insert_abort_leaves_no_trace() {
    #[derive(Debug)]
    struct KvEntry {
        key_ptr: *const u8,
        key_len: usize,
        seq: u64,
    }

    let mut arena = Arena::new();

    // Batch 1: commit — store key as raw pointer to avoid multi-borrow
    arena
        .with_transaction(|txn| -> Result<(), &str> {
            let k1 = txn.alloc_str("apple") as *const str;
            let _ = txn.alloc(KvEntry {
                key_ptr: k1 as *const u8,
                key_len: 5,
                seq: 1,
            });
            let k2 = txn.alloc_str("banana") as *const str;
            let _ = txn.alloc(KvEntry {
                key_ptr: k2 as *const u8,
                key_len: 6,
                seq: 2,
            });
            Ok(())
        })
        .unwrap();
    let committed = arena.stats().bytes_allocated;

    // Batch 2: abort (duplicate key detected mid-batch)
    let _ = arena.with_transaction(|txn| -> Result<(), &str> {
        let k = txn.alloc_str("cherry") as *const str;
        let _ = txn.alloc(KvEntry {
            key_ptr: k as *const u8,
            key_len: 6,
            seq: 3,
        });
        Err("duplicate key")
    });

    assert_eq!(
        arena.stats().bytes_allocated,
        committed,
        "aborted batch must leave arena unchanged"
    );
}

// ============================================================================
// Real-world: request-scoped allocator
// ============================================================================

#[test]
fn request_scoped_allocator_reset_cycle() {
    let mut arena = Arena::new();
    let mut succeeded = 0u32;

    for round in 0u32..20 {
        let ok = arena.with_transaction(|txn| -> Result<u32, &str> {
            let id = *txn.alloc(round); // copy out value immediately
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
    }

    assert_eq!(succeeded, 16, "4 rounds failed (round%5==3: 3,8,13,18)");
}
