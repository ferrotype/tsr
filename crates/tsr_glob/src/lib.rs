//! The language-server glob grammar of the pinned `internal/glob`.
//!
//! This is not the configuration matcher: that dialect lives in `tsr_tsoptions`
//! and the two agree on almost nothing. Here `*` spans one path segment and
//! then cannot meet the separator that follows it, `**` may only sit next to a
//! separator, and a separator element consumes a whole run of `/`.
//!
//! Patterns and inputs are bytes. The pin accepts a literal `0xff` and matches
//! it byte-wise, while range bounds are decoded as Go runes.
//!
//! The pin notes this grammar is only intended for testing. It is ported as it
//! is, including the two behaviours a caller must know about: a negated range
//! `[!a-z]` stores its flag and then ignores it, and [`match_elements`] panics
//! when a separator element consumes the rest of the input.

use std::fmt;
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

/// The parser's four errors. Their text is the pinned contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    StarStarPlacement,
    UnmatchedBrace,
    BadRange,
    InvalidUtf8,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StarStarPlacement => "** may only be adjacent to '/'",
            Self::UnmatchedBrace => "unmatched '{'",
            Self::BadRange => "'[' patterns must be of the form [x-y]",
            Self::InvalidUtf8 => "invalid UTF-8 encoding",
        })
    }
}
impl std::error::Error for Error {}

/// Source type: tsc/internal/glob/glob.go:element
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    /// One or more `/` separators.
    Slash,
    /// Bytes containing none of the special characters. May be empty.
    Literal(Vec<u8>),
    Star,
    AnyChar,
    StarStar,
    /// `{a,b}`; a member may be empty.
    Group(Vec<Glob>),
    /// `[low-high]`. `negate` is stored and never read, as in the pin.
    CharRange {
        negate: bool,
        low: i32,
        high: i32,
    },
}
impl Element {
    /// Each element renders itself; a range renders without its negate flag.
    /// port: tsc/internal/glob/glob.go:charRange.String
    pub fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::Slash => out.push(b'/'),
            Self::Literal(bytes) => out.extend_from_slice(bytes),
            Self::Star => out.push(b'*'),
            Self::AnyChar => out.push(b'?'),
            Self::StarStar => out.extend_from_slice(b"**"),
            Self::Group(members) => {
                out.push(b'{');
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    member.write_to(out);
                }
                out.push(b'}');
            }
            Self::CharRange { low, high, .. } => {
                out.push(b'[');
                push_rune(out, *low);
                out.push(b'-');
                push_rune(out, *high);
                out.push(b']');
            }
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_to(&mut out);
        out
    }
}
/// Go's `string(rune)`: anything that is not a Unicode scalar value is U+FFFD.
fn push_rune(out: &mut Vec<u8>, rune: i32) {
    let scalar = u32::try_from(rune)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or('\u{fffd}');
    out.extend_from_slice(scalar.encode_utf8(&mut [0; 4]).as_bytes());
}

/// Source type: tsc/internal/glob/glob.go:Glob
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Glob {
    pub elements: Vec<Element>,
}
impl Glob {
    /// port: tsc/internal/glob/glob.go:Parse
    pub fn parse(pattern: &[u8]) -> Result<Self, Error> {
        parse(pattern, false).map(|(glob, _)| glob)
    }
    /// port: tsc/internal/glob/glob.go:Glob.String
    pub fn write_to(&self, out: &mut Vec<u8>) {
        for element in &self.elements {
            element.write_to(out);
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_to(&mut out);
        out
    }
    /// # Panics
    /// When a separator element consumes the rest of the input, as the pin does.
    /// port: tsc/internal/glob/glob.go:Glob.Match
    pub fn matches(&self, input: &[u8]) -> bool {
        match_elements(&self.elements, input)
    }
    /// Appends exactly one literal, possibly empty, and returns the tail. `}`
    /// and `,` end a literal only while parsing inside a group.
    /// port: tsc/internal/glob/glob.go:Glob.parseLiteral
    pub fn parse_literal<'a>(&mut self, pattern: &'a [u8], nested: bool) -> &'a [u8] {
        let special: &[u8] = if nested { b"*?{[/}," } else { b"*?{[/" };
        let end = pattern
            .iter()
            .position(|b| special.contains(b))
            .unwrap_or(pattern.len());
        self.elements
            .push(Element::Literal(pattern[..end].to_vec()));
        &pattern[end..]
    }
}
impl fmt::Display for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.to_bytes()))
    }
}

