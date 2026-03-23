# fastarena — Complete Usage Guide

> Zero-dependency bump-pointer arena allocator with RAII transactions, nested savepoints, optional destructor tracking, and `ArenaVec` — built for compilers, storage engines, and high-throughput request-scoped workloads.

---

## Table of Contents

- [Why fastarena?](#why-fastarena)
- [Installation](#installation)
- [Quick Start](#quick-start)
  - [Basic Allocation](#basic-allocation)
  - [Transactions — Auto-Rollback on Failure](#transactions--auto-rollback-on-failure)
  - [Nested Savepoints](#nested-savepoints)
  - [ArenaVec with `finish()`](#arenavec-with-finish--transfer-ownership-to-the-arena)
  - [Transaction Budgets](#transaction-budgets--cap-memory-per-request)
  - [Drop-Tracking](#drop-tracking--opt-in-destructor-execution)
- [API Reference](#api-reference)
  - [Arena](#arena)
  - [Transaction](#transaction)
  - [ArenaVec](#arenavec)
  - [ArenaStats](#arenastats)
  - [Checkpoint](#checkpoint)
- [Performance](#performance)
- [Best Practices](#best-practices)
- [Feature Flags](#feature-flags)
- [Safety Considerations](#safety-considerations)
- [MSRV](#minimum-supported-rust-version)

---

## Why fastarena?

| Feature | fastarena | bumpalo | typed-arena |
|---------|-----------|---------|-------------|
| O(1) bump allocation | Yes | Yes | Yes |
| Heterogeneous types | Yes | Yes | No |
| Checkpoint / Rewind | Yes (O(1) snapshot) | No | No |
| RAII Transaction | Yes | No | No |
| Nested Savepoints | Yes | No | No |
| Transaction Budget | Yes | No | No |
| Transaction Metrics | Yes (`TxnDiff`) | No | No |
| `with_transaction` closure API | Yes | No | No |
| `ArenaVec::finish()` ownership transfer | Yes | No | N/A |
| Drop-tracking (opt-in) | Yes | No | Forced |
| Fallible allocation | Yes | Partial | No |
| `alloc_cache_aligned` | Yes | No | No |
| O(1) stats with utilization | Yes | Partial | No |
| Zero dependencies | Yes | Yes | Yes |

---

## Installation

```toml
[dependencies]
fastarena = "0.1.1"
```

With destructor tracking:

```toml
[dependencies]
fastarena = { version = "0.1.1", features = ["drop-tracking"] }
```

---

## Quick Start

### Basic Allocation

```rust
use fastarena::Arena;

let mut arena = Arena::new();

let x: &mut u64 = arena.alloc(42);
let squares: &mut [u32] = arena.alloc_slice(0u32..8);
let s: &str = arena.alloc_str("hello");

// Zero-cost reset — pages stay warm, no OS calls
arena.reset();
```

### Transactions — Auto-Rollback on Failure

No other arena allocator gives you RAII transactions. Allocations succeed or roll back as a unit — no leaks, no manual cleanup.

> **Note:** Each `alloc` call returns a `&mut T` that borrows the transaction.
> Don't hold references across subsequent allocations — use the value and let it
> drop before calling `alloc` again.

```rust
use fastarena::Arena;

let mut arena = Arena::new();

// Ok commits, Err rolls back — all allocations are atomic
let result: Result<u32, &str> = arena.with_transaction(|txn| {
    txn.alloc_str("fastarena");
    let score = txn.alloc(100u32);
    Ok(*score)
});
assert_eq!(result, Ok(100));

// Failed transaction — everything allocated inside is gone
arena.with_transaction(|txn| {
    txn.alloc(1u32);
    txn.alloc(2u32);
    Err("abort")  // both u32s rolled back automatically
});

// Infallible variant — commits even through panic
let val = arena.with_transaction_infallible(|txn| {
    *txn.alloc(7u32) * 6
});
assert_eq!(val, 42);
```

### Nested Savepoints

Transactions nest to arbitrary depth. Each savepoint is independently committable — roll back an inner scope without losing outer work.

```rust
use fastarena::Arena;

let mut arena = Arena::new();
let mut outer = arena.transaction();
outer.alloc_str("top-level");

{
    let mut inner = outer.savepoint();
    inner.alloc_str("speculative-opt");
    inner.alloc(999u32);
    // dropped without commit — inner work discarded, outer untouched
}

outer.alloc_str("confirmed");
outer.commit();  // "top-level" + "confirmed" survive
```

### ArenaVec with `finish()` — Transfer Ownership to the Arena

`ArenaVec` is a growable vector backed by arena memory. Call `finish()` to hand ownership to the arena — no destructor run, no copy. The slice lives as long as the arena.

```rust
use fastarena::{Arena, ArenaVec};

let mut arena = Arena::new();

let items: &mut [u32] = {
    let mut v = ArenaVec::new(&mut arena);
    for i in 0..1024 {
        v.push(i);
    }
    v.finish()  // ArenaVec consumed, slice now arena-owned
};

assert_eq!(items.len(), 1024);
assert_eq!(items[512], 512);
```

### Transaction Budgets — Cap Memory per Request

Set a byte budget on any transaction. Exceed it and `alloc` panics (or `try_alloc` returns `None`). Zero-cost when unlimited.

> **Note:** `alloc(vec![0u8; N])` stores only the `Vec` struct (24 bytes) in the
> arena — the heap buffer is *not* arena-tracked. To budget actual data bytes,
> use `alloc_slice` or `alloc_slice_copy` instead.

```rust
use fastarena::Arena;

let mut arena = Arena::new();
let mut txn = arena.transaction();
txn.set_limit(4096);  // hard cap

// GOOD — alloc_slice copies data into arena, budget sees all bytes
txn.alloc_slice(vec![0u8; 2048]);  // ok — 2048 arena bytes
// txn.alloc_slice(vec![0u8; 4096]);  // panics: budget exceeded (2048 + 4096 > 4096)

// BAD — alloc(vec![...]) stores only the 24-byte Vec struct, heap is untracked
// txn.alloc(vec![0u8; 9999]);  // would NOT panic — budget sees 24 bytes

let remaining = txn.budget_remaining();  // introspect at any time
txn.commit();
```

### Drop-Tracking — Opt-In Destructor Execution

By default, fastarena never runs destructors (zero overhead). Enable `drop-tracking` to run them in LIFO order on `reset()` / `rewind()`.

```toml
[dependencies]
fastarena = { version = "0.1.1", features = ["drop-tracking"] }
```

```rust
use fastarena::Arena;

let mut arena = Arena::new();
let cp = arena.checkpoint();

arena.alloc(String::from("hello"));
arena.alloc(String::from("world"));

// With drop-tracking: drops fire in LIFO order ("world", then "hello")
// Without drop-tracking: no destructors, memory reclaimed instantly
arena.rewind(cp);
```

---

## API Reference

### Arena

The main bump-pointer allocator.

#### Constructors

| Method | Description |
|--------|-------------|
| `Arena::new()` | Creates arena with 64 KiB initial block |
| `Arena::with_capacity(bytes)` | Creates arena with custom initial block size |

```rust
let arena = Arena::new();
let arena = Arena::with_capacity(1024 * 1024); // 1 MiB initial
```

#### Allocation Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `alloc` | `fn alloc<T>(&mut self, val: T) -> &mut T` | O(1) single value |
| `alloc_slice` | `fn alloc_slice<T, I>(&mut self, iter: I) -> &mut [T]` | From `ExactSizeIterator` |
| `alloc_slice_copy` | `fn alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> &mut [T]` | Single `memcpy` for `Copy` types |
| `alloc_str` | `fn alloc_str(&mut self, s: &str) -> &str` | Copy string into arena |
| `alloc_uninit` | `fn alloc_uninit<T>(&mut self) -> &mut MaybeUninit<T>` | Uninitialized slot |
| `alloc_raw` | `fn alloc_raw(&mut self, size: usize, align: usize) -> NonNull<u8>` | Raw bytes with alignment |
| `alloc_zeroed` | `fn alloc_zeroed(&mut self, size: usize, align: usize) -> NonNull<u8>` | Zeroed raw bytes |
| `alloc_cache_aligned` | `fn alloc_cache_aligned(&mut self, size: usize) -> NonNull<u8>` | 64-byte aligned (SIMD-friendly) |

```rust
let mut arena = Arena::new();

let p = arena.alloc(Point { x: 1.0, y: 2.0 });
let slice = arena.alloc_slice(0u32..100);
let s = arena.alloc_str("hello");
let raw = arena.alloc_raw(4096, 4096);   // page-aligned
let buf = arena.alloc_cache_aligned(256); // cache-line aligned
```

#### Fallible Allocation

All allocation methods have `try_*` variants returning `Option`:

| Method | Signature |
|--------|-----------|
| `try_alloc` | `fn try_alloc<T>(&mut self, val: T) -> Option<&mut T>` |
| `try_alloc_slice` | `fn try_alloc_slice<T, I>(&mut self, iter: I) -> Option<&mut [T]>` |
| `try_alloc_slice_copy` | `fn try_alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> Option<&mut [T]>` |
| `try_alloc_str` | `fn try_alloc_str(&mut self, s: &str) -> Option<&str>` |
| `try_alloc_raw` | `fn try_alloc_raw(&mut self, size: usize, align: usize) -> Option<NonNull<u8>>` |
| `try_alloc_zeroed` | `fn try_alloc_zeroed(&mut self, size: usize, align: usize) -> Option<NonNull<u8>>` |
| `try_alloc_cache_aligned` | `fn try_alloc_cache_aligned(&mut self, size: usize) -> Option<NonNull<u8>>` |

```rust
let mut arena = Arena::with_capacity(64);
match arena.try_alloc(42u64) {
    Some(val) => { /* success */ }
    None => { /* out of memory */ }
}
```

#### Checkpoint / Rewind / Reset

| Method | Signature | Complexity | Description |
|--------|-----------|------------|-------------|
| `checkpoint` | `fn checkpoint(&self) -> Checkpoint` | O(1) | Snapshot current position |
| `rewind` | `fn rewind(&mut self, cp: Checkpoint)` | O(k) | Roll back to checkpoint |
| `reset` | `fn reset(&mut self)` | O(p) | Reset entire arena |

> **`reset()` complexity:** O(p) where p = peak block count since last reset.
> A `high_water_mark` tracks the highest block index ever reached. On reset,
> only blocks 0..=high_water_mark have their offsets zeroed — all their
> capacity is fully reusable with no waste. Single-block arenas pay O(1).
> No memory is freed — OS pages stay mapped and TLB-warm.

```rust
let mut arena = Arena::new();
arena.alloc(1u64);

let cp = arena.checkpoint();   // O(1) — copies 3 integers
arena.alloc(2u64);
arena.alloc(3u64);
arena.rewind(cp);              // 2 and 3 gone; blocks retained for reuse

// reset() — reclaims all memory for reuse, O(peak_blocks)
arena.reset();                 // fast, warm pages, no OS calls
```

#### Transaction Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `transaction` | `fn transaction(&mut self) -> Transaction<'_>` | Open manual transaction |
| `with_transaction` | `fn with_transaction<F, T, E>(&mut self, f: F) -> Result<T, E>` | Ok=commit, Err=rollback |
| `with_transaction_infallible` | `fn with_transaction_infallible<F, T>(&mut self, f: F) -> T` | Commits even through panic |
| `transaction_depth` | `fn transaction_depth(&self) -> usize` | Current nesting depth |

#### Introspection

| Method | Signature | Complexity |
|--------|-----------|------------|
| `stats` | `fn stats(&self) -> ArenaStats` | O(1) |
| `block_count` | `fn block_count(&self) -> usize` | O(1) |

---

### Transaction

A scoped RAII guard over an `Arena`. Auto-rolls back on drop unless committed.

#### Lifecycle

| Method | Signature | Description |
|--------|-----------|-------------|
| `commit` | `fn commit(self) -> TxnStatus` | Keep all allocations |
| `rollback` | `fn rollback(self) -> TxnStatus` | Explicit rollback |
| `savepoint` | `fn savepoint(&mut self) -> Transaction<'_>` | Open nested transaction |

#### Budget

| Method | Signature | Description |
|--------|-----------|-------------|
| `set_limit` | `fn set_limit(&mut self, bytes: usize)` | Set byte cap |
| `budget_remaining` | `fn budget_remaining(&self) -> Option<usize>` | Remaining budget |

#### Introspection

| Method | Signature | Description |
|--------|-----------|-------------|
| `bytes_used` | `fn bytes_used(&self) -> usize` | Bytes allocated since open |
| `diff` | `fn diff(&self) -> TxnDiff` | Allocation metrics |
| `depth` | `fn depth(&self) -> usize` | Nesting depth (1=top-level) |
| `arena_depth` | `fn arena_depth(&self) -> usize` | Arena's total nesting depth |
| `checkpoint` | `fn checkpoint(&self) -> Checkpoint` | The saved checkpoint |
| `arena_mut` | `fn arena_mut(&mut self) -> &mut Arena` | Direct arena access |
| `is_committed` | `fn is_committed(&self) -> bool` | Whether committed |

#### Allocation

Transaction exposes all the same allocation methods as Arena (`alloc`, `alloc_slice`, `alloc_str`, `alloc_raw`, `try_alloc`, etc.) — all budget-checked.

#### Usage Patterns

**Closure API (recommended):**
```rust
let result = arena.with_transaction(|txn| {
    let x = txn.alloc(21u32);
    Ok(*x * 2)
}); // Ok(42) — committed

let result = arena.with_transaction(|txn| {
    txn.alloc(1u32);
    Err("fail")
}); // Err("fail") — rolled back
```

**Manual API:**
```rust
let mut txn = arena.transaction();
txn.alloc(1u32);
txn.alloc(2u32);
txn.commit(); // or drop to rollback
```

**Nested savepoints:**
```rust
let mut outer = arena.transaction();
outer.alloc(1u32);

{
    let mut inner = outer.savepoint();
    inner.alloc(2u32);
    // dropped — only inner rolled back
}

outer.commit(); // 1 survives
```

**Budget enforcement:**
```rust
let mut txn = arena.transaction();
txn.set_limit(1024);
txn.alloc_slice(vec![0u8; 512]);       // ok — 512 arena bytes
// txn.alloc_slice(vec![0u8; 1024]);   // panics: budget exceeded
txn.try_alloc_slice(vec![0u8; 1024]);  // returns None
txn.commit();
```

---

### ArenaVec

An append-only growable vector backed by arena memory.

#### Construction

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `fn new(arena: &'arena mut Arena) -> Self` | Empty vector |
| `with_capacity` | `fn with_capacity(arena: &'arena mut Arena, cap: usize) -> Self` | Pre-allocated |

#### Operations

| Method | Signature | Description |
|--------|-----------|-------------|
| `push` | `fn push(&mut self, val: T)` | Amortized O(1) append |
| `pop` | `fn pop(&mut self) -> Option<T>` | Remove last element |
| `clear` | `fn clear(&mut self)` | Clear, keep capacity |
| `extend_exact` | `fn extend_exact<I>(&mut self, iter: I)` | Batch from `ExactSizeIterator` |
| `extend_from_slice` | `fn extend_from_slice(&mut self, slice: &[T]) where T: Copy` | Single `memcpy` |
| `finish` | `fn finish(self) -> &'arena mut [T]` | Transfer to arena |

#### Capacity

| Method | Signature | Description |
|--------|-----------|-------------|
| `reserve` | `fn reserve(&mut self, additional: usize)` | Reserve (may over-allocate) |
| `reserve_exact` | `fn reserve_exact(&mut self, additional: usize)` | Reserve exactly |
| `try_reserve` | `fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError>` | Fallible |
| `capacity` | `fn capacity(&self) -> usize` | Total slots |

#### Introspection

| Method | Signature |
|--------|-----------|
| `len` | `fn len(&self) -> usize` |
| `is_empty` | `fn is_empty(&self) -> bool` |
| `as_slice` | `fn as_slice(&self) -> &[T]` |
| `as_mut_slice` | `fn as_mut_slice(&mut self) -> &mut [T]` |

#### Trait Implementations

`ArenaVec` implements `Index<usize>`, `IndexMut<usize>`, `Extend<T>`, `IntoIterator` (owned, `&`, `&mut`), and `ExactSizeIterator`.

#### The `finish()` Semantic

This is the key differentiator from `bumpalo::collections::Vec`:

```rust
// finish() — ArenaVec consumed, no destructor runs, arena owns the data
let slice: &mut [u32] = {
    let mut v = ArenaVec::new(&mut arena);
    v.push(1); v.push(2); v.push(3);
    v.finish()
}; // slice lives as long as the arena

// drop without finish() — element destructors run immediately
{
    let mut v = ArenaVec::new(&mut arena);
    v.push(String::from("hello"));
} // String::drop() fires here; arena retains backing memory
```

#### Inside Transactions

Use `txn.arena_mut()` to create an `ArenaVec` within a transaction:

```rust
let mut txn = arena.transaction();
let slice = {
    let mut v = ArenaVec::new(txn.arena_mut());
    v.extend_exact([1u32, 2, 3]);
    v.finish()
};
txn.commit(); // slice survives
```

---

### ArenaStats

O(1) memory usage snapshot.

```rust
pub struct ArenaStats {
    pub bytes_allocated: usize,   // In-use bytes
    pub bytes_reserved: usize,    // Total reserved across all blocks
    pub block_count: usize,       // Number of blocks
}

impl ArenaStats {
    pub fn utilization(&self) -> f64;  // bytes_allocated / bytes_reserved
    pub fn bytes_idle(&self) -> usize; // bytes_reserved - bytes_allocated
}
```

```rust
let stats = arena.stats();
println!("{:.1}% utilized", stats.utilization() * 100.0);
println!("{} bytes idle", stats.bytes_idle());
```

---

### Checkpoint

Opaque snapshot of arena position. `Copy` — zero-cost to pass around.

```rust
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint { /* opaque */ }
```

---

## Performance

### Head-to-Head: fastarena vs bumpalo vs typed-arena

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc 1k items | **894 ns** | 937 ns | 1072 ns |
| alloc_slice n=64 | **10 ns** | 53 ns | 84 ns |
| alloc_slice n=1024 | **64 ns** | 518 ns | — |
| alloc_str (100x) | 202 ns | **190 ns** | — |
| ArenaVec n=16 | **30 ns** | 42 ns | 34 ns |
| ArenaVec n=256 | **263 ns** | 346 ns | 516 ns |
| ArenaVec n=4096 | **3.4 µs** | 8.5 µs | 11.1 µs |
| 10k allocs + reset | **15.0 µs** | 15.1 µs | 2.8 µs† |
| reset (1 block) | **20 ns** | 696 ns | — |
| 128 KB alloc | 63 ns | **27 ns** | — |

† typed-arena drops and re-creates the arena each iteration; not directly comparable.

### Fast Path vs Box/Vec

| Benchmark | fastarena | Box/Vec | Speedup |
|-----------|-----------|---------|---------|
| alloc 1k u64 | **864 ns** | 15732 ns | **18x** |
| alloc_slice n=512 | **59 ns** | 65 ns | ~1x |
| alloc_slice n=4096 | **246 ns** | 236 ns | ~1x |
| 10k allocs + reset | **15.5 µs** | 211.8 µs | **14x** |
| `Arena::new` | **22 ns** | — | — |
| `checkpoint()` | **93 ns** | — | — |
| `reset` 1 block | **20 ns** | — | — |
| `commit` 16 allocs | **1.3 µs** | — | — |

### Why fastarena excels

- **5-8x faster slice allocation** than bumpalo (batch write in tight loop)
- **2-3x faster ArenaVec** than bumpalo/typed-arena for bulk collection building
- **Tied on alloc** — on par with bumpalo for single-item allocation
- **14x faster than Box** for bulk alloc + reclaim cycles
- **35x faster reset** than bumpalo for single-block arenas (O(peak) block reuse)
- **Zero dependencies**: No external crates required

### Complexity

| Operation | Complexity |
|-----------|------------|
| `alloc` (fast path) | O(1) |
| `alloc` (new block) | O(1) amortized |
| `checkpoint` | O(1) |
| `rewind` | O(k) where k ≈ 0–1 |
| `reset` | O(p) where p = peak blocks since last reset |
| `stats` | O(1) |

---

## Best Practices

### 1. Pre-allocate for Known Workloads

```rust
// Avoids early block chaining
let arena = Arena::with_capacity(1024 * 1024);
```

### 2. Reset Over Recreate

```rust
// Good: warm pages, no OS calls, O(peak_blocks) — all capacity reused
let mut arena = Arena::new();
for batch in batches {
    process(&mut arena, batch);
    arena.reset();  // zeros only blocks used since last reset
}
```

`reset()` tracks a `high_water_mark` internally — it only touches blocks
that were actually used, leaving untouched blocks' offsets at zero.
All memory is fully reusable with zero waste.

### 3. Use Transactions for Fallible Operations

```rust
arena.with_transaction(|txn| {
    let parsed = parse(txn, input)?;
    let optimized = optimize(txn, parsed)?;
    Ok(optimized)
}); // auto-rollback on any Err
```

### 4. Use `finish()` When Building Then Freezing

```rust
let slice: &mut [u32] = {
    let mut v = ArenaVec::new(&mut arena);
    for i in 0..n { v.push(compute(i)); }
    v.finish()  // arena owns it now
};
```

### 5. Set Budgets in Server Contexts

```rust
let mut txn = arena.transaction();
txn.set_limit(64 * 1024); // 64 KB per request
handle_request(&mut txn);
txn.commit();
```

---

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `drop-tracking` | Off | Run destructors in LIFO order on `reset`/`rewind` |

```toml
# Default (zero-cost, no destructor calls)
fastarena = "0.1.1"

# With drop-tracking
fastarena = { version = "0.1.1", features = ["drop-tracking"] }
```

---

## Safety Considerations

### Lifetime Guarantees

Arena-allocated references are invalidated by `reset()` / `rewind()`:

```rust
let mut arena = Arena::new();
let x = arena.alloc(42);
arena.reset();
// x is now dangling — UB if used
```

### No Thread Safety

`Arena` is `!Send` + `!Sync`. For multi-threaded use:

```rust
// Thread-local (recommended for request-scoped workloads)
thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
}

// Or wrap in Mutex (adds contention)
let arena = Mutex::new(Arena::new());
```

### `finish()` vs `drop()`

- `finish()` — destructors **not** called; arena owns the data
- `drop()` without `finish()` — destructors run immediately; arena retains memory

---

## Minimum Supported Rust Version

**Rust 1.66.0**
