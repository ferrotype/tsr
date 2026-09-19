//! Generated diagnostic identities and default messages from the pinned Go compiler.
//!
//! This crate supplies message data. Formatting, locale negotiation and diagnostic
//! production belong to later compiler slices.

mod generated;

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
