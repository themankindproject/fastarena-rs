#[cfg(feature = "drop-tracking")]
use std::panic::{catch_unwind, AssertUnwindSafe};
#[cfg(feature = "drop-tracking")]
use std::ptr;

#[cfg(feature = "drop-tracking")]
use super::inline_vec::InlineVec;

#[cfg(feature = "drop-tracking")]
const DROP_INLINE_CAP: usize = 32;

#[cfg(feature = "drop-tracking")]
type DropEntry = (*mut u8, unsafe fn(*mut u8));

#[cfg(feature = "drop-tracking")]
type DropRangeEntry = (*mut u8, usize, unsafe fn(*mut u8, usize));

/// Tracks allocations with non-trivial destructors so that [`Arena::reset`]
/// and [`Arena::rewind`] can run them in LIFO order before reclaiming memory.
///
/// Zero-sized and fully eliminated by the compiler when the `drop-tracking`
/// feature is disabled — every method becomes an unconditional no-op.
#[cfg(feature = "drop-tracking")]
pub(crate) struct DropRegistry {
    entries: InlineVec<DropEntry, DROP_INLINE_CAP>,
    range_entries: InlineVec<DropRangeEntry, DROP_INLINE_CAP>,
}

#[cfg_attr(not(feature = "drop-tracking"), allow(dead_code))]
#[cfg(feature = "drop-tracking")]
impl DropRegistry {
    #[inline]
    pub(crate) fn new() -> Self {
        DropRegistry {
            entries: InlineVec::new(),
            range_entries: InlineVec::new(),
        }
    }

    /// Register `ptr` for destruction if `T: Drop`. No-op for `Copy` types.
    #[inline]
    pub(crate) fn register<T>(&mut self, ptr: *mut T) {
        if std::mem::needs_drop::<T>() {
            unsafe fn drop_shim<T>(p: *mut u8) {
                ptr::drop_in_place(p as *mut T);
            }
            self.entries.push((ptr as *mut u8, drop_shim::<T>));
        }
    }

    /// Register a slice of `count` elements starting at `ptr` for destruction
    /// if `T: Drop`. O(1) registration instead of O(n) individual calls.
    #[inline]
    pub(crate) fn register_slice<T>(&mut self, ptr: *mut T, count: usize) {
        if count == 0 {
            return;
        }
        if std::mem::needs_drop::<T>() {
            unsafe fn drop_range_shim<T>(p: *mut u8, count: usize) {
                let ptr = p as *mut T;
                for i in 0..count {
                    ptr::drop_in_place(ptr.add(i));
                }
            }
            self.range_entries
                .push((ptr as *mut u8, count, drop_range_shim::<T>));
        }
    }

    /// Run and remove all entries with index `>= target_len`, in LIFO order.
    /// Called by `rewind` to drop only objects allocated after a checkpoint.
    #[allow(dead_code)]
    pub(crate) fn run_drops_until(&mut self, target_len: usize) {
        while self.range_entries.len() > target_len {
            let (p, count, shim) = self.range_entries.pop().unwrap();
            let result = catch_unwind(AssertUnwindSafe(|| unsafe { shim(p, count) }));
            if result.is_err() {
                while self.range_entries.len() > target_len {
                    let (p, count, shim) = self.range_entries.pop().unwrap();
                    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { shim(p, count) }));
                }
                std::panic::panic_any("DropRegistry: destructor panicked");
            }
        }
        while self.entries.len() > target_len {
            let (p, shim) = self.entries.pop().unwrap();
            let result = catch_unwind(AssertUnwindSafe(|| unsafe { shim(p) }));
            if result.is_err() {
                while self.entries.len() > target_len {
                    let (p, shim) = self.entries.pop().unwrap();
                    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { shim(p) }));
                }
                std::panic::panic_any("DropRegistry: destructor panicked");
            }
        }
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn run_all_drops(&mut self) {
        self.run_drops_until(0);
    }
    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.range_entries.len() + self.entries.len()
    }
}

#[cfg(not(feature = "drop-tracking"))]
pub(crate) struct DropRegistry;

#[cfg(not(feature = "drop-tracking"))]
#[allow(dead_code)]
impl DropRegistry {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        DropRegistry
    }
    #[inline(always)]
    pub(crate) fn register<T>(&mut self, _: *mut T) {}
    #[inline(always)]
    pub(crate) fn register_slice<T>(&mut self, _: *mut T, _: usize) {}
    #[inline(always)]
    pub(crate) fn run_drops_until(&mut self, _: usize) {}
    #[inline(always)]
    pub(crate) fn run_all_drops(&mut self) {}
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        0
    }
}
