use std::marker::PhantomData;
use std::mem;
use std::ptr::{self, NonNull};

use fastarena::Arena;

pub struct ArenaVec<'arena, T> {
    arena: &'arena mut Arena,
    ptr: NonNull<T>,
    len: usize,
    cap: usize,
    _marker: PhantomData<T>,
}

impl<'arena, T> ArenaVec<'arena, T> {
    pub fn new(arena: &'arena mut Arena) -> Self {
        ArenaVec {
            arena,
            ptr: NonNull::dangling(),
            len: 0,
            cap: 0,
            _marker: PhantomData,
        }
    }

    pub fn with_capacity(arena: &'arena mut Arena, cap: usize) -> Self {
        let mut v = ArenaVec::new(arena);
        if cap > 0 && mem::size_of::<T>() > 0 {
            v.grow_to(cap);
        } else if mem::size_of::<T>() == 0 {
            v.cap = cap;
        }
        v
    }

    #[inline]
    pub fn push(&mut self, val: T) {
        if self.len == self.cap {
            self.grow();
        }
        unsafe { self.ptr.as_ptr().add(self.len).write(val) };
        self.len += 1;
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    pub fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for item in iter {
            self.push(item);
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr() as *const T, self.len) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    pub fn finish(self) -> &'arena mut [T] {
        let ptr = self.ptr.as_ptr();
        let len = self.len;
        mem::forget(self);
        unsafe { std::slice::from_raw_parts_mut(ptr, len) }
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
