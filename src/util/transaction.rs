use core::mem::{self, ManuallyDrop};
use core::ptr::NonNull;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::arena::allocator::CACHE_LINE_SIZE;
use crate::arena::{Arena, Checkpoint};

/// Outcome of [`Transaction::commit`] or [`Transaction::rollback`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnStatus {
    Committed,
    RolledBack,
}

/// Allocation metrics for a single transaction scope, returned by [`Transaction::diff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxnDiff {
    /// Bytes allocated (including padding) since the transaction opened.
    pub bytes_allocated: usize,
    /// Arena blocks written to during this transaction. `>= 1`.
    pub blocks_touched: usize,
}

/// A scoped RAII transaction over an [`Arena`].
///
/// Obtained via [`Arena::transaction`]. All allocations made through this guard
/// are rolled back when it is dropped, unless [`commit`](Transaction::commit)
/// is called first.
///
/// `commit()` calls `mem::forget(self)` — `Drop` is **never** invoked after a
/// successful commit. Rollback after commit is structurally impossible.
///
/// # Nesting
///
/// Call [`savepoint`](Transaction::savepoint) to open a nested transaction. The
/// child borrows the arena from the parent; the parent is inaccessible while
/// the child is alive. Committing the child merges its allocations into the
/// parent scope; dropping it rolls back only the child's work.
///
/// # Budget
///
/// [`set_limit`](Transaction::set_limit) imposes a byte cap. `alloc*` methods
/// panic when exceeded; `try_alloc*` methods return `None`.
///
/// The budget tracks bytes written to arena blocks. Heap allocations *inside*
/// values (e.g. `Vec`, `String`, `Box`) are **not** counted — only the struct
/// size (`size_of::<T>()`) is budgeted.
///
/// ## What to use instead
///
/// To budget actual data bytes, allocate raw data with `alloc_slice` /
/// `alloc_slice_copy` / `alloc_str` and build the container yourself:
///
/// ```ignore
/// // BAD — budgets 24 bytes (Vec struct), heap buffer is untracked
/// txn.alloc(vec![0u8; 4096]);
///
/// // GOOD — budgets all 4096 bytes (data lives in arena, no heap)
/// let buf: &mut [u8] = txn.alloc_slice(vec![0u8; 4096]);
///
/// // GOOD — same, zero-copy for Copy types
/// let buf: &mut [u8] = txn.alloc_slice_copy(&[0u8; 4096]);
/// ```
///
/// If you *need* `Vec`/`String` inside a budgeted transaction, split the
/// operation: allocate the raw data via Transaction methods (budget-checked),
/// then construct the container via [`arena_mut`](Transaction::arena_mut).
/// Allocations through `arena_mut` still participate in rollback but **bypass**
/// the budget pre-check.
pub struct Transaction<'arena> {
    pub(crate) arena: &'arena mut Arena,
    checkpoint: Checkpoint,
    committed: bool,
    bytes_at_open: usize,
    block_at_open: usize,
    depth: usize,
    limit: Option<usize>,
}

impl<'arena> Transaction<'arena> {
    pub(crate) fn new(arena: &'arena mut Arena) -> Self {
        let checkpoint = arena.checkpoint();
        let bytes_at_open = checkpoint.bytes_allocated;
        let block_at_open = checkpoint.block_idx;
        arena.txn_depth += 1;
        let depth = arena.txn_depth;
        Transaction {
            arena,
            checkpoint,
            committed: false,
            bytes_at_open,
            block_at_open,
            depth,
            limit: None,
        }
    }
}

impl<'arena> Transaction<'arena> {
    /// Commit the transaction, keeping all allocations.
    ///
    /// Uses `ManuallyDrop` to prevent `Drop` from ever running — rollback after
    /// commit is impossible.
    pub fn commit(self) -> TxnStatus {
        let mut this = ManuallyDrop::new(self);
        this.arena.txn_depth = this.arena.txn_depth.wrapping_sub(1);
        TxnStatus::Committed
    }

