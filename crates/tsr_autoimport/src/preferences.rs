//! Request preferences; only extraction-affecting values enter an index key.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, RwLock},
};
use tsr_jsstring::JsString;

#[derive(Clone, Debug, Default)]
pub struct Preferences {
    pub module_specifier: Option<String>,
    pub ending: Option<String>,
    pub exclude_specifiers: Vec<String>,
    pub exclude_files: Vec<JsString>,
    pub directory_search: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BuildKey {
    files: Vec<JsString>,
    directory_search: Option<bool>,
}
impl Preferences {
    pub(crate) fn build_key(&self) -> BuildKey {
        let mut files = self.exclude_files.clone();
        files.sort();
        files.dedup();
        BuildKey {
            files,
            directory_search: self.directory_search,
        }
    }
    pub fn file_matcher(&self, sensitive: bool) -> Option<tsr_tsoptions::glob::SpecMatcher> {
        tsr_tsoptions::glob::SpecMatcher::new(
            &self.exclude_files,
            b"",
            tsr_tsoptions::glob::Usage::Exclude,
            sensitive,
        )
    }
    // port: tsc/internal/modulespecifiers/util.go:IsExcludedByRegex
    pub fn excludes(&self, specifier: &[u8]) -> bool {
        self.exclude_specifiers.iter().any(|pattern| {
            string_to_regex(pattern).is_some_and(|re| re.is_match(&go_utf8(specifier)))
        })
    }
}

// Go's regexp decoder treats each malformed byte as RuneError.
fn go_utf8(mut bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if std::str::from_utf8(bytes).is_ok() {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut repaired = String::new();
    while !bytes.is_empty() {
        let (rune, width) = tsr_jsstring::wtf8::decode_utf8(bytes);
        repaired.push(char::from_u32(rune as u32).unwrap_or(char::REPLACEMENT_CHARACTER));
        bytes = &bytes[width..];
    }
    std::borrow::Cow::Owned(repaired.into_bytes())
}

type RegexCache = HashMap<(String, bool), Option<Arc<regex::bytes::Regex>>>;
static REGEXES: LazyLock<RwLock<RegexCache>> = LazyLock::new(|| RwLock::new(HashMap::new()));

// port: tsc/internal/modulespecifiers/util.go:stringToRegex
fn string_to_regex(mut pattern: &str) -> Option<Arc<regex::bytes::Regex>> {
    let mut insensitive = false;
    if pattern.len() > 2 && pattern.starts_with('/') {
        if let Some(last) = pattern.rfind('/').filter(|&n| n > 0) {
            let bytes = pattern.as_bytes();
            if !(1..last).any(|n| bytes[n] == b'/' && bytes[n - 1] != b'\\') {
                insensitive = pattern[last + 1..].contains('i');
                pattern = &pattern[1..last];
            }
        }
    }
    let key = (pattern.to_owned(), insensitive);
    if let Some(cached) = REGEXES.read().unwrap().get(&key) {
        return cached.clone();
    }
    let compiled = crate::regexp::compile(pattern, insensitive).map(Arc::new);
    let mut cache = REGEXES.write().unwrap();
    if cache.len() > 1000 {
        cache.clear();
    }
    cache.entry(key).or_insert(compiled).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_exclusion_patterns_preserve_ascii_classes_quotes_and_flags() {
        for (pattern, text, expected) in [
            ("/^@scope\\/private/i", "@SCOPE/private", true),
            ("\\w+", "λ", false),
            ("\\s", "\u{b}", false),
            ("\\Qpkg+[x]\\E", "pkg+[x]", true),
            ("[a-z&&b]", "&", true),
            ("(?x)a b", "ab", false),
            ("\\p{Latin}", "ƛ", true),
            ("\\p{Tolong_Siki}", "\u{11db0}", true),
            ("(?i)\u{a7ce}", "\u{a7cf}", true),
            ("(?i)[^k]", "\u{212a}", false),
            ("(?i)[[:upper:]]", "\u{212a}", true),
            ("(?i)\\W", "ſ", false),
            ("\\p{latin}", "a", false),
            ("\\p{Emoji}", "😀", false),
            ("\\1", "\u{1}", false),
            ("\\123", "S", true),
            ("a{1001}", "a", false),
            ("(?:a{50}){50}", "a", false),
            ("(", "anything", false),
            ("a{no}", "a{no}", true),
            ("a{01}", "a{01}", true),
            ("(?P<1>a)(?P<1>b)", "ab", true),
            ("(?ii-i)a", "A", false),
            ("(?ii)a", "A", true),
            ("(?)a", "a", true),
            ("^pkg/", "other/pkg/", false),
        ] {
            let prefs = Preferences {
                exclude_specifiers: vec![pattern.into()],
                ..Default::default()
            };
            assert_eq!(prefs.excludes(text.as_bytes()), expected, "{pattern}");
        }
    }
}
