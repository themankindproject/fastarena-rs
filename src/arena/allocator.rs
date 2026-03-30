use std::mem::{self, MaybeUninit};
use std::ptr::NonNull;

use super::block::{align_up, Block, MIN_BLOCK_ALIGN};
use super::boxed::ArenaBox;
use super::stats::ArenaStats;
use crate::util::{
    inline_vec::InlineVec,
    transaction::{run_with_transaction, run_with_transaction_infallible, Transaction},
};

#[cfg(feature = "drop-tracking")]
use crate::util::drop_registry::DropRegistry;

/// Floor for block allocation — prevents degenerate tiny blocks.
const MIN_BLOCK_SIZE: usize = 64;
/// Default first-block size (64 KiB). Chosen to cover typical per-request
/// arena usage without an early spill to a second block.
const DEFAULT_BLOCK_SIZE: usize = 64 * 1_024;
/// Hard ceiling for a single block (16 MiB). Blocks larger than this add
/// pressure to the OS virtual-memory subsystem with no locality benefit.
const MAX_BLOCK_SIZE: usize = 16 * 1_024 * 1_024;
/// Number of block pointers stored inline before spilling to the heap.
/// Most workloads stay within this limit, avoiding a heap alloc for the
/// block list itself.
const BLOCKS_INLINE_CAP: usize = 8;

/// Cache-line size on x86-64 / ARM64 hardware.
pub(crate) const CACHE_LINE_SIZE: usize = 64;

/// An opaque snapshot of arena state used by [`Arena::rewind`].
///
/// Obtained via [`Arena::checkpoint`]. Must only be passed back to the arena
/// that produced it — using it with a different arena panics.
#[derive(Debug, Clone, Copy)]
#[must_use = "checkpoint is useless unless passed to Arena::rewind"]
pub struct Checkpoint {
    pub(crate) block_idx: usize,
    pub(crate) offset: usize,
    pub(crate) bytes_allocated: usize,
    #[cfg(feature = "drop-tracking")]
    pub(crate) drop_registry_len: usize,
}

impl std::fmt::Display for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Checkpoint(block={}, offset={}, bytes={})",
            self.block_idx, self.offset, self.bytes_allocated
        )
    }
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
///
/// # Multiple allocations and the borrow checker
///
/// All `alloc*` methods return `&mut T`. This prevents making multiple allocations
/// simultaneously because the borrow checker sees the arena as mutably borrowed.
/// The following code does NOT compile:
///
/// ```ignore
/// let mut arena = Arena::new();
/// let x = arena.alloc(1i32);  // &mut i32
/// let y = arena.alloc(2i32);  // ERROR: cannot borrow arena as mutable more than once
/// ```
///
/// **Workarounds:**
///
/// 1. **Immediate consumption** — transform the value before allocating another:
///    ```rust
///    use fastarena::Arena;
///
///    let mut arena = Arena::new();
///    let x = arena.alloc(1i32);
///    let sum = *x + 10;  // consume x
///    let y = arena.alloc(sum);
///    ```
///
/// 2. **Store as raw pointer** — convert the reference to a raw pointer after allocation:
///    ```rust
///    use fastarena::Arena;
///
///    let mut arena = Arena::new();
///    let x: *mut i32 = arena.alloc(1i32) as *mut _;
///    let y = arena.alloc(2i32);
///    // use x and y independently
///    ```
///
/// 3. **Use [`crate::vec::ArenaVec`]** — for multiple items of the same type:
///    ```rust
///    use fastarena::{Arena, ArenaVec};
///
///    let mut arena = Arena::new();
///    let mut vec = ArenaVec::new(&mut arena);
///    vec.push(1);
///    vec.push(2);
///    let slice = vec.finish();  // &mut [i32]
///    ```
///
/// 4. **Use [`ArenaBox<T>`]** — for owned allocation with drop semantics:
///    ```rust
///    use fastarena::{Arena, ArenaBox};
///
///    let mut arena = Arena::new();
///    let x = arena.alloc_box(1i32);
///    // x has ownership semantics - can be moved or dropped
///    assert_eq!(*x, 1);
///    ```
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
    pub(crate) cur_base: *mut u8,
    pub(crate) cur_ptr: *mut u8,
    /// End pointer of the current block (= cur_base + capacity). Cached to
    /// eliminate block array access on every fast-path allocation.
    pub(crate) cur_end: *mut u8,
    /// Highest block index ever reached — used by `reset()` to iterate only
    /// the blocks that were actually touched, instead of all retained blocks.
    high_water_mark: usize,
    /// Maximum contiguous free space across all retained blocks (post-current).
    /// Used by `alloc_slow` to skip the block scan entirely when no retained
    /// block can satisfy the request. Updated lazily on block transitions.
    largest_remaining: usize,
}

