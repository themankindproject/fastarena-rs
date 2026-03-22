//! Transaction budget examples.
//!
//! Set a byte budget on any transaction. Exceed it and `alloc` panics
//! (or `try_alloc` returns `None`).
//!
//! **Important:** `alloc(vec![0u8; N])` stores only the `Vec` struct (24 bytes)
//! in the arena — the heap buffer is *not* arena-tracked. Use `alloc_slice`
//! to budget actual byte slices.

use fastarena::Arena;

fn main() {
    // --- Basic budget with alloc_slice ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.set_limit(4096); // hard cap

        txn.alloc_slice(vec![0u8; 2048]); // ok — 2048 arena bytes
        let remaining = txn.budget_remaining();
        println!("after 2048 bytes: remaining={remaining:?}");
        txn.commit();
    }

    // --- alloc panics when budget exceeded ---
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut arena = Arena::new();
        let mut txn = arena.transaction();
        txn.set_limit(4096);
        txn.alloc_slice(vec![0u8; 2048]);
        txn.alloc_slice(vec![0u8; 4096]); // 2048 + 4096 > 4096 => panic
    }));
    assert!(result.is_err(), "expected budget-exceeded panic");
    println!("alloc_slice beyond budget => panic (as expected)");

    // --- try_alloc returns None when budget exceeded ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.set_limit(4);
        txn.alloc(0u32); // uses 4 bytes
        let r = txn.try_alloc(0u32);
        assert!(r.is_none(), "budget exceeded — try_alloc returns None");
        println!("try_alloc on exhausted budget => None");
        txn.commit();
    }

    // --- try_alloc_str respects budget ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.set_limit(3);
        let r = txn.try_alloc_str("hello"); // 5 bytes > 3
        assert!(r.is_none());
        let ok = txn.try_alloc_str("hi"); // 2 bytes <= 3
        assert_eq!(ok, Some("hi"));
        println!("try_alloc_str: 'hello' rejected, 'hi' accepted");
        txn.commit();
    }

    // --- try_alloc_slice respects budget ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.set_limit(100);
        let r = txn.try_alloc_slice(vec![0u8; 200]); // 200 > 100
        assert!(r.is_none(), "slice too large for budget");
        let ok = txn.try_alloc_slice(vec![0u8; 50]);
        assert!(ok.is_some());
        assert_eq!(ok.unwrap().len(), 50);
        println!("try_alloc_slice: 200 rejected, 50 accepted");
        txn.commit();
    }

    // --- No limit: budget_remaining is None ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.alloc(1u64);
        assert_eq!(txn.budget_remaining(), None);
        println!("no limit => budget_remaining = None");
        txn.commit();
    }

    // --- Why alloc(vec![...]) is misleading for budgets ---
    let mut arena = Arena::new();
    {
        let mut txn = arena.transaction();
        txn.set_limit(100);
        // This stores 24 bytes (Vec struct), NOT 1000 bytes
        txn.alloc(vec![0u8; 1000]);
        println!(
            "alloc(vec![0u8; 1000]): arena used={} (Vec struct only), heap=~1000 (untracked)",
            txn.bytes_used()
        );
        txn.commit();
    }

    println!("\nAll budget examples passed.");
}
