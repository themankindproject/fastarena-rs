//! Supporting utilities for the arena allocator.
//!
//! This module contains:
//! - `InlineVec<T, N>`: inline-first vector with optional heap spill
//! - `DropRegistry`: tracks destructors for `drop-tracking` feature
//! - `Transaction`: RAII scope for transactional allocation with rollback

pub mod drop_registry;
pub mod inline_vec;
pub mod transaction;

pub use transaction::{Transaction, TxnDiff, TxnStatus};
