use core::mem::{self, ManuallyDrop};
use core::ptr::NonNull;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::arena::{Arena, Checkpoint, CACHE_LINE_SIZE};

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
        let bytes_at_open = arena.stats().bytes_allocated;
        let block_at_open = arena.current;
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
        unsafe {
            let arena_ptr = core::ptr::addr_of_mut!(*this.arena);
            (*arena_ptr).txn_depth = (*arena_ptr).txn_depth.wrapping_sub(1);
        }
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
    /// is checked against `size_of::<T>()` before padding, so actual
    /// consumption may be marginally higher.
    pub fn set_limit(&mut self, bytes: usize) {
        self.limit = Some(bytes);
    }

    /// Bytes remaining in the budget, or `None` if no limit is set.
    pub fn budget_remaining(&self) -> Option<usize> {
        self.limit.map(|lim| lim.saturating_sub(self.bytes_used()))
    }

    #[inline]
    fn budget_ok(&self, additional: usize) -> bool {
        self.limit
            .is_none_or(|lim| self.bytes_used().saturating_add(additional) <= lim)
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
    #[inline(always)]
    pub fn bytes_used(&self) -> usize {
        self.arena
            .stats()
            .bytes_allocated
            .saturating_sub(self.bytes_at_open)
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
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        self.arena.txn_depth = self.arena.txn_depth.saturating_sub(1);
        if !self.committed {
            self.arena.rewind(self.checkpoint);
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
pub fn run_with_transaction<'arena, F, T, E>(arena: &'arena mut Arena, f: F) -> Result<T, E>
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
pub fn run_with_transaction_infallible<'arena, F, T>(arena: &'arena mut Arena, f: F) -> T
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
