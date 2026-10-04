use std::collections::{HashMap, HashSet};
use tsr_jsstring::{
    helpers::{simple_lower_go, simple_upper_go, to_lower_go},
    wtf8::decode_utf8,
};

pub trait Named {
    fn name(&self) -> &[u8];
}
#[derive(Clone, Debug)]
pub struct Index<T> {
    entries: Vec<T>,
    index: HashMap<i32, Vec<usize>>,
}
impl<T> Default for Index<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }
}
impl<T: Named> Index<T> {
    pub fn entries_mut(&mut self) -> &mut [T] {
        &mut self.entries
    }
    pub fn entries(&self) -> &[T] {
        &self.entries
    }
    // port: tsc/internal/ls/autoimport/index.go:Index.insertAsWords
    pub fn insert(&mut self, value: T) {
        let name = value.name();
        assert!(!name.is_empty(), "Cannot index entry with empty name");
        let mut seen = HashSet::new();
        for (i, start) in word_indices(name).into_iter().enumerate() {
            let (rune, _) = decode_utf8(&name[start..]);
            if rune == 0xfffd {
                continue;
            }
            let rune = if i == 0 {
                simple_upper_go(rune)
            } else {
                simple_lower_go(rune)
            };
            if seen.insert(rune) {
                self.index.entry(rune).or_default().push(self.entries.len());
            }
        }
        self.entries.push(value);
    }
    // port: tsc/internal/ls/autoimport/index.go:Index.Find
    pub fn find(&self, name: &[u8], case_sensitive: bool) -> Vec<&T> {
        if name.is_empty() {
            return Vec::new();
        }
        let (first, _) = decode_utf8(name);
        if first == 0xfffd {
            return Vec::new();
        }
        self.index
            .get(&simple_upper_go(first))
            .into_iter()
            .flatten()
            .map(|&i| &self.entries[i])
            .filter(|e| {
                if case_sensitive {
                    e.name() == name
                } else {
                    tsr_jsstring::equal_fold(e.name(), name)
                }
            })
            .collect()
    }
    // port: tsc/internal/ls/autoimport/index.go:Index.SearchWordPrefix
    pub fn search(&self, prefix: &[u8]) -> Vec<&T> {
        if prefix.is_empty() {
            return self.entries.iter().collect();
        }
        let prefix = to_lower_go(prefix);
        let (first, _) = decode_utf8(&prefix);
        if first == 0xfffd {
            return Vec::new();
        }
        let (upper, lower) = (simple_upper_go(first), simple_lower_go(first));
        let starts = self.index.get(&upper).into_iter().flatten();
        let words = (upper != lower)
            .then(|| self.index.get(&lower))
            .flatten()
            .into_iter()
            .flatten();
        starts
            .chain(words)
            .map(|&i| &self.entries[i])
            .filter(|e| contains_chars_in_order(e.name(), &prefix))
            .collect()
    }
    // port: tsc/internal/ls/autoimport/index.go:Index.Clone
    #[must_use]
    pub fn filtered(&self, mut filter: impl FnMut(&T) -> bool) -> Self
    where
        T: Clone,
    {
        let mut result = Self::default();
        let remap: Vec<_> = self
            .entries
            .iter()
            .map(|entry| {
                if !filter(entry) {
                    return None;
                }
                let index = result.entries.len();
                result.entries.push(entry.clone());
                Some(index)
            })
            .collect();
        for (&key, values) in &self.index {
            let values: Vec<_> = values.iter().filter_map(|&i| remap[i]).collect();
            if !values.is_empty() {
                result.index.insert(key, values);
            }
        }
        result
    }
}
// port: tsc/internal/ls/autoimport/index.go:containsCharsInOrder
pub fn contains_chars_in_order(text: &[u8], pattern: &[u8]) -> bool {
    let text = to_lower_go(text);
    let pattern = to_lower_go(pattern);
    let mut target = 0;
    let mut pos = 0;
    while pos < text.len() {
        let (rune, size) = decode_utf8(&text[pos..]);
        pos += size;
        if target < pattern.len() {
            let (wanted, width) = decode_utf8(&pattern[target..]);
            if rune == wanted {
                target += width;
            }
        }
    }
    target == pattern.len()
}
// port: tsc/internal/ls/autoimport/util.go:wordIndices
pub fn word_indices(text: &[u8]) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut pos = 0;
    let mut previous = 0;
    while pos < text.len() {
        let (rune, size) = decode_utf8(&text[pos..]);
        if pos == 0 {
            indices.push(0);
        } else if rune == i32::from(b'_') {
            if pos + 1 < text.len() && text[pos + 1] != b'_' {
                indices.push(pos + 1);
            }
        } else if crate::unicode::is_upper(rune)
            && (crate::unicode::is_lower(previous)
            // The pin advances one byte here, not one rune. Preserve its
            // boundary behavior for a non-ASCII capital followed by lowercase.
            || pos+1 < text.len() && crate::unicode::is_lower(decode_utf8(&text[pos+1..]).0))
        {
            indices.push(pos);
        }
        previous = rune;
        pos += size;
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Debug)]
    struct Entry(&'static str, &'static str);
    impl Named for Entry {
        fn name(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    // source: tsc/internal/ls/autoimport/index_test.go:TestIndexClone
    #[test]
    fn clone_filters_and_reindexes_without_changing_the_original() {
        let mut index = Index::default();
        for e in [
            Entry("fooBar", "a"),
            Entry("bazQux", "b"),
            Entry("fooQux", "a"),
        ] {
            index.insert(e);
        }
        let clone = index.filtered(|e| e.1 != "b");
        assert_eq!(index.entries().len(), 3);
        assert_eq!(clone.entries().len(), 2);
        assert_eq!(clone.find(b"fooBar", true).len(), 1);
        assert_eq!(clone.find(b"FOOBAR", false).len(), 1);
        assert!(clone.find(b"bazQux", true).is_empty());
        assert_eq!(clone.search(b"foo").len(), 2);
        assert!(index.filtered(|_| false).index.is_empty());
        assert!(Index::<Entry>::default()
            .filtered(|_| true)
            .entries()
            .is_empty());
    }
    // source: tsc/internal/ls/autoimport/util_test.go:TestWordIndices
    #[test]
    fn word_boundaries_match_go_byte_indices() {
        for (name, expected) in [
            ("camelCase", vec![0, 5]),
            ("snake_case", vec![0, 6]),
            ("ParseURL", vec![0, 5]),
            ("XMLHttpRequest", vec![0, 3, 7]),
            ("hello", vec![0]),
            ("HELLO", vec![0]),
            ("parseHTML5Parser", vec![0, 5, 10]),
            ("__proto__", vec![0, 2]),
            ("_private_member", vec![0, 9]),
            ("a", vec![0]),
            ("A", vec![0]),
            ("test__double__underscore", vec![0, 6, 14]),
        ] {
            assert_eq!(word_indices(name.as_bytes()), expected, "{name}");
        }
        // Lu/Ll rather than Rust's derived Uppercase/Lowercase properties.
        assert!(!crate::unicode::is_upper('Ⅲ' as i32));
        assert!(!crate::unicode::is_lower('ª' as i32));
        assert!(crate::unicode::is_upper('Σ' as i32));
        assert!(crate::unicode::is_lower('ς' as i32));
    }
    #[test]
    fn search_preserves_name_and_word_buckets_and_go_simple_folding() {
        let mut index = Index::default();
        for name in ["fooBar", "BarFoo", "fizzBuzz", "Σigma", "\u{212a}elvin"] {
            index.insert(Entry(name, ""));
        }
        assert_eq!(
            index.search(b"b").iter().map(|e| e.0).collect::<Vec<_>>(),
            ["BarFoo", "fooBar", "fizzBuzz"]
        );
        assert_eq!(
            index.search(b"fb").iter().map(|e| e.0).collect::<Vec<_>>(),
            ["fooBar", "fizzBuzz"]
        );
        assert_eq!(index.find("σIGMA".as_bytes(), false).len(), 1);
        // Go buckets by ToUpper, not EqualFold: the Kelvin sign stays
        // U+212A and therefore is not a candidate for an ASCII K lookup.
        assert!(index.find(b"kelvin", false).is_empty());
        assert_eq!(index.find("\u{212a}ELVIN".as_bytes(), false).len(), 1);
        assert!(index.find(b"\xff", false).is_empty());
        assert!(index.search(b"\xff").is_empty());
    }
}