/// Under `nested` the parser stops at `}` or `,` and hands the residual back,
/// which is what makes a group parseable.
/// port: tsc/internal/glob/glob.go:parse
pub fn parse(mut pattern: &[u8], nested: bool) -> Result<(Glob, &[u8]), Error> {
    let mut glob = Glob::default();
    while let Some(&first) = pattern.first() {
        match first {
            b'/' => {
                pattern = &pattern[1..];
                glob.elements.push(Element::Slash);
            }
            b'*' if pattern.get(1) == Some(&b'*') => {
                let after_non_slash = glob
                    .elements
                    .last()
                    .is_some_and(|last| *last != Element::Slash);
                if after_non_slash || pattern.get(2).is_some_and(|&b| b != b'/') {
                    return Err(Error::StarStarPlacement);
                }
                pattern = &pattern[2..];
                glob.elements.push(Element::StarStar);
            }
            b'*' => {
                pattern = &pattern[1..];
                glob.elements.push(Element::Star);
            }
            b'?' => {
                pattern = &pattern[1..];
                glob.elements.push(Element::AnyChar);
            }
            b'{' => {
                let mut members = Vec::new();
                while pattern[0] != b'}' {
                    let (member, rest) = parse(&pattern[1..], true)?;
                    if rest.is_empty() {
                        return Err(Error::UnmatchedBrace);
                    }
                    pattern = rest;
                    members.push(member);
                }
                pattern = &pattern[1..];
                glob.elements.push(Element::Group(members));
            }
            b'}' | b',' if nested => return Ok((glob, pattern)),
            b'}' | b',' => pattern = glob.parse_literal(pattern, false),
            b'[' => {
                pattern = &pattern[1..];
                if pattern.is_empty() {
                    return Err(Error::BadRange);
                }
                let negate = pattern[0] == b'!';
                if negate {
                    pattern = &pattern[1..];
                }
                let (low, size) = read_range_rune(pattern)?;
                pattern = pattern[size..].strip_prefix(b"-").ok_or(Error::BadRange)?;
                let (high, size) = read_range_rune(pattern)?;
                pattern = pattern[size..].strip_prefix(b"]").ok_or(Error::BadRange)?;
                glob.elements.push(Element::CharRange { negate, low, high });
            }
            _ => pattern = glob.parse_literal(pattern, nested),
        }
    }
    Ok((glob, b""))
}

/// The decoded value gates the error and the size then selects it: a properly
/// encoded U+FFFD is accepted, an empty input is a bad range, an invalid byte is
/// invalid UTF-8.
/// port: tsc/internal/glob/glob.go:readRangeRune
pub fn read_range_rune(input: &[u8]) -> Result<(i32, usize), Error> {
    let (rune, size) = decode_utf8(input);
    if rune == RUNE_ERROR {
        match size {
            0 => return Err(Error::BadRange),
            1 => return Err(Error::InvalidUtf8),
            _ => {}
        }
    }
    Ok((rune, size))
}

/// The bytes before the first separator and the bytes after that separator
/// run, so a trailing run leaves an empty remainder.
/// port: tsc/internal/glob/glob.go:split
pub fn split(input: &[u8]) -> (&[u8], &[u8]) {
    let Some(index) = input.iter().position(|&b| b == b'/') else {
        return (input, b"");
    };
    let rest = input[index..]
        .iter()
        .position(|&b| b != b'/')
        .map_or(input.len(), |n| index + n);
    (&input[..index], &input[rest..])
}

/// Backtracks on `*` within one segment and on `**` across segments, and tries
/// each group alternative with the remaining elements appended.
///
/// # Panics
/// The separator arm consumes a run of `/` without a length guard, so an input
/// whose run reaches the end indexes past it. The pin does the same.
/// port: tsc/internal/glob/glob.go:match
pub fn match_elements(mut elements: &[Element], mut input: &[u8]) -> bool {
    while let Some((element, rest)) = elements.split_first() {
        elements = rest;
        match element {
            Element::Slash => {
                if input.first() != Some(&b'/') {
                    return false;
                }
                while input[0] == b'/' {
                    input = &input[1..];
                }
            }
            Element::StarStar => {
                // Skip the separator that follows.
                elements = elements.get(1..).unwrap_or(&[]);
                if elements.is_empty() {
                    return true;
                }
                while !input.is_empty() {
                    if match_elements(elements, input) {
                        return true;
                    }
                    input = split(input).1;
                }
                return false;
            }
            Element::Literal(bytes) => {
                let Some(rest) = input.strip_prefix(bytes.as_slice()) else {
                    return false;
                };
                input = rest;
            }
            Element::Star => {
                let (segment, rest) = split(input);
                input = rest;
                let end = elements
                    .iter()
                    .position(|e| *e == Element::Slash)
                    .unwrap_or(elements.len());
                let (segment_elements, remaining) = elements.split_at(end);
                elements = remaining;
                if segment_elements.is_empty() {
                    continue;
                }
                if !(0..segment.len()).any(|i| match_elements(segment_elements, &segment[i..])) {
                    return false;
                }
            }
            Element::AnyChar => {
                if input.first().is_none_or(|&b| b == b'/') {
                    return false;
                }
                input = &input[1..];
            }
            Element::Group(members) => {
                return members.iter().any(|member| {
                    let branch: Vec<Element> =
                        member.elements.iter().chain(elements).cloned().collect();
                    match_elements(&branch, input)
                });
            }
            Element::CharRange { low, high, .. } => {
                if input.first().is_none_or(|&b| b == b'/') {
                    return false;
                }
                let (rune, size) = decode_utf8(input);
                if rune < *low || rune > *high {
                    return false;
                }
                input = &input[size..];
            }
        }
    }
    input.is_empty()
}