impl Arena {
    /// Create an arena with a 64 KiB initial block.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let x = arena.alloc(42u64);
    /// assert_eq!(*x, 42);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BLOCK_SIZE)
    }

    /// Create an arena with a custom initial block size.
    ///
    /// Choose a value close to expected peak usage to avoid early block
    /// chaining.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::with_capacity(1024 * 1024); // 1 MiB
    /// let _ = arena.alloc(1u64);
    /// assert_eq!(arena.stats().bytes_reserved, 1024 * 1024);
    /// ```
    #[must_use]
    pub fn with_capacity(initial_bytes: usize) -> Self {
        let size = initial_bytes.max(MIN_BLOCK_SIZE);
        let block = Block::new(size, MIN_BLOCK_ALIGN);
        let base = block.base;
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
            cur_base: base,
            cur_ptr: base,
            cur_end: unsafe { base.add(size) },
            high_water_mark: 0,
            largest_remaining: 0,
        }
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Arena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats();
        f.debug_struct("Arena")
            .field("bytes_allocated", &stats.bytes_allocated)
            .field("bytes_reserved", &stats.bytes_reserved)
            .field("block_count", &stats.block_count)
            .field("txn_depth", &self.txn_depth)
            .finish()
    }
}

impl Arena {
    /// Allocate a value of type `T`, returning an exclusive reference.
    ///
    /// Without `drop-tracking`, the destructor of `T` is never called.
    /// Arena memory is reclaimed in bulk by [`reset`](Arena::reset) or when
    /// the arena itself is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let x: &mut u64 = arena.alloc(42);
    /// assert_eq!(*x, 42);
    /// *x = 100;
    /// assert_eq!(*x, 100);
    /// ```
    #[inline]
    pub fn alloc<T>(&mut self, val: T) -> &mut T {
        if mem::size_of::<T>() == 0 {
            return unsafe { &mut *NonNull::dangling().as_ptr() };
        }
        let ptr = self.alloc_raw_inner(mem::size_of::<T>(), mem::align_of::<T>());
        unsafe {
            let typed = ptr.as_ptr().cast::<T>();
            typed.write(val);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register(typed);
            &mut *typed
        }
    }

