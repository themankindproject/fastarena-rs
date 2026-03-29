//! Core arena allocator implementation.
//!
//! This module provides the main `Arena` type along with supporting types
//! (`Block`, `Checkpoint`, `ArenaStats`) for bump-pointer allocation.

pub(crate) mod allocator;
mod block;
mod boxed;
mod stats;

pub use allocator::{Arena, Checkpoint};
pub use boxed::ArenaBox;
pub use stats::ArenaStats;
