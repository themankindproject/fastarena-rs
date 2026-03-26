use core::mem::{ManuallyDrop, MaybeUninit};
use core::ptr;
use std::alloc::{alloc, dealloc, Layout};

/// Heap storage descriptor, stored in the union when spilled to the heap.
#[derive(Clone, Copy)]
struct HeapBuf<T> {
    /// Pointer to heap-allocated buffer.
    ptr: *mut T,
    /// Capacity of the heap buffer.
    cap: usize,
}

/// Storage union: inline array or heap buffer.
///
/// Uses a union to avoid the overhead of an enum discriminant while
/// maintaining safety through separate `on_heap` tracking.
union Storage<T, const N: usize> {
    /// Inline storage: fixed-size array on the stack.
    inline: ManuallyDrop<[MaybeUninit<T>; N]>,
    /// Heap storage: dynamically allocated buffer.
    heap: ManuallyDrop<HeapBuf<T>>,
}

/// A growable vector that stores the first `N` elements inline (no heap
/// allocation) and spills to a heap buffer only when that capacity is exceeded.
///
/// Used internally for `Arena::blocks` (`N = 8`) and `DropRegistry::entries`
/// (`N = 32`). Typical arena workloads never exceed these limits, so no heap
/// allocation occurs for either collection during the arena's lifetime.
pub(crate) struct InlineVec<T, const N: usize> {
    /// Storage union (inline or heap).
    data: Storage<T, N>,
    /// Number of valid elements.
    len: usize,
    /// Whether data is stored on heap (true) or inline (false).
    on_heap: bool,
}

unsafe impl<T: Send, const N: usize> Send for InlineVec<T, N> where [T; N]: Send {}
unsafe impl<T: Sync, const N: usize> Sync for InlineVec<T, N> where [T; N]: Sync {}

impl<T, const N: usize> InlineVec<T, N> {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        const { assert!(N > 0, "InlineVec requires N > 0") };
        InlineVec {
            data: Storage {
                inline: ManuallyDrop::new(unsafe { std::mem::MaybeUninit::uninit().assume_init() }),
            },
            len: 0,
            on_heap: false,
        }
    }

    /// Returns the number of elements.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Appends an element. Stays inline if capacity allows; spills to heap on overflow.
    #[inline(always)]
    pub(crate) fn push(&mut self, val: T) {
        if !self.on_heap && self.len < N {
            unsafe { (*self.data.inline)[self.len].write(val) };
            self.len += 1;
            return;
        }
        self.push_slow(val);
    }

    #[cold]
    fn push_slow(&mut self, val: T) {
        if self.on_heap {
            if self.len == unsafe { (*self.data.heap).cap } {
                self.heap_grow();
            }
            unsafe { (*self.data.heap).ptr.add(self.len).write(val) };
            self.len += 1;
        } else {
            self.promote_and_push(val);
        }
    }

    /// Removes and returns the last element, or `None` if empty.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(if self.on_heap {
            unsafe { (*self.data.heap).ptr.add(self.len).read() }
        } else {
            unsafe { (*self.data.inline)[self.len].assume_init_read() }
        })
    }

    /// Returns a shared reference to the element at index `i`.
    #[inline(always)]
    pub(crate) fn get(&self, i: usize) -> &T {
        debug_assert!(i < self.len);
        if self.on_heap {
            unsafe { &*(*self.data.heap).ptr.add(i) }
        } else {
            unsafe { (*self.data.inline)[i].assume_init_ref() }
        }
    }

    /// Returns a mutable reference to the element at index `i`.
    #[inline(always)]
    pub(crate) fn get_mut(&mut self, i: usize) -> &mut T {
        debug_assert!(i < self.len);
        if self.on_heap {
            unsafe { &mut *(*self.data.heap).ptr.add(i) }
        } else {
            unsafe { (*self.data.inline)[i].assume_init_mut() }
        }
    }

    /// Returns a shared slice view of all elements.
    #[inline(always)]
    pub(crate) fn as_slice(&self) -> &[T] {
        if self.on_heap {
            unsafe { std::slice::from_raw_parts((*self.data.heap).ptr.cast_const(), self.len) }
        } else {
            unsafe {
                std::slice::from_raw_parts((*self.data.inline).as_ptr().cast::<T>(), self.len)
            }
        }
    }

    /// Returns a mutable slice view of all elements.
    #[inline(always)]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        if self.on_heap {
            unsafe { std::slice::from_raw_parts_mut((*self.data.heap).ptr, self.len) }
        } else {
            unsafe {
                std::slice::from_raw_parts_mut(
                    (*self.data.inline).as_mut_ptr().cast::<T>(),
                    self.len,
                )
            }
        }
    }

    /// Returns a forward iterator over elements.
    #[inline]
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }
    /// Returns a mutable forward iterator over elements.
    #[inline]
    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    /// Moves inline storage to a heap buffer and appends `val`.
    #[cold]
    fn promote_and_push(&mut self, val: T) {
        let new_cap = N.checked_mul(2).expect("InlineVec: capacity overflow");
        let new_ptr = heap_alloc::<T>(new_cap);
        unsafe { ptr::copy_nonoverlapping((*self.data.inline).as_ptr().cast::<T>(), new_ptr, N) };
        self.data = Storage {
            heap: ManuallyDrop::new(HeapBuf {
                ptr: new_ptr,
                cap: new_cap,
            }),
        };
        self.on_heap = true;
        unsafe { new_ptr.add(self.len).write(val) };
        self.len += 1;
    }

    /// Doubles the heap buffer capacity.
    #[cold]
    fn heap_grow(&mut self) {
        let old_cap = unsafe { (*self.data.heap).cap };
        let old_ptr = unsafe { (*self.data.heap).ptr };
        let new_cap = old_cap
            .checked_mul(2)
            .expect("InlineVec: capacity overflow");
        let new_ptr = heap_alloc::<T>(new_cap);
        unsafe { ptr::copy_nonoverlapping(old_ptr, new_ptr, self.len) };
        let old_layout = Layout::array::<T>(old_cap).expect("InlineVec: layout overflow");
        if old_layout.size() > 0 {
            unsafe { dealloc(old_ptr.cast::<u8>(), old_layout) };
        }
        self.data = Storage {
            heap: ManuallyDrop::new(HeapBuf {
                ptr: new_ptr,
                cap: new_cap,
            }),
        };
    }
}