    /// Allocate a contiguous slice of `T` from an `ExactSizeIterator`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let slice = arena.alloc_slice(0u32..5);
    /// assert_eq!(slice, &[0, 1, 2, 3, 4]);
    /// ```
    /// # Panics
    ///
    /// Panics if the iterator's `ExactSizeIterator::len` lies and more elements
    /// are produced than reported.
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
            let start = ptr.as_ptr().cast::<T>();
            Self::write_slice_bulk::<T, _>(start, &mut iter, len, total);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register_slice(start, len);
            std::slice::from_raw_parts_mut(start, len)
        }
    }

    /// Allocate a contiguous slice from a slice of `Copy` items using a single memcpy.
    /// Significantly faster than `alloc_slice` for small-to-medium `Copy` types.
    ///
    /// # Panics
    ///
    /// Panics if the system is out of memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let src: &[u64] = &[1, 2, 3, 4];
    /// let dst = arena.alloc_slice_copy(src);
    /// assert_eq!(dst, &[1, 2, 3, 4]);
    /// ```
    #[inline]
    pub fn alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> &mut [T] {
        let len = src.len();
        if len == 0 {
            return &mut [];
        }
        let total = mem::size_of::<T>().checked_mul(len).expect("overflow");
        let ptr = self.alloc_raw_inner(total, mem::align_of::<T>());
        unsafe {
            let dst = ptr.as_ptr().cast::<T>();
            std::ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register_slice(dst, len);
            std::slice::from_raw_parts_mut(dst, len)
        }
    }

    /// Copy a string slice into the arena and return a reference to it.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let s: &str = arena.alloc_str("hello world");
    /// assert_eq!(s, "hello world");
    /// ```
    #[inline(always)]
    pub fn alloc_str(&mut self, s: &str) -> &str {
        if s.is_empty() {
            return "";
        }
        // align=1, so cur_ptr needs no adjustment - dedicated fast path
        let ptr_val = self.cur_ptr as usize;
        let new_end = ptr_val + s.len();
        if new_end <= self.cur_end as usize {
            let ptr = self.cur_ptr;
            self.cur_ptr = new_end as *mut u8;
            unsafe {
                std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len());
                return std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, s.len()));
            }
        }
        self.alloc_slow_str(s)
    }

    #[cold]
    fn alloc_slow_str(&mut self, s: &str) -> &str {
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
            return unsafe { &mut *NonNull::dangling().as_ptr() };
        }
        let ptr = self.alloc_raw_inner(size, align);
        unsafe { &mut *ptr.as_ptr().cast::<MaybeUninit<T>>() }
    }

    /// Allocate an owned value `T` from the arena, returning an [`ArenaBox`].
    ///
    /// Unlike `alloc()` which returns `&mut T`, `alloc_box()` returns `ArenaBox<T>`.
    /// This provides ownership semantics, but note that the arena still uses interior
    /// mutability internally.
    ///
    /// Note: The arena must not be reset or rewound while any `ArenaBox` is still in use.
    #[inline]
    pub fn alloc_box<T>(&mut self, val: T) -> ArenaBox<'_, T> {
        ArenaBox::new(self, val)
    }

    /// Allocate `size` bytes with the given `align` alignment, initialized to zero.
    ///
    /// # Panics
    ///
    /// Panics if `align` is not a power of two or the system is out of memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.alloc_zeroed(32, 8);
    /// let buf = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), 32) };
    /// assert!(buf.iter().all(|&b| b == 0));
    /// ```
    #[inline]
    pub fn alloc_zeroed(&mut self, size: usize, align: usize) -> NonNull<u8> {
        if size == 0 {
            return NonNull::dangling();
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
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.alloc_cache_aligned(128);
    /// assert_eq!(ptr.as_ptr() as usize % 64, 0);
    /// ```
    #[inline]
    pub fn alloc_cache_aligned(&mut self, size: usize) -> NonNull<u8> {
        self.alloc_raw(size, CACHE_LINE_SIZE)
    }

    /// Low-level allocation of `size` uninitialised bytes at `align` alignment.
    ///
    /// # Panics
    /// Panics if `align` is not a power of two or the system is out of memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.alloc_raw(64, 32);
    /// assert_eq!(ptr.as_ptr() as usize % 32, 0);
    /// ```
    #[inline]
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> NonNull<u8> {
        assert!(align.is_power_of_two(), "align must be a power of two");
        if size == 0 {
            return NonNull::dangling();
        }
        self.alloc_raw_inner(size, align)
    }
}

impl Arena {
    /// Fallible variant of [`alloc`](Arena::alloc). Returns `None` on OOM.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let x = arena.try_alloc(42u64);
    /// assert_eq!(*x.unwrap(), 42);
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc<T>(&mut self, val: T) -> Option<&mut T> {
        if mem::size_of::<T>() == 0 {
            return Some(unsafe { &mut *NonNull::dangling().as_ptr() });
        }
        let ptr = self.try_alloc_raw_inner(mem::size_of::<T>(), mem::align_of::<T>())?;
        Some(unsafe {
            let typed = ptr.as_ptr().cast::<T>();
            typed.write(val);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register(typed);
            &mut *typed
        })
    }

    /// Fallible variant of [`alloc_slice`](Arena::alloc_slice).
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let s = arena.try_alloc_slice(0u32..4);
    /// assert_eq!(s.unwrap(), &[0, 1, 2, 3]);
    /// ```
    #[inline]
    #[must_use]
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
            let start = ptr.as_ptr().cast::<T>();
            Self::write_slice_bulk::<T, _>(start, &mut iter, len, total);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register_slice(start, len);
            std::slice::from_raw_parts_mut(start, len)
        })
    }

    /// Fallible variant of [`alloc_str`](Arena::alloc_str).
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let s = arena.try_alloc_str("hello");
    /// assert_eq!(s, Some("hello"));
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc_str(&mut self, s: &str) -> Option<&str> {
        if s.is_empty() {
            return Some("");
        }
        let ptr = self.try_alloc_raw_inner(s.len(), 1)?;
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), ptr.as_ptr(), s.len());
            Some(core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                ptr.as_ptr(),
                s.len(),
            )))
        }
    }

    /// Fallible variant of [`alloc_raw`](Arena::alloc_raw).
    ///
    /// # Panics
    ///
    /// Panics if `align` is not a power of two.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.try_alloc_raw(128, 64);
    /// assert!(ptr.is_some());
    /// assert_eq!(ptr.unwrap().as_ptr() as usize % 64, 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc_raw(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        assert!(align.is_power_of_two(), "align must be a power of two");
        if size == 0 {
            return Some(NonNull::dangling());
        }
        self.try_alloc_raw_inner(size, align)
    }

    /// Fallible variant of [`alloc_slice_copy`](Arena::alloc_slice_copy).
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let s = arena.try_alloc_slice_copy(&[10u64, 20, 30]);
    /// assert_eq!(s.unwrap(), &[10, 20, 30]);
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc_slice_copy<T: Copy>(&mut self, src: &[T]) -> Option<&mut [T]> {
        let len = src.len();
        if len == 0 {
            return Some(&mut []);
        }
        let total = mem::size_of::<T>().checked_mul(len)?;
        let ptr = self.try_alloc_raw_inner(total, mem::align_of::<T>())?;
        unsafe {
            let dst = ptr.as_ptr().cast::<T>();
            std::ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
            #[cfg(feature = "drop-tracking")]
            self.drop_registry.register_slice(dst, len);
            Some(std::slice::from_raw_parts_mut(dst, len))
        }
    }

    /// Fallible variant of [`alloc_zeroed`](Arena::alloc_zeroed).
    ///
    /// # Panics
    ///
    /// Panics if `align` is not a power of two.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.try_alloc_zeroed(64, 8);
    /// assert!(ptr.is_some());
    /// let buf = unsafe { std::slice::from_raw_parts(ptr.unwrap().as_ptr(), 64) };
    /// assert!(buf.iter().all(|&b| b == 0));
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc_zeroed(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        assert!(align.is_power_of_two(), "align must be a power of two");
        if size == 0 {
            return Some(NonNull::dangling());
        }
        let ptr = self.try_alloc_raw_inner(size, align)?;
        unsafe { ptr.as_ptr().write_bytes(0, size) };
        Some(ptr)
    }

    /// Fallible variant of [`alloc_cache_aligned`](Arena::alloc_cache_aligned).
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let ptr = arena.try_alloc_cache_aligned(128);
    /// assert!(ptr.is_some());
    /// assert_eq!(ptr.unwrap().as_ptr() as usize % 64, 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn try_alloc_cache_aligned(&mut self, size: usize) -> Option<NonNull<u8>> {
        self.try_alloc_raw(size, CACHE_LINE_SIZE)
    }
}

