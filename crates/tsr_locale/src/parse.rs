//! BCP 47 scanner and recovery from x/text v0.38.0 internal/language/parse.go.
use crate::{data, lookup, Locale};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    Syntax,
    Unknown(String),
    DuplicateKey,
}
impl ParseError {
    pub fn subtag(&self) -> &str {
        if let Self::Unknown(value) = self {
            value
        } else {
            ""
        }
    }
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax => f.write_str("language: tag is not well-formed"),
            Self::Unknown(value) => {
                write!(f, "language: subtag {value:?} is well-formed but unknown")
            }
            Self::DuplicateKey => {
                f.write_str("language: different values for same key in -u extension")
            }
        }
    }
}
impl std::error::Error for ParseError {}

struct Parser {
    tokens: Vec<String>,
    at: usize,
    error: Option<ParseError>,
}
impl Parser {
    fn error(&mut self, error: ParseError) {
        if self.error.is_none() || error == ParseError::Syntax {
            self.error = Some(error);
        }
    }
    fn token(&self) -> &str {
        self.tokens.get(self.at).map_or("", String::as_str)
    }
    fn take(&mut self) -> String {
        let value = self.token().to_owned();
        self.at += 1;
        value
    }
    fn lookup(&mut self, table: &[(&str, u16)], name: String, letters: bool) -> u16 {
        if letters && !name.bytes().all(|c| c.is_ascii_alphabetic()) {
            self.error(ParseError::Syntax);
            return 0;
        }
        lookup(table, &name).unwrap_or_else(|| {
            self.error(ParseError::Unknown(name));
            0
        })
    }
    fn core(&mut self, normalize_extlang: bool) -> (Locale, Vec<String>, Vec<String>) {
        let mut out = Locale::default();
        let name = self.take().to_ascii_lowercase();
        out.language = self.lookup(data::LANGUAGES, name, true);
        let mut extlangs = Vec::new();
        while self.token().len() == 3 && self.token().as_bytes()[0].is_ascii_alphabetic() {
            let extlang = self.take().to_ascii_lowercase();
            if normalize_extlang {
                let id = self.lookup(data::LANGUAGES, extlang, true);
                if id != 0 {
                    out.language = id;
                }
            } else {
                extlangs.push(extlang);
            }
        }
        if self.token().len() == 4 && self.token().as_bytes()[0].is_ascii_alphabetic() {
            let mut name = self.take().to_ascii_lowercase();
            name[..1].make_ascii_uppercase();
            out.script = self.lookup(data::SCRIPTS, name, true);
        }
        if (2..=3).contains(&self.token().len()) {
            let name = self.take().to_ascii_uppercase();
            if name.len() == 2 || name.bytes().all(|c| c.is_ascii_digit()) {
                out.region = self.lookup(data::REGIONS, name, false);
            } else {
                self.error(ParseError::Syntax);
            }
        }
        let mut variants = Vec::new();
        let mut needs_sort = false;
        let mut last = None;
        let mut stopped = None;
        while self.token().len() >= 4 {
            let name = self.take().to_ascii_lowercase();
            if let Some(id) = lookup(data::VARIANTS, &name) {
                let previous_bytes = variants
                    .iter()
                    .map(|(_, name): &(u16, String)| name.len() + 1)
                    .sum::<usize>();
                variants.push((id, name.clone()));
                if !needs_sort {
                    if last.is_none_or(|last| last < id) {
                        last = Some(id);
                    } else {
                        needs_sort = true;
                        if variants.len() > 8 {
                            stopped = Some((id, name, previous_bytes));
                            break;
                        }
                    }
                }
            } else {
                self.error(ParseError::Unknown(name));
            }
        }
        variants.sort_by_key(|(id, _)| *id);
        variants.dedup_by_key(|(id, _)| *id);
        if let Some((id, name, previous_bytes)) = stopped {
            // parseVariants stops at the first out-of-order variant after eight.
            // Its in-place resize leaves scanner.end on the current token when
            // the deduplicated range has unchanged length, retaining that token
            // in the recovered tag. Otherwise the sorted range replaces it.
            let bytes = variants
                .iter()
                .map(|(_, name)| name.len() + 1)
                .sum::<usize>();
            if bytes == previous_bytes {
                variants.push((id, name));
                if !self.token().is_empty() {
                    self.error(ParseError::Syntax);
                }
            } else {
                self.error(ParseError::Syntax);
            }
            self.at = self.tokens.len();
        }
        (
            out,
            variants.into_iter().map(|(_, v)| v).collect(),
            extlangs,
        )
    }
    fn extensions(&mut self) -> Vec<String> {
        let mut result: Vec<(String, Vec<String>)> = Vec::new();
        while self.token().len() == 1 {
            let name = self.take().to_ascii_lowercase();
            let mut value = Vec::new();
            match name.as_str() {
                "x" => {
                    while !self.token().is_empty() {
                        value.push(self.take().to_ascii_lowercase());
                    }
                }
                "u" => {
                    while self.token().len() > 2 {
                        value.push(self.take().to_ascii_lowercase());
                    }
                    // Go orders attributes by their first three bytes.
                    value.sort_by(|a, b| a.as_bytes()[..3].cmp(&b.as_bytes()[..3]));
                    let mut keys: Vec<(String, Vec<String>)> = Vec::new();
                    while self.token().len() == 2 {
                        let key = self.take().to_ascii_lowercase();
                        let mut types = Vec::new();
                        while self.token().len() > 2 {
                            types.push(self.take().to_ascii_lowercase());
                        }
                        keys.push((key, types));
                    }
                    keys.sort_by(|a, b| a.0.cmp(&b.0));
                    let mut previous: Option<(String, Vec<String>)> = None;
                    for (key, types) in keys {
                        if let Some((old, old_types)) = &previous {
                            if old == &key {
                                if old_types != &types {
                                    self.error(ParseError::DuplicateKey);
                                }
                                continue;
                            }
                        }
                        value.push(key.clone());
                        value.extend(types.iter().cloned());
                        previous = Some((key, types));
                    }
                }
                "t" => {
                    if (2..=3).contains(&self.token().len())
                        && self.token().as_bytes()[1].is_ascii_alphabetic()
                    {
                        let (mut core, variants, extlangs) = self.core(false);
                        core.tail = variants.join("-");
                        let mut tag = core.tag_string();
                        if !extlangs.is_empty() {
                            let at = tag.find('-').unwrap_or(tag.len());
                            tag.insert_str(at, &format!("-{}", extlangs.join("-")));
                        }
                        value.extend(tag.to_ascii_lowercase().split('-').map(str::to_owned));
                    }
                    while self.token().len() == 2 && self.token().as_bytes()[1].is_ascii_digit() {
                        value.push(self.take().to_ascii_lowercase());
                        while self.token().len() >= 3 {
                            value.push(self.take().to_ascii_lowercase());
                        }
                    }
                }
                _ => {
                    while self.token().len() >= 2 {
                        value.push(self.take().to_ascii_lowercase());
                    }
                }
            }
            if value.is_empty() {
                self.error(ParseError::Syntax);
            } else {
                result.push((name.clone(), value));
            }
            if name == "x" {
                break;
            }
        }
        result.sort_by(|a, b| match (a.0.as_str(), b.0.as_str()) {
            ("x", "x") => std::cmp::Ordering::Equal,
            ("x", _) => std::cmp::Ordering::Greater,
            (_, "x") => std::cmp::Ordering::Less,
            _ => a.0.cmp(&b.0),
        });
        result
            .into_iter()
            .flat_map(|(name, value)| std::iter::once(name).chain(value))
            .collect()
    }
}
impl Locale {
    pub fn parse_detailed(input: &str) -> (Self, Option<ParseError>) {
        let input = input.replace('_', "-");
        let lower = input.to_ascii_lowercase();
        if let Ok(index) =
            data::GRANDFATHERED.binary_search_by_key(&lower.as_str(), |&(name, _)| name)
        {
            return Self::parse_detailed(data::GRANDFATHERED[index].1);
        }
        let mut parser = Parser {
            tokens: Vec::new(),
            at: 0,
            error: None,
        };
        for part in input.split('-') {
            if part.is_empty() || part.len() > 8 || !part.bytes().all(|c| c.is_ascii_alphanumeric())
            {
                parser.error(ParseError::Syntax);
            } else {
                parser.tokens.push(part.to_owned());
            }
        }
        let mut out;
        if parser.token().len() <= 1 {
            if !parser.token().eq_ignore_ascii_case("x") {
                return (Self::default(), Some(ParseError::Syntax));
            }
            let tail = parser.extensions().join("-");
            out = Self {
                private: !tail.is_empty(),
                tail,
                ..Self::default()
            };
        } else if parser.token().len() >= 4 {
            return (Self::default(), Some(ParseError::Syntax));
        } else {
            let (value, mut variants, _) = parser.core(true);
            out = value;
            if parser.token().len() == 1 {
                variants.extend(parser.extensions());
            } else if !parser.token().is_empty() {
                parser.error(ParseError::Syntax);
            }
            out.tail = variants.join("-");
        }
        if parser.error.is_none() {
            out.canonicalize(false);
        }
        (out, parser.error)
    }
}