impl<T, const N: usize> std::ops::Index<usize> for InlineVec<T, N> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        assert!(i < self.len, "index {i} out of bounds (len={})", self.len);
        if self.on_heap {
            unsafe { &*(*self.data.heap).ptr.add(i) }
        } else {
            unsafe { (*self.data.inline)[i].assume_init_ref() }
        }
    }
}
impl<T, const N: usize> std::ops::IndexMut<usize> for InlineVec<T, N> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        assert!(i < self.len, "index {i} out of bounds (len={})", self.len);
        if self.on_heap {
            unsafe { &mut *(*self.data.heap).ptr.add(i) }
        } else {
            unsafe { (*self.data.inline)[i].assume_init_mut() }
        }
    }
}
impl<'a, T, const N: usize> IntoIterator for &'a InlineVec<T, N> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<'a, T, const N: usize> IntoIterator for &'a mut InlineVec<T, N> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T, const N: usize> Drop for InlineVec<T, N> {
    fn drop(&mut self) {
        if std::mem::needs_drop::<T>() {
            if self.on_heap {
                for i in 0..self.len {
                    unsafe { ptr::drop_in_place((*self.data.heap).ptr.add(i)) }
                }
            } else {
                for i in 0..self.len {
                    unsafe { (*self.data.inline)[i].assume_init_drop() }
                }
            }
        }
        if self.on_heap {
            let cap = unsafe { (*self.data.heap).cap };
            let raw = unsafe { (*self.data.heap).ptr }.cast::<u8>();
            let layout = Layout::array::<T>(cap).expect("InlineVec: layout overflow on drop");
            if layout.size() > 0 {
                unsafe { dealloc(raw, layout) };
            }
        }
    }
}

