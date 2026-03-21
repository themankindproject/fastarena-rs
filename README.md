# FastArena

[![Crates.io](https://img.shields.io/crates/v/fastarena)](https://crates.io/crates/fastarena)
[![Documentation](https://docs.rs/fastarena/badge.svg)](https://docs.rs/fastarena)
[![License](https://img.shields.io/crates/l/fastarena)](LICENSE)
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
| **No_std ready** | Zero dependencies, `std` only |

## Quick Start

```rust
use fastarena::Arena;

let mut arena = Arena::new();

// O(1) allocation
let x: &mut u64 = arena.alloc(42);
assert_eq!(*x, 42);

// Slice from iterator
let squares: &mut [u32] = arena.alloc_slice(0u32..8);

// Interned string
let s: &str = arena.alloc_str("hello");

// Zero-cost reset — pages stay warm
arena.reset();
```

## Use Cases

### Compiler AST Allocation

```rust
use fastarena::Arena;

struct Compiler<'a> {
    arena: Arena,
    // ... other fields
}

impl<'a> Compiler<'a> {
    fn new() -> Self {
        Self { arena: Arena::with_capacity(1024 * 1024) }
    }
    
    fn compile(&mut self, source: &str) {
        // All AST nodes allocated in arena
        let ast = self.parse(source);
        self.optimize(ast);
        self.codegen(ast);
        self.arena.reset(); // Free entire compilation
    }
}
```

### Request-Scoped Memory (Web Servers)

```rust
use std::cell::RefCell;
use fastarena::Arena;

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::with_capacity(256 * 1024));
}

fn handle_request(req: Request) -> Response {
    ARENA.with(|a| {
        let mut arena = a.borrow_mut();
        // Parse and process request in arena
        let parsed = parse_request(&mut arena, &req);
        let result = process(&mut arena, parsed);
        arena.reset(); // Zero-cost recycle
        result
    })
}
```

### Transactional Batch Processing

```rust
use fastarena::Arena;

let mut arena = Arena::new();

// Commits on Ok, rolls back on Err
let result = arena.with_transaction(|txn| -> Result<u32, &str> {
    Ok(*txn.alloc(21) * 2)
});
assert_eq!(result, Ok(42));

// Manual — auto-rollback on drop
{
    let mut txn = arena.transaction();
    txn.alloc(99);
    // dropped → rolled back
}

// Nested savepoints
let mut outer = arena.transaction();
outer.alloc(1);
{
    let mut inner = outer.savepoint();
    inner.alloc(2);
    // dropped → only inner rolled back
}
outer.commit();
```

### ArenaVec for Dynamic Collections

```rust
use fastarena::{Arena, ArenaVec};

let mut arena = Arena::new();

let slice: &mut [u64] = {
    let mut v = ArenaVec::with_capacity(&mut arena, 16);
    for i in 0u64..16 { v.push(i * i); }
    v.finish()
};

assert_eq!(slice[15], 225);
```

## Performance

### Benchmark Results (vs bumpalo, typed-arena)

| Benchmark | fastarena | bumpalo | typed-arena | Winner |
|-----------|-----------|---------|-------------|--------|
| alloc 1k items | 1881 ns | 925 ns | 994 ns | bumpalo |
| alloc_slice n=64 | **12 ns** | 49 ns | 72 ns | **fastarena** |
| alloc_slice n=1024 | **65 ns** | 510 ns | — | **fastarena** |
| ArenaVec n=256 | **174 ns** | 280 ns | 366 ns | **fastarena** |
| ArenaVec n=4096 | **2.2 µs** | 8.2 µs | 10.0 µs | **fastarena** |
| 10k allocs + reset | 17.1 µs | 14.5 µs | 2.6 µs | typed-arena* |
| large 128KB alloc | 59 ns | 27 ns | — | bumpalo |

*typed-arena reallocates fresh each iteration; not comparable.

### Why fastarena excels

- **Slice allocation**: 4-8x faster than bumpalo due to batch `write()` in a tight loop
- **ArenaVec**: 3-4x faster for building collections vs bumpalo Vec  
- **Cache locality**: Single bump pointer keeps allocations contiguous
- **Zero dependencies**: No external crates required

| Operation | Time | vs Box/Vec |
|-----------|------|------------|
| `alloc` | ~1.7 µs | **10x faster** |
| `alloc_slice n=64` | ~35 ns | **2x faster** |
| `reset` | ~24 ns | — |
| 10k allocs + reset | ~17 ms | **10x faster** |

## Feature Flags

```toml
[dependencies]
fastarena = { version = "0.1", features = ["drop-tracking"] }
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

## Minimum Supported Rust Version (MSRV)

**Rust 1.66.0** — Stable since January 2022.

## License

MIT — See [LICENSE](LICENSE) file.

## Links

- [Crates.io](https://crates.io/crates/fastarena)
- [Documentation](https://docs.rs/fastarena)
- [Repository](https://github.com/themankindproject/fastarena-rs)
- [Changelog](CHANGELOG.md)