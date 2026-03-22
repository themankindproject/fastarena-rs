# Changelog

All notable changes to the `fastarena` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-21

Initial release of fastarena, a high-performance bump-pointer arena allocator with RAII transactions, nested savepoints, optional destructor tracking, and `ArenaVec`.

### Added

#### Core Types
- `Arena` - The main arena allocator with support for allocation, transactions, and reset
- `Checkpoint` - O(1) snapshot of arena state for manual rewind operations
- `Transaction<'a>` - A transactional scope with checkpoint/commit semantics
- `TxnStatus` - Outcome enum for transaction commit/rollback operations
- `TxnDiff` - Allocation metrics for transaction scopes
- `ArenaVec<'a, T>` - Append-only growable vector backed by arena memory
- `ArenaStats` - Statistics about arena memory usage (bytes_allocated, bytes_reserved, block_count)

#### Allocation Methods
- `Arena::alloc<T>()` - Allocate a single value with O(1) amortised complexity
- `Arena::alloc_slice()` - Allocate a slice from any `ExactSizeIterator`
- `Arena::alloc_str()` - Copy a string slice into the arena
- `Arena::alloc_uninit()` - Allocate uninitialised space for a type
- `Arena::alloc_zeroed()` - Allocate zeroed memory with specified size and alignment
- `Arena::alloc_cache_aligned()` - Allocate memory aligned to 64-byte cache lines
- `Arena::alloc_raw()` - Low-level allocation with custom size and alignment

#### Fallible Allocation Methods
- `Arena::try_alloc()` - Fallible allocation returning `None` on OOM
- `Arena::try_alloc_slice()` - Fallible slice allocation
- `Arena::try_alloc_str()` - Fallible string allocation
- `Arena::try_alloc_raw()` - Fallible raw allocation

#### Transaction API
- `Arena::transaction()` - Open a manual transaction with auto-rollback on drop
- `Arena::with_transaction()` - Execute a closure in a transaction; commits on `Ok`, rolls back on `Err`
- `Arena::with_transaction_infallible()` - Execute an infallible closure; always commits
- `Transaction::commit()` - Explicitly commit a transaction using `mem::forget`
- `Transaction::rollback()` - Explicitly rollback a transaction
- `Transaction::savepoint()` - Open a nested transaction (savepoint) within a parent
- `Transaction::set_limit()` - Set a byte budget cap for the transaction
- `Transaction::budget_remaining()` - Query remaining budget in bytes
- `Transaction::diff()` - Get allocation metrics since transaction opened

#### Lifecycle Methods
- `Arena::checkpoint()` - Capture current allocation state as an opaque `Checkpoint`
- `Arena::rewind()` - Roll back all allocations made after a checkpoint
- `Arena::reset()` - Reset the entire arena for zero-cost reuse
- `Arena::stats()` - Get O(1) snapshot of memory usage
- `Arena::block_count()` - Get number of blocks owned by the arena

#### ArenaVec Methods
- `ArenaVec::new()` - Create an empty vector (no allocation until first push)
- `ArenaVec::with_capacity()` - Pre-allocate for known capacity
- `ArenaVec::push()` - Append elements with amortised O(1) complexity
- `ArenaVec::pop()` - Remove and return the last element
- `ArenaVec::extend()` - Append all items from an iterator
- `ArenaVec::extend_from_slice()` - Efficiently copy from slice using SIMD-optimized memcpy
- `ArenaVec::reserve()` - Pre-allocate capacity for additional elements
- `ArenaVec::reserve_exact()` - Reserve exact capacity (no extra)
- `ArenaVec::try_reserve()` - Fallible reserve that returns `Err` on allocation failure
- `ArenaVec::finish()` - Transfer ownership to arena, returning `&'arena mut [T]`
- `ArenaVec::as_slice()` / `ArenaVec::as_mut_slice()` - Borrow as slice
- Indexing via `Index` and `IndexMut` traits
- `Extend<T>` trait implementation for `ArenaVec`

#### Arena Methods
- `Arena::new()` - Create arena with 64 KiB initial block
- `Arena::with_capacity()` - Create arena with custom initial block size

