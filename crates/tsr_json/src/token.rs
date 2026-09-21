//! JSON kinds and exact scalar tokens. Numbers need not fit binary64.
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Invalid,
    Null,
    False,
    True,
    String,
    Number,
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}
impl Kind {
    pub const fn from_byte(b: u8) -> Self {
        match b {
            b'n' => Self::Null,
            b'f' => Self::False,
            b't' => Self::True,
            b'"' => Self::String,
            b'-' | b'0'..=b'9' => Self::Number,
            b'{' => Self::BeginObject,
            b'}' => Self::EndObject,
            b'[' => Self::BeginArray,
            b']' => Self::EndArray,
            _ => Self::Invalid,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Null => "null",
            Self::False => "false",
            Self::True => "true",
            Self::String => "string",
            Self::Number => "number",
            Self::BeginObject => "{",
            Self::EndObject => "}",
            Self::BeginArray => "[",
            Self::EndArray => "]",
        }
    }
    pub(crate) const fn semantic_name(self) -> Option<&'static str> {
        match self {
            Self::Invalid => None,
            Self::False | Self::True => Some("boolean"),
            Self::BeginObject | Self::EndObject => Some("object"),
            Self::BeginArray | Self::EndArray => Some("array"),
            _ => Some(self.name()),
        }
    }
    pub(crate) const fn is_end(self) -> bool {
        matches!(self, Self::EndObject | Self::EndArray)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    Null,
    Boolean(bool),
    String(Cow<'a, [u8]>),
    Number(Cow<'a, [u8]>),
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}
impl Token<'_> {
    pub fn int(value: i64) -> Self {
        Self::Number(Cow::Owned(value.to_string().into_bytes()))
    }
    pub fn uint(value: u64) -> Self {
        Self::Number(Cow::Owned(value.to_string().into_bytes()))
    }
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Null => Kind::Null,
            Self::Boolean(false) => Kind::False,
            Self::Boolean(true) => Kind::True,
            Self::String(_) => Kind::String,
            Self::Number(_) => Kind::Number,
            Self::BeginObject => Kind::BeginObject,
            Self::EndObject => Kind::EndObject,
            Self::BeginArray => Kind::BeginArray,
            Self::EndArray => Kind::EndArray,
        }
    }
    pub fn text(&self) -> &[u8] {
        match self {
            Self::Null => b"null",
            Self::Boolean(false) => b"false",
            Self::Boolean(true) => b"true",
            Self::String(s) | Self::Number(s) => s,
            Self::BeginObject => b"{",
            Self::EndObject => b"}",
            Self::BeginArray => b"[",
            Self::EndArray => b"]",
        }
    }
    pub fn into_owned(self) -> Token<'static> {
        match self {
            Self::String(s) => Token::String(Cow::Owned(s.into_owned())),
            Self::Number(s) => Token::Number(Cow::Owned(s.into_owned())),
            Self::Null => Token::Null,
            Self::Boolean(b) => Token::Boolean(b),
            Self::BeginObject => Token::BeginObject,
            Self::EndObject => Token::EndObject,
            Self::BeginArray => Token::BeginArray,
            Self::EndArray => Token::EndArray,
        }
    }
}
