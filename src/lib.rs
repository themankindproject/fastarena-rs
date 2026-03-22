//! A zero-dependency bump-pointer arena allocator with RAII transactions,
//! nested savepoints, and `ArenaVec`.
//!
//! # Features
//!
//! - **O(1) amortised allocation** — bump pointer advance with a single bounds check
//! - **Checkpoint / rewind** — snapshot arena state in O(1), undo allocations
//! - **RAII transactions** — auto-rollback on drop, explicit commit via `mem::forget`
//! - **Transaction budget** — cap bytes consumed per transaction
//! - **Zero-cost reset** — reuse all allocated blocks without OS calls
//! - **`ArenaVec<T>`** — append-only growable vector backed by arena memory
//!
//! # Quick Start
//!
//! ```
//! use fastarena::Arena;
//!
//! let mut arena = Arena::new();
//! let x = arena.alloc(42u64);
//! let s = arena.alloc_str("hello");
//! arena.reset(); // zero-cost reset
//! ```
//!
//! # Feature Flags
//!
//! | Flag | Default | Description |
//! |------|---------|-------------|
//! | `drop-tracking` | Off | Run destructors in LIFO order on `reset`/`rewind` |

mod arena;
mod util;
mod vec;

pub use arena::ArenaStats;
pub use arena::{Arena, Checkpoint, CACHE_LINE_SIZE};
pub use util::{Transaction, TxnDiff, TxnStatus};
pub use vec::{ArenaVec, TryReserveError};