/// Allocates memory on the heap for `cap` elements of type `T`.
///
/// # Panics
///
/// Panics if allocation fails or if `cap` is so large that `Layout::array`
/// would overflow.
fn heap_alloc<T>(cap: usize) -> *mut T {
    let layout = Layout::array::<T>(cap).expect("InlineVec: layout overflow");
    if layout.size() == 0 {
        return std::ptr::NonNull::dangling().as_ptr();
    }
    let raw = unsafe { alloc(layout) };
    assert!(!raw.is_null(), "InlineVec: out of memory");
    raw.cast::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn push_within_inline() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        for i in 0u32..4 {
            v.push(i);
        }
        assert!(!v.on_heap);
        assert_eq!(v.as_slice(), &[0, 1, 2, 3]);
    }

    #[test]
    fn push_triggers_promotion() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        for i in 0u32..5 {
            v.push(i);
        }
        assert!(v.on_heap);
        for i in 0u32..5 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn values_preserved_across_promotion() {
        let mut v: InlineVec<u64, 4> = InlineVec::new();
        for x in [0xAAAAu64, 0xBBBB, 0xCCCC, 0xDDDD, 0xEEEE] {
            v.push(x);
        }
        assert_eq!(v.as_slice(), &[0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD, 0xEEEE]);
    }

    #[test]
    fn push_many_grows_heap() {
        let mut v: InlineVec<u64, 4> = InlineVec::new();
        for i in 0u64..128 {
            v.push(i);
        }
        for i in 0u64..128 {
            assert_eq!(v[i as usize], i);
        }
    }

    #[test]
    fn pop_inline() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        v.push(1);
        v.push(2);
        assert_eq!(v.pop(), Some(2));
        assert_eq!(v.pop(), Some(1));
        assert_eq!(v.pop(), None);
    }

    #[test]
    fn pop_heap() {
        let mut v: InlineVec<u32, 2> = InlineVec::new();
        v.push(10);
        v.push(20);
        v.push(30);
        assert_eq!(v.pop(), Some(30));
    }

    #[test]
    fn as_slice_inline() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        v.push(7);
        v.push(8);
        assert_eq!(v.as_slice(), &[7u32, 8]);
    }

    #[test]
    fn as_slice_heap() {
        let mut v: InlineVec<u32, 2> = InlineVec::new();
        v.push(1);
        v.push(2);
        v.push(3);
        assert_eq!(v.as_slice(), &[1u32, 2, 3]);
    }

    #[test]
    fn iter_yields_all() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        for i in 0u32..6 {
            v.push(i);
        }
        let got: Vec<u32> = v.iter().copied().collect();
        assert_eq!(got, &[0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn iter_mut_modifies() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        for i in 0u32..4 {
            v.push(i);
        }
        for x in v.iter_mut() {
            *x *= 2;
        }
        assert_eq!(v.as_slice(), &[0, 2, 4, 6]);
    }

    #[test]
    fn index_mut() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        v.push(0);
        v.push(0);
        v[0] = 99;
        v[1] = 77;
        assert_eq!((v[0], v[1]), (99, 77));
    }

    #[test]
    #[should_panic = "out of bounds"]
    fn oob_panics() {
        let mut v: InlineVec<u32, 4> = InlineVec::new();
        v.push(1);
        let _ = v[1];
    }

    #[test]
    fn no_heap_alloc_for_small_usage() {
        let mut v: InlineVec<u64, 8> = InlineVec::new();
        for i in 0u64..8 {
            v.push(i);
        }
        assert!(!v.on_heap);
    }

    #[test]
    fn drop_runs_dtors_inline() {
        static N: AtomicUsize = AtomicUsize::new(0);
        struct D;
        impl Drop for D {
            fn drop(&mut self) {
                N.fetch_add(1, Ordering::Relaxed);
            }
        }
        N.store(0, Ordering::Relaxed);
        {
            let mut v: InlineVec<D, 4> = InlineVec::new();
            v.push(D);
            v.push(D);
            v.push(D);
        }
        assert_eq!(N.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn drop_runs_dtors_heap() {
        static N: AtomicUsize = AtomicUsize::new(0);
        struct D;
        impl Drop for D {
            fn drop(&mut self) {
                N.fetch_add(1, Ordering::Relaxed);
            }
        }
        N.store(0, Ordering::Relaxed);
        {
            let mut v: InlineVec<D, 2> = InlineVec::new();
            for _ in 0..8 {
                v.push(D);
            }
        }
        assert_eq!(N.load(Ordering::Relaxed), 8);
    }
}
