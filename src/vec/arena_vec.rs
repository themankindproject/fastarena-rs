use core::marker::PhantomData;
use core::mem::{self, ManuallyDrop};
use core::ptr::{self, NonNull};

use crate::arena::Arena;

/// Error returned by [`ArenaVec::try_reserve`] when allocation fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryReserveError {
    /// Computed capacity would overflow `usize`.
    CapacityOverflow,
    /// The arena is out of memory.
    AllocError,
}

impl From<core::alloc::LayoutError> for TryReserveError {
    fn from(_: core::alloc::LayoutError) -> Self {
        TryReserveError::CapacityOverflow
    }
}

impl std::fmt::Display for TryReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryReserveError::CapacityOverflow => {
                f.write_str("capacity overflow: requested size exceeds usize::MAX")
            }
            TryReserveError::AllocError => f.write_str("arena out of memory"),
        }
    }
}

impl std::error::Error for TryReserveError {}

/// An append-only growable vector backed by arena memory.
///
/// Elements 0..`capacity` are stored in a single arena allocation. Growth
/// copies elements to a larger allocation and abandons the old one — the arena
/// reclaims both on `reset`. This gives amortised O(1) push with the same
/// cache locality as a `Vec`.
///
/// # Destructor behaviour
///
/// - **`finish()`** → elements are arena-owned; `ArenaVec` does not run their
///   destructors. If `drop-tracking` is enabled they will be dropped by
///   [`Arena::reset`] / [`Arena::rewind`].
/// - **`drop` without `finish()`** → element destructors run immediately. The
///   backing memory is not freed (the arena retains it).
///
/// # Example
///
/// ```rust
/// use fastarena::{Arena, ArenaVec};
///
/// let mut arena = Arena::new();
/// let slice: &mut [u32] = {
///     let mut v = ArenaVec::new(&mut arena);
///     v.push(1); v.push(2); v.push(3);
///     v.finish()
/// };
/// assert_eq!(slice, &[1, 2, 3]);
/// ```
pub struct ArenaVec<'arena, T> {
    arena: &'arena mut Arena,
    ptr: NonNull<T>,
    len: usize,
    cap: usize,
    _marker: PhantomData<T>,
}

impl<'arena, T> ArenaVec<'arena, T> {
    /// Create an empty `ArenaVec`. No allocation is made until the first push.
    pub fn new(arena: &'arena mut Arena) -> Self {
        ArenaVec {
            arena,
            ptr: NonNull::dangling(),
            len: 0,
            cap: 0,
            _marker: PhantomData,
        }
    }

    /// Create an `ArenaVec` pre-allocated for `cap` elements, avoiding growth
    /// copies when the final size is known upfront.
    pub fn with_capacity(arena: &'arena mut Arena, cap: usize) -> Self {
        let mut v = ArenaVec::new(arena);
        if cap > 0 && mem::size_of::<T>() > 0 {
            v.grow_to(cap);
        } else if mem::size_of::<T>() == 0 {
            v.cap = cap;
        }
        v
    }

    /// Append `val`. Amortised O(1).
    #[inline]
    pub fn push(&mut self, val: T) {
        if self.len == self.cap {
            self.grow();
        }
        unsafe { self.ptr.as_ptr().add(self.len).write(val) };
        self.len += 1;
    }

    /// Try to append `val`, returning it back on OOM.
    ///
    /// Returns `Ok(())` on success, `Err(val)` if the arena is out of memory.
    #[inline]
    pub fn try_push(&mut self, val: T) -> Result<(), T> {
        if self.len == self.cap && self.try_grow().is_err() {
            return Err(val);
        }
        unsafe { self.ptr.as_ptr().add(self.len).write(val) };
        self.len += 1;
        Ok(())
    }

    /// Remove and return the last element, or `None` if empty.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Remove all elements from the vector without freeing memory.
    ///
    /// Element destructors are run if `T: Drop`. Capacity is preserved.
    #[inline]
    pub fn clear(&mut self) {
        if mem::needs_drop::<T>() {
            for i in 0..self.len {
                unsafe { ptr::drop_in_place(self.ptr.as_ptr().add(i)) }
            }
        }
        self.len = 0;
    }