    /// Roll back all allocations made since this transaction opened.
    ///
    /// Equivalent to dropping the guard, but communicates intent explicitly.
    pub fn rollback(self) -> TxnStatus {
        drop(self);
        TxnStatus::RolledBack
    }

    /// Open a nested transaction (savepoint) within this one.
    pub fn savepoint(&mut self) -> Transaction<'_> {
        Transaction::new(self.arena)
    }
}

impl<'arena> Transaction<'arena> {
    /// Set a byte budget for this transaction.
    ///
    /// `alloc*` panics when exceeded; `try_alloc*` returns `None`. The limit
    /// is checked against `size_of::<T>()` (the inline struct size), so
    /// heap allocations inside values (`Vec`, `String`, `Box`) are **not**
    /// tracked — only the struct footprint is counted against the budget.
    ///
    /// To budget actual data bytes, use [`alloc_slice`](Transaction::alloc_slice),
    /// [`alloc_str`](Transaction::alloc_str), or
    /// [`alloc_slice_copy`](Arena::alloc_slice_copy) (via
    /// [`arena_mut`](Transaction::arena_mut), which itself bypasses the budget).
    pub fn set_limit(&mut self, bytes: usize) {
        self.limit = Some(bytes);
    }

    /// Bytes remaining in the budget, or `None` if no limit is set.
    ///
    /// This reflects the arena bytes written via Transaction allocation
    /// methods. It does **not** account for heap allocations inside values
    /// (e.g. `Vec` buffers) or allocations made through
    /// [`arena_mut`](Transaction::arena_mut).
    pub fn budget_remaining(&self) -> Option<usize> {
        self.limit.map(|lim| lim.saturating_sub(self.bytes_used()))
    }

    #[inline]
    fn budget_ok(&self, additional: usize) -> bool {
        match self.limit {
            None => true,
            Some(lim) => self.bytes_used().saturating_add(additional) <= lim,
        }
    }

    #[cold]
    fn budget_panic(&self, n: usize) -> ! {
        panic!(
            "arena transaction budget exceeded: limit={}, used={}, requested={}",
            self.limit.unwrap_or(0),
            self.bytes_used(),
            n
        )
    }
}

impl<'arena> Transaction<'arena> {
    /// Bytes allocated by this transaction since it opened. O(1).
    ///
    /// Includes all arena bytes written — both via Transaction methods and via
    /// [`arena_mut`](Transaction::arena_mut). Includes alignment padding.
    ///
    /// **Note:** The value is only fully accurate after a block-boundary
    /// crossing. Allocations within the current block that haven't triggered a
    /// block switch are tracked via the live pointer offset and included in the
    /// total.
    #[inline(always)]
    pub fn bytes_used(&self) -> usize {
        let current_used = self.arena.cur_ptr as usize - self.arena.cur_base as usize;
        (self.arena.bytes_allocated + current_used).saturating_sub(self.bytes_at_open)
    }

    /// Whether `commit()` has been called.
    #[inline(always)]
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Nesting depth: `1` = top-level, `2` = first savepoint, etc.
    #[inline(always)]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Current nesting depth on the backing arena.
    #[inline(always)]
    pub fn arena_depth(&self) -> usize {
        self.arena.txn_depth
    }

    /// The checkpoint this transaction holds.
    #[inline(always)]
    pub fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }

    /// Exclusive access to the backing arena.
    ///
    /// Use when you need to pass the arena to a helper that takes `&mut Arena`
    /// directly (e.g. constructing an [`ArenaVec`](crate::ArenaVec) inside a
    /// transaction).
    ///
    /// **Warning:** Allocations made through the returned `&mut Arena` **bypass**
    /// the transaction's budget. They still participate in rollback, but their
    /// bytes are not pre-checked against [`set_limit`](Transaction::set_limit).
    /// Budget tracking resumes once a Transaction allocation method is called.
    #[inline]
    pub fn arena_mut(&mut self) -> &mut Arena {
        self.arena
    }

    /// Allocation metrics since this transaction opened. O(1).
    pub fn diff(&self) -> TxnDiff {
        TxnDiff {
            bytes_allocated: self.bytes_used(),
            blocks_touched: self.arena.current.saturating_sub(self.block_at_open) + 1,
        }
    }
}