#### Feature Flags
- `drop-tracking` - Opt-in destructor execution on `reset()` / `rewind()` in LIFO order
  - Zero overhead when disabled (all methods become no-ops)
  - Enables `Arena::register_drop()` for manual destructor registration

#### Internal Optimizations
- `InlineVec<T, N>` - Union-based storage with inline capacity (no heap for ≤N elements)
  - Used for `Arena::blocks` with 8-block inline capacity
  - Used for `DropRegistry::entries` with 32-entry inline capacity
- `Block` - Memory block abstraction with bump-pointer allocation
- `DropRegistry` - Tracks pointers with non-trivial destructors (feature-gated)

#### Constants
- `CACHE_LINE_SIZE` - 64-byte cache line size for x86-64 / ARM64 hardware
- Default block size: 64 KiB
- Maximum block size: 16 MiB
- Growth factor: 2x exponential growth

### Performance Characteristics

| Operation              | Complexity      | Notes                                   |
|------------------------|-----------------|----------------------------------------|
| `alloc` (fast path)    | O(1), ~2 ns     | Bump + bounds check                    |
| `alloc` (new block)    | O(1) amortised  | Exponential growth, capped at 16 MiB   |
| `checkpoint`           | O(1)            | Copies 3 integers                      |
| `rewind`               | O(k) blocks     | k ≈ 0–1 in practice                   |
| `reset`                | O(b) blocks     | b is typically 1–4                     |
| `stats`                | O(1)            | Reads 3 incremental counters           |
| `ArenaVec::push`       | O(1) amortised  | Copies on growth, old buffer abandoned |

### Design Principles

- **Zero dependencies** - `std` only, no external crates

- **RAII guarantees** - Borrow checker prevents use-after-rewind at compile time
- **Zero-cost abstractions** - All stats maintained incrementally; no runtime overhead for safety checks
- **Memory efficiency** - Inline storage for small workloads eliminates heap allocations

### Documentation

- Comprehensive README with examples for:
  - Basic allocation patterns
  - Transaction usage (closure-based and manual)
  - Nested savepoints
  - ArenaVec usage
  - Thread-local arena pattern
  - Compiler AST allocation pattern
- Inline documentation for all public APIs
- Feature flag documentation

### System Requirements

- Rust 1.66+ (edition 2021)
- Tested on Linux (x86-64, ARM64)

### Performance Optimizations

- **Layout caching in Block** - Store `Layout` to avoid recomputation on `Drop`
- **Simplified delta calculation** - Direct pointer arithmetic in `alloc_raw_inner`
- **ManuallyDrop in Transaction::commit** - Prevents Drop while properly managing txn_depth
- **set_current_block helper** - Eliminates duplicated cursor update code in reset/rewind
- **saturating_add in InlineVec::push** - Cleaner overflow semantics
- **Block::try_alloc overflow protection** - Use `checked_add` for safe arithmetic
- **std::ptr::dangling_mut()** - Replace manual align_of pointer creation

### Bug Fixes

- **InlineVec::new()** - Fixed uninitialized read (UB) using `mem::zeroed()`
- **ArenaVec::extend** - Cache `mem::size_of` to avoid repeated calls

### Code Quality

- **Clippy fixes** - All warnings resolved:
  - `manual_dangling_ptr` → use `std::ptr::dangling_mut()`
  - `unnecessary_map_or` → use `is_none_or()`
  - `missing_safety_doc` → Added `# Safety` sections
- **Feature-gated DropRegistry** - Field only exists when `drop-tracking` enabled

### Documentation

- **Updated README.md** with benchmark comparisons vs bumpalo/typed-arena
- **Enhanced USAGE.md** with detailed explanations:
  - Complete Arena API documentation
  - ArenaVec growth strategy and capacity management
  - Transaction patterns and use cases
  - Updated benchmark results

### Benchmarks

Added comparison benchmarks (`benches/arena_comparison.rs`) vs popular arena allocators:

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc_slice n=64 | **12 ns** | 49 ns | 72 ns |
| alloc_slice n=1024 | **65 ns** | 510 ns | — |
| ArenaVec n=4096 | **2.2 µs** | 8.2 µs | 10.0 µs |

fastarena wins on slice/vector workloads due to batch write optimization.

---
