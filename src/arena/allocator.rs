use std::mem::{self, MaybeUninit};
use std::ptr::NonNull;

use super::block::{align_up, Block};
use super::stats::ArenaStats;
use crate::util::{
    inline_vec::InlineVec,
    transaction::{run_with_transaction, run_with_transaction_infallible, Transaction},
};

#[cfg(feature = "drop-tracking")]
use crate::util::drop_registry::DropRegistry;

const MIN_BLOCK_SIZE: usize = 64;
const DEFAULT_BLOCK_SIZE: usize = 64 * 1_024;
const MAX_BLOCK_SIZE: usize = 16 * 1_024 * 1_024;
const BLOCKS_INLINE_CAP: usize = 8;

/// Cache-line size on x86-64 / ARM64 hardware.
pub const CACHE_LINE_SIZE: usize = 64;

/// An opaque snapshot of arena state used by [`Arena::rewind`].
///
/// Obtained via [`Arena::checkpoint`]. Must only be passed back to the arena
/// that produced it — using it with a different arena panics.
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    pub(crate) block_idx: usize,
    pub(crate) offset: usize,
    pub(crate) bytes_allocated: usize,
    #[cfg(feature = "drop-tracking")]
    pub(crate) drop_registry_len: usize,
}

/// A bump-pointer arena allocator with RAII transactions, checkpoint/rewind,
/// and zero-cost reset/reuse.
///
/// # Allocation model
///
/// Every allocation method takes `&mut self`. The borrow checker statically
/// prevents `rewind` or `reset` while any live reference exists — there is no
/// runtime cost for this guarantee.
///
/// # Destructor behaviour
///
/// Without the `drop-tracking` feature, destructors are **never called** for
/// arena-allocated values. This is intentional: the performance advantage of
/// arena allocation comes from bulk reclamation. Enable `drop-tracking` to opt
/// in to LIFO destructor execution on `reset` / `rewind`.
///
/// # When NOT to use an arena
///
/// - Objects that need independent lifetimes (use `Box` or `Rc`).
/// - Frequent arbitrary-order removal (use a slab allocator).
/// - Multi-threaded access (wrap in a `Mutex` or use thread-local arenas).
///
/// # Thread-local pattern
///
/// ```ignore
/// thread_local! {
///     static ARENA: RefCell<Arena> = RefCell::new(Arena::with_capacity(64 * 1024));
/// }
/// fn handle_request(req: &Request) {
///     ARENA.with(|a| {
///         let mut arena = a.borrow_mut();
///         process(&mut arena, req);
///         arena.reset();
///     })
/// }
/// ```
pub struct Arena {
    blocks: InlineVec<Block, BLOCKS_INLINE_CAP>,
    pub(crate) current: usize,
    next_block_size: usize,
    pub(crate) bytes_allocated: usize,
    bytes_reserved: usize,
    pub(crate) txn_depth: usize,
    #[cfg(feature = "drop-tracking")]
    pub(crate) drop_registry: DropRegistry,
    #[cfg(not(feature = "drop-tracking"))]
    _drop_registry: (),
    cur_ptr: *mut u8,
    cur_end: *mut u8,
}

