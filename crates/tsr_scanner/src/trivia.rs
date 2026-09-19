//! Trivia advancement and lazy comment ranges over source bytes.

use ts_ast::SyntaxKind;
use ts_core::TextRange;
use ts_jsstring::wtf8::decode_utf8;

use crate::utilities::{decode_last_utf8, is_line_break, is_white_space_like};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkipTriviaOptions {
    pub stop_after_line_break: bool,
    pub stop_at_comments: bool,
    pub in_jsdoc: bool,
}

fn index(pos: i64) -> usize {
    usize::try_from(pos).expect("source byte position must not be negative")
}

/// port: tsc/internal/scanner/scanner.go:couldStartTrivia
pub fn could_start_trivia(text: &[u8], pos: i64) -> bool {
    match text[index(pos)] {
        b'\r' | b'\n' | b'\t' | b'\x0b' | b'\x0c' | b' ' | b'/' | b'<' | b'|' | b'=' | b'>' => true,
        b'#' => pos == 0,
        byte => byte > 127,
    }
}

/// port: tsc/internal/scanner/scanner.go:SkipTrivia
pub fn skip_trivia(text: &[u8], pos: i64) -> i64 {
    skip_trivia_ex(text, pos, None)
}

/// port: tsc/internal/scanner/scanner.go:SkipTriviaEx
pub fn skip_trivia_ex(text: &[u8], mut pos: i64, options: Option<&SkipTriviaOptions>) -> i64 {
    if pos < 0 {
        return pos;
    }
    let options = options.copied().unwrap_or_default();
    let length = text.len() as i64;
    let mut can_consume_star = false;
    loop {
        if pos >= length {
            return pos;
        }
        let (ch, width) = decode_utf8(&text[index(pos)..]);
        match char::from_u32(ch as u32) {
            Some('\n' | '\r') => {
                if ch == i32::from(b'\r') && pos + 1 < length && text[index(pos + 1)] == b'\n' {
                    pos += 1;
                }
                pos += 1;
                if options.stop_after_line_break {
                    return pos;
                }
                can_consume_star = options.in_jsdoc;
                continue;
            }
            Some('\t' | '\x0b' | '\x0c' | ' ') => {
                pos += 1;
                continue;
            }
            Some('/') => {
                if options.stop_at_comments {
                    return pos;
                }
                if pos + 1 < length {
                    match text[index(pos + 1)] {
                        b'/' => {
                            pos += 2;
                            while pos < length {
                                let (ch, size) = decode_utf8(&text[index(pos)..]);
                                if is_line_break(ch) {
                                    break;
                                }
                                pos += size as i64;
                            }
                            can_consume_star = false;
                            continue;
                        }
                        b'*' => {
                            pos += 2;
                            while pos < length {
                                if text[index(pos)] == b'*'
                                    && pos + 1 < length
                                    && text[index(pos + 1)] == b'/'
                                {
                                    pos += 2;
                                    break;
                                }
                                pos += decode_utf8(&text[index(pos)..]).1 as i64;
                            }
                            can_consume_star = false;
                            continue;
                        }
                        _ => {}
                    }
                }
            }
            Some('<' | '|' | '=' | '>') => {
                if is_conflict_marker_trivia(text, pos) {
                    pos = scan_conflict_marker_trivia(text, pos);
                    can_consume_star = false;
                    continue;
                }
            }
            Some('#') => {
                if pos == 0 && is_shebang_trivia(text, pos) {
                    pos = scan_shebang_trivia(text, pos);
                    can_consume_star = false;
                    continue;
                }
            }
            Some('*') => {
                if can_consume_star {
                    pos += 1;
                    can_consume_star = false;
                    continue;
                }
            }
            _ => {
                if ch > 127 && is_white_space_like(ch) {
                    pos += width as i64;
                    continue;
                }
            }
        }
        return pos;
    }
}

/// port: tsc/internal/scanner/scanner.go:isConflictMarkerTrivia
pub(crate) fn is_conflict_marker_trivia(text: &[u8], pos: i64) -> bool {
    assert!(pos >= 0, "pos < 0");
    let next = pos.wrapping_add(1);
    if next >= text.len() as i64 || text[index(next)] != text[index(pos)] {
        return false;
    }
    let mut at_line_start = pos == 0 || is_line_break(i32::from(text[index(pos - 1)]));
    if !at_line_start && pos >= 2 {
        // Deliberate source quirk: the decoder sees text[..pos-2], not text[..pos].
        at_line_start = is_line_break(decode_last_utf8(&text[..index(pos - 2)]).0);
    }
    if at_line_start {
        let ch = text[index(pos)];
        let after_marker = pos.wrapping_add(7);
        if after_marker < text.len() as i64 {
            if (0..7).any(|offset| text[index(pos + offset)] != ch) {
                return false;
            }
            return ch == b'=' || text[index(after_marker)] == b' ';
        }
    }
    false
}

