# Changelog

All notable changes to `fastarena` will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `arenavec!` macro — concise ArenaVec construction with three forms:
  - `arenavec![in &mut arena]` — empty
  - `arenavec![in &mut arena; 1, 2, 3]` — list of elements
  - `arenavec![in &mut arena; 0u32; 10]` — repeated element (requires `T: Clone`)
  - Single allocation in repeat form via `with_capacity` + `extend_exact`
- `ArenaVec::truncate(len)` — shorten vector, dropping excess elements
- `ArenaVec::resize(len, val)` — grow with cloned value or shrink (requires `T: Clone`)
- `Display` impl for `ArenaStats` (shows allocated/reserved, block count, utilization %)
- `Display` impl for `TxnStatus` (`"committed"` / `"rolled back"`)
- `Display` impl for `TxnDiff` (`"N bytes in M block(s)"`)
- `Display` impl for `Checkpoint` (shows block index, offset, bytes)
- `#[must_use]` on `Checkpoint`, `ArenaStats`, `TxnStatus`, `TxnDiff`

### Changed

- Refactored `alloc_slow` / `alloc_slow_try` — extracted `finish_slow_path`,
  `alloc_new_block`, `try_alloc_new_block` helpers to eliminate ~60 lines of
  duplicated allocation logic.
- Refactored `DropRegistry::run_drops_until` — extracted `run_slot` and
  `drain_remaining` helpers to deduplicate panic-drain logic.

## [0.1.2] - 2026-03-24

### Added

- `ArenaVec::try_push` — fallible push that returns `Err(val)` on OOM instead of panicking
- `ArenaVec::try_reserve_exact` — fallible variant of `reserve_exact`
- `Transaction::alloc_slice_copy` — budget-checked `memcpy` for `Copy` slices (previously only available on `Arena`)
- `Transaction::try_alloc_slice_copy` — fallible variant of `alloc_slice_copy` on `Transaction`
- `Transaction::try_alloc_cache_aligned` — fallible variant of `alloc_cache_aligned` on `Transaction`
- `try_alloc_slice_copy` — fallible variant of `alloc_slice_copy` (`Copy` types via single `memcpy`)
- `try_alloc_zeroed` — fallible variant of `alloc_zeroed` (zeroed raw bytes)
- `try_alloc_cache_aligned` — fallible variant of `alloc_cache_aligned` (64-byte aligned)
- `Debug` impl for `Arena` (shows stats, block_count, txn_depth)
- `Debug` impl for `ArenaVec` (debug-list format, requires `T: Debug`)
- `Display` and `std::error::Error` impls for `TryReserveError`
- `ArenaVecIntoIter` re-exported from crate root
- Miri CI job in GitHub Actions (runs on nightly, skips slow/large tests)

### Fixed

- **Overflow safety:** `ArenaVec::extend_exact` and `ArenaVec::extend_from_slice` now use
  `checked_add` for the length computation (`self.len + add_len`) instead of raw addition,
  preventing silent `usize` wrap on overflow.
- **Lifetime ergonomics:** `as_slice` and `as_mut_slice` on `ArenaVec` now return `&[T]`
  / `&mut [T]` tied to the borrow lifetime instead of `&'arena [T]` / `&'arena mut [T]`.
  `IntoIterator` for `&ArenaVec` / `&mut ArenaVec` now accepts any borrow lifetime
  (`&'a ArenaVec<'arena, T>`) instead of requiring `&'arena ArenaVec<'arena, T>`.
- **Soundness:** ZST allocation used `align as *mut T` to fabricate pointers, which
  is technically UB for alignments > 1. Replaced with `NonNull::dangling()` in
  `alloc`, `alloc_uninit`, `alloc_raw`, `alloc_zeroed`, `try_alloc`, and
  `try_alloc_raw`.
- **Miri compatibility:** Changed `Block::base` and `Arena::cur_base` from `usize` to
  `*mut u8` for strict provenance compliance. Added zero-sized allocation checks
  in `InlineVec` to fix Miri UB. Block alignment is now tracked, bypassing the fast
  path for high-alignment requests (>64 bytes).

## [0.1.1] - 2026-03-23

### Fixed

- **Soundness:** `InlineVec` `Send`/`Sync` trait bounds were too broad — the
  blanket impls (`unsafe impl<T: Send> Send for InlineVec<T, N>`) did not
  account for the raw pointer in `HeapBuf`, making it unsound for types
  containing `!Send`/`!Sync` raw pointers. Now requires `[T; N]: Send`/`Sync`
  as a where clause.

- **Safety:** Replaced `unwrap_unchecked()` with `unwrap()` in `alloc_slice`,
  `try_alloc_slice`, and `ArenaVec::extend_exact`. A malicious
  `ExactSizeIterator` lying about its length could cause UB via
  `unwrap_unchecked`.

- **Robustness:** Test copies of `Block::try_alloc` and `InlineVec::heap_grow`
  now use `checked_add`/`checked_mul` to match the library code and prevent
  silent overflow in release mode.

### Performance

