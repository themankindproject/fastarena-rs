# FastArena Usage Guide

> Zero-dependency bump-pointer arena allocator with RAII transactions, nested savepoints, and `ArenaVec` — built for compilers, storage engines, and high-throughput request-scoped workloads.

---

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Core API](#core-api)
  - [Arena](#arena)
  - [ArenaVec](#arenavec)
  - [Transaction](#transaction)
  - [ArenaStats](#arenastats)
- [Allocation Patterns](#allocation-patterns)
- [Transaction Usage](#transaction-usage)
- [Checkpoint and Rewind](#checkpoint-and-rewind)
- [Memory Management](#memory-management)
- [Performance](#performance)
- [Best Practices](#best-practices)
- [Feature Flags](#feature-flags)
- [MSRV Policy](#msrv-policy)
- [Safety Considerations](#safety-considerations)

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fastarena = "0.1"
```

For destructor tracking:

```toml
[dependencies]
fastarena = { version = "0.1", features = ["drop-tracking"] }
```

---

## Quick Start

### Basic Allocation

```rust
use fastarena::Arena;

let mut arena = Arena::new();

// Single value allocation
let num = arena.alloc(42);
assert_eq!(*num, 42);

// Slice from iterator
let squares = arena.alloc_slice(0u32..100);

// String interning
let hello = arena.alloc_str("Hello, World!");

// Zero-cost reset
arena.reset();
assert_eq!(arena.stats().bytes_allocated, 0);
```

### ArenaVec

```rust
use fastarena::{Arena, ArenaVec};

let mut arena = Arena::new();

let mut vec = ArenaVec::new(&mut arena);
vec.push(1);
vec.push(2);
vec.push(3);

// finish() transfers ownership to arena (no drop runs)
let slice = vec.finish();

assert_eq!(slice, &[1, 2, 3]);
```

---

## Core API

### ArenaVec

An append-only growable vector backed by arena memory. Similar API to `std::Vec` but backed by arena allocation for O(1) push and zero-cost bulk reclamation.

**Key differences from `Vec`:**
- `finish()` transfers ownership to arena without running destructors
- No individual element deallocation (append-only)
- Memory reclaimed in bulk via `arena.reset()`
- Can grow beyond arena's current block size automatically

#### Creation

**`ArenaVec::new()`** creates an empty vector with no initial allocation. Memory is allocated on first `push`:

```rust
let mut vec = ArenaVec::new(&mut arena);
// No allocation yet
vec.push(1); // First allocation happens here
```

**`ArenaVec::with_capacity()`** pre-allocates space for a known number of elements, avoiding growth copies:

```rust
let mut vec = ArenaVec::with_capacity(&mut arena, 1000);
// Space for 1000 elements pre-allocated
```

#### Operations

| Method | Description |
|--------|-------------|
| `push(val)` | Appends element. Amortized O(1) due to doubling growth |
| `pop()` | Removes and returns last element, or `None` if empty |
| `extend(iter)` | Extends from `ExactSizeIterator` with batch write optimization |
| `extend_from_slice(slice)` | Efficiently copies slice using SIMD-optimized memcpy |
| `reserve(n)` | Ensures capacity for `n` additional elements |
| `reserve_exact(n)` | Reserves exactly `n` additional elements |
| `try_reserve(n)` | Fallible version that returns `Err` on allocation failure |
| `len()` | Returns current element count |
| `is_empty()` | Returns `true` if vector has no elements |
| `capacity()` | Returns total allocated slots |
| `as_slice()` | Returns immutable slice view |
| `as_mut_slice()` | Returns mutable slice view |

#### `finish()`

Consumes the `ArenaVec` and returns a mutable slice backed by arena memory. **Important:** Element destructors will **not** be called after `finish()`. Use this when you want the arena to own the data.

```rust
let mut arena = Arena::new();
let mut vec = ArenaVec::new(&mut arena);
vec.push(1);
vec.push(2);
let slice = vec.finish();
// slice is &mut [u32] owned by arena
// vec is consumed, no drop runs
```

#### `extend_from_slice()`

Efficiently copies elements from a slice using SIMD-optimized `memcpy`. Faster than individual `push()` calls for bulk data:

```rust
let mut arena = Arena::new();
let mut vec = ArenaVec::new(&mut arena);
let data = [1, 2, 3, 4, 5, 6, 7, 8];
vec.extend_from_slice(&data);
// All 8 elements copied in one memcpy call
```

#### Capacity Management

The `reserve` methods pre-allocate space to avoid repeated growth copies:

**`reserve(additional)`** ensures capacity for at least `additional` more elements (may allocate more):

```rust
let mut arena = Arena::new();
let mut vec = ArenaVec::with_capacity(&mut arena, 0);
vec.reserve(1000); // Capacity >= 1000
```

**`reserve_exact(additional)`** reserves exactly `additional` elements (no extra capacity):

```rust
vec.reserve_exact(100);
```

**`try_reserve(additional)`** is the fallible variant that returns `Err` instead of panicking on allocation failure:

```rust
if vec.try_reserve(10000).is_err() {
    eprintln!("Arena out of memory");
}
```

#### Growth Strategy

`ArenaVec` uses a doubling growth strategy: when capacity is exhausted, it doubles capacity and copies existing elements. This ensures amortized O(1) push while maintaining reasonable memory overhead (max 50% wasted space).

For known sizes, use `with_capacity()` or `reserve()` to avoid growth copies entirely.

---

### Transaction

A scoped RAII transaction over an `Arena` that provides automatic rollback on failure. Transactions are ideal for speculative operations where you want to either commit all changes or roll back to the initial state.

**Key concepts:**
- **Auto-rollback**: If a transaction is dropped without calling `commit()`, all allocations are rolled back
- **Nested savepoints**: Create nested transactions that can be partially rolled back
- **Budget enforcement**: Limit memory usage per transaction
- **Metrics tracking**: Monitor allocation behavior within transactions

#### Basic Operations

| Method | Description |
|--------|-------------|
| `commit()` | Commits transaction, keeping all allocations |
| `rollback()` | Explicitly rolls back (same as dropping without commit) |
| `savepoint()` | Creates a nested transaction (child of current) |
| `set_limit(bytes)` | Sets byte budget; panics on exceeded |
| `budget_remaining()` | Returns remaining budget or `None` if unlimited |
| `bytes_used()` | Bytes allocated since transaction opened |
| `diff()` | Returns `TxnDiff` with allocation metrics |

#### Transaction-Scoped Allocation

Transaction exposes the same allocation methods as Arena:

```rust
txn.alloc(val)           // Like arena.alloc
txn.alloc_str(s)         // Like arena.alloc_str
txn.try_alloc(val)       // Like arena.try_alloc
txn.alloc_slice(iter)    // Like arena.alloc_slice
// ... all alloc variants available
```

#### Closure API (Recommended)

The `with_transaction` closure API is recommended for cleaner error handling:

```rust
let mut arena = Arena::new();

// Commits on Ok, rolls back on Err
let result = arena.with_transaction(|txn| -> Result<u32, &str> {
    let x = txn.alloc(21);
    Ok(*x * 2)
});
assert_eq!(result, Ok(42)); // Transaction committed

// Rollback on Err
let result = arena.with_transaction(|txn| {
    txn.alloc(1);
    txn.alloc(2);
    Err("validation failed") // Rolls back both allocations
});
assert!(result.is_err()); // Transaction rolled back
```

#### Manual API

For more control, use the manual transaction API:

```rust
let mut arena = Arena::new();

// Explicit commit
{
    let mut txn = arena.transaction();
    txn.alloc(1);
    txn.alloc(2);
    txn.commit(); // Both allocations kept
}

// Auto-rollback on drop
{
    let mut txn = arena.transaction();
    txn.alloc(99);
    // dropped without commit → rolled back
}
```

#### Nested Savepoints

Transactions support nested savepoints for partial rollback:

```rust
let mut arena = Arena::new();
let mut outer = arena.transaction();
outer.alloc(1); // Will be kept

{
    let mut inner = outer.savepoint();
    inner.alloc(2); // Will be rolled back
    inner.alloc(3); // Will be rolled back
    // inner dropped without commit → rolled back
}

outer.alloc(4); // Will be kept
outer.commit(); // Keep 1 and 4
```

#### Budget Enforcement

Set a byte limit to prevent unbounded allocation:

```rust
let mut arena = Arena::new();
let mut txn = arena.transaction();
txn.set_limit(1024); // Max 1KB

// This panics - exceeds budget
// txn.alloc(vec![0u8; 2000]);
```

#### Metrics

Track allocation behavior:

```rust
let mut txn = arena.transaction();
txn.alloc(1);
txn.alloc(2);

let diff = txn.diff();
println!("Bytes: {}", diff.bytes_allocated);
println!("Blocks touched: {}", diff.blocks_touched);
```

---

### ArenaStats

Memory usage snapshot.

| Field | Type | Description |
|-------|------|-------------|
| `bytes_allocated` | `usize` | Bytes in live allocations |
| `bytes_reserved` | `usize` | Total bytes across all blocks |
| `block_count` | `usize` | Number of owned blocks |

#### Methods

```rust
pub fn utilization(&self) -> f64  // Fraction in use [0.0, 1.0]
pub fn bytes_idle(&self) -> usize  // Reserved but not allocated
```

---

## Allocation Patterns

### Zero-Sized Types (ZST)

ZSTs don't allocate but count toward `len()`:

```rust
let mut arena = Arena::new();
let vec: ArenaVec<()> = ArenaVec::new(&mut arena);
for _ in 0..1000 { vec.push(()); }
assert_eq!(vec.len(), 1000);
```

### Alignment Handling

Automatic alignment to type requirements:

```rust
let mut arena = Arena::new();
let value = arena.alloc(42u64); // Automatically 8-byte aligned
```

### Multiple Blocks

Automatic block allocation when current block is exhausted:

```rust
let mut arena = Arena::with_capacity(64);
for i in 0..1000 { arena.alloc(i); }
println!("Blocks: {}", arena.block_count());
```

---

## Checkpoint and Rewind

### Basic Rewind

```rust
let mut arena = Arena::new();
arena.alloc(1);
let cp = arena.checkpoint();
arena.alloc(2);
arena.alloc(3);
arena.rewind(cp);
// Only allocation 1 remains
```

### Multiple Checkpoints

```rust
let mut arena = Arena::new();
arena.alloc(1);

let cp1 = arena.checkpoint();
arena.alloc(2);

let cp2 = arena.checkpoint();
arena.alloc(3);

// Rewind to different points
arena.rewind(cp1); // Only 1
arena.alloc(4);
arena.rewind(cp2); // 1, 2, then 4
```

---

## Memory Management

### Bulk Reclamation

```rust
let mut arena = Arena::new();
for i in 0..10000 {
    arena.alloc(i);
}
arena.reset(); // All 10,000 allocations freed at once
```

### Drop Tracking

Enable `drop-tracking` for automatic destructor execution:

```rust
// Cargo.toml: fastarena = { features = ["drop-tracking"] }

let mut arena = Arena::new();
arena.alloc(String::from("hello"));
arena.reset(); // String destructor runs
```

### Block Reuse

Blocks are retained after reset/rewind:

```rust
let mut arena = Arena::new();
for _ in 0..10 {
    arena.alloc(vec![0u8; 1000]);
    arena.reset(); // Memory stays allocated
}
```

---

## Performance

### Benchmarks (vs bumpalo, typed-arena)

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc 1k items | 1881 ns | 925 ns | 994 ns |
| alloc_slice n=64 | **12 ns** | 49 ns | 72 ns |
| ArenaVec n=4096 | **2.2 µs** | 8.2 µs | 10.0 µs |
| 10k allocs + reset | 17.1 µs | 14.5 µs | 2.6 µs* |
| large 128KB alloc | 59 ns | 27 ns | — |

*typed-arena reallocates fresh each iteration; not comparable.

### Complexity

| Operation | Complexity |
|-----------|-------------|
| `alloc` (fast path) | O(1) |
| `alloc` (new block) | O(1) amortized |
| `checkpoint` | O(1) |
| `rewind` | O(k) where k ≈ 0-1 |
| `reset` | O(b) where b = blocks |
| `stats` | O(1) |

---

## Best Practices

### 1. Pre-allocate Initial Capacity

```rust
// Bad: May trigger early block chaining
let arena = Arena::new();

// Good: Avoids block chaining for known workloads
let arena = Arena::with_capacity(1024 * 1024);
```

### 2. Reset Over Recreate

```rust
// Bad: Allocates new memory each time
for batch in batches {
    let mut arena = Arena::new();
    process(&mut arena, batch);
}

// Good: Reuses warm pages
let mut arena = Arena::new();
for batch in batches {
    process(&mut arena, batch);
    arena.reset();
}
```

### 3. Use Transactions for Batch Operations

```rust
let mut arena = Arena::new();
arena.with_transaction(|txn| {
    for item in items {
        txn.alloc(process(item));
    }
    Ok(results)
});
```

### 4. Use ArenaVec for Dynamic Collections

```rust
// ArenaVec has similar performance to Vec with arena backing
let mut vec = ArenaVec::new(&mut arena);
for item in items {
    vec.push(transform(item));
}
let slice = vec.finish();
```

---

## Feature Flags

```toml
[dependencies]
# Default (no features)
fastarena = "0.1"

# Enable drop tracking
fastarena = { version = "0.1", features = ["drop-tracking"] }
```

| Feature | Default | Description |
|---------|---------|-------------|
| `drop-tracking` | Off | Run destructors in LIFO order on `reset`/`rewind` |

---

## MSRV Policy

**Minimum Supported Rust Version: 1.66.0**

The MSRV may increase in minor releases. When updating, check the changelog.

---

## Safety Considerations

### Lifetime Guarantees

Arena-allocated references are valid until `reset()` or `rewind()` is called:

```rust
let mut arena = Arena::new();
let x = arena.alloc(42);
arena.reset();
// ⚠️ 'x' is now a dangling pointer!
```

### No Thread Safety

Arena is not thread-safe. For multi-threaded use:

```rust
// Thread-local (recommended for request-scoped)
thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
}

// Or shared with mutex
use std::sync::Mutex;
let arena = Mutex::new(Arena::new());
```

### Drop semantically Changed with ArenaVec

- **`finish()`**: Destructors NOT called; arena owns memory
- **`drop` without `finish()`**: Destructors run immediately; arena retains backing memory

---

## License

MIT — See [LICENSE](LICENSE) file.

## Links

- [Crates.io](https://crates.io/crates/fastarena)
- [Documentation](https://docs.rs/fastarena)
- [Repository](https://github.com/themankindproject/fastarena-rs)
- [Changelog](CHANGELOG.md)