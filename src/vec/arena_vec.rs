use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr::{self, NonNull};

use crate::arena::Arena;

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
    pub fn push(&mut self, val: T) {
        if self.len == self.cap {
            self.grow();
        }
        unsafe { self.ptr.as_ptr().add(self.len).write(val) };
        self.len += 1;
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

    /// Append all items from `iter`.
    #[inline]
    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let add_len = iter.len();
        if add_len > 0 {
            let new_len = self.len + add_len;
            let size = mem::size_of::<T>();
            if size > 0 && new_len > self.cap {
                self.grow_to(new_len);
            }
            unsafe {
                let dst = self.ptr.as_ptr().add(self.len);
                for i in 0..add_len {
                    dst.add(i).write(iter.next().unwrap_unchecked());
                }
            }
            self.len = new_len;
        }
    }

    /// Extend from an existing slice by copying its elements.
    #[inline]
    pub fn extend_from_slice(&mut self, slice: &[T])
    where
        T: Copy,
    {
        let add_len = slice.len();
        if add_len == 0 {
            return;
        }
        let new_len = self.len + add_len;
        if mem::size_of::<T>() > 0 && new_len > self.cap {
            self.grow_to(new_len);
        }
        unsafe {
            let dst = self.ptr.as_ptr().add(self.len);
            ptr::copy_nonoverlapping(slice.as_ptr(), dst, add_len);
        }
        self.len = new_len;
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Reserves capacity for at least `additional` more elements.
    ///
    /// May panic if capacity overflows.
    pub fn reserve(&mut self, additional: usize) {
        let required = self.len.saturating_add(additional);
        if required > self.cap {
            self.grow_to(required);
        }
    }

    /// Reserves exact capacity for `additional` elements.
    ///
    /// May panic if capacity overflows.
    pub fn reserve_exact(&mut self, additional: usize) {
        self.reserve(additional);
    }

    /// Tries to reserve capacity for at least `additional` more elements.
    ///
    /// Returns `Err` on allocation failure.
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), ()> {
        let required = self.len.saturating_add(additional);
        if required > self.cap {
            self.try_grow_to(required)?;
        }
        Ok(())
    }

    fn try_grow_to(&mut self, new_cap: usize) -> Result<(), ()> {
        if mem::size_of::<T>() == 0 {
            self.cap = new_cap;
            return Ok(());
        }
        let bytes = new_cap.checked_mul(mem::size_of::<T>()).ok_or(())?;
        let raw = self
            .arena
            .try_alloc_raw(bytes, mem::align_of::<T>())
            .ok_or(())?;
        let new_ptr = raw.as_ptr() as *mut T;
        if self.len > 0 {
            unsafe { ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr, self.len) };
        }
        self.ptr = unsafe { NonNull::new_unchecked(new_ptr) };
        self.cap = new_cap;
        Ok(())
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr() as *const T, self.len) }
    }

    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Consume the `ArenaVec`, returning a `&'arena mut [T]` backed by arena
    /// memory. The arena borrow is released; element destructors will **not**
    /// be called by `ArenaVec` after this point.
    pub fn finish(self) -> &'arena mut [T] {
        let this = ManuallyDrop::new(self);
        unsafe { std::slice::from_raw_parts_mut(this.ptr.as_ptr(), this.len) }
    }

    fn grow(&mut self) {
        let new_cap = if self.cap == 0 { 4 } else { self.cap * 2 };
        self.grow_to(new_cap);
    }

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

impl<T> std::ops::Index<usize> for ArenaVec<'_, T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        assert!(i < self.len, "index {i} out of bounds (len={})", self.len);
        unsafe { &*self.ptr.as_ptr().add(i) }
    }
}

impl<T> std::ops::IndexMut<usize> for ArenaVec<'_, T> {
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

impl<'arena, T> Extend<T> for ArenaVec<'arena, T> {
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        for item in iter {
            self.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            v.extend(0u32..5);
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
}