/// port: tsc/internal/scanner/scanner.go:scanConflictMarkerTrivia
pub(crate) fn scan_conflict_marker_trivia(text: &[u8], mut pos: i64) -> i64 {
    // Scanner callers emit the source's optional diagnostic before entering here.
    let (mut ch, mut width) = decode_utf8(&text[index(pos)..]);
    let length = text.len() as i64;
    if ch == i32::from(b'<') || ch == i32::from(b'>') {
        while pos < length && !is_line_break(ch) {
            pos += width as i64;
            (ch, width) = decode_utf8(&text[index(pos)..]);
        }
    } else {
        assert!(
            ch == i32::from(b'|') || ch == i32::from(b'='),
            "Assertion failed: ch must be either '|' or '='"
        );
        while pos < length {
            let current = text[index(pos)];
            if (current == b'=' || current == b'>')
                && i32::from(current) != ch
                && is_conflict_marker_trivia(text, pos)
            {
                break;
            }
            pos += 1;
        }
    }
    pos
}

/// port: tsc/internal/scanner/scanner.go:isShebangTrivia
pub(crate) fn is_shebang_trivia(text: &[u8], pos: i64) -> bool {
    if text.len() < 2 {
        return false;
    }
    assert!(
        pos == 0,
        "Shebangs check must only be done at the start of the file"
    );
    text.starts_with(b"#!")
}

/// port: tsc/internal/scanner/scanner.go:scanShebangTrivia
pub(crate) fn scan_shebang_trivia(text: &[u8], mut pos: i64) -> i64 {
    pos = pos.wrapping_add(2);
    while pos < text.len() as i64 {
        let (ch, width) = decode_utf8(&text[index(pos)..]);
        if is_line_break(ch) {
            break;
        }
        pos += width as i64;
    }
    pos
}

/// port: tsc/internal/scanner/scanner.go:GetShebang
pub fn get_shebang(text: &[u8]) -> &[u8] {
    if !is_shebang_trivia(text, 0) {
        return b"";
    }
    &text[..index(scan_shebang_trivia(text, 0))]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommentRange {
    pub loc: TextRange,
    pub kind: SyntaxKind,
    pub has_trailing_new_line: bool,
}

/// port: tsc/internal/scanner/scanner.go:GetLeadingCommentRanges
pub fn get_leading_comment_ranges(
    text: &[u8],
    pos: i64,
) -> impl Iterator<Item = CommentRange> + '_ {
    iterate_comment_ranges(text, pos, false)
}

/// port: tsc/internal/scanner/scanner.go:GetTrailingCommentRanges
pub fn get_trailing_comment_ranges(
    text: &[u8],
    pos: i64,
) -> impl Iterator<Item = CommentRange> + '_ {
    iterate_comment_ranges(text, pos, true)
}

struct CommentRanges<'src> {
    text: &'src [u8],
    pos: i64,
    trailing: bool,
    collecting: bool,
    started: bool,
    finished: bool,
    pending: Option<CommentRange>,
}

/// port: tsc/internal/scanner/scanner.go:iterateCommentRanges
fn iterate_comment_ranges(text: &[u8], pos: i64, trailing: bool) -> CommentRanges<'_> {
    CommentRanges {
        text,
        pos,
        trailing,
        collecting: trailing,
        started: false,
        finished: false,
        pending: None,
    }
}

