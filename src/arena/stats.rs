/// A snapshot of arena memory usage returned by [`super::allocator::Arena::stats`].
///
/// All counters are maintained incrementally; `stats()` is O(1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[must_use = "arena stats provide memory usage information"]
pub struct ArenaStats {
    /// Bytes committed to live allocations, including alignment padding.
    /// Restored by [`super::allocator::Arena::rewind`] and zeroed by [`super::allocator::Arena::reset`].
    pub bytes_allocated: usize,

    /// Total bytes reserved across all owned blocks. Only grows — blocks are
    /// retained across rewinds and resets. Always `>= bytes_allocated`.
    pub bytes_reserved: usize,

    /// Number of blocks owned by the arena, including idle ones retained for
    /// reuse after a rewind or reset.
    pub block_count: usize,
}

impl ArenaStats {
    /// Fraction of reserved memory that is actively in use, in `[0.0, 1.0]`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn utilization(&self) -> f64 {
        if self.bytes_reserved == 0 {
            0.0
        } else {
            self.bytes_allocated as f64 / self.bytes_reserved as f64
        }
    }

    /// Bytes reserved but not currently allocated.
    #[must_use]
    pub fn bytes_idle(&self) -> usize {
        self.bytes_reserved.saturating_sub(self.bytes_allocated)
    }
}

impl std::fmt::Display for ArenaStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} allocated / {} reserved ({} blocks, {:.1}% util)",
            self.bytes_allocated,
            self.bytes_reserved,
            self.block_count,
            self.utilization() * 100.0
        )
    }
}
