//! Error classes and locations are part of the codec contract, including when
//! the destination or output has already changed before a failure.
use crate::Kind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Syntax(Box<SyntaxError>),
    Semantic(Box<SemanticError>),
    Eof,
    Io {
        action: &'static str,
        message: String,
    },
    Message(String),
    InvalidIndent,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: usize,
    pub pointer: String,
    pub message: String,
    pub duplicate_name: bool,
    pub unexpected_eof: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticError {
    pub marshal: bool,
    pub offset: usize,
    pub pointer: String,
    pub kind: Kind,
    pub value: Option<Vec<u8>>,
    /// The source contract's type name; custom codecs supply their own name.
    pub type_name: &'static str,
    pub cause: Option<Error>,
}
impl Error {
    pub(crate) fn syntax(offset: usize, pointer: String, message: impl Into<String>) -> Self {
        Self::Syntax(Box::new(SyntaxError {
            offset,
            pointer,
            message: message.into(),
            duplicate_name: false,
            unexpected_eof: false,
        }))
    }
    pub(crate) fn truncated(offset: usize, pointer: String) -> Self {
        Self::Syntax(Box::new(SyntaxError {
            offset,
            pointer,
            message: "unexpected EOF".into(),
            duplicate_name: false,
            unexpected_eof: true,
        }))
    }
    pub(crate) fn duplicate(offset: usize, pointer: String) -> Self {
        Self::Syntax(Box::new(SyntaxError {
            offset,
            pointer,
            message: "duplicate object member name".into(),
            duplicate_name: true,
            unexpected_eof: false,
        }))
    }
    pub fn is_unexpected_eof(&self) -> bool {
        match self {
            Self::Syntax(e) => e.unexpected_eof,
            Self::Semantic(e) => e.cause.as_ref().is_some_and(Self::is_unexpected_eof),
            _ => false,
        }
    }
    pub fn is_duplicate_name(&self) -> bool {
        match self {
            Self::Syntax(e) => e.duplicate_name,
            Self::Semantic(e) => e.cause.as_ref().is_some_and(Self::is_duplicate_name),
            _ => false,
        }
    }
}
pub(crate) fn quote(s: &str) -> String {
    tsr_jsstring::go_quote(s.as_bytes())
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => f.write_str("EOF"),
            Self::Message(s) => f.write_str(s),
            Self::InvalidIndent => {
                f.write_str("jsontext: indentation must contain only spaces and tabs")
            }
            Self::Io { action, message } => write!(f, "jsontext: {action} error: {message}"),
            Self::Syntax(e) => {
                write!(f, "jsontext: {}", e.message)?;
                let mut pointer = e.pointer.as_str();
                if e.duplicate_name {
                    let (parent, name) = pointer.rsplit_once('/').unwrap_or(("", ""));
                    write!(f, " {}", quote(&name.replace("~1", "/").replace("~0", "~")))?;
                    pointer = parent;
                }
                if !pointer.is_empty() {
                    write!(f, " within {}", quote(&truncate_pointer(pointer)))?;
                }
                if e.offset != 0 && !e.duplicate_name {
                    write!(f, " after offset {}", e.offset)?;
                }
                Ok(())
            }
            Self::Semantic(e) => {
                write!(
                    f,
                    "json: cannot {}",
                    if e.marshal { "marshal" } else { "unmarshal" }
                )?;
                if let Some(kind) = e.kind.semantic_name() {
                    write!(f, " JSON {kind}")?;
                }
                if let Some(value) = &e.value {
                    if !value.is_empty() && value.len() < 100 {
                        write!(f, " {}", String::from_utf8_lossy(value))?;
                    }
                }
                write!(
                    f,
                    " {} Go {}",
                    if e.marshal { "from" } else { "into" },
                    e.type_name
                )?;
                let syntax = match &e.cause {
                    Some(Self::Syntax(s)) => Some(s),
                    _ => None,
                };
                if !e.pointer.is_empty() {
                    if syntax.is_none_or(|s| {
                        !(s.pointer == e.pointer
                            || s.pointer.starts_with(&(e.pointer.clone() + "/")))
                    }) {
                        write!(f, " within {}", quote(&truncate_pointer(&e.pointer)))?;
                    }
                } else if e.offset > 0 && syntax.is_none_or(|s| e.offset > s.offset) {
                    write!(f, " after offset {}", e.offset)?;
                }
                if let Some(cause) = &e.cause {
                    let text = cause.to_string();
                    write!(
                        f,
                        ": {}",
                        if syntax.is_some() {
                            text.strip_prefix("jsontext: ").unwrap_or(&text)
                        } else {
                            &text
                        }
                    )?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Semantic(e) => e.cause.as_ref().map(|e| e as &dyn std::error::Error),
            _ => None,
        }
    }
}

// Pinned jsonwire.TruncatePointer: keep whole names when possible and do not
// split a UTF-8 code point. The structured pointer remains untruncated.
fn truncate_pointer(text: &str) -> std::borrow::Cow<'_, str> {
    if text.len() <= 100 {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut left = 50;
    let mut right = text.len() - 50;
    if let Some(index) = bytes[..left]
        .iter()
        .rposition(|b| *b == b'/')
        .filter(|i| *i > 0)
    {
        left = index;
    }
    if let Some(index) = bytes[right..].iter().position(|b| *b == b'/') {
        right += index + 1;
    }
    while !text.is_char_boundary(left) {
        left -= 1;
    }
    while !text.is_char_boundary(right) {
        right += 1;
    }
    let cut = &text[left..right];
    let mut middle = match cut.bytes().filter(|b| *b == b'/').count() {
        0 => "…",
        1 => "…/…",
        _ => "…/…/…",
    };
    if cut.starts_with('/') && middle != "…" {
        middle = middle.strip_prefix('…').unwrap();
    }
    if cut.ends_with('/') && middle != "…" {
        middle = middle.strip_suffix('…').unwrap();
    }
    format!("{}{middle}{}", &text[..left], &text[right..]).into()
}