    /// Append all items from `iter`.
    ///
    /// Requires `ExactSizeIterator` to pre-compute capacity and avoid repeated
    /// reallocation during growth.
    #[inline]
    pub fn extend_exact<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let add_len = iter.len();
        if add_len > 0 {
            let new_len = self
                .len
                .checked_add(add_len)
                .expect("ArenaVec: capacity overflow");
            let size = mem::size_of::<T>();
            if size > 0 && new_len > self.cap {
                self.grow_to(new_len);
            }
            unsafe {
                let dst = self.ptr.as_ptr().add(self.len);
                for i in 0..add_len {
                    dst.add(i).write(iter.next().unwrap());
                }
            }
            self.len = new_len;
        }
    }

    /// Copies elements from a slice into the vector.
    ///
    /// This is more efficient than `extend` when the source is already a slice,
    /// as it can use a single `memcpy`-style copy via `ptr::copy_nonoverlapping`.
    ///
    /// # Panics
    ///
    /// Panics if the new length exceeds the arena's capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::{Arena, ArenaVec};
    ///
    /// let mut arena = Arena::new();
    /// let mut v = ArenaVec::new(&mut arena);
    /// v.extend_from_slice(&[1u32, 2, 3, 4]);
    /// assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
    /// ```
    #[inline]
    pub fn extend_from_slice(&mut self, slice: &[T])
    where
        T: Copy,
    {
        let add_len = slice.len();
        if add_len == 0 {
            return;
        }
        let new_len = self
            .len
            .checked_add(add_len)
            .expect("ArenaVec: capacity overflow");
        if mem::size_of::<T>() > 0 && new_len > self.cap {
            self.grow_to(new_len);
        }
        unsafe {
            let dst = self.ptr.as_ptr().add(self.len);
            ptr::copy_nonoverlapping(slice.as_ptr(), dst, add_len);
        }
        self.len = new_len;
    }

    /// Returns the number of elements in the vector.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the vector contains no elements.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the current capacity of the vector.
    ///
    /// Capacity is the number of elements the vector can hold without
    /// reallocating. For ZSTs (zero-sized types), capacity is tracked
    /// independently of actual memory.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Reserves capacity for at least `additional` more elements in the vector.
    ///
    /// After calling `reserve`, the vector will have capacity for at least
    /// `self.len() + additional` elements without reallocating. This does not
    /// change the vector's length.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::{Arena, ArenaVec};
    ///
    /// let mut arena = Arena::new();
    /// let mut v = ArenaVec::new(&mut arena);
    /// v.push(1u32);
    /// v.reserve(10);
    /// assert!(v.capacity() >= 11);
    /// ```
    pub fn reserve(&mut self, additional: usize) {
        let required = self.len.saturating_add(additional);
        if required > self.cap {
            self.grow_to(required);
        }
    }

    /// Reserves exactly `additional` additional elements of capacity.
    ///
    /// For arena-allocated vectors, this is identical to [`reserve`](Self::reserve)
    /// since arena memory is not subject to fragmentation. The capacity may
    /// exceed `len + additional` if the arena's growth strategy requires it.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize`.
    pub fn reserve_exact(&mut self, additional: usize) {
        let required = self
            .len
            .checked_add(additional)
            .expect("ArenaVec: capacity overflow");
        if required > self.cap {
            self.grow_to(required);
        }
    }

    /// Attempts to reserve exactly `additional` additional elements of capacity.
    ///
    /// Returns an error instead of panicking when the capacity overflows or the
    /// arena is out of memory.
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), TryReserveError> {
        let required = self
            .len
            .checked_add(additional)
            .ok_or(TryReserveError::CapacityOverflow)?;
        if required > self.cap {
            self.try_grow_to(required)?;
        }
        Ok(())
    }

    /// Attempts to reserve capacity for at least `additional` more elements.
    ///
    /// Unlike [`reserve`](Self::reserve), this returns an error instead of
    /// panicking when memory cannot be allocated.
    ///
    /// # Errors
    ///
    /// Returns [`CapacityOverflow`](TryReserveError::CapacityOverflow) if the
    /// required capacity would overflow `usize`. Returns
    /// [`AllocError`](TryReserveError::AllocError) if the arena is out of memory.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastarena::{Arena, ArenaVec};
    ///
    /// let mut arena = Arena::new();
    /// let mut v: ArenaVec<u32> = ArenaVec::new(&mut arena);
    /// assert!(v.try_reserve(100).is_ok());
    /// ```
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError> {
        let required = self.len.saturating_add(additional);
        if required > self.cap {
            self.try_grow_to(required)?;
        }
        Ok(())
    }

    fn try_grow_to(&mut self, new_cap: usize) -> Result<(), TryReserveError> {
        if mem::size_of::<T>() == 0 {
            self.cap = new_cap;
            return Ok(());
        }
        let bytes = new_cap
            .checked_mul(mem::size_of::<T>())
            .ok_or(TryReserveError::CapacityOverflow)?;
        let raw = self
            .arena
            .try_alloc_raw(bytes, mem::align_of::<T>())
            .ok_or(TryReserveError::AllocError)?;
        let new_ptr = raw.as_ptr() as *mut T;
        if self.len > 0 {
            unsafe { ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr, self.len) };
        }
        self.ptr = unsafe { NonNull::new_unchecked(new_ptr) };
        self.cap = new_cap;
        Ok(())
    }

    /// Returns a slice view of the vector's current contents.
    ///
    /// The slice length equals `self.len()` at the time of the call.
    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr() as *const T, self.len) }
    }

    /// Returns a mutable slice view of the vector's current contents.
    ///
    /// The slice length equals `self.len()` at the time of the call.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Consume the `ArenaVec`, returning a `&'arena mut [T]` backed by arena
    /// memory. The arena borrow is released; element destructors will **not**
    /// be called by `ArenaVec` after this point.
    pub fn finish(self) -> &'arena mut [T] {
        let this = ManuallyDrop::new(self);
        unsafe { core::slice::from_raw_parts_mut(this.ptr.as_ptr(), this.len) }
    }

    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let new_cap = if self.cap == 0 {
            4
        } else {
            self.cap
                .checked_mul(2)
                .expect("ArenaVec: capacity overflow")
        };
        self.grow_to(new_cap);
    }

    #[cold]
    fn try_grow(&mut self) -> Result<(), TryReserveError> {
        let new_cap = if self.cap == 0 {
            4
        } else {
            self.cap
                .checked_mul(2)
                .ok_or(TryReserveError::CapacityOverflow)?
        };
        self.try_grow_to(new_cap)
    }

    #[cold]
    fn grow_to(&mut self, new_cap: usize) {
        if mem::size_of::<T>() == 0 {
            self.cap = new_cap;
            return;
        }
        let bytes = new_cap
            .checked_mul(mem::size_of::<T>())
            .expect("ArenaVec: capacity overflow");
        let raw = self.arena.alloc_raw(bytes, mem::align_of::<T>());
        let new_ptr = raw.as_ptr() as *mut T;
        if self.len > 0 {
            unsafe { ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr, self.len) };
        }
        self.ptr = unsafe { NonNull::new_unchecked(new_ptr) };
        self.cap = new_cap;
    }
}

