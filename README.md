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
| **ArenaBox** | Owned allocation type (`Box`-like) for ownership without heap |

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

### Multiple Allocations and the Borrow Checker

All `alloc*` methods return `&mut T`. This prevents making multiple allocations simultaneously because the borrow checker sees the arena as mutably borrowed. Workarounds include immediate consumption, raw pointers, `ArenaVec`, and the new `ArenaBox<T>` type:

```rust
use fastarena::{Arena, ArenaBox};

let mut arena = Arena::new();
let x = arena.alloc_box(1i32);
// x has ownership semantics - can be moved or dropped
assert_eq!(*x, 1);
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
fastarena = { version = "0.1.3", features = ["drop-tracking"] }
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

Medians below mix a **fresh** Criterion `--quick` sweep (Linux x86-64) with earlier full runs; re-measure before publishing release notes. Absolute timings vary by CPU, frequency, and Rust version.

### Reproducing benchmarks

```bash
# Full comparison + in-crate benches (slow)
cargo bench --bench arena_comparison --bench arena_bench

# Shorter iteration (Criterion)
cargo bench --bench arena_comparison --bench arena_bench -- --quick

# Examples: filter by substring
cargo bench --bench arena_bench reset -- --quick
cargo bench --bench arena_comparison alloc_str -- --quick
```

### Head-to-head: fastarena vs bumpalo vs typed-arena

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc 1k items | **995 ns** | 1162 ns | 1779 ns |
| alloc_slice n=64 ‡ | **58 ns** | 68 ns | 93 ns |
| alloc_slice n=1024 ‡ | **124 ns** | 562 ns | — |
| alloc_str (100×) § | 163 ns | **151 ns** | — |
| ArenaVec n=16 | 51 ns | 60 ns | **27 ns** |
| ArenaVec n=256 | 510 ns | 418 ns | **393 ns** |
| ArenaVec n=4096 | **2.9 µs** | 9.3 µs | 14.1 µs |
| 10k allocs + reset | **14.4 µs** | 14.8 µs | 3.1 µs† |
| reset (1 block) ※ | **25 ns** | 35 ns | — |
| reset (4 blocks) ※ | 175 ns | **72 ns** | — |
| reset (8 blocks) ※ | 376 ns | **214 ns** | — |
| 128 KB alloc | 116 ns | **53 ns** | — |

† typed-arena benchmark allocates once per fresh arena in a tight loop; not directly comparable to reset + reuse.

‡ `alloc_slice` / `alloc_slice_copy` in `arena_bench`: one arena per benchmark, `reset()` each iteration (measures fill + bump, not `Block::new` per sample).

§ `alloc_str` ×100: one arena / bump per benchmark, **100 copies then `reset()`** each iteration (request-style reuse).

※ `arena_bench` `reset/*`: `Arena::with_capacity(64)` / matching bump capacity, two bursts of `blocks × 8` × `u64` allocs separated by `reset()` (workload-dependent; bumpalo can be faster once multiple chunks are involved).

### Fast path benchmarks (vs std Box/Vec)

| Benchmark | fastarena | Box/Vec | Speedup |
|-----------|-----------|---------|---------|
| alloc 1k u64 | **1050 ns** | 52.8 µs | **~50×** |
| alloc_slice n=512 ‡ | **62 ns** | 74 ns | ~1.2× |
| alloc_slice n=4096 ‡ | **231 ns** | 463 ns | ~2× |
| 10k allocs + reset | **24.9 µs** | 336 µs | **~13×** |
| `Arena::new` | **55.7 ns** | — | — |
| `checkpoint()` | **204 ns** | — | — |
| `reset` 1 block ※ | **25 ns** | 35 ns | — |
| `commit` 16 allocs | **1.69 µs** | — | — |

### Why fastarena excels

- **~4–5× faster `alloc_slice` than bumpalo** at n=1024 in these benchmarks (batch fill into arena memory)
- **`ArenaVec`** — faster than bumpalo for larger n (e.g. n=4096); typed-arena can win when n is small (and in this run, up through n=256) where its pattern dominates
- **Scalar `alloc`** — competitive with bumpalo (~995 ns vs ~1162 ns for 1k× u64 here)
- **`alloc_str` (hot loop)** — within ~10% of bumpalo when reusing one arena and `reset()` per batch (§)
- **~50× faster than `Box`** for 1k allocs + arena `reset` vs 1k `Box::new` + drop (see `arena_bench`)
- **`reset`** — on the `arena_bench` micro-workload, single-chunk reuse favors fastarena; multi-chunk `reset` can favor bumpalo (※) — always profile your own allocation pattern
- **Zero dependencies**: No external crates required

## Feature Flags

```toml
[dependencies]
fastarena = { version = "0.1.3", features = ["drop-tracking"] }
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
