//! Snapshot-owned auto-import data. The index never retains a checker handle.
mod cache;
pub use cache::Cache;
pub mod edits;
pub mod fix;
pub mod index;
pub mod packages;
mod registry;
pub mod specifiers;
mod unicode;
pub use registry::{Export, ExportId, ExportSyntax, Registry};
