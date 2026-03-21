#[cfg(feature = "drop-tracking")]
use std::ptr;

#[cfg(feature = "drop-tracking")]
use super::inline_vec::InlineVec;

#[cfg(feature = "drop-tracking")]
const DROP_INLINE_CAP: usize = 32;

#[cfg(feature = "drop-tracking")]
type DropEntry = (*mut u8, unsafe fn(*mut u8));

/// Tracks allocations with non-trivial destructors so that [`Arena::reset`]
/// and [`Arena::rewind`] can run them in LIFO order before reclaiming memory.
///
/// Zero-sized and fully eliminated by the compiler when the `drop-tracking`
/// feature is disabled — every method becomes an unconditional no-op.
#[cfg(feature = "drop-tracking")]
pub(crate) struct DropRegistry {
    entries: InlineVec<DropEntry, DROP_INLINE_CAP>,
}

#[cfg_attr(not(feature = "drop-tracking"), allow(dead_code))]
#[cfg(feature = "drop-tracking")]
impl DropRegistry {
    #[inline]
    pub(crate) fn new() -> Self {
        DropRegistry {
            entries: InlineVec::new(),
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

    /// Run and remove all entries with index `>= target_len`, in LIFO order.
    /// Called by `rewind` to drop only objects allocated after a checkpoint.
    #[allow(dead_code)]
    pub(crate) fn run_drops_until(&mut self, target_len: usize) {
        while self.entries.len() > target_len {
            let (p, shim) = self.entries.pop().unwrap();
            unsafe { shim(p) };
        }
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn run_all_drops(&mut self) {
        self.run_drops_until(0);
    }
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
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
    pub(crate) fn run_drops_until(&mut self, _: usize) {}
    #[inline(always)]
    pub(crate) fn run_all_drops(&mut self) {}
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        0
    }
}
