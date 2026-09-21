//! Shared scanner configuration and signed source ranges.
//!
//! This dependency slice does not implement compiler options or the remaining
//! core algorithms. Integer newtypes retain Go's open numeric value domain.

pub mod collections;
pub mod path;
pub mod pattern;

/// Pinned `core.ScriptKind`; reserved and unknown integers remain representable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScriptKind(pub i32);

impl ScriptKind {
    pub const UNKNOWN: Self = Self(0);
    pub const JS: Self = Self(1);
    pub const JSX: Self = Self(2);
    pub const TS: Self = Self(3);
    pub const TSX: Self = Self(4);
    pub const JSON: Self = Self(6);

    /// port: tsc/internal/core/core.go:GetScriptKindFromFileName
    pub fn from_file_name(file_name: &[u8]) -> Self {
        let Some(dot) = file_name.iter().rposition(|&byte| byte == b'.') else {
            return Self::UNKNOWN;
        };
        // Every recognized suffix is ASCII. Unicode lowercase cannot make a
        // non-ASCII code point equal to any of these ASCII suffix characters.
        let extension = &file_name[dot..];
        for (suffix, kind) in [
            (b".js".as_slice(), Self::JS),
            (b".cjs", Self::JS),
            (b".mjs", Self::JS),
            (b".jsx", Self::JSX),
            (b".ts", Self::TS),
            (b".cts", Self::TS),
            (b".mts", Self::TS),
            (b".tsx", Self::TSX),
            (b".json", Self::JSON),
        ] {
            if extension.eq_ignore_ascii_case(suffix) {
                return kind;
            }
        }
        Self::UNKNOWN
    }

    /// port: tsc/internal/core/core.go:EnsureScriptKindFromFileName
    pub fn ensure_from_file_name(file_name: &[u8]) -> Self {
        let kind = Self::from_file_name(file_name);
        if kind == Self::UNKNOWN {
            Self::TS
        } else {
            kind
        }
    }

    /// port: tsc/internal/core/core.go:GetDefaultExtensionForScriptKind
    pub fn default_extension(self) -> &'static [u8] {
        match self {
            Self::JS => b".js",
            Self::JSX => b".jsx",
            Self::TSX => b".tsx",
            Self::JSON => b".json",
            _ => b".ts",
        }
    }
}

/// Pinned byte-valued `core.Tristate`, including unknown external values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tristate(pub u8);

impl Tristate {
    /// Exact bytes, without JSON tokenization or whitespace trimming.
    /// port: tsc/internal/core/tristate.go:Tristate.UnmarshalJSON
    pub fn unmarshal_json(bytes: &[u8]) -> Self {
        match bytes {
            b"true" => Self::TRUE,
            b"false" => Self::FALSE,
            _ => Self::UNKNOWN,
        }
    }
    /// port: tsc/internal/core/tristate.go:Tristate.MarshalJSON
    pub fn marshal_json(self) -> &'static [u8] {
        match self {
            Self::TRUE => b"true",
            Self::FALSE => b"false",
            _ => b"null",
        }
    }
    pub const UNKNOWN: Self = Self(0);
    pub const FALSE: Self = Self(1);
    pub const TRUE: Self = Self(2);
    pub fn is_true(self) -> bool {
        self == Self::TRUE
    }
    pub fn is_false(self) -> bool {
        self == Self::FALSE
    }
    pub fn is_unknown(self) -> bool {
        self == Self::UNKNOWN
    }
    pub fn is_true_or_unknown(self) -> bool {
        self.is_true() || self.is_unknown()
    }
    pub fn is_false_or_unknown(self) -> bool {
        self.is_false() || self.is_unknown()
    }
    #[must_use]
    pub fn default_if_unknown(self, value: Self) -> Self {
        if self.is_unknown() {
            value
        } else {
            self
        }
    }
}

impl From<bool> for Tristate {
    fn from(value: bool) -> Self {
        if value {
            Self::TRUE
        } else {
            Self::FALSE
        }
    }
}

/// Pinned `core.ScriptTarget` values, including None and JSON.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptTarget(pub i32);

impl ScriptTarget {
    pub const NONE: Self = Self(0);
    pub const ES5: Self = Self(1);
    pub const ES2015: Self = Self(2);
    pub const ES2016: Self = Self(3);
    pub const ES2017: Self = Self(4);
    pub const ES2018: Self = Self(5);
    pub const ES2019: Self = Self(6);
    pub const ES2020: Self = Self(7);
    pub const ES2021: Self = Self(8);
    pub const ES2022: Self = Self(9);
    pub const ES2023: Self = Self(10);
    pub const ES2024: Self = Self(11);
    pub const ES2025: Self = Self(12);
    pub const ESNEXT: Self = Self(99);
    pub const JSON: Self = Self(100);
    pub const LATEST: Self = Self::ESNEXT;
    pub const LATEST_STANDARD: Self = Self::ES2025;
}

/// Pinned `core.LanguageVariant`; unknown integers remain representable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LanguageVariant(pub i32);

impl LanguageVariant {
    pub const STANDARD: Self = Self(0);
    pub const JSX: Self = Self(1);
}

/// Go stores each position as int32, although its accessors use machine ints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRange {
    pos: i32,
    end: i32,
}

impl TextRange {
    /// Narrows deliberately, including synthetic negative positions.
    /// port: tsc/internal/core/text.go:NewTextRange
    pub const fn new(pos: i64, end: i64) -> Self {
        Self {
            pos: pos as i32,
            end: end as i32,
        }
    }

    /// port: tsc/internal/core/text.go:TextRange.Pos
    pub fn pos(self) -> i64 {
        i64::from(self.pos)
    }

    /// port: tsc/internal/core/text.go:TextRange.End
    pub fn end(self) -> i64 {
        i64::from(self.end)
    }

    /// The subtraction wraps at int32 before Go widens it to int.
    /// port: tsc/internal/core/text.go:TextRange.Len
    pub fn len(self) -> i64 {
        i64::from(self.end.wrapping_sub(self.pos))
    }

    pub fn is_empty(self) -> bool {
        self.pos == self.end
    }
}

pub mod compiler_options;
pub use compiler_options::*;

mod go_sort;
mod text_change;
pub use go_sort::sort as sort_like_go;
pub use text_change::{apply_bulk_edits, TextChange, UnappliableEdits};

pub mod helpers;
mod names;
mod ranges;
pub mod slices;

pub mod shared;
