//! The Go standard-library operations the harness files call, over bytes:
//! `strings.Builder` as a shared writer, `strings.Replace`, `strings.Cut*`,
//! `path.Dir` and `path.Clean`, and `utf8.DecodeRune`.
use crate::execute::tsc::Writer;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A `*strings.Builder` the pin shares between the system's writers, the
/// tracer and the runner. Every write is one append under the lock.
#[derive(Debug, Default)]
pub struct StringBuilder {
    bytes: Mutex<Vec<u8>>,
}

impl StringBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<u8>> {
        self.bytes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `String()`.
    pub fn string(&self) -> Vec<u8> {
        self.lock().clone()
    }

    /// `Len()`.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// `Reset()`.
    pub fn reset(&self) {
        self.lock().clear();
    }

    /// `WriteString`.
    pub fn write_string(&self, text: &[u8]) {
        self.lock().extend_from_slice(text);
    }
}

impl Writer for StringBuilder {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write_string(bytes);
        Ok(bytes.len())
    }
}

/// `strings.Index`.
pub fn index(text: &[u8], sep: &[u8]) -> Option<usize> {
    if sep.is_empty() {
        return Some(0);
    }
    text.windows(sep.len()).position(|window| window == sep)
}

/// `strings.Contains`.
pub fn contains(text: &[u8], sep: &[u8]) -> bool {
    index(text, sep).is_some()
}

/// `strings.Replace(text, old, new, n)` for a non-empty `old`; a negative `n`
/// replaces every occurrence.
pub fn replace(text: &[u8], old: &[u8], new: &[u8], n: isize) -> Vec<u8> {
    assert!(
        !old.is_empty(),
        "replace: an empty old string is not used here"
    );
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text;
    let mut done = 0;
    while n < 0 || done < n {
        let Some(at) = index(rest, old) else { break };
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(new);
        rest = &rest[at + old.len()..];
        done += 1;
    }
    out.extend_from_slice(rest);
    out
}

/// `strings.ReplaceAll`.
pub fn replace_all(text: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    replace(text, old, new, -1)
}

/// `strings.CutPrefix`.
pub fn cut_prefix<'a>(text: &'a [u8], prefix: &[u8]) -> (&'a [u8], bool) {
    match text.strip_prefix(prefix) {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

/// `strings.CutSuffix`.
pub fn cut_suffix<'a>(text: &'a [u8], suffix: &[u8]) -> (&'a [u8], bool) {
    match text.strip_suffix(suffix) {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

/// `strings.TrimPrefix`.
pub fn trim_prefix<'a>(text: &'a [u8], prefix: &[u8]) -> &'a [u8] {
    cut_prefix(text, prefix).0
}

/// `strings.TrimSuffix`.
pub fn trim_suffix<'a>(text: &'a [u8], suffix: &[u8]) -> &'a [u8] {
    cut_suffix(text, suffix).0
}

/// `utf8.DecodeRune`: the rune at the start of `bytes` and its width; an
/// invalid or truncated sequence is `U+FFFD` of width one, empty input is
/// `U+FFFD` of width zero.
pub fn decode_rune(bytes: &[u8]) -> (char, usize) {
    let Some(&first) = bytes.first() else {
        return (char::REPLACEMENT_CHARACTER, 0);
    };
    let width = match first {
        0x00..=0x7f => return (char::from(first), 1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (char::REPLACEMENT_CHARACTER, 1),
    };
    match bytes.get(..width).map(std::str::from_utf8) {
        Some(Ok(text)) => (
            text.chars()
                .next()
                .expect("a decoded sequence has one rune"),
            width,
        ),
        _ => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// Go's `path.Clean`: the shortest equivalent slash-separated path.
pub fn path_clean(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let rooted = path[0] == b'/';
    let n = path.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if path[r] == b'/' || path[r] == b'.' && (r + 1 == n || path[r + 1] == b'/') {
            // empty path element, or a . element
            r += 1;
        } else if path[r] == b'.'
            && r + 1 < n
            && path[r + 1] == b'.'
            && (r + 2 == n || path[r + 2] == b'/')
        {
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if rooted && out.len() != 1 || !rooted && !out.is_empty() {
                out.push(b'/');
            }
            while r < n && path[r] != b'/' {
                out.push(path[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return b".".to_vec();
    }
    out
}

/// Go's `path.Dir`: everything but the last element, cleaned.
pub fn path_dir(path: &[u8]) -> Vec<u8> {
    let directory = match path.iter().rposition(|&b| b == b'/') {
        Some(slash) => &path[..=slash],
        None => b"",
    };
    path_clean(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_dir_follows_go() {
        // Expectations from Go's path package documentation and source.
        for (input, expected) in [
            (&b"/a/b/c.ts"[..], &b"/a/b"[..]),
            (b"/a", b"/"),
            (b"/", b"/"),
            (b"", b"."),
            (b"a", b"."),
            (b"a/b", b"a"),
            (b"D:/x/y", b"D:/x"),
            (b"D:/x", b"D:"),
            (b"D:", b"."),
            (b"/a//b/", b"/a/b"),
            (b"/a/../b/c", b"/b"),
        ] {
            assert_eq!(
                path_dir(input),
                expected,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
        assert_eq!(path_clean(b"../a/./b/.."), b"../a");
        assert_eq!(path_clean(b"a/../.."), b"..");
    }

    #[test]
    fn replace_counts_occurrences() {
        assert_eq!(replace(b"a.b.c", b".", b"-", 1), b"a-b.c");
        assert_eq!(replace_all(b"a.b.c", b".", b"-"), b"a-b-c");
        assert_eq!(replace(b"abc", b"x", b"-", 1), b"abc");
    }

    #[test]
    fn decode_rune_matches_go() {
        assert_eq!(decode_rune("\u{fffd}@".as_bytes()), ('\u{fffd}', 3));
        assert_eq!(decode_rune(b"\xff@"), ('\u{fffd}', 1));
        assert_eq!(decode_rune(b"\xef\xbf"), ('\u{fffd}', 1));
        assert_eq!(decode_rune(b"\xed\xa0\x80"), ('\u{fffd}', 1));
        assert_eq!(decode_rune("é".as_bytes()), ('é', 2));
    }
}
