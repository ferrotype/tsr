//! Declaration transformation and the diagnostics produced while serializing
//! public types (`transformers/declarations`). The transform returns the
//! printable declaration file and the diagnostics the pin's
//! `GetDiagnostics` reports.
mod class_assignments;
mod classes;
mod common_js;
mod diagnostics;
mod expando;
mod exports;
mod host;
mod members;
mod nodes;
mod reports;
mod statements;
mod supplemental;
mod tracker;
mod transform;
mod types;
mod util;
mod visitor;

pub use host::{DeclarationEmitHost, OutputPaths};
pub use supplemental::SupplementalReferencesTransformer;
pub use transform::{transform_declarations, DeclarationOptions, DeclarationTransform};