impl<'arena> Transaction<'arena> {
    /// Allocate a value of type `T` into the arena.
    ///
    /// See [`Arena::alloc`] for details.
    ///
    /// The budget (if set) is checked against `size_of::<T>()`. Heap
    /// allocations *inside* `val` (e.g. `Vec` buffers, `String` heap data)
    /// are **not** counted — only the struct footprint is budgeted.
    #[inline]
    pub fn alloc<T>(&mut self, val: T) -> &mut T {
        if !self.budget_ok(mem::size_of::<T>()) {
            self.budget_panic(mem::size_of::<T>());
        }
        self.arena.alloc(val)
    }

    /// Allocate a contiguous slice from an `ExactSizeIterator`.
    ///
    /// See [`Arena::alloc_slice`] for details.
    #[inline]
    pub fn alloc_slice<T, I>(&mut self, iter: I) -> &mut [T]
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let iter = iter.into_iter();
        let approx = mem::size_of::<T>().saturating_mul(iter.len());
        if !self.budget_ok(approx) {
            self.budget_panic(approx);
        }
        self.arena.alloc_slice(iter)
    }

    /// Copy a string slice into the arena.
    ///
    /// See [`Arena::alloc_str`] for details.
    #[inline]
    pub fn alloc_str(&mut self, s: &str) -> &str {
        if !self.budget_ok(s.len()) {
            self.budget_panic(s.len());
        }
        self.arena.alloc_str(s)
    }

    /// Allocate uninitialised space for a `T`.
    ///
    /// See [`Arena::alloc_uninit`] for details.
    #[inline]
    pub fn alloc_uninit<T>(&mut self) -> &mut std::mem::MaybeUninit<T> {
        if !self.budget_ok(mem::size_of::<T>()) {
            self.budget_panic(mem::size_of::<T>());
        }
        self.arena.alloc_uninit::<T>()
    }

    /// Allocate `size` bytes with `align` alignment, zeroed.
    ///
    /// See [`Arena::alloc_zeroed`] for details.
    #[inline]
    pub fn alloc_zeroed(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_zeroed(size, align)
    }

    /// Allocate `size` bytes aligned to a 64-byte cache line.
    ///
    /// See [`Arena::alloc_cache_aligned`] for details.
    #[inline]
    pub fn alloc_cache_aligned(&mut self, size: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_raw(size, CACHE_LINE_SIZE)
    }

    /// Low-level allocation of `size` uninitialised bytes at `align` alignment.
    ///
    /// See [`Arena::alloc_raw`] for details.
    #[inline]
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_raw(size, align)
    }

    /// Copy a slice of `Copy` items into the arena using a single `memcpy`.
    ///
    /// See [`Arena::alloc_slice_copy`] for details.
    #[inline]
    pub fn alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> &mut [T] {
        let total = mem::size_of::<T>().saturating_mul(src.len());
        if !self.budget_ok(total) {
            self.budget_panic(total);
        }
        self.arena.alloc_slice_copy(src)
    }

    /// Fallible allocation of a value of type `T`. Returns `None` on OOM.
    ///
    /// See [`Arena::try_alloc`] for details.
    #[inline]
    pub fn try_alloc<T>(&mut self, val: T) -> Option<&mut T> {
        if !self.budget_ok(mem::size_of::<T>()) {
            return None;
        }
        self.arena.try_alloc(val)
    }

    /// Fallible slice allocation. Returns `None` on OOM.
    ///
    /// See [`Arena::try_alloc_slice`] for details.
    #[inline]
    pub fn try_alloc_slice<T, I>(&mut self, iter: I) -> Option<&mut [T]>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let iter = iter.into_iter();
        let approx = mem::size_of::<T>().saturating_mul(iter.len());
        if !self.budget_ok(approx) {
            return None;
        }
        self.arena.try_alloc_slice(iter)
    }

    /// Fallible string allocation. Returns `None` on OOM.
    ///
    /// See [`Arena::try_alloc_str`] for details.
    #[inline]
    pub fn try_alloc_str(&mut self, s: &str) -> Option<&str> {
        if !self.budget_ok(s.len()) {
            return None;
        }
        self.arena.try_alloc_str(s)
    }

    /// Fallible raw allocation. Returns `None` on OOM.
    ///
    /// See [`Arena::try_alloc_raw`] for details.
    #[inline]
    pub fn try_alloc_raw(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        if !self.budget_ok(size) {
            return None;
        }
        self.arena.try_alloc_raw(size, align)
    }

    /// Fallible copy of a `Copy` slice into the arena. Returns `None` on OOM.
    ///
    /// See [`Arena::alloc_slice_copy`] for details.
    #[inline]
    pub fn try_alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> Option<&mut [T]> {
        let total = mem::size_of::<T>().saturating_mul(src.len());
        if !self.budget_ok(total) {
            return None;
        }
        self.arena.try_alloc_slice_copy(src)
    }

    /// Fallible cache-line-aligned allocation. Returns `None` on OOM.
    ///
    /// See [`Arena::alloc_cache_aligned`] for details.
    #[inline]
    pub fn try_alloc_cache_aligned(&mut self, size: usize) -> Option<NonNull<u8>> {
        if !self.budget_ok(size) {
            return None;
        }
        self.arena.try_alloc_raw(size, CACHE_LINE_SIZE)
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        self.arena.txn_depth = self.arena.txn_depth.saturating_sub(1);
        if !self.committed {
            #[cold]
            fn do_rewind(arena: &mut Arena, checkpoint: Checkpoint) {
                arena.rewind(checkpoint);
            }
            do_rewind(self.arena, self.checkpoint);
        }
    }
}

