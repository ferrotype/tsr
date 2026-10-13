//! Owned compiler observations only: no checker, AST or baseline-envelope data.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub line: usize,
    pub source_text: Vec<u8>,
    pub display: Display,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Display {
    Type {
        text: Vec<u8>,
    },
    Symbol {
        name: Vec<u8>,
        declarations: Vec<SymbolDeclaration>,
        declaration_count: usize,
    },
}

/// Raw declaration location, before host basename/library-position decoration.
/// Lines and UTF-16 characters are zero based, matching the pinned walker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolDeclaration {
    pub file: Vec<u8>,
    pub line: usize,
    pub character: isize,
}