impl<T> core::ops::Index<usize> for ArenaVec<'_, T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        assert!(i < self.len, "index {i} out of bounds (len={})", self.len);
        unsafe { &*self.ptr.as_ptr().add(i) }
    }
}

impl<T> core::ops::IndexMut<usize> for ArenaVec<'_, T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        assert!(i < self.len, "index {i} out of bounds (len={})", self.len);
        unsafe { &mut *self.ptr.as_ptr().add(i) }
    }
}

impl<T> Drop for ArenaVec<'_, T> {
    fn drop(&mut self) {
        if mem::needs_drop::<T>() {
            for i in 0..self.len {
                unsafe { ptr::drop_in_place(self.ptr.as_ptr().add(i)) }
            }
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ArenaVec<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.as_slice().iter()).finish()
    }
}

impl<'arena, T> Extend<T> for ArenaVec<'arena, T> {
    /// Extends the vector by consuming items from the iterator one by one.
    ///
    /// This trait impl accepts any `IntoIterator`, unlike the [`extend_exact`](ArenaVec::extend_exact)
    /// method which requires `ExactSizeIterator` to pre-allocate capacity.
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        for item in iter {
            self.push(item);
        }
    }
}

impl<'arena, T> IntoIterator for ArenaVec<'arena, T> {
    type Item = T;
    type IntoIter = ArenaVecIntoIter<'arena, T>;
    fn into_iter(self) -> Self::IntoIter {
        ArenaVecIntoIter {
            inner: self,
            start: 0,
        }
    }
}