/// Execute a closure within a transaction on the given arena.
///
/// The closure receives a mutable [`Transaction`] guard. If the closure returns
/// `Ok`, the transaction commits and all allocations are kept. If the closure
/// returns `Err`, the transaction rolls back automatically.
///
/// # Example
///
/// ```rust
/// use fastarena::Arena;
///
/// let mut arena = Arena::new();
/// let result: Result<u32, &str> = arena.with_transaction(|txn| {
///     txn.alloc(42u32);
///     Ok(*txn.alloc(1u32))
/// });
/// assert_eq!(result, Ok(1));
/// ```
pub(crate) fn run_with_transaction<'arena, F, T, E>(arena: &'arena mut Arena, f: F) -> Result<T, E>
where
    F: FnOnce(&mut Transaction<'arena>) -> Result<T, E>,
{
    let mut txn = Transaction::new(arena);
    match f(&mut txn) {
        Ok(val) => {
            txn.commit();
            Ok(val)
        }
        Err(e) => Err(e),
    }
}

/// Execute an infallible closure within a transaction on the given arena.
///
/// Unlike [`run_with_transaction`], this function always commits, even if the
/// closure panics. The panic is re-raised after the commit.
///
/// If you want rollback-on-panic, use [`run_with_transaction`] instead.
///
/// # Example
///
/// ```rust
/// use fastarena::Arena;
///
/// let mut arena = Arena::new();
/// let value = arena.with_transaction_infallible(|txn| {
///     *txn.alloc(10u32) + *txn.alloc(20u32)
/// });
/// assert_eq!(value, 30);
/// ```
pub(crate) fn run_with_transaction_infallible<'arena, F, T>(arena: &'arena mut Arena, f: F) -> T
where
    F: FnOnce(&mut Transaction<'arena>) -> T,
{
    let mut txn = Transaction::new(arena);
    let result = catch_unwind(AssertUnwindSafe(|| f(&mut txn)));
    txn.commit();
    match result {
        Ok(val) => val,
        Err(payload) => std::panic::panic_any(payload),
    }
}