impl Arena {
    /// Capture the current allocation position as an opaque [`Checkpoint`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let _ = arena.alloc(1u64);
    /// let cp = arena.checkpoint();
    /// let _ = arena.alloc(2u64);
    /// arena.rewind(cp);
    /// assert_eq!(arena.stats().bytes_allocated, 8);
    /// ```
    #[inline(always)]
    pub fn checkpoint(&self) -> Checkpoint {
        let current_offset = self.cur_ptr as usize - self.cur_base as usize;
        Checkpoint {
            block_idx: self.current,
            offset: current_offset,
            bytes_allocated: self.bytes_allocated + current_offset,
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
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let cp = arena.checkpoint();
    /// let x = arena.alloc(0xDEADu64);
    /// arena.rewind(cp);
    /// // x is now dangling — memory reclaimed for reuse
    /// assert_eq!(arena.stats().bytes_allocated, 0);
    /// ```
    pub fn rewind(&mut self, cp: Checkpoint) {
        assert!(
            cp.block_idx < self.blocks.len(),
            "rewind: checkpoint block_idx {} out of range (arena has {} blocks)",
            cp.block_idx,
            self.blocks.len()
        );
        debug_assert!(
            cp.offset <= self.blocks[cp.block_idx].capacity,
            "rewind: checkpoint offset {} exceeds block capacity {}",
            cp.offset,
            self.blocks[cp.block_idx].capacity
        );

        #[cfg(feature = "drop-tracking")]
        self.drop_registry.run_drops_until(cp.drop_registry_len);

        for i in (cp.block_idx + 1)..=self.current {
            self.blocks.get_mut(i).offset = 0;
        }
        self.blocks.get_mut(cp.block_idx).offset = cp.offset;
        self.bytes_allocated = cp.bytes_allocated - cp.offset;
        self.set_current_block(cp.block_idx);
    }

    /// Reset the entire arena so all memory is available for reuse.
    ///
    /// No memory is freed — OS pages stay mapped and TLB-warm. If
    /// `drop-tracking` is enabled, all registered destructors run first.
    ///
    /// Complexity is O(`peak_blocks`) — only blocks that were actually used
    /// since the last reset are zeroed. Single-block arenas pay O(1).
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// for _ in 0..100 { let _ = arena.alloc(0u64); }
    /// arena.reset();
    /// assert_eq!(arena.stats().bytes_allocated, 0);
    /// ```
    pub fn reset(&mut self) {
        #[cfg(feature = "drop-tracking")]
        self.drop_registry.run_all_drops();
        for i in 0..=self.high_water_mark {
            self.blocks.get_mut(i).offset = 0;
        }
        self.high_water_mark = 0;
        self.bytes_allocated = 0;
        // Set block 0 directly without recomputing largest_remaining via set_current_block
        let b0 = self.blocks.get(0);
        self.current = 0;
        self.cur_base = b0.base;
        self.cur_ptr = b0.base;
        self.cur_end = unsafe { b0.base.add(b0.capacity) };
        // largest_remaining = max capacity among retained blocks (compute once)
        self.largest_remaining = if self.blocks.len() > 1 {
            let mut max_rem = 0;
            for i in 1..self.blocks.len() {
                let cap = self.blocks.get(i).capacity;
                if cap > max_rem {
                    max_rem = cap;
                }
            }
            max_rem
        } else {
            0
        };
    }
}

impl Arena {
    /// Open a [`Transaction`] on this arena.
    ///
    /// All allocations made through the guard are rolled back automatically
    /// when it is dropped, unless [`Transaction::commit`] is called first.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let mut txn = arena.transaction();
    /// txn.alloc(1u32);
    /// txn.alloc(2u32);
    /// txn.commit();
    /// assert!(arena.stats().bytes_allocated >= 8);
    /// ```
    #[inline]
    pub fn transaction(&mut self) -> Transaction<'_> {
        Transaction::new(self)
    }

    /// Execute a closure inside a transaction.
    ///
    /// `Ok` commits; `Err` rolls back all allocations before returning.
    ///
    /// # Errors
    ///
    /// Returns the closure's error value after rolling back all allocations.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let result = arena.with_transaction(|txn| -> Result<u32, &str> {
    ///     let x = txn.alloc(21u32);
    ///     Ok(*x * 2)
    /// });
    /// assert_eq!(result, Ok(42));
    /// ```
    #[inline]
    pub fn with_transaction<F, T, E>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    {
        run_with_transaction(self, f)
    }

    /// Execute an infallible closure inside a transaction; always commits,
    /// even if the closure panics. The panic is re-raised after the commit.
    ///
    /// If you want rollback-on-panic, use [`Arena::with_transaction`] instead.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// let val = arena.with_transaction_infallible(|txn| {
    ///     *txn.alloc(7u32) * 6
    /// });
    /// assert_eq!(val, 42);
    /// ```
    #[inline]
    pub fn with_transaction_infallible<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Transaction<'_>) -> T,
    {
        run_with_transaction_infallible(self, f)
    }

    /// Current number of open transactions and savepoints.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// assert_eq!(arena.transaction_depth(), 0);
    /// {
    ///     let mut txn = arena.transaction();
    ///     assert_eq!(txn.depth(), 1);
    ///     txn.commit();
    /// }
    /// assert_eq!(arena.transaction_depth(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn transaction_depth(&self) -> usize {
        self.txn_depth
    }
}

