//! Byte-preserving text and the distinct API, LSP and scanner position contracts.
//! See `SLICE.md` for supported helpers and deferred compiler integration.

pub mod escape;
pub mod helpers;
pub mod jsstring;
pub mod line_map;
pub mod lsp;
pub mod position_map;
pub mod scanner_positions;
pub mod source_text;
pub mod wtf8;

mod case_tables;
mod go_fold_generated;
mod go_print_generated;
mod go_quote;
mod simple_fold;
pub use simple_fold::equal_fold;

pub use escape::{LiteralEscapeFlags, QuoteChar};
pub use go_quote::go_quote;
pub use jsstring::{JsString, Validity};
pub use line_map::LspLineMap;
pub use lsp::{LspPosition, PositionEncoding};
pub use position_map::PositionMap;
pub use source_text::SourceText;
