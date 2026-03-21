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

### Arena

The main bump-pointer allocator. Every allocation returns an exclusive reference to arena-allocated memory.

#### `new()`

```rust
pub fn new() -> Self
```

Creates an arena with a 64 KiB initial block.

```rust
let arena = Arena::new();
```

#### `with_capacity()`

```rust
pub fn with_capacity(initial_bytes: usize) -> Self
```

Creates an arena with a custom initial block size. Choose a value close to expected peak usage to avoid early block chaining.

```rust
// Pre-allocate 1MB for expected workload
let arena = Arena::with_capacity(1024 * 1024);
```

#### `alloc()`

```rust
pub fn alloc<T>(&mut self, val: T) -> &mut T
```

Allocates a value of type `T`, returning an exclusive reference.

**Note:** Without `drop-tracking`, the destructor of `T` is never called.

```rust
struct Point { x: f64, y: f64 }

let mut arena = Arena::new();
let p = arena.alloc(Point { x: 1.0, y: 2.0 });
p.x = 3.0;
```

#### `alloc_slice()`

```rust
pub fn alloc_slice<T, I>(&mut self, iter: I) -> &mut [T]
where
    I: IntoIterator<Item = T>,
    I::IntoIter: ExactSizeIterator,
```

Allocates a contiguous slice from an `ExactSizeIterator`.

```rust
let mut arena = Arena::new();
let slice = arena.alloc_slice(0u32..100);
assert_eq!(slice.len(), 100);
```

#### `alloc_str()`

```rust
pub fn alloc_str(&mut self, s: &str) -> &str
```

Copies a string slice into the arena using SIMD-optimized `memcpy`.

```rust
let mut arena = Arena::new();
let s = arena.alloc_str("embedded string");
```

#### `alloc_uninit()`

```rust
pub fn alloc_uninit<T>(&mut self) -> &mut MaybeUninit<T>
```

Allocates space for `T` without initializing it. Caller must fully initialize before observation.

```rust
let mut arena = Arena::new();
let slot = arena.alloc_uninit::<u64>();
slot.write(42);
let val: &u64 = unsafe { slot.assume_init_ref() };
assert_eq!(*val, 42);
```

#### `alloc_raw()`, `alloc_zeroed()`, `alloc_cache_aligned()`

Low-level allocation methods for custom size/alignment requirements.

```rust
let mut arena = Arena::new();

// Raw bytes with alignment
let ptr = arena.alloc_raw(64, 64); // 64-byte aligned

// Zeroed memory
let ptr = arena.alloc_zeroed(128, 8);

// Cache-line aligned (64 bytes) for SIMD
let simd_buffer = arena.alloc_cache_aligned(256);
```

#### Fallible Allocation

```rust
pub fn try_alloc<T>(&mut self, val: T) -> Option<&mut T>
pub fn try_alloc_slice<T, I>(&mut self, iter: I) -> Option<&mut [T]>
pub fn try_alloc_str(&mut self, s: &str) -> Option<&str>
pub fn try_alloc_raw(&mut self, size: usize, align: usize) -> Option<NonNull<u8>>
```

Fallible variants that return `None` on OOM instead of panicking.

```rust
let mut arena = Arena::new();
if let Some(value) = arena.try_alloc(42) {
    // Success
}
```

#### `reset()`

```rust
pub fn reset(&mut self)
```

Resets the entire arena. No memory is freed — OS pages stay mapped and TLB-warm. With `drop-tracking`, destructors run first.

```rust
let mut arena = Arena::new();
// ... allocations ...
arena.reset(); // All memory available again
```

#### `checkpoint()` / `rewind()`

```rust
pub fn checkpoint(&self) -> Checkpoint
pub fn rewind(&mut self, cp: Checkpoint)
```

Snapshot and rollback allocations.

```rust
let mut arena = Arena::new();
arena.alloc(1);
let cp = arena.checkpoint();
arena.alloc(2);
arena.alloc(3);
arena.rewind(cp);
// Now only allocation 1 remains
```

#### `transaction()`

```rust
pub fn transaction(&mut self) -> Transaction<'_>
```

Opens a transactional scope with auto-rollback on drop.