impl Arena {
    /// Register a raw pointer for destructor execution.
    ///
    /// Only available with the `drop-tracking` feature. Call this after
    /// [`Arena::alloc_uninit`] once the value is fully initialised.
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
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::new();
    /// for _ in 0..100 { let _ = arena.alloc(0u64); }
    /// let stats = arena.stats();
    /// assert!(stats.bytes_allocated >= 800);
    /// println!("{:.1}% utilized", stats.utilization() * 100.0);
    /// ```
    #[inline(always)]
    pub fn stats(&self) -> ArenaStats {
        let current_used = self.cur_ptr as usize - self.cur_base as usize;
        ArenaStats {
            bytes_allocated: self.bytes_allocated + current_used,
            bytes_reserved: self.bytes_reserved,
            block_count: self.blocks.len(),
        }
    }

    /// Number of blocks currently owned by the arena.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::Arena;
    ///
    /// let mut arena = Arena::with_capacity(64);
    /// for _ in 0..100 { let _ = arena.alloc(0u64); }
    /// assert!(arena.block_count() > 1);
    /// ```
    #[inline(always)]
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

impl Arena {
    /// Write elements from an iterator into an arena-allocated buffer.
    ///
    /// For `Copy` types with small total size (≤256 bytes), collects into a
    /// stack scratch buffer then bulk-copies via `copy_nonoverlapping`,
    /// amortizing per-element iterator protocol overhead. For non-`Copy` types
    /// or larger allocations, writes directly in place.
    #[inline(always)]
    unsafe fn write_slice_bulk<T, I: Iterator<Item = T>>(
        dst: *mut T,
        iter: &mut I,
        len: usize,
        total_bytes: usize,
    ) {
        if !mem::needs_drop::<T>() && mem::size_of::<T>() > 0 && total_bytes <= 256 {
            // Stack scratch buffer → single bulk memcpy to destination.
            // Amortizes per-element iterator overhead for small Copy allocations.
            let mut buf = [0u8; 256];
            let buf_ptr = buf.as_mut_ptr().cast::<T>();
            for i in 0..len {
                buf_ptr.add(i).write(iter.next().unwrap());
            }
            std::ptr::copy_nonoverlapping(buf_ptr, dst, len);
        } else {
            for i in 0..len {
                dst.add(i).write(iter.next().unwrap());
            }
        }
    }