impl Arena {
    /// Create an arena with a 64 KiB initial block.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BLOCK_SIZE)
    }

    /// Create an arena with a custom initial block size.
    ///
    /// Choose a value close to expected peak usage to avoid early block
    /// chaining.
    pub fn with_capacity(initial_bytes: usize) -> Self {
        let size = initial_bytes.max(MIN_BLOCK_SIZE);
        let block = Block::new(size);
        let base = block.base;
        let capacity = block.capacity;
        let mut blocks: InlineVec<Block, BLOCKS_INLINE_CAP> = InlineVec::new();
        blocks.push(block);
        Arena {
            blocks,
            current: 0,
            next_block_size: size.saturating_mul(2).min(MAX_BLOCK_SIZE),
            bytes_allocated: 0,
            bytes_reserved: size,
            txn_depth: 0,
            #[cfg(feature = "drop-tracking")]
            drop_registry: DropRegistry::new(),
            #[cfg(not(feature = "drop-tracking"))]
            _drop_registry: (),
            cur_ptr: base as *mut u8,
            cur_end: (base + capacity) as *mut u8,
        }
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl Arena {
    /// Allocate a value of type `T`, returning an exclusive reference.
    ///
    /// Without `drop-tracking`, the destructor of `T` is never called.
    /// Arena memory is reclaimed in bulk by [`reset`](Arena::reset) or when
    /// the arena itself is dropped.
    #[inline]
    pub fn alloc<T>(&mut self, val: T) -> &mut T {
        if mem::size_of::<T>() == 0 {
            return unsafe { &mut *std::ptr::dangling_mut::<T>() };
        }
        let ptr = self.alloc_raw_inner(mem::size_of::<T>(), mem::align_of::<T>());
        unsafe {
            let typed = ptr.as_ptr() as *mut T;
            typed.write(val);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register(typed);
            &mut *typed
        }
    }

    /// Allocate a contiguous slice of `T` from an `ExactSizeIterator`.
    #[inline]
    pub fn alloc_slice<T, I>(&mut self, iter: I) -> &mut [T]
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        if len == 0 {
            return &mut [];
        }
        let total = mem::size_of::<T>().checked_mul(len).expect("overflow");
        let ptr = self.alloc_raw_inner(total, mem::align_of::<T>());
        unsafe {
            let start = ptr.as_ptr() as *mut T;
            for i in 0..len {
                let elem = start.add(i);
                elem.write(iter.next().unwrap_unchecked());
                #[cfg(feature = "drop-tracking")]
                self.drop_registry.register(elem);
            }
            std::slice::from_raw_parts_mut(start, len)
        }
    }

    /// Copy a string slice into the arena and return a reference to it.
    #[inline(always)]
    pub fn alloc_str(&mut self, s: &str) -> &str {
        if s.is_empty() {
            return "";
        }
        let ptr = self.alloc_raw_inner(s.len(), 1);
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), ptr.as_ptr(), s.len());
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr.as_ptr(), s.len()))
        }
    }

    /// Allocate space for a `T` without initialising it.
    ///
    /// The caller must fully initialise the value before it can be observed.
    ///
    /// ```rust
    /// use fastarena::Arena;
    /// let mut arena = Arena::new();
    /// let slot = arena.alloc_uninit::<u64>();
    /// slot.write(42);
    /// let val: &u64 = unsafe { slot.assume_init_ref() };
    /// assert_eq!(*val, 42);
    /// ```
    #[inline]
    pub fn alloc_uninit<T>(&mut self) -> &mut MaybeUninit<T> {
        let size = mem::size_of::<T>();
        let align = mem::align_of::<T>();
        if size == 0 {
            return unsafe { &mut *(align as *mut MaybeUninit<T>) };
        }
        let ptr = self.alloc_raw_inner(size, align);
        unsafe { &mut *(ptr.as_ptr() as *mut MaybeUninit<T>) }
    }

    /// Allocate `size` bytes with the given `align` alignment, initialized to zero.
    ///
    /// # Panics
    ///
    /// Panics if `align` is not a power of two or the system is out of memory.
    #[inline]
    pub fn alloc_zeroed(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if size == 0 {
            return unsafe { NonNull::new_unchecked(align as *mut u8) };
        }
        let ptr = self.alloc_raw(size, align);
        unsafe { ptr.as_ptr().write_bytes(0, size) };
        ptr
    }

    /// Allocate `size` bytes aligned to a 64-byte cache line boundary.
    ///
    /// This is useful for data structures that benefit from cache-line-aligned
    /// access, such as SIMD operations or lock-free data structures.
    ///
    /// # Panics
    ///
    /// Panics if the system is out of memory.
    #[inline]
    pub fn alloc_cache_aligned(&mut self, size: usize) -> NonNull<u8> {
        self.alloc_raw(size, CACHE_LINE_SIZE)
    }

    /// Low-level allocation of `size` uninitialised bytes at `align` alignment.
    ///
    /// # Panics
    /// Panics if `align` is not a power of two or the system is out of memory.
    #[inline]
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> NonNull<u8> {
        assert!(align.is_power_of_two(), "align must be a power of two");
        if size == 0 {
            return unsafe { NonNull::new_unchecked(align as *mut u8) };
        }
        self.alloc_raw_inner(size, align)
    }
}

impl Arena {
    /// Fallible variant of [`alloc`](Arena::alloc). Returns `None` on OOM.
    #[inline]
    pub fn try_alloc<T>(&mut self, val: T) -> Option<&mut T> {
        if mem::size_of::<T>() == 0 {
            return Some(unsafe { &mut *std::ptr::dangling_mut::<T>() });
        }
        let ptr = self.try_alloc_raw_inner(mem::size_of::<T>(), mem::align_of::<T>())?;
        Some(unsafe {
            let typed = ptr.as_ptr() as *mut T;
            typed.write(val);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register(typed);
            &mut *typed
        })
    }