```rust
let mut arena = Arena::new();
{
    let mut txn = arena.transaction();
    txn.alloc(1);
    txn.commit();
}
```

#### `stats()`

```rust
pub fn stats(&self) -> ArenaStats
```

Returns memory usage snapshot. O(1).

```rust
let arena = Arena::new();
let stats = arena.stats();
println!("Allocated: {} bytes", stats.bytes_allocated);
println!("Reserved: {} bytes", stats.bytes_reserved);
println!("Blocks: {}", stats.block_count);
```

---

### ArenaVec

An append-only growable vector backed by arena memory.

#### Creation

```rust
// Empty vector - no allocation until first push
let mut vec = ArenaVec::new(&mut arena);

// Pre-allocated
let mut vec = ArenaVec::with_capacity(&mut arena, 1000);
```

#### Operations

```rust
pub fn push(&mut self, val: T)      // Amortized O(1)
pub fn pop(&mut self) -> Option<T>
pub fn len(&self) -> usize
pub fn is_empty(&self) -> bool
pub fn capacity(&self) -> usize
pub fn as_slice(&self) -> &[T]
pub fn as_mut_slice(&mut self) -> &mut [T]
```

#### `finish()`

```rust
pub fn finish(self) -> &'arena mut [T]
```

Consumes the vector, returning a slice backed by arena memory. Element destructors will **not** be called.

```rust
let mut arena = Arena::new();
let mut vec = ArenaVec::new(&mut arena);
vec.push(1);
vec.push(2);
let slice = vec.finish();
// slice is &mut [u32] owned by arena
```

---

### Transaction

A scoped RAII transaction over an `Arena`.

#### Basic Operations

```rust
pub fn commit(mut self) -> TxnStatus     // Keep allocations
pub fn rollback(self) -> TxnStatus       // Discard allocations
pub fn savepoint(&mut self) -> Transaction<'_> // Nested transaction
```

#### Budget Control

```rust
pub fn set_limit(&mut self, bytes: usize)      // Set byte budget
pub fn budget_remaining(&self) -> Option<usize> // Get remaining budget
```

#### Metrics

```rust
pub fn bytes_used(&self) -> usize  // Bytes allocated since open
pub fn diff(&self) -> TxnDiff      // Allocation metrics
```

#### Transaction-Scoped Allocation

Transaction exposes the same allocation methods as Arena:

```rust
txn.alloc(val)           // Like arena.alloc
txn.alloc_str(s)         // Like arena.alloc_str
txn.try_alloc(val)       // Like arena.try_alloc
// ... etc
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

## Transaction Usage

### Closure API

```rust
let mut arena = Arena::new();

// Commits on Ok, rolls back on Err
let result = arena.with_transaction(|txn| -> Result<u32, &str> {
    Ok(*txn.alloc(21) * 2)
});
assert_eq!(result, Ok(42));

// Rollback on Err or panic
let result = arena.with_transaction(|txn| {
    txn.alloc(1);
    txn.alloc(2);
    Err("something failed") // Rolls back
});
assert!(result.is_err());
```

### Manual API

```rust
let mut arena = Arena::new();

// Explicit commit
{
    let mut txn = arena.transaction();
    txn.alloc(1);
    txn.commit();
}

// Auto-rollback on drop
{
    let mut txn = arena.transaction();
    txn.alloc(99);
    // dropped without commit → rolled back
}
```

### Nested Savepoints

```rust
let mut outer = arena.transaction();
outer.alloc(1);

{
    let mut inner = outer.savepoint();
    inner.alloc(2);
    inner.commit(); // Keep inner allocations
}

outer.commit(); // Keep everything
```

### Budget Enforcement

```rust
let mut txn = arena.transaction();
txn.set_limit(1024); // 1KB max

// Panics if over budget
txn.alloc(vec![0u8; 2000]);
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

### Benchmarks

| Operation | Time | vs Box/Vec |
|-----------|------|------------|
| `alloc u64` | ~1.7 µs | **10x faster** |
| `alloc_slice n=64` | ~35 ns | **2x faster** |
| `reset 1 block` | ~24 ns | — |
| `10k allocs + reset` | ~17 ms | **10x faster** |

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
- [Repository](https://github.com/themankindproject/fastarena)
- [Changelog](CHANGELOG.md)