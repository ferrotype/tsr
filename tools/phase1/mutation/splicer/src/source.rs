//! Line-addressed source text, the span digest and the mutant key.
//!
//! Lines are split on `\n` only and numbered from 1. A span `[start, end]` is
//! the exact bytes of lines `start..=end`, each with its terminating `\n` when
//! the file has one. `scripts/phase1_mutation_plan.py` defines the same digest
//! (`span_sha256`) so the coverage binding can recompute it without this tool.

use sha2::{Digest, Sha256};

/// A source file with its line starts, for line/column addressing.
pub struct Source {
    pub text: String,
    starts: Vec<usize>,
}

impl Source {
    pub fn new(text: String) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        Self { text, starts }
    }

    /// The number of lines; a trailing `\n` does not start another line.
    pub fn line_count(&self) -> usize {
        if self.text.ends_with('\n') || self.text.is_empty() {
            self.starts.len() - 1
        } else {
            self.starts.len()
        }
    }

    /// Line `line` (1-based) without its `\n`, or `None` past the end.
    pub fn line(&self, line: usize) -> Option<&str> {
        if line == 0 || line > self.line_count() {
            return None;
        }
        let start = self.starts[line - 1];
        let end = self
            .starts
            .get(line)
            .map_or(self.text.len(), |next| next - 1);
        Some(&self.text[start..end])
    }

    /// The UTF-8 byte offset within `line` of character column `column`
    /// (proc-macro2 columns count characters).
    pub fn byte_column(&self, line: usize, column: usize) -> Option<usize> {
        let text = self.line(line)?;
        if column == text.chars().count() {
            return Some(text.len());
        }
        text.char_indices().nth(column).map(|(offset, _)| offset)
    }

    /// The absolute byte offset of `line`/`byte_column`.
    pub fn offset(&self, line: usize, byte_column: usize) -> Option<usize> {
        let text = self.line(line)?;
        (byte_column <= text.len() && text.is_char_boundary(byte_column))
            .then(|| self.starts[line - 1] + byte_column)
    }

    /// The bytes of lines `start..=end`, or `None` when the range is invalid.
    pub fn span_bytes(&self, start: usize, end: usize) -> Option<&[u8]> {
        if start == 0 || start > end || end > self.line_count() {
            return None;
        }
        let from = self.starts[start - 1];
        let to = self.starts.get(end).copied().unwrap_or(self.text.len());
        Some(&self.text.as_bytes()[from..to])
    }

    /// The sha256 of lines `start..=end`.
    pub fn span_sha256(&self, start: usize, end: usize) -> Option<String> {
        self.span_bytes(start, end).map(sha256_hex)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex digit"));
        out.push(char::from_digit(u32::from(byte & 15), 16).expect("hex digit"));
    }
    out
}

/// The stable mutant key: the first 16 hex digits of
/// sha256(`op|file|function|site_line|operator|span_sha256`).
pub fn mutant_key(
    op: &str,
    file: &str,
    function: &str,
    site_line: usize,
    operator: &str,
    span_sha256: &str,
) -> String {
    let text = format!("{op}|{file}|{function}|{site_line}|{operator}|{span_sha256}");
    sha256_hex(text.as_bytes())[..16].to_owned()
}

/// Whether a trimmed line is one of the scope's line-prefix `port:` markers
/// (`phase1_scope.unmapped_ids`, xtask `scan_markers`).
pub fn is_marker_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["/// port:", "//! port:", "// port:"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// Whether a line is only a `//` comment (doc comments included) or blank.
pub fn is_comment_or_blank(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::{mutant_key, sha256_hex, Source};

    /// Shared with scripts/tests/test_phase1_mutation_plan.py: both sides must
    /// produce these digests for this text, so the Python binding check and the
    /// splicer agree on what a span is.
    pub const FIXTURE: &str = "fn a() {}\n// port: tsc/x.go:B\nfn b() -> bool {\n    true\n}\n";

    #[test]
    fn spans_are_whole_lines_with_their_newlines() {
        let source = Source::new(FIXTURE.to_owned());
        assert_eq!(source.line_count(), 5);
        assert_eq!(source.line(3), Some("fn b() -> bool {"));
        assert_eq!(
            source.span_bytes(2, 5),
            Some(&b"// port: tsc/x.go:B\nfn b() -> bool {\n    true\n}\n"[..])
        );
        assert_eq!(source.span_bytes(0, 1), None);
        assert_eq!(source.span_bytes(3, 6), None);
        let unterminated = Source::new("a\nb".to_owned());
        assert_eq!(unterminated.line_count(), 2);
        assert_eq!(unterminated.span_bytes(2, 2), Some(&b"b"[..]));
    }

    #[test]
    fn digests_match_the_shared_golden_values() {
        let source = Source::new(FIXTURE.to_owned());
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let span = source.span_sha256(2, 5).unwrap();
        assert_eq!(
            span,
            "6e551632f6e6d937576fdb5620d1506633db7d318a675812e26c0eb79045ea82"
        );
        assert_eq!(
            mutant_key(
                "tsc/x.go:B",
                "crates/x/src/lib.rs",
                "b",
                3,
                "return:true",
                &span
            ),
            "bb20205aa0809b53"
        );
    }

    #[test]
    fn columns_convert_from_characters_to_bytes() {
        let source = Source::new("let é = { x };\n".to_owned());
        assert_eq!(source.byte_column(1, 5), Some(6));
        assert_eq!(source.byte_column(1, 14), Some(15));
        assert_eq!(source.offset(1, 6), Some(6));
        assert_eq!(source.offset(1, 5), None, "not a character boundary");
    }
}
