# fastarena

[![Crates.io](https://img.shields.io/crates/v/fastarena)](https://crates.io/crates/fastarena)
[![Documentation](https://docs.rs/fastarena/badge.svg)](https://docs.rs/fastarena)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Build Status](https://img.shields.io/github/actions/workflow/status/themankindproject/fastarena-rs/ci.yml)](https://github.com/themankindproject/fastarena-rs/actions)
![Rust Version](https://img.shields.io/badge/rust-1.66%2B-blue)

A zero-dependency bump-pointer arena allocator with RAII transactions, nested savepoints, optional destructor tracking, and `ArenaVec` — built for compilers, storage engines, and high-throughput request-scoped workloads.

## Why FastArena?

| Feature | Description |
|---------|-------------|
| **Zero-copy** | Allocations return direct references, no indirection |
| **O(1) allocation** | Single bounds check + bump pointer advance |
| **Zero-cost reset** | Reuse all memory without OS calls or page faults |
| **Transactions** | RAII guard with commit/rollback, nested savepoints |
| **Drop-tracking** | Opt-in destructor execution — zero-cost when off |
| **Budget enforcement** | Cap bytes per transaction for request-scoped safety |

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
use fastarena::Arena;
use fastarena::ArenaVec;

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

> **Note:** The budget tracks bytes written to arena blocks only. Heap allocations
> inside values (e.g. `Vec`, `String`) are **not** tracked — use `alloc_slice`
> or `alloc_slice_copy` to budget actual data bytes.

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
fastarena = { version = "0.1.2", features = ["drop-tracking"] }
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

## Use Cases

- **Compiler AST / parsers** — allocate all nodes per pass, reset in bulk
- **Graphs and cyclic structures** — same-lifetime references enable safe cycles without `Rc`/`RefCell`
- **Trees with parent pointers** — back-references trivially supported
- **Heterogeneous types** — allocate `Node`, `Edge`, `Token` in a single arena
- **Phase-oriented bulk alloc/free** — many objects created, bulk-freed via `reset()` or `rewind()`
- **Request-scoped memory** — thread-local arena per HTTP request, zero-cost recycle
- **Transactional batch processing** — commit on success, auto-rollback on failure, nested savepoints
- **Dynamic collections** — `ArenaVec` with O(1) push, arena-backed lifetime

See [USAGE.md](USAGE.md) for full examples.

## Performance

### Head-to-head: fastarena vs bumpalo vs typed-arena

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
| reset (4 blocks) | **167 ns** | — | — |
| 128 KB alloc | 63 ns | **27 ns** | — |

† typed-arena drops and re-creates the arena each iteration; not directly comparable.

### Fast path benchmarks (vs std Box/Vec)

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

## Feature Flags

```toml
[dependencies]
fastarena = { version = "0.1.2", features = ["drop-tracking"] }
```

| Flag | Default | Description |
|------|---------|-------------|
| `drop-tracking` | Off | Run destructors in LIFO order on `reset`/`rewind` |

## When NOT to Use an Arena

- **Objects with independent lifetimes** — use `Box<T>` or `Rc<T>`
- **Frequent arbitrary-order removal** — use a slab allocator
- **Thread-shared allocation** — wrap in a `Mutex` or use thread-local arenas

## Documentation

See [USAGE.md](USAGE.md) for complete API reference.