    /// Fallible variant of [`alloc_slice`](Arena::alloc_slice).
    #[inline]
    pub fn try_alloc_slice<T, I>(&mut self, iter: I) -> Option<&mut [T]>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        if len == 0 {
            return Some(&mut []);
        }
        let total = mem::size_of::<T>().checked_mul(len)?;
        let ptr = self.try_alloc_raw_inner(total, mem::align_of::<T>())?;
        Some(unsafe {
            let start = ptr.as_ptr() as *mut T;
            for i in 0..len {
                let elem = start.add(i);
                elem.write(iter.next().unwrap_unchecked());
                #[cfg(feature = "drop-tracking")]
                self.drop_registry.register(elem);
            }
            std::slice::from_raw_parts_mut(start, len)
        })
    }

    /// Fallible variant of [`alloc_str`](Arena::alloc_str).
    #[inline]
    pub fn try_alloc_str(&mut self, s: &str) -> Option<&str> {
        let bytes = self.try_alloc_slice(s.bytes())?;
        Some(unsafe { std::str::from_utf8_unchecked(bytes) })
    }

    /// Fallible variant of [`alloc_raw`](Arena::alloc_raw).
    #[inline]
    pub fn try_alloc_raw(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        assert!(align.is_power_of_two(), "align must be a power of two");
        if size == 0 {
            return Some(unsafe { NonNull::new_unchecked(align as *mut u8) });
        }
        self.try_alloc_raw_inner(size, align)
    }
}

impl Arena {
    /// Capture the current allocation position as an opaque [`Checkpoint`].
    #[inline(always)]
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            block_idx: self.current,
            offset: self.blocks[self.current].offset,
            bytes_allocated: self.bytes_allocated,
            #[cfg(feature = "drop-tracking")]
            drop_registry_len: self.drop_registry.len(),
        }
    }

    /// Roll back all allocations made after `cp` was taken.
    ///
    /// Blocks opened after the checkpoint have their offsets reset to zero and
    /// are retained for immediate reuse — no OS calls are made. If the
    /// `drop-tracking` feature is enabled, destructors for post-checkpoint
    /// objects are run in LIFO order before memory is reclaimed.
    ///
    /// # Panics
    /// Panics if `cp` was not produced by this arena.
    pub fn rewind(&mut self, cp: Checkpoint) {
        assert!(
            cp.block_idx < self.blocks.len(),
            "rewind: checkpoint block_idx {} out of range (arena has {} blocks)",
            cp.block_idx,
            self.blocks.len()
        );
        assert!(
            cp.offset <= self.blocks[cp.block_idx].capacity,
            "rewind: checkpoint offset {} exceeds block capacity {}",
            cp.offset,
            self.blocks[cp.block_idx].capacity
        );

        #[cfg(feature = "drop-tracking")]
        self.drop_registry.run_drops_until(cp.drop_registry_len);

        for i in (cp.block_idx + 1)..self.blocks.len() {
            self.blocks[i].reset();
        }
        self.blocks[cp.block_idx].offset = cp.offset;
        self.bytes_allocated = cp.bytes_allocated;
        self.set_current_block(cp.block_idx);
    }

    /// Reset the entire arena so all memory is available for reuse.
    ///
    /// No memory is freed — OS pages stay mapped and TLB-warm. If
    /// `drop-tracking` is enabled, all registered destructors run first.
    pub fn reset(&mut self) {
        #[cfg(feature = "drop-tracking")]
        self.drop_registry.run_all_drops();
        for block in self.blocks.iter_mut() {
            block.reset();
        }
        self.bytes_allocated = 0;
        self.set_current_block(0);
    }
}

impl Arena {
    /// Open a [`Transaction`] on this arena.
    ///
    /// All allocations made through the guard are rolled back automatically
    /// when it is dropped, unless [`Transaction::commit`] is called first.
    #[inline]
    pub fn transaction(&mut self) -> Transaction<'_> {
        Transaction::new(self)
    }

    /// Execute a closure inside a transaction.
    ///
    /// `Ok` commits; `Err` rolls back all allocations before returning.
    #[inline]
    pub fn with_transaction<F, T, E>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    {
        run_with_transaction(self, f)
    }

    /// Execute an infallible closure inside a transaction; always commits.
    ///
    /// If the closure panics, the transaction rolls back via `Drop` before
    /// the panic propagates.
    #[inline]
    pub fn with_transaction_infallible<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Transaction<'_>) -> T,
    {
        run_with_transaction_infallible(self, f)
    }

    /// Current number of open transactions and savepoints.
    #[inline]
    pub fn transaction_depth(&self) -> usize {
        self.txn_depth
    }
}