impl Iterator for CommentRanges<'_> {
    type Item = CommentRange;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if !self.started {
            self.started = true;
            if self.pos == 0 {
                self.collecting = true;
                if is_shebang_trivia(self.text, 0) {
                    self.pos = scan_shebang_trivia(self.text, 0);
                }
            }
        }
        let length = self.text.len() as i64;
        while self.pos >= 0 && self.pos < length {
            let (ch, width) = decode_utf8(&self.text[index(self.pos)..]);
            match char::from_u32(ch as u32) {
                Some('\n' | '\r') => {
                    if ch == i32::from(b'\r')
                        && self.pos + 1 < length
                        && self.text[index(self.pos + 1)] == b'\n'
                    {
                        self.pos += 1;
                    }
                    self.pos += 1;
                    if self.trailing {
                        break;
                    }
                    self.collecting = true;
                    if let Some(pending) = &mut self.pending {
                        pending.has_trailing_new_line = true;
                    }
                }
                Some('\t' | '\x0b' | '\x0c' | ' ') => {
                    self.pos += 1;
                }
                Some('/') => {
                    let next = self.text.get(index(self.pos + 1)).copied().unwrap_or(0);
                    if next != b'/' && next != b'*' {
                        break;
                    }
                    let kind = if next == b'/' {
                        SyntaxKind::SingleLineCommentTrivia
                    } else {
                        SyntaxKind::MultiLineCommentTrivia
                    };
                    let start = self.pos;
                    self.pos += 2;
                    let mut has_trailing_new_line = false;
                    if next == b'/' {
                        while self.pos < length {
                            let (ch, width) = decode_utf8(&self.text[index(self.pos)..]);
                            if is_line_break(ch) {
                                has_trailing_new_line = true;
                                break;
                            }
                            self.pos += width as i64;
                        }
                    } else {
                        let remaining = &self.text[index(self.pos)..];
                        self.pos = remaining
                            .windows(2)
                            .position(|pair| pair == b"*/")
                            .map_or(length, |offset| self.pos + offset as i64 + 2);
                    }
                    if self.collecting {
                        let old = self.pending.replace(CommentRange {
                            loc: TextRange::new(start, self.pos),
                            kind,
                            has_trailing_new_line,
                        });
                        if old.is_some() {
                            return old;
                        }
                    }
                }
                _ => {
                    if ch > 127 && is_white_space_like(ch) {
                        if is_line_break(ch) {
                            if let Some(pending) = &mut self.pending {
                                pending.has_trailing_new_line = true;
                            }
                        }
                        self.pos += width as i64;
                        continue;
                    }
                    break;
                }
            }
        }
        self.finished = true;
        self.pending.take()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        get_leading_comment_ranges, get_trailing_comment_ranges, is_conflict_marker_trivia,
        skip_trivia_ex, CommentRange, SkipTriviaOptions,
    };

    fn ranges(iter: impl Iterator<Item = CommentRange>) -> Vec<(i64, i64, bool)> {
        iter.map(|range| {
            (
                range.loc.pos(),
                range.loc.end(),
                range.has_trailing_new_line,
            )
        })
        .collect()
    }

    #[test]
    fn comment_ranges_keep_go_unicode_line_break_distinctions() {
        // Pinned Go observations: Unicode separators do not stop trailing ranges
        // or start collecting leading ranges, although they mark a pending range.
        assert_eq!(
            ranges(get_trailing_comment_ranges(b"/*a*/\r\n/*b*/x", 0)),
            [(0, 5, false)]
        );
        assert_eq!(
            ranges(get_leading_comment_ranges(b"/*a*/\r\n/*b*/x", 0)),
            [(0, 5, true), (7, 12, false)]
        );
        assert_eq!(
            ranges(get_trailing_comment_ranges(
                "/*a*/\u{2028}/*b*/x".as_bytes(),
                0
            )),
            [(0, 5, true), (8, 13, false)]
        );
        assert!(get_leading_comment_ranges("x\u{2028}/*a*/".as_bytes(), 1)
            .next()
            .is_none());
        assert_eq!(
            ranges(get_trailing_comment_ranges("x\u{2028}/*a*/".as_bytes(), 1)),
            [(4, 9, false)]
        );
        assert_eq!(
            ranges(get_leading_comment_ranges(b"#!x\n//a\n", 0)),
            [(4, 7, true)]
        );
        assert!(get_trailing_comment_ranges(b"#!x\n//a\n", 0)
            .next()
            .is_none());
    }

    #[test]
    fn conflict_marker_line_start_keeps_the_pinned_slice_boundary() {
        assert!(!is_conflict_marker_trivia(
            "\u{2028}======= x".as_bytes(),
            3
        ));
        assert!(is_conflict_marker_trivia(
            "\u{2028}xy======= x".as_bytes(),
            5
        ));
        assert!(!is_conflict_marker_trivia(b"=======", 0));
        assert!(is_conflict_marker_trivia(b"=======x", 0));
        assert!(is_conflict_marker_trivia(b"||||||| x", 0));
    }

    #[test]
    fn skip_options_keep_ascii_line_break_and_jsdoc_star_rules() {
        let options = SkipTriviaOptions {
            stop_after_line_break: true,
            ..SkipTriviaOptions::default()
        };
        assert_eq!(skip_trivia_ex(b" \r\n*", 0, Some(&options)), 3);
        assert_eq!(
            skip_trivia_ex(" \u{2028}*".as_bytes(), 0, Some(&options)),
            4
        );
        let options = SkipTriviaOptions {
            in_jsdoc: true,
            ..SkipTriviaOptions::default()
        };
        assert_eq!(skip_trivia_ex(b"\n **x", 0, Some(&options)), 3);
        assert_eq!(skip_trivia_ex(b"", -4, None), -4);
        assert_eq!(skip_trivia_ex(b"x", i64::MAX, None), i64::MAX);
    }
}
