//! Fallible allocation examples.
//!
//! All allocation methods have `try_*` variants returning `Option`.

use fastarena::Arena;

fn main() {
    // --- try_alloc success ---
    let mut arena = Arena::new();
    match arena.try_alloc(42u64) {
        Some(val) => println!("try_alloc success: {val}"),
        None => println!("try_alloc failed"),
    }

    // --- try_alloc_slice success ---
    let s = arena.try_alloc_slice(0u32..8);
    println!("try_alloc_slice: {s:?}");

    // --- try_alloc_str success ---
    let s = arena.try_alloc_str("hello");
    println!("try_alloc_str: {s:?}");

    // --- try_alloc_raw success ---
    let p = arena.try_alloc_raw(16, 8);
    println!("try_alloc_raw: {}", p.is_some());

    // --- try_alloc_slice empty ---
    let empty: Option<&mut [u32]> = arena.try_alloc_slice(std::iter::empty());
    println!("try_alloc_slice empty: len={}", empty.unwrap().len());

    // --- try_alloc on a very small arena (forces OOM scenario) ---
    let mut tiny = Arena::with_capacity(64);
    match tiny.try_alloc(1u64) {
        Some(v) => println!("tiny arena try_alloc: {v}"),
        None => println!("tiny arena try_alloc: OOM"),
    }

    // --- try_alloc_str in transaction respects budget ---
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    txn.set_limit(3);
    let r = txn.try_alloc_str("hello");
    println!("try_alloc_str 'hello' with 3-byte budget: {r:?}");
    txn.commit();

    println!("\nAll fallible examples passed.");
}
