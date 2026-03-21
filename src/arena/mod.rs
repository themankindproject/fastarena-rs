//! Core arena allocator implementation.
//!
//! This module provides the main `Arena` type along with supporting types
//! (`Block`, `Checkpoint`, `ArenaStats`) for bump-pointer allocation.

mod allocator;
mod block;
mod stats;

pub use allocator::{Arena, Checkpoint, CACHE_LINE_SIZE};
pub use stats::ArenaStats;
