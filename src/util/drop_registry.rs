#[cfg(feature = "drop-tracking")]
use std::panic::{catch_unwind, AssertUnwindSafe};
#[cfg(feature = "drop-tracking")]
use std::ptr;

#[cfg(feature = "drop-tracking")]
use super::inline_vec::InlineVec;

#[cfg(feature = "drop-tracking")]
const DROP_INLINE_CAP: usize = 32;

/// A single drop entry — either a single element or a contiguous slice.
#[cfg(feature = "drop-tracking")]
enum DropSlot {
    /// One element at `ptr` with its type-erased drop shim.
    Single {
        ptr: *mut u8,
        shim: unsafe fn(*mut u8),
    },
    /// `count` contiguous elements at `ptr` with a type-erased slice-drop shim.
    Range {
        ptr: *mut u8,
        count: usize,
        shim: unsafe fn(*mut u8, usize),
    },
}

/// Tracks allocations with non-trivial destructors so that [`Arena::reset`]
/// and [`Arena::rewind`] can run them in LIFO order before reclaiming memory.
///
/// Zero-sized and fully eliminated by the compiler when the `drop-tracking`
/// feature is disabled — every method becomes an unconditional no-op.
#[cfg(feature = "drop-tracking")]
pub(crate) struct DropRegistry {
    slots: InlineVec<DropSlot, DROP_INLINE_CAP>,
}

#[cfg_attr(not(feature = "drop-tracking"), allow(dead_code))]
#[cfg(feature = "drop-tracking")]
impl DropRegistry {
    #[inline]
    pub(crate) fn new() -> Self {
        DropRegistry {
            slots: InlineVec::new(),
        }
    }

    /// Register `ptr` for destruction if `T: Drop`. No-op for `Copy` types.
    #[inline]
    pub(crate) fn register<T>(&mut self, ptr: *mut T) {
        if std::mem::needs_drop::<T>() {
            unsafe fn drop_shim<T>(p: *mut u8) {
                ptr::drop_in_place(p as *mut T);
            }
            self.slots.push(DropSlot::Single {
                ptr: ptr as *mut u8,
                shim: drop_shim::<T>,
            });
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
            self.slots.push(DropSlot::Range {
                ptr: ptr as *mut u8,
                count,
                shim: drop_range_shim::<T>,
            });
        }
    }

    /// Run and remove all entries with index `>= target_len`, in LIFO order.
    /// Called by `rewind` to drop only objects allocated after a checkpoint.
    #[allow(dead_code)]
    pub(crate) fn run_drops_until(&mut self, target_len: usize) {
        while self.slots.len() > target_len {
            let slot = self.slots.pop().unwrap();
            let result = Self::run_slot(slot);
            if result.is_err() {
                // Drain remaining entries, ignoring further panics, then re-raise.
                self.drain_remaining(target_len);
                std::panic::panic_any("DropRegistry: destructor panicked");
            }
        }
    }

    /// Run a single drop slot, catching any panic.
    fn run_slot(slot: DropSlot) -> Result<(), ()> {
        match slot {
            DropSlot::Single { ptr, shim } => {
                catch_unwind(AssertUnwindSafe(|| unsafe { shim(ptr) })).map_err(|_| ())
            }
            DropSlot::Range { ptr, count, shim } => {
                catch_unwind(AssertUnwindSafe(|| unsafe { shim(ptr, count) })).map_err(|_| ())
            }
        }
    }

    /// Drain all remaining slots, ignoring panics.
    fn drain_remaining(&mut self, target_len: usize) {
        while self.slots.len() > target_len {
            let slot = self.slots.pop().unwrap();
            let _ = Self::run_slot(slot);
        }
    }

    /// Run and remove all registered drops. Called by [`Arena::reset`].
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn run_all_drops(&mut self) {
        self.run_drops_until(0);
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

/// Zero-sized stub when `drop-tracking` is disabled — every method is a no-op.
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
