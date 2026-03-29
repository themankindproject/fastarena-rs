use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::ptr::NonNull;

use super::allocator::Arena;

unsafe impl<T> Send for ArenaBox<'_, T> {}
unsafe impl<T> Sync for ArenaBox<'_, T> {}

/// An owned allocation from an arena, similar to `Box<T>`.
///
/// Unlike `Box<T>` which allocates on the heap, `ArenaBox<T>` stores its data
/// within arena memory. This provides the same API as `Box<T>` while maintaining
/// the performance benefits of bump-pointer allocation.
///
/// The lifetime `'arena` is only used to ensure the arena is not reset or rewound
/// while any `ArenaBox` pointing to its memory is still in use. However, note that
/// **this is not enforced at compile time** — misuse can cause undefined behavior.
///
/// **Note:** `ArenaBox` does not run destructors when dropped. If you need
/// destructors to run, enable the `drop-tracking` feature and use `Arena::reset()`
/// or `Arena::rewind()` which will run all registered destructors in LIFO order.
///
/// # Example
///
/// ```rust
/// use fastarena::{Arena, ArenaBox};
///
/// let mut arena = Arena::new();
///
/// let x = arena.alloc_box(42);
///
/// // ArenaBox derefs to &T and &mut T
/// assert_eq!(*x, 42);
///
/// // Can modify through mutable deref
/// let mut x = x;
/// *x = 200;
/// assert_eq!(*x, 200);
/// ```
#[repr(transparent)]
pub struct ArenaBox<'arena, T> {
    ptr: NonNull<T>,
    _marker: PhantomData<&'arena Arena>,
}

impl<'arena, T> ArenaBox<'arena, T> {
    #[inline]
    pub(crate) fn new(arena: &'arena mut Arena, val: T) -> Self {
        let ptr = arena.alloc(val) as *mut T;
        ArenaBox {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        let this = ManuallyDrop::new(self);
        unsafe { core::ptr::read(this.ptr.as_ptr()) }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }
}

impl<'arena, T> Deref for ArenaBox<'arena, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<'arena, T> core::ops::DerefMut for ArenaBox<'arena, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for ArenaBox<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for ArenaBox<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> core::fmt::Pointer for ArenaBox<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
    }
}
