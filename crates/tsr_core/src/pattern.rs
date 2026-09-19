//! Byte-preserving single-wildcard patterns from the pinned core package.
use std::sync::Arc;

/// The zero value is invalid, exactly as the source `Pattern{}` value is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pattern {
    pub text: Arc<[u8]>,
    pub star_index: isize,
}

impl Pattern {
    /// port: tsc/internal/core/pattern.go:TryParsePattern
    pub fn parse(pattern: &[u8]) -> Self {
        let star_index = pattern.iter().position(|&byte| byte == b'*');
        if star_index.is_none_or(|index| !pattern[index + 1..].contains(&b'*')) {
            Self {
                text: Arc::from(pattern),
                star_index: star_index.map_or(-1, |index| index as isize),
            }
        } else {
            Self::default()
        }
    }

    /// port: tsc/internal/core/pattern.go:Pattern.IsValid
    pub fn is_valid(&self) -> bool {
        self.star_index == -1 || self.star_index < self.text.len() as isize
    }

    /// port: tsc/internal/core/pattern.go:Pattern.Matches
    pub fn matches(&self, candidate: &[u8]) -> bool {
        if self.star_index == -1 {
            return self.text.as_ref() == candidate;
        }
        // len(Text)-1 is -1 for the invalid zero value in Go. The length guard
        // still succeeds, so its following invalid suffix slice must panic.
        candidate.len() >= self.text.len().saturating_sub(1)
            && candidate.starts_with(&self.text[..self.star_index as usize])
            && candidate.ends_with(&self.text[self.star_index as usize + 1..])
    }

    /// port: tsc/internal/core/pattern.go:Pattern.MatchedText
    pub fn matched_text<'a>(&self, candidate: &'a [u8]) -> &'a [u8] {
        assert!(self.matches(candidate), "candidate does not match pattern");
        if self.star_index == -1 {
            return &candidate[..0];
        }
        let start = self.star_index as usize;
        let end = candidate.len() - (self.text.len() - start - 1);
        &candidate[start..end]
    }
}

pub fn try_parse_pattern(pattern: &[u8]) -> Pattern {
    Pattern::parse(pattern)
}

/// The default value represents Go's zero `T` when no pattern matches.
/// port: tsc/internal/core/pattern.go:FindBestPatternMatch
pub fn find_best_pattern_match<T: Clone + Default>(
    values: &[T],
    get_pattern: impl Fn(&T) -> Pattern,
    candidate: &[u8],
) -> T {
    let mut best = T::default();
    let mut longest_match_prefix_length = -1;
    for value in values {
        let pattern = get_pattern(value);
        if (pattern.star_index == -1 || pattern.star_index > longest_match_prefix_length)
            && pattern.matches(candidate)
        {
            best = value.clone();
            longest_match_prefix_length = pattern.star_index;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_empty_overlapping_and_invalid_bytes() {
        assert!(Pattern::parse(b"").is_valid());
        assert!(Pattern::parse(b"").matches(b""));
        let pattern = Pattern::parse(b"ab*ab");
        assert!(!pattern.matches(b"ab"));
        assert_eq!(pattern.matched_text(b"abXab"), b"X");
        assert_eq!(pattern.matched_text(b"abab"), b"");
        let pattern = Pattern::parse(b"\xff*\xfe");
        assert_eq!(pattern.star_index, 1);
        assert_eq!(pattern.matched_text(b"\xff\x80\xfe"), b"\x80");
        assert_eq!(Pattern::parse(b"*").matched_text(b""), b"");
        assert_eq!(Pattern::parse(b"exact").matched_text(b"exact"), b"");
    }

    #[test]
    fn best_match_preserves_source_order_even_after_an_exact_match() {
        let patterns: [&[u8]; 5] = [b"", b"ab*", b"a*", b"abc", b"*c"];
        let get = |index: &usize| Pattern::parse(patterns[*index]);
        assert_eq!(find_best_pattern_match(&[1, 2], get, b"abc"), 1);
        assert_eq!(find_best_pattern_match(&[1, 1], get, b"abc"), 1);
        // An exact match resets the longest prefix to -1 in the source.
        assert_eq!(find_best_pattern_match(&[1, 3, 4], get, b"abc"), 4);
        assert_eq!(find_best_pattern_match(&[4, 3], get, b"abc"), 3);
        assert_eq!(find_best_pattern_match(&[1, 2], get, b"zzz"), 0);
    }

    #[test]
    fn invalid_parse_and_nonmatching_extraction_preserve_panics() {
        let pattern = Pattern::parse(b"a*b*c");
        assert_eq!(pattern, Pattern::default());
        assert!(!pattern.is_valid());
        assert!(std::panic::catch_unwind(|| pattern.matches(b"anything")).is_err());
        let failure = std::panic::catch_unwind(|| Pattern::parse(b"a*").matched_text(b"b"));
        assert!(failure.is_err());
    }
}
