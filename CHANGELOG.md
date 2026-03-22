# Changelog

All notable changes to `fastarena` will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-23

Initial release.

### Added

#### Arena

- `Arena::new()` — 64 KiB initial block
- `Arena::with_capacity(bytes)` — custom initial block size
- `Arena::alloc<T>(val)` — O(1) single value allocation
- `Arena::alloc_slice(iter)` — contiguous slice from `ExactSizeIterator`
- `Arena::alloc_slice_copy(src)` — single `memcpy` for `Copy` types
- `Arena::alloc_str(s)` — copy string into arena
- `Arena::alloc_uninit<T>()` — uninitialized slot
- `Arena::alloc_raw(size, align)` — raw bytes with alignment
- `Arena::alloc_zeroed(size, align)` — zeroed raw bytes
- `Arena::alloc_cache_aligned(size)` — 64-byte aligned (SIMD-friendly)
- `Arena::try_alloc`, `try_alloc_slice`, `try_alloc_str`, `try_alloc_raw` — fallible variants returning `None` on OOM
- `Arena::checkpoint()` — O(1) snapshot of allocation position
- `Arena::rewind(cp)` — roll back to checkpoint, retains blocks
- `Arena::reset()` — reclaim all memory (zero-cost, pages stay mapped)
- `Arena::stats()` — O(1) memory usage snapshot (`ArenaStats`)
- `Arena::block_count()` — number of allocated blocks

#### Transactions

- `Arena::transaction()` — open RAII transaction (auto-rollback on drop)
- `Arena::with_transaction(f)` — `Ok` commits, `Err` rolls back
- `Arena::with_transaction_infallible(f)` — always commits, even through panic
- `Transaction::commit()` / `Transaction::rollback()`
- `Transaction::savepoint()` — nested transaction at arbitrary depth
- `Transaction::set_limit(bytes)` — byte budget enforcement
- `Transaction::budget_remaining()` — query remaining budget
- `Transaction::bytes_used()` / `Transaction::diff()` — O(1) allocation metrics
- `Transaction::depth()` / `Transaction::arena_depth()` — nesting introspection
- `Transaction::arena_mut()` — direct arena access within transaction
- All `Arena` allocation methods available on `Transaction` (budget-checked)

#### ArenaVec

- `ArenaVec::new(arena)` — empty vector, no allocation until first push
- `ArenaVec::with_capacity(arena, cap)` — pre-allocated
- `ArenaVec::push(val)` — amortized O(1)
- `ArenaVec::pop()` — remove last element
- `ArenaVec::clear()` — drop elements, keep capacity
- `ArenaVec::extend_exact(iter)` — batch from `ExactSizeIterator`
- `ArenaVec::extend_from_slice(slice)` — single `memcpy` for `Copy` types
- `ArenaVec::reserve(n)` / `ArenaVec::reserve_exact(n)` / `ArenaVec::try_reserve(n)`
- `ArenaVec::finish()` — transfer ownership to arena, returns `&'arena mut [T]`
- `ArenaVec::as_slice()` / `ArenaVec::as_mut_slice()`
- Implements `Index`, `IndexMut`, `Extend<T>`, `IntoIterator`

#### Feature Flags

- `drop-tracking` — opt-in LIFO destructor execution on `reset()` / `rewind()`
  - `Arena::register_drop(ptr)` for manual registration
  - Zero-cost when disabled (all paths compiled out)

#### Types

- `Checkpoint` — opaque snapshot (Copy, 3 usizes)
- `Transaction<'a>` — RAII transaction guard
- `TxnStatus` — `Committed` / `RolledBack`
- `TxnDiff` — `bytes_allocated`, `blocks_touched`
- `ArenaStats` — `bytes_allocated`, `bytes_reserved`, `block_count`, `utilization()`, `bytes_idle()`
- `TryReserveError` — `CapacityOverflow` / `AllocError`

#### Constants

- `CACHE_LINE_SIZE = 64`

### Design

- **Zero dependencies** — `std` only
- **O(1) allocation** — single bounds check + bump pointer advance
- **Zero-cost reset** — blocks retained, no OS calls, TLB-warm
- **RAII transactions** — auto-rollback on drop, `mem::forget` on commit
- **Unbounded nesting** — savepoints compose to arbitrary depth
- **InlineVec** — union-based storage, first N entries stay on stack (no heap)
- **Block growth** — 1.5x exponential, capped at 16 MiB

### Performance

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc 1k items | **863 ns** | 897 ns | 988 ns |
| alloc_slice n=64 | **12 ns** | 53 ns | 78 ns |
| alloc_slice n=1024 | **63 ns** | 531 ns | — |
| ArenaVec n=256 | **231 ns** | 291 ns | 406 ns |
| ArenaVec n=4096 | **3.4 µs** | 8.4 µs | 9.2 µs |
| 10k allocs + reset | **14.1 µs** | 14.4 µs | 2.6 µs |

### System Requirements

- Rust 1.66+ (edition 2021)
- `no_std` not supported

### Documentation

- README with Quick Start, feature showcases, and benchmarks
- USAGE.md with full API reference and best practices
- Inline `///` docs on all public items