    /// Fast-path allocation: tries the current block, falls back to `alloc_slow`.
    #[inline(always)]
    pub(crate) fn alloc_raw_inner(&mut self, size: usize, align: usize) -> NonNull<u8> {
        // For high-alignment requests, always use slow path to allocate a fresh block.
        if align > MIN_BLOCK_ALIGN {
            return self.alloc_slow(size, align);
        }
        let ptr_val = self.cur_ptr as usize;
        let aligned = align_up(ptr_val, align);
        let new_ptr = aligned.wrapping_add(size);
        if new_ptr <= self.cur_end as usize {
            self.cur_ptr = new_ptr as *mut u8;
            return unsafe { NonNull::new_unchecked(aligned as *mut u8) };
        }
        self.alloc_slow(size, align)
    }

    /// Fast-path fallible allocation: tries the current block, falls back to `alloc_slow_try`.
    #[inline(always)]
    pub(crate) fn try_alloc_raw_inner(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        // For high-alignment requests, always use slow path to allocate a fresh block.
        if align > MIN_BLOCK_ALIGN {
            return self.alloc_slow_try(size, align);
        }
        let ptr_val = self.cur_ptr as usize;
        let aligned = align_up(ptr_val, align);
        let new_ptr = aligned.checked_add(size)?;
        if new_ptr <= self.cur_end as usize {
            self.cur_ptr = new_ptr as *mut u8;
            return Some(unsafe { NonNull::new_unchecked(aligned as *mut u8) });
        }
        self.alloc_slow_try(size, align)
    }