/// Owning iterator over an [`ArenaVec`]. Drains elements front-to-back and
/// drops any remaining elements (in LIFO order) when the iterator itself is dropped.
pub struct ArenaVecIntoIter<'arena, T> {
    inner: ArenaVec<'arena, T>,
    start: usize,
}

impl<'arena, T> Iterator for ArenaVecIntoIter<'arena, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start >= self.inner.len {
            return None;
        }
        let val = unsafe { self.inner.ptr.as_ptr().add(self.start).read() };
        self.start += 1;
        Some(val)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.inner.len - self.start;
        (remaining, Some(remaining))
    }
}

impl<'arena, T> ExactSizeIterator for ArenaVecIntoIter<'arena, T> {}

impl<T> Drop for ArenaVecIntoIter<'_, T> {
    fn drop(&mut self) {
        if core::mem::needs_drop::<T>() {
            for i in self.start..self.inner.len {
                unsafe { core::ptr::drop_in_place(self.inner.ptr.as_ptr().add(i)) }
            }
        }
        self.inner.len = 0;
    }
}

impl<'a, T> IntoIterator for &'a ArenaVec<'_, T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T> IntoIterator for &'a mut ArenaVec<'_, T> {
    type Item = &'a mut T;
    type IntoIter = core::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn push_and_index() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::new(&mut arena);
        for i in 0u32..8 {
            v.push(i);
        }
        for i in 0u32..8 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn pop_order() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::new(&mut arena);
        v.push(1u32);
        v.push(2);
        v.push(3);
        assert_eq!(v.pop(), Some(3));
        assert_eq!(v.pop(), Some(2));
        assert_eq!(v.pop(), Some(1));
        assert_eq!(v.pop(), None);
    }

    #[test]
    fn finish_slice() {
        let mut arena = Arena::new();
        let s = {
            let mut v = ArenaVec::new(&mut arena);
            v.extend_exact(0u32..5);
            v.finish()
        };
        assert_eq!(s, &[0, 1, 2, 3, 4]);
        let _ = arena.alloc(99u32);
    }

    #[test]
    fn grow_many() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::new(&mut arena);
        for i in 0u64..256 {
            v.push(i);
        }
        for i in 0u64..256 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn with_capacity_no_realloc() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::<u64>::with_capacity(&mut arena, 16);
        let cap0 = v.capacity();
        for i in 0u64..16 {
            v.push(i);
        }
        assert_eq!(v.capacity(), cap0);
        v.finish();
    }

    #[test]
    fn drop_runs_dtors() {
        static N: AtomicUsize = AtomicUsize::new(0);
        struct D;
        impl Drop for D {
            fn drop(&mut self) {
                N.fetch_add(1, Ordering::Relaxed);
            }
        }
        N.store(0, Ordering::Relaxed);
        {
            let mut arena = Arena::new();
            let mut v = ArenaVec::new(&mut arena);
            v.push(D);
            v.push(D);
            v.push(D);
        }
        assert_eq!(N.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn finish_skips_dtors() {
        static N: AtomicUsize = AtomicUsize::new(0);
        struct D;
        impl Drop for D {
            fn drop(&mut self) {
                N.fetch_add(1, Ordering::Relaxed);
            }
        }
        N.store(0, Ordering::Relaxed);
        let mut arena = Arena::new();
        let v = {
            let mut av = ArenaVec::new(&mut arena);
            av.push(D);
            av.push(D);
            av.finish()
        };
        assert_eq!(N.load(Ordering::Relaxed), 0);
        let _ = v;
    }

    #[test]
    fn zst_push() {
        let mut arena = Arena::new();
        let mut v: ArenaVec<()> = ArenaVec::new(&mut arena);
        for _ in 0..1000 {
            v.push(());
        }
        assert_eq!(v.len(), 1000);
    }

    #[test]
    #[should_panic]
    fn oob_panics() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::new(&mut arena);
        v.push(1u32);
        let _ = v[1];
    }

    #[test]
    fn into_iter_yields_forward_order() {
        let mut arena = Arena::new();
        let mut v = ArenaVec::new(&mut arena);
        v.push(1u32);
        v.push(2);
        v.push(3);
        let collected: Vec<u32> = v.into_iter().collect();
        assert_eq!(collected, &[1, 2, 3]);
    }
}
