//! `tsc/internal/semver/version_range.go`.

use std::cmp::Ordering;
use std::fmt;

use crate::version::{parts, uint_component, wildcard};
use crate::Version;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VersionRange {
    alternatives: Vec<Vec<Comparator>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Comparator {
    operator: Operator,
    operand: Version,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operator {
    Less,
    LessEqual,
    Equal,
    GreaterEqual,
    Greater,
}

impl Operator {
    fn text(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Equal => "=",
            Self::GreaterEqual => ">=",
            Self::Greater => ">",
        }
    }
}

impl VersionRange {
    pub fn parse(text: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(text).ok()?;
        let mut alternatives = Vec::new();
        for range in text.trim_matches(go_space).split("||") {
            let range = range.trim_matches(go_space);
            if range.is_empty() {
                continue;
            }
            let tokens: Vec<_> = range
                .split(regex_space)
                .filter(|token| !token.is_empty())
                .collect();
            let comparators = if tokens.len() == 3 && tokens[1] == "-" {
                parse_hyphen(tokens[0], tokens[2])?
            } else {
                let mut comparators = Vec::new();
                for token in tokens {
                    let token = token.trim_matches(go_space);
                    let (operator, operand) = if token.starts_with(['<', '>', '=', '~', '^']) {
                        let size =
                            usize::from(token.starts_with("<=") || token.starts_with(">=")) + 1;
                        (&token[..size], &token[size..])
                    } else {
                        ("", token)
                    };
                    comparators.extend(parse_comparator(operator, operand)?);
                }
                comparators
            };
            alternatives.push(comparators);
        }
        Some(Self { alternatives })
    }

    /// A nil version sorts below every actual version, as in Go Version.Compare.
    pub fn test(&self, version: Option<&Version>) -> bool {
        self.alternatives.is_empty()
            || self.alternatives.iter().any(|alternative| {
                alternative.iter().all(|comparator| {
                    let order = Version::compare(version, Some(&comparator.operand));
                    match comparator.operator {
                        Operator::Less => order == Ordering::Less,
                        Operator::LessEqual => order != Ordering::Greater,
                        Operator::Equal => order == Ordering::Equal,
                        Operator::GreaterEqual => order != Ordering::Less,
                        Operator::Greater => order == Ordering::Greater,
                    }
                })
            })
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // An empty individual alternative writes nothing. In an OR expression,
        // preserve its separator; only a completely empty result becomes '*'.
        let mut written = false;
        for (i, alternative) in self.alternatives.iter().enumerate() {
            if i != 0 {
                f.write_str(" || ")?;
                written = true;
            }
            for (j, comparator) in alternative.iter().enumerate() {
                if j != 0 {
                    f.write_str(" ")?;
                }
                write!(f, "{}{}", comparator.operator.text(), comparator.operand)?;
                written = true;
            }
        }
        if !written {
            f.write_str("*")?;
        }
        Ok(())
    }
}

pub fn try_parse_version_range(text: &[u8]) -> (VersionRange, bool) {
    match VersionRange::parse(text) {
        Some(range) => (range, true),
        None => (VersionRange::default(), false),
    }
}

// strings.TrimSpace and regexp's Perl \s are intentionally different. The
// latter is ASCII-only and excludes vertical tab, unlike TrimSpace.
fn go_space(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{0085}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}
fn regex_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

struct PartialVersion<'a> {
    version: Version,
    major: &'a str,
    minor: &'a str,
    patch: &'a str,
}

fn parse_partial(text: &str) -> Option<PartialVersion<'_>> {
    let parts = parts(text, true)?;
    let minor = parts.minor.unwrap_or("*");
    let patch = parts.patch.unwrap_or("*");
    let mut version = Version::default();
    if !wildcard(parts.major) {
        version.major = uint_component(parts.major).ok()?;
        if !wildcard(minor) {
            version.minor = uint_component(minor).ok()?;
            if !wildcard(patch) {
                version.patch = uint_component(patch).ok()?;
            }
        }
    }
    // The source partial regex does not apply Version's stricter identifier
    // validation. For example 1.2.3-01 and 1.2.3-.. remain valid range operands.
    if let Some(pre) = parts.prerelease {
        version.prerelease = pre.split('.').map(str::to_owned).collect();
    }
    if let Some(build) = parts.build {
        version.build = build.split('.').map(str::to_owned).collect();
    }
    Some(PartialVersion {
        version,
        major: parts.major,
        minor,
        patch,
    })
}

fn parse_hyphen(left: &str, right: &str) -> Option<Vec<Comparator>> {
    let left = parse_partial(left)?;
    let right = parse_partial(right)?;
    let mut result = Vec::new();
    if !wildcard(left.major) {
        result.push(Comparator {
            operator: Operator::GreaterEqual,
            operand: left.version,
        });
    }
    if !wildcard(right.major) {
        let (operator, operand) = if wildcard(right.minor) {
            (Operator::Less, right.version.increment_major())
        } else if wildcard(right.patch) {
            (Operator::Less, right.version.increment_minor())
        } else {
            (Operator::LessEqual, right.version)
        };
        result.push(Comparator { operator, operand });
    }
    Some(result)
}

fn parse_comparator(operator: &str, text: &str) -> Option<Vec<Comparator>> {
    let partial = parse_partial(text)?;
    if wildcard(partial.major) {
        return Some(if matches!(operator, "<" | ">") {
            vec![Comparator {
                operator: Operator::Less,
                operand: Version::zero_prerelease(),
            }]
        } else {
            Vec::new()
        });
    }
    let mut version = partial.version;
    let minor_wildcard = wildcard(partial.minor);
    let patch_wildcard = wildcard(partial.patch);
    Some(match operator {
        "~" | "^" => {
            let upper = if (operator == "~" && minor_wildcard)
                || (operator == "^" && (version.major > 0 || minor_wildcard))
            {
                version.increment_major()
            } else if operator == "~" || version.minor > 0 || patch_wildcard {
                version.increment_minor()
            } else {
                version.increment_patch()
            };
            vec![
                Comparator {
                    operator: Operator::GreaterEqual,
                    operand: version,
                },
                Comparator {
                    operator: Operator::Less,
                    operand: upper,
                },
            ]
        }
        "<" | ">=" => {
            if minor_wildcard || patch_wildcard {
                version.prerelease = vec!["0".into()];
            }
            vec![Comparator {
                operator: if operator == "<" {
                    Operator::Less
                } else {
                    Operator::GreaterEqual
                },
                operand: version,
            }]
        }
        "<=" | ">" => {
            let mut op = if operator == "<=" {
                Operator::LessEqual
            } else {
                Operator::Greater
            };
            if minor_wildcard || patch_wildcard {
                op = if operator == "<=" {
                    Operator::Less
                } else {
                    Operator::GreaterEqual
                };
                version = if minor_wildcard {
                    version.increment_major()
                } else {
                    version.increment_minor()
                };
                version.prerelease = vec!["0".into()];
            }
            vec![Comparator {
                operator: op,
                operand: version,
            }]
        }
        "=" | "" => {
            if minor_wildcard || patch_wildcard {
                let mut upper = if minor_wildcard {
                    version.increment_major()
                } else {
                    version.increment_minor()
                };
                version.prerelease = vec!["0".into()];
                upper.prerelease = vec!["0".into()];
                vec![
                    Comparator {
                        operator: Operator::GreaterEqual,
                        operand: version,
                    },
                    Comparator {
                        operator: Operator::Less,
                        operand: upper,
                    },
                ]
            } else {
                vec![Comparator {
                    operator: Operator::Equal,
                    operand: version,
                }]
            }
        }
        _ => return None,
    })
}
