use std::mem::{self, ManuallyDrop};
use std::ptr::NonNull;

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
    pub depth: usize,
    limit: Option<usize>,
}

impl<'arena> Transaction<'arena> {
    pub(crate) fn new(arena: &'arena mut Arena) -> Self {
        let checkpoint = arena.checkpoint();
        let bytes_at_open = arena.bytes_allocated;
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
    /// Uses `mem::forget` to prevent `Drop` from ever running — rollback after
    /// commit is impossible.
    pub fn commit(self) -> TxnStatus {
        let this = ManuallyDrop::new(self);
        unsafe {
            let arena_ptr: *mut Arena = this.arena as *const _ as *mut _;
            (*arena_ptr).txn_depth = (*arena_ptr).txn_depth.saturating_sub(1);
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
    #[inline]
    pub fn alloc<T>(&mut self, val: T) -> &mut T {
        if !self.budget_ok(mem::size_of::<T>()) {
            self.budget_panic(mem::size_of::<T>());
        }
        self.arena.alloc(val)
    }

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

    #[inline]
    pub fn alloc_str(&mut self, s: &str) -> &str {
        if !self.budget_ok(s.len()) {
            self.budget_panic(s.len());
        }
        self.arena.alloc_str(s)
    }

    #[inline]
    pub fn alloc_uninit<T>(&mut self) -> &mut std::mem::MaybeUninit<T> {
        if !self.budget_ok(mem::size_of::<T>()) {
            self.budget_panic(mem::size_of::<T>());
        }
        self.arena.alloc_uninit::<T>()
    }

    #[inline]
    pub fn alloc_zeroed(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_zeroed(size, align)
    }

    #[inline]
    pub fn alloc_cache_aligned(&mut self, size: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_raw(size, CACHE_LINE_SIZE)
    }

    #[inline]
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if !self.budget_ok(size) {
            self.budget_panic(size);
        }
        self.arena.alloc_raw(size, align)
    }

    #[inline]
    pub fn try_alloc<T>(&mut self, val: T) -> Option<&mut T> {
        if !self.budget_ok(mem::size_of::<T>()) {
            return None;
        }
        self.arena.try_alloc(val)
    }

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

    #[inline]
    pub fn try_alloc_str(&mut self, s: &str) -> Option<&str> {
        if !self.budget_ok(s.len()) {
            return None;
        }
        self.arena.try_alloc_str(s)
    }

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

pub fn run_with_transaction_infallible<'arena, F, T>(arena: &'arena mut Arena, f: F) -> T
where
    F: FnOnce(&mut Transaction<'arena>) -> T,
{
    let mut txn = Transaction::new(arena);
    let result = f(&mut txn);
    txn.commit();
    result
}
