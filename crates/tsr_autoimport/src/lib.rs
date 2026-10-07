//! Snapshot-owned auto-import data. The index never retains a checker handle.
mod cache;
mod dependencies;
pub use cache::Cache;
pub use dependencies::{Dependencies, DependencyTracker};
pub mod edits;
mod import_adder;
pub use import_adder::ImportAdder;
pub mod fix;
pub mod index;
mod package_names;
pub mod packages;
pub mod ranking;
mod realpaths;
mod registry;
pub mod specifiers;
pub mod type_nodes;
mod unicode;
pub use registry::{
    export_id_for_symbol, lookup_export, module_augmentations, symbol_to_export, Export, ExportId,
    ExportSyntax, Registry,
};

pub mod preferences;
pub use preferences::Preferences;

mod regexp;

#[cfg(test)]
mod tests;