impl Arena {
    /// Register a raw pointer for destructor execution.
    ///
    /// Only available with the `drop-tracking` feature. Call this after
    /// [`alloc_uninit`] once the value is fully initialised.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a fully initialised `T` allocated from this arena.
    /// Calling this twice for the same pointer causes a double-drop.
    #[cfg(feature = "drop-tracking")]
    pub unsafe fn register_drop<T>(&mut self, ptr: *mut T) {
        self.drop_registry.register(ptr);
    }

    /// # Safety
    ///
    /// This is a no-op when `drop-tracking` is disabled. No safety requirements
    /// apply since the function does nothing.
    #[cfg(not(feature = "drop-tracking"))]
    pub unsafe fn register_drop<T>(&mut self, _ptr: *mut T) {}

    /// Return a point-in-time snapshot of memory usage. O(1).
    #[inline(always)]
    pub fn stats(&self) -> ArenaStats {
        ArenaStats {
            bytes_allocated: self.bytes_allocated,
            bytes_reserved: self.bytes_reserved,
            block_count: self.blocks.len(),
        }
    }

    /// Number of blocks currently owned by the arena.
    #[inline(always)]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

impl Arena {
    #[inline(always)]
    pub(crate) fn alloc_raw_inner(&mut self, size: usize, align: usize) -> NonNull<u8> {
        let aligned_ptr = align_up(self.cur_ptr as usize, align) as *mut u8;
        let next = unsafe { aligned_ptr.add(size) };
        if next <= self.cur_end {
            self.bytes_allocated += next as usize - self.cur_ptr as usize;
            self.cur_ptr = next;
            return unsafe { NonNull::new_unchecked(aligned_ptr) };
        }
        self.alloc_slow(size, align)
    }

    #[inline(always)]
    pub(crate) fn try_alloc_raw_inner(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        let aligned_ptr = align_up(self.cur_ptr as usize, align) as *mut u8;
        let next = unsafe { aligned_ptr.add(size) };
        if next <= self.cur_end {
            self.bytes_allocated += next as usize - self.cur_ptr as usize;
            self.cur_ptr = next;
            return Some(unsafe { NonNull::new_unchecked(aligned_ptr) });
        }
        self.alloc_slow_try(size, align)
    }

    #[cold]
    fn alloc_slow(&mut self, size: usize, align: usize) -> NonNull<u8> {
        for i in (self.current + 1)..self.blocks.len() {
            let block = &mut self.blocks[i];
            if let Some((ptr, delta)) = block.try_alloc(size, align) {
                self.bytes_allocated += delta;
                self.set_current_block(i);
                return ptr;
            }
        }
        let block_size = self.next_block_for(size, align);
        let mut block = Block::new(block_size);
        let (ptr, delta) = block
            .try_alloc(size, align)
            .expect("fresh block must satisfy request");
        self.bytes_reserved += block_size;
        self.bytes_allocated += delta;
        self.blocks.push(block);
        self.set_current_block(self.blocks.len() - 1);
        ptr
    }

    #[cold]
    fn alloc_slow_try(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        for i in (self.current + 1)..self.blocks.len() {
            let block = &mut self.blocks[i];
            if let Some((ptr, delta)) = block.try_alloc(size, align) {
                self.bytes_allocated += delta;
                self.set_current_block(i);
                return Some(ptr);
            }
        }
        let block_size = self.next_block_for(size, align);
        let mut block = Block::try_new(block_size)?;
        let (ptr, delta) = block.try_alloc(size, align)?;
        self.bytes_reserved += block_size;
        self.bytes_allocated += delta;
        self.blocks.push(block);
        self.set_current_block(self.blocks.len() - 1);
        Some(ptr)
    }

    #[inline(always)]
    fn set_current_block(&mut self, idx: usize) {
        let block = &self.blocks[idx];
        self.current = idx;
        self.cur_ptr = (block.base + block.offset) as *mut u8;
        self.cur_end = (block.base + block.capacity) as *mut u8;
    }

    fn next_block_for(&mut self, size: usize, align: usize) -> usize {
        let worst = size
            .checked_add(align - 1)
            .expect("allocation size overflow");
        let sz = self
            .next_block_size
            .max(worst)
            .next_power_of_two()
            .min(MAX_BLOCK_SIZE);
        self.next_block_size = sz.saturating_add(sz / 2).min(MAX_BLOCK_SIZE);
        sz
    }
}
