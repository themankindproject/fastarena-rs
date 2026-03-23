//! Arena-backed vector types.
//!
//! Provides `ArenaVec<T>`, an append-only growable vector that allocates
//! memory from an arena instead of the heap.

pub use arena_vec::{ArenaVec, ArenaVecIntoIter, TryReserveError};

mod arena_vec;