- **`reset()` is now O(peak_blocks) with zero wasted capacity.** Tracks a
  `high_water_mark` (highest block index ever reached). On reset, only blocks
  0..=high_water_mark have their offsets zeroed — all memory is fully reusable.
  Single-block arenas pay O(1); multi-block arenas pay O(peak) instead of
  O(retained).

| Benchmark | fastarena | bumpalo | typed-arena |
|-----------|-----------|---------|-------------|
| alloc 1k items | **894 ns** | 937 ns | 1072 ns |
| alloc_slice n=64 | **10 ns** | 53 ns | 84 ns |
| alloc_slice n=1024 | **64 ns** | 518 ns | — |
| ArenaVec n=256 | **263 ns** | 346 ns | 516 ns |
| ArenaVec n=4096 | **3.4 µs** | 8.5 µs | 11.1 µs |
| 10k allocs + reset | **15.0 µs** | 15.1 µs | 2.8 µs |
| reset (1 block) | **20 ns** | 696 ns | — |

### Added

- `examples/` directory with 10 runnable examples covering all usage patterns:
  - `basic_allocation` — alloc, alloc_slice, alloc_str, alloc_uninit, alloc_raw, alloc_zeroed, alloc_cache_aligned, reset
  - `transactions` — with_transaction, with_transaction_infallible, manual commit/rollback, diff, bytes_used
  - `savepoints` — nested savepoints (3 levels), partial rollback, depth tracking
  - `arena_vec` — push, finish, extend_exact, extend_from_slice, pop, reserve, IndexMut, clear, ArenaVec in transactions
  - `budgets` — set_limit, budget_remaining, try_alloc budget checks, alloc_slice for byte budgeting, alloc(vec![...]) pitfall
  - `checkpoint_rewind` — checkpoint, rewind, reset, pre-checkpoint data survives
  - `stats` — ArenaStats, utilization, bytes_idle, block_count, diff
  - `fallible` — try_alloc, try_alloc_slice, try_alloc_str, try_alloc_raw
  - `real_world` — compiler speculative optimization, LSM batch abort, request-scoped reset cycles
  - `drop_tracking` — LIFO destructors on reset/rewind, transaction rollback/commit semantics

### Fixed (Documentation)

- **Documentation:** Budget examples in README.md and USAGE.md used
  `alloc(vec![0u8; N])` which stores only the 24-byte `Vec` struct in the
  arena — the N-byte heap buffer is allocated via the system allocator and is
  invisible to the budget. Replaced with `alloc_slice(vec![0u8; N])` which
  correctly copies data into the arena and is fully budget-tracked.

- **Documentation:** Transactions example in README.md and USAGE.md held a
  reference (`let name = txn.alloc_str(...)`) across a subsequent `txn.alloc()`
  call, which is a double mutable borrow that doesn't compile.

- **Documentation:** Savepoints example in README.md and USAGE.md held
  `let parser_ast = outer.alloc_str(...)` while later calling
  `outer.savepoint()`, which is a double mutable borrow that doesn't compile.

- **Documentation:** `Transaction::set_limit` docstring suggested using
  `alloc_slice_copy` "via `arena_mut`" for byte budgeting, but `arena_mut()`
  itself bypasses the budget. Clarified the trade-off.

### Changed

- **Documentation:** `Transaction` struct docstring now includes a
  "What to use instead" section with GOOD/BAD code examples showing how to
  correctly budget actual data bytes with `alloc_slice` / `alloc_slice_copy`
  instead of `alloc(vec![...])`.

- **Documentation:** `Transaction::set_limit` docstring now explicitly states
  that heap allocations inside values (`Vec`, `String`, `Box`) are not tracked
  and recommends `alloc_slice`, `alloc_str`, or `alloc_slice_copy` for
  byte-level budgeting.

- **Documentation:** `Transaction::budget_remaining` docstring now notes it
  doesn't account for heap allocations inside values or allocations made
  through `arena_mut`.

- **Documentation:** `Transaction::arena_mut` docstring now includes a warning
  that allocations through the returned `&mut Arena` bypass the budget
  pre-check but still participate in rollback.

- **Documentation:** `Transaction::alloc` docstring now notes the budget is
  checked against `size_of::<T>()` only — heap data inside the value is not
  counted.

- **Documentation:** `Transaction::bytes_used` docstring now clarifies it
  includes bytes from both Transaction methods and `arena_mut` allocations,
  and notes the accuracy caveat around block-boundary flushing.

- **Documentation:** `lib.rs` budget feature description now notes "tracks
  inline sizes only; heap allocations inside values are not counted".

- **Documentation:** `CHANGELOG.md` transaction entries now clarify budget
  tracks `size_of::<T>()` only and `arena_mut` bypasses budget.

### Removed

- `CACHE_LINE_SIZE` from public API (`pub(crate)` now). Use the literal `64`
  or `alloc_cache_aligned(size)` directly.
- `run_with_transaction` / `run_with_transaction_infallible` from public API
  (`pub(crate)` now). Use `Arena::with_transaction` /
  `Arena::with_transaction_infallible` instead.

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

[0.1.3]: https://github.com/themankindproject/fastarena-rs/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/themankindproject/fastarena-rs/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/themankindproject/fastarena-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/themankindproject/fastarena-rs/releases/tag/v0.1.0