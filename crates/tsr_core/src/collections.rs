//! Shared collection contracts used by compiler consumers.
//!
//! Each type preserves the pinned operation's order, sharing and scope rules;
//! ordinary Rust collections remain appropriate where those rules coincide.

mod cow;
mod ordered_map;
mod ordered_set;

pub use cow::{CopyOnWriteMap, CopyOnWriteMapScope, CopyOnWriteSet, CopyOnWriteSetScope};
pub use ordered_map::{MapChange, OrderedMap};
pub use ordered_set::OrderedSet;