    /// Slow path: scans retained blocks for space, then allocates a new one.
    #[cold]
    fn alloc_slow(&mut self, size: usize, align: usize) -> NonNull<u8> {
        self.finish_slow_path(size, align);

        // For high-alignment requests, skip block scan entirely and allocate a new block.
        if align > MIN_BLOCK_ALIGN {
            return self.alloc_new_block(size, align);
        }

        // Skip block scan entirely when no retained block has enough free space.
        if size <= self.largest_remaining {
            for i in (self.current + 1)..self.blocks.len() {
                let block = self.blocks.get_mut(i);
                if block.align >= align {
                    if let Some((ptr, delta)) = block.try_alloc(size, align) {
                        self.bytes_allocated += delta;
                        self.set_current_block(i);
                        return ptr;
                    }
                }
            }
        }

        self.alloc_new_block(size, align)
    }

    /// Slow path (fallible): scans retained blocks for space, then tries to allocate a new one.
    #[cold]
    fn alloc_slow_try(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        self.finish_slow_path(size, align);

        // For high-alignment requests, skip block scan entirely and allocate a new block.
        if align > MIN_BLOCK_ALIGN {
            return self.try_alloc_new_block(size, align);
        }

        // Skip block scan entirely when no retained block has enough free space.
        if size <= self.largest_remaining {
            for i in (self.current + 1)..self.blocks.len() {
                let block = self.blocks.get_mut(i);
                if block.align >= align {
                    if let Some((ptr, delta)) = block.try_alloc(size, align) {
                        self.bytes_allocated += delta;
                        self.set_current_block(i);
                        return Some(ptr);
                    }
                }
            }
        }

        self.try_alloc_new_block(size, align)
    }

    /// Common slow-path setup: flushes the current block's state.
    #[inline]
    fn finish_slow_path(&mut self, _size: usize, _align: usize) {
        self.bytes_allocated += self.cur_ptr as usize - self.cur_base as usize;
        self.blocks.get_mut(self.current).offset = self.cur_ptr as usize - self.cur_base as usize;
    }

    /// Allocate a new block (infallible). Panics if allocation fails.
    #[inline]
    fn alloc_new_block(&mut self, size: usize, align: usize) -> NonNull<u8> {
        let block_size = self.next_block_for(size, align);
        let mut block = Block::new(block_size, align);
        let (ptr, delta) = block
            .try_alloc(size, align)
            .expect("fresh block must satisfy request");
        self.bytes_reserved += block_size;
        self.bytes_allocated += delta;
        self.blocks.push(block);
        self.set_current_block(self.blocks.len() - 1);
        ptr
    }

    /// Allocate a new block (fallible). Returns None if allocation fails.
    #[inline]
    fn try_alloc_new_block(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        let block_size = self.next_block_for(size, align);
        let mut block = Block::try_new(block_size, align)?;
        let (ptr, delta) = block.try_alloc(size, align)?;
        self.bytes_reserved += block_size;
        self.bytes_allocated += delta;
        self.blocks.push(block);
        self.set_current_block(self.blocks.len() - 1);
        Some(ptr)
    }

    /// Sets `idx` as the active block and updates cached pointers.
    #[inline(always)]
    fn set_current_block(&mut self, idx: usize) {
        let block = self.blocks.get(idx);
        self.current = idx;
        if idx > self.high_water_mark {
            self.high_water_mark = idx;
        }
        self.cur_base = block.base;
        self.cur_ptr = unsafe { block.base.add(block.offset) };
        self.cur_end = unsafe { block.base.add(block.capacity) };
        self.largest_remaining = self.compute_largest_remaining(idx);
    }

    /// Computes the size of the next block, growing 1.5× up to `MAX_BLOCK_SIZE`.
    fn next_block_for(&mut self, size: usize, align: usize) -> usize {
        let worst = size
            .checked_add(align - 1)
            .expect("allocation size overflow");

        let rounded = self
            .next_block_size
            .max(worst)
            .max(align * 2)
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .min(MAX_BLOCK_SIZE)
            .max(worst)
            .max(align * 2);

        self.next_block_size = rounded.saturating_add(rounded / 2).min(MAX_BLOCK_SIZE);
        rounded
    }

    /// Computes the maximum contiguous free space in blocks after `from_idx`.
    #[inline]
    fn compute_largest_remaining(&self, from_idx: usize) -> usize {
        let mut max_rem = 0;
        for i in (from_idx + 1)..self.blocks.len() {
            let rem = self.blocks.get(i).capacity - self.blocks.get(i).offset;
            if rem > max_rem {
                max_rem = rem;
            }
        }
        max_rem
    }
}
