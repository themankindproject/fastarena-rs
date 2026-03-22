use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;

pub(crate) struct Block {
    ptr: NonNull<u8>,
    pub(crate) base: usize,
    pub(crate) capacity: usize,
    pub(crate) offset: usize,
}

impl Block {
    pub(crate) fn new(capacity: usize) -> Self {
        Self::try_new(capacity).expect("arena: out of memory")
    }

    pub(crate) fn try_new(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }
        let layout = Layout::from_size_align(capacity, 8).ok()?;
        let ptr = NonNull::new(unsafe { alloc(layout) })?;
        let base = ptr.as_ptr() as usize;
        Some(Block {
            ptr,
            base,
            capacity,
            offset: 0,
        })
    }

    #[inline]
    pub(crate) fn try_alloc(&mut self, size: usize, align: usize) -> Option<(NonNull<u8>, usize)> {
        let aligned = align_up(self.base + self.offset, align);
        let new_offset = (aligned - self.base) + size;
        if new_offset > self.capacity {
            return None;
        }
        let delta = new_offset - self.offset;
        self.offset = new_offset;
        Some((unsafe { NonNull::new_unchecked(aligned as *mut u8) }, delta))
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn remaining(&self) -> usize {
        self.capacity - self.offset
    }
    #[inline]
    pub(crate) fn reset(&mut self) {
        self.offset = 0;
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity, 8).unwrap();
        unsafe { dealloc(self.ptr.as_ptr(), layout) };
    }
}

#[inline(always)]
pub(crate) fn align_up(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (addr + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_zero() {
        assert!(Block::try_new(0).is_none());
    }

    #[test]
    fn try_alloc_ok() {
        let mut b = Block::new(256);
        let (ptr, delta) = b.try_alloc(8, 8).unwrap();
        assert_eq!(ptr.as_ptr() as usize % 8, 0);
        assert_eq!(delta, 8);
    }

    #[test]
    fn alloc_padding_in_delta() {
        let mut b = Block::new(256);
        b.try_alloc(1, 1).unwrap();
        let (_, d) = b.try_alloc(8, 8).unwrap();
        assert_eq!(d, 15);
    }

    #[test]
    fn alloc_none_when_full() {
        let mut b = Block::new(16);
        b.try_alloc(16, 1).unwrap();
        assert!(b.try_alloc(1, 1).is_none());
    }

    #[test]
    fn reset_reuse() {
        let mut b = Block::new(64);
        let (p1, _) = b.try_alloc(32, 8).unwrap();
        b.reset();
        let (p2, _) = b.try_alloc(32, 8).unwrap();
        assert_eq!(p1.as_ptr(), p2.as_ptr());
    }

    #[test]
    fn align_up_cases() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(65, 64), 128);
    }
}
