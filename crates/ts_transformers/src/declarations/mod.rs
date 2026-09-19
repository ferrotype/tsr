//! Declaration transformation and the diagnostics produced while serializing
//! public types. Unsupported transformations are errors, never empty output.
mod class_assignments;
mod classes;
mod common_js;
mod diagnostics;
mod expando;
mod exports;
mod nodes;
mod reports;
mod statements;
mod tracker;
mod transform;
mod util;
mod visitor;

pub use transform::{transform_declarations, DeclarationOptions, DeclarationTransform};
