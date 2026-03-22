//! ArenaVec examples.
//!
//! `ArenaVec` is a growable vector backed by arena memory. Call `finish()`
//! to hand ownership to the arena — no destructor run, no copy.

use fastarena::{Arena, ArenaVec};

fn main() {
    let mut arena = Arena::new();

    // --- Basic push and index ---
    {
        let mut v = ArenaVec::new(&mut arena);
        for i in 0u32..10 {
            v.push(i * i);
        }
        println!("len={} cap={}", v.len(), v.capacity());
        println!("v[9] = {}", v[9]);
    }

    // --- finish() — ArenaVec consumed, slice now arena-owned ---
    let items: &mut [u32] = {
        let mut v = ArenaVec::new(&mut arena);
        for i in 0..1024 {
            v.push(i);
        }
        v.finish()
    };
    assert_eq!(items.len(), 1024);
    assert_eq!(items[512], 512);
    println!("finish() => slice of {} items", items.len());

    // --- extend_exact from ExactSizeIterator ---
    let slice = {
        let mut v = ArenaVec::new(&mut arena);
        v.extend_exact([10u64, 20, 30, 40, 50]);
        v.finish()
    };
    assert_eq!(slice, &[10, 20, 30, 40, 50]);
    println!("extend_exact => {slice:?}");

    // --- extend_from_slice (memcpy) ---
    {
        let mut v = ArenaVec::new(&mut arena);
        v.extend_from_slice(&[100u32, 200, 300]);
        println!("extend_from_slice => {:?}", v.as_slice());
    }

    // --- pop ---
    {
        let mut v = ArenaVec::new(&mut arena);
        v.extend_exact([1u32, 2, 3]);
        assert_eq!(v.pop(), Some(3));
        assert_eq!(v.pop(), Some(2));
        println!("after 2 pops: len={}", v.len());
    }

    // --- with_capacity (avoid reallocations) ---
    {
        let mut v = ArenaVec::<u64>::with_capacity(&mut arena, 32);
        assert_eq!(v.capacity(), 32);
        for i in 0u64..32 {
            v.push(i);
        }
        assert_eq!(v.capacity(), 32, "no reallocation expected");
        v.finish();
        println!("with_capacity: no realloc for 32 elements");
    }

    // --- inside a transaction via arena_mut() ---
    {
        let mut txn = arena.transaction();
        let _slice = {
            let mut v = ArenaVec::new(txn.arena_mut());
            v.extend_exact([1u32, 2, 3]);
            v.finish()
        };
        txn.commit();
        println!("ArenaVec in txn committed");
    }

    // --- reserve / try_reserve ---
    {
        let mut v = ArenaVec::<u32>::new(&mut arena);
        v.reserve(100);
        println!("after reserve(100): cap={}", v.capacity());
        assert!(v.try_reserve(1000).is_ok());
        println!("try_reserve(1000): cap={}", v.capacity());
    }

    // --- IndexMut ---
    {
        let mut v = ArenaVec::new(&mut arena);
        v.push(10u32);
        v.push(20);
        v[0] = 99;
        println!("IndexMut: v[0]={}", v[0]);
    }

    // --- clear ---
    {
        let mut v = ArenaVec::new(&mut arena);
        v.extend_exact([1u32, 2, 3]);
        v.clear();
        assert!(v.is_empty());
        println!("clear => is_empty={}", v.is_empty());
    }
}
