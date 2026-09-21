//! Generated diagnostic identities and default messages from the pinned Go compiler.
//!
//! Message data, argument formatting and pinned locale negotiation are shared
//! by compiler diagnostics and test drivers.

mod format;
mod generated;
mod locales_generated;
mod localize;
pub use format::{format, stringify_args, try_format, valid_argument, Argument};
pub use localize::{localize, localized_messages, AdHocMessage, TranslationTable};

pub use generated::*;

/// The numeric discriminants are part of the diagnostic wire contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Category {
    Warning = 0,
    Error = 1,
    Suggestion = 2,
    Message = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub code: i32,
    pub category: Category,
    pub key: &'static str,
    pub text: &'static str,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub elided_in_compatibility_pyramid: bool,
}

/// Find a pinned message by diagnostic code without allocating a lookup table.
pub fn by_code(code: i32) -> Option<&'static Message> {
    MESSAGES
        .binary_search_by_key(&code, |message| message.code)
        .ok()
        .map(|index| &MESSAGES[index])
}

/// Find a pinned localization key. Unknown keys stay distinguishable from English text.
pub fn by_key(key: &str) -> Option<&'static Message> {
    KEY_INDEX
        .binary_search_by(|&(candidate, _)| candidate.cmp(key))
        .ok()
        .map(|index| &MESSAGES[KEY_INDEX[index].1])
}

/// Byte keys preserve the repository text boundary. Unknown non-UTF8 keys do
/// not become a lossy lookup of some other generated key.
pub fn by_key_bytes(key: &[u8]) -> Option<&'static Message> {
    std::str::from_utf8(key).ok().and_then(by_key)
}
impl Category {
    /// port: tsc/internal/diagnostics/diagnostics.go:Category.Name
    pub fn name(self) -> &'static str {
        Self::name_raw(self as i32)
    }
    pub fn name_raw(value: i32) -> &'static str {
        match value {
            0 => "warning",
            1 => "error",
            2 => "suggestion",
            3 => "message",
            _ => panic!("Unhandled diagnostic category"),
        }
    }
    /// Source operation: tsc/internal/diagnostics/stringer_generated.go:Category.String
    /// port: tsc/internal/diagnostics/stringer_generated.go
    pub fn string_raw(value: i32) -> String {
        match value {
            0 => "CategoryWarning".into(),
            1 => "CategoryError".into(),
            2 => "CategorySuggestion".into(),
            3 => "CategoryMessage".into(),
            _ => format!("Category({value})"),
        }
    }
}
impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::string_raw(*self as i32))
    }
}
