//! The pinned API protocol (`tsc/internal/api/proto.go`): wire methods, handle
//! scalars and DTOs generated from the `gen-proto` export by `cargo xtask gen
//! api`, plus the handwritten codecs of its special mappings. Encoding follows
//! json v2 as the pin's `internal/json` configures it: `omitempty` leaves out
//! empty JSON values, `omitzero` leaves out Go zero values, nil slices encode
//! as `[]`, unknown names are skipped on decode and a null decodes a struct
//! to its zero value.
pub mod codecs;
#[allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    reason = "generated from the pinned export"
)]
mod generated;

pub use codecs::{
    raw_is_empty, CompilerOptionsValue, DocumentIdentifier, ImportAdderActionKind, JsonMap,
    PackageJsonValue, ProjectReferenceValue, RawBinary, StringListMap, TypeAcquisitionValue,
};
pub use generated::*;

#[cfg(test)]
mod tests;
