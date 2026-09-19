//! The pinned TypeScript parser over owner-backed AST storage.
//!
//! Scanner input borrows a source owner outside the parser. Speculation restores
//! exactly the source checkpoint fields; allocated nodes and counters survive.

mod access;
mod ambiguity;
mod bindings;
mod classes;
mod declarations;
mod diagnostics;
mod expressions;
mod factory;
mod identifiers;
mod imports;
mod js_syntax;
mod jsdoc;
mod json;
mod jsx;
mod lazy_jsdoc;
mod list_buffer;
mod lists;
mod modifiers;
mod orchestration;
mod pragmas;
mod predicates;
mod primary;
mod recursion;
mod references;
mod reparser;
mod signatures;
mod source_text;
mod state;
mod statements;
mod tokens;
mod types;
mod worker;

pub(crate) use factory::ParserFactory;
pub use lazy_jsdoc::ParserJsDocProvider;
pub use orchestration::{
    parse_isolated_entity_name, parse_source_file, parse_source_file_with_counters,
};
pub(crate) use state::{JSDocInfo, Parser, ParsingContext};
pub use worker::on_parser_worker;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use worker::spawn_parser_worker;

pub mod parse_flags {
    pub const NONE: u32 = 0;
    pub const YIELD: u32 = 1 << 0;
    pub const AWAIT: u32 = 1 << 1;
    pub const TYPE: u32 = 1 << 2;
    pub const IGNORE_MISSING_OPEN_BRACE: u32 = 1 << 4;
    pub const JS_DOC: u32 = 1 << 5;
}

pub(crate) const STACK_RED_ZONE: usize = 128 * 1024;
pub(crate) const STACK_SEGMENT: usize = 2 * 1024 * 1024;

#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod state_tests;

#[cfg(test)]
mod lazy_jsdoc_tests;

#[cfg(test)]
mod recursion_tests;
