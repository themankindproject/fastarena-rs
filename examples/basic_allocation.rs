//! Basic allocation examples.
//!
//! Demonstrates `alloc`, `alloc_slice`, `alloc_slice_copy`, `alloc_str`,
//! `alloc_uninit`, `alloc_raw`, `alloc_zeroed`, and `alloc_cache_aligned`.

use fastarena::Arena;

fn main() {
    let mut arena = Arena::new();

    // Single value allocation
    let x: &mut u64 = arena.alloc(42);
    println!("alloc(42u64) = {x}");

    // Slice from a range (ExactSizeIterator)
    let squares: &mut [u32] = arena.alloc_slice(0u32..8);
    println!("alloc_slice(0..8) = {squares:?}");

    // Fast memcpy for Copy types
    let src = [10u32, 20, 30, 40];
    let copied: &mut [u32] = arena.alloc_slice_copy(&src);
    println!("alloc_slice_copy = {copied:?}");

    // String allocation
    let s: &str = arena.alloc_str("hello, arena!");
    println!("alloc_str = {s}");

    // Uninitialized slot (caller must initialize before reading)
    let slot = arena.alloc_uninit::<u64>();
    slot.write(0xDEAD_BEEF);
    let val: &mut u64 = unsafe { slot.assume_init_mut() };
    println!("alloc_uninit wrote = {val:#x}");

    // Raw bytes with page alignment
    let raw = arena.alloc_raw(4096, 4096);
    println!(
        "alloc_raw(4096, 4096) ptr = {:p} (aligned: {})",
        raw,
        raw.as_ptr() as usize % 4096 == 0
    );

    // Zeroed allocation
    let zeroed = arena.alloc_zeroed(256, 8);
    let slice = unsafe { std::slice::from_raw_parts(zeroed.as_ptr(), 256) };
    println!("alloc_zeroed all zero? {}", slice.iter().all(|&b| b == 0));

    // Cache-line aligned allocation
    let buf = arena.alloc_cache_aligned(128);
    println!(
        "alloc_cache_aligned ptr = {:p} ({}-byte aligned: {})",
        buf,
        64,
        buf.as_ptr() as usize % 64 == 0
    );

    // With custom initial capacity
    let big_arena = Arena::with_capacity(1024 * 1024); // 1 MiB initial
    println!("with_capacity arena stats: {:?}", big_arena.stats());

    // Zero-cost reset — pages stay warm
    arena.reset();
    println!("After reset: {:?}", arena.stats());
}
