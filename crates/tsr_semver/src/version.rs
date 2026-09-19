//! `tsc/internal/semver/version.go`.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Version {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    pub(crate) patch: u32,
    pub(crate) prerelease: Vec<String>,
    pub(crate) build: Vec<String>,
}

impl Version {
    pub fn parse(text: &[u8]) -> Result<Self, ParseError> {
        let (version, error) = try_parse_version(text);
        match error {
            Some(error) => Err(error),
            None => Ok(version),
        }
    }

    pub fn must_parse(text: &[u8]) -> Self {
        Self::parse(text).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn major(&self) -> u32 {
        self.major
    }
    pub fn minor(&self) -> u32 {
        self.minor
    }
    pub fn patch(&self) -> u32 {
        self.patch
    }
    pub fn prerelease(&self) -> &[String] {
        &self.prerelease
    }
    pub fn build(&self) -> &[String] {
        &self.build
    }

    /// Source pointer ordering includes nil; build metadata is ignored.
    pub fn compare(left: Option<&Self>, right: Option<&Self>) -> Ordering {
        let (left, right) = match (left, right) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => (left, right),
        };
        left.major
            .cmp(&right.major)
            .then_with(|| left.minor.cmp(&right.minor))
            .then_with(|| left.patch.cmp(&right.patch))
            .then_with(|| compare_prereleases(&left.prerelease, &right.prerelease))
    }

    pub(crate) fn increment_major(&self) -> Self {
        Self {
            major: self.major.wrapping_add(1),
            ..Self::default()
        }
    }
    pub(crate) fn increment_minor(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor.wrapping_add(1),
            ..Self::default()
        }
    }
    pub(crate) fn increment_patch(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch.wrapping_add(1),
            ..Self::default()
        }
    }
    pub(crate) fn zero_prerelease() -> Self {
        Self {
            prerelease: vec!["0".into()],
            ..Self::default()
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (marker, parts) in [("-", &self.prerelease), ("+", &self.build)] {
            if !parts.is_empty() {
                f.write_str(marker)?;
                for (i, part) in parts.iter().enumerate() {
                    if i != 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(part)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    InvalidVersion(Box<[u8]>),
    ComponentOverflow(Box<[u8]>),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion(input) => write!(
                f,
                "Could not parse version string from {}",
                ts_jsstring::go_quote(input)
            ),
            Self::ComponentOverflow(input) => write!(
                f,
                "strconv.ParseUint: parsing {}: value out of range",
                ts_jsstring::go_quote(input)
            ),
        }
    }
}
impl std::error::Error for ParseError {}

pub(crate) struct Parts<'a> {
    pub major: &'a str,
    pub minor: Option<&'a str>,
    pub patch: Option<&'a str>,
    pub prerelease: Option<&'a str>,
    pub build: Option<&'a str>,
}

// RE2's (?i)[a-z] includes the two non-ASCII simple-fold equivalents of
// ASCII letters, U+017F and U+212A. Its \d remains ASCII-only.
pub(crate) fn folded_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{017f}' | '\u{212a}')
}
pub(crate) fn numeric(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text.len() == 1 || !text.starts_with('0'))
}
pub(crate) fn wildcard(text: &str) -> bool {
    matches!(text, "x" | "X" | "*")
}

/// Match the original outer regex before parsing any numeric components. This
/// ordering preserves the partial Version returned alongside a parse error.
pub(crate) fn parts(text: &str, partial: bool) -> Option<Parts<'_>> {
    let end = text.find(['-', '+']).unwrap_or(text.len());
    let mut numbers = text[..end].split('.');
    let major = numbers.next()?;
    let minor = numbers.next();
    let patch = numbers.next();
    if numbers.next().is_some()
        || [Some(major), minor, patch]
            .into_iter()
            .flatten()
            .any(|part| !(numeric(part) || partial && wildcard(part)))
    {
        return None;
    }
    let mut prerelease = None;
    let mut build = None;
    if end != text.len() {
        patch?;
        let qualifier = &text[end + 1..];
        if text.as_bytes()[end] == b'-' {
            if let Some((pre, metadata)) = qualifier.split_once('+') {
                prerelease = Some(pre);
                build = Some(metadata);
            } else {
                prerelease = Some(qualifier);
            }
        } else {
            build = Some(qualifier);
        }
    }
    if [prerelease, build].into_iter().flatten().any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|c| folded_letter(c) || c.is_ascii_digit() || matches!(c, '-' | '.'))
    }) {
        return None;
    }
    Some(Parts {
        major,
        minor,
        patch,
        prerelease,
        build,
    })
}

pub(crate) fn uint_component(text: &str) -> Result<u32, ParseError> {
    text.parse()
        .map_err(|_| ParseError::ComponentOverflow(text.as_bytes().into()))
}

/// Like Go TryParseVersion, retain already-parsed fields on failure. Numeric
/// overflow assigns uint32::MAX to the failing component before returning.
pub fn try_parse_version(text: &[u8]) -> (Version, Option<ParseError>) {
    let invalid = || ParseError::InvalidVersion(text.into());
    let mut version = Version::default();
    let Some(parts) = std::str::from_utf8(text)
        .ok()
        .and_then(|text| parts(text, false))
    else {
        return (version, Some(invalid()));
    };
    for (index, source) in [Some(parts.major), parts.minor, parts.patch]
        .into_iter()
        .enumerate()
    {
        let target = match index {
            0 => &mut version.major,
            1 => &mut version.minor,
            _ => &mut version.patch,
        };
        if let Some(source) = source {
            match uint_component(source) {
                Ok(value) => *target = value,
                Err(error) => {
                    *target = u32::MAX;
                    return (version, Some(error));
                }
            }
        }
    }
    if let Some(pre) = parts.prerelease {
        if !pre.split('.').all(|part| {
            numeric(part)
                || (part
                    .chars()
                    .next()
                    .is_some_and(|c| folded_letter(c) || c == '-')
                    && part
                        .chars()
                        .all(|c| folded_letter(c) || c.is_ascii_digit() || c == '-'))
        }) {
            return (version, Some(invalid()));
        }
        version.prerelease = pre.split('.').map(str::to_owned).collect();
    }
    if let Some(build) = parts.build {
        if !build.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| folded_letter(c) || c.is_ascii_digit() || c == '-')
        }) {
            return (version, Some(invalid()));
        }
        version.build = build.split('.').map(str::to_owned).collect();
    }
    (version, None)
}

fn compare_prereleases(left: &[String], right: &[String]) -> Ordering {
    if left.is_empty() {
        return if right.is_empty() {
            Ordering::Equal
        } else {
            Ordering::Greater
        };
    }
    if right.is_empty() {
        return Ordering::Less;
    }
    for (left, right) in left.iter().zip(right) {
        let order = compare_identifier(left, right);
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}
fn compare_identifier(left: &str, right: &str) -> Ordering {
    let lexical = left.cmp(right);
    if lexical == Ordering::Equal {
        return lexical;
    }
    match (numeric(left), numeric(right)) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => match (left.parse::<u32>(), right.parse::<u32>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            _ => left.len().cmp(&right.len()).then(lexical),
        },
        (false, false) => lexical,
    }
}
