//! Snapshot-owned auto-import data. The index never retains a checker handle.
mod cache;
pub use cache::Cache;
pub mod edits;
mod import_adder;
pub use import_adder::ImportAdder;
pub mod fix;
pub mod index;
pub mod packages;
mod registry;
pub mod specifiers;
pub mod type_nodes;
mod unicode;
pub use registry::{export_id_for_symbol, Export, ExportId, ExportSyntax, Registry};

pub mod preferences;
pub use preferences::Preferences;

mod regexp;
mod regexp_unicode_generated;

#[cfg(test)]
mod tests;
