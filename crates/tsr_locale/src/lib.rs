//! Locale identity and diagnostic language selection from pinned x/text data.
//!
//! Parsing preserves the usable portion of a failed tag. `Locale::default()` is
//! the unspecified tag; absence is represented separately by `Option<Locale>`.
mod likely;
mod matching;
mod parse;
mod tables_generated;

pub use parse::ParseError;
use std::fmt;
use tables_generated as data;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Locale {
    language: u16,
    script: u16,
    region: u16,
    tail: String,
    private: bool,
}

/// Source variable: tsc/internal/locale/locale.go:Default
pub static DEFAULT: Locale = Locale {
    language: 0,
    script: 0,
    region: 0,
    tail: String::new(),
    private: false,
};

impl Locale {
    /// port: tsc/internal/locale/locale.go:Parse
    pub fn parse(input: &str) -> (Self, bool) {
        let (value, error) = Self::parse_detailed(input);
        (value, error.is_none())
    }
    pub fn is_default(&self) -> bool {
        self == &DEFAULT
    }
    /// The underlying BCP 47 representation, including `und` for unspecified.
    pub fn tag_string(&self) -> String {
        if self.private {
            return self.tail.clone();
        }
        let mut out = data::LANGUAGE_NAMES
            .binary_search_by_key(&self.language, |&(id, _)| id)
            .map_or("und", |index| data::LANGUAGE_NAMES[index].1)
            .to_owned();
        if self.script != 0 {
            out.push('-');
            out.push_str(data::SCRIPT_NAMES[usize::from(self.script)]);
        }
        if self.region != 0 {
            out.push('-');
            out.push_str(data::REGION_NAMES[usize::from(self.region)]);
        }
        if !self.tail.is_empty() {
            out.push('-');
            out.push_str(&self.tail);
        }
        out
    }
    /// Index in the pinned diagnostic locale roster; `None` means confidence
    /// below Low. Index zero is English and has no translated table.
    pub fn diagnostic_match(&self) -> Option<usize> {
        matching::select(self)
    }

    fn canonicalize(&mut self, macro_language: bool) {
        while let Ok(index) = data::ALIASES.binary_search_by_key(&self.language, |row| row[0]) {
            let kind = data::ALIAS_TYPES[index];
            match kind {
                // Pinned AliasType: Deprecated=0, Macro=1, Legacy=2.
                0 => {
                    if self.language == lookup(data::LANGUAGES, "mo").unwrap() && self.region == 0 {
                        self.region = lookup(data::REGIONS, "MD").unwrap();
                    }
                    self.language = data::ALIASES[index][1];
                }
                2 => {
                    if self.language == lookup(data::LANGUAGES, "sh").unwrap() && self.script == 0 {
                        self.script = lookup(data::SCRIPTS, "Latn").unwrap();
                    }
                    self.language = data::ALIASES[index][1];
                    break;
                }
                1 if macro_language => {
                    self.language = data::ALIASES[index][1];
                    break;
                }
                _ => break,
            }
        }
        if self.script == lookup(data::SCRIPTS, "Qaai").unwrap() {
            self.script = lookup(data::SCRIPTS, "Zinh").unwrap();
        }
        if let Ok(index) = data::REGION_ALIASES.binary_search_by_key(&self.region, |row| row[0]) {
            self.region = data::REGION_ALIASES[index][1];
        }
    }
}
impl fmt::Display for Locale {
    /// port: tsc/internal/locale/locale.go:Locale.String
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_default() {
            Ok(())
        } else {
            f.write_str(&self.tag_string())
        }
    }
}

/// Explicit request-local carrier: an absent value is distinct from an
/// explicitly stored Default. Cloning it retains the parent request's value.
#[derive(Clone, Debug, Default)]
pub struct LocaleContext(Option<Locale>);
impl LocaleContext {
    /// port: tsc/internal/locale/locale.go:WithLocale
    #[must_use]
    pub fn with_locale(&self, locale: Locale) -> Self {
        Self(Some(locale))
    }
    /// port: tsc/internal/locale/locale.go:FromContext
    pub fn locale(&self) -> &Locale {
        self.0.as_ref().unwrap_or(&DEFAULT)
    }
    /// port: tsc/internal/locale/locale.go:HasLocale
    pub fn has_locale(&self) -> bool {
        self.0.is_some()
    }
}
fn lookup(table: &[(&str, u16)], key: &str) -> Option<u16> {
    table
        .binary_search_by_key(&key, |&(name, _)| name)
        .ok()
        .map(|i| table[i].1)
}
