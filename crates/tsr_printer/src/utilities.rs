//! Comment classification and indentation helpers of
//! `tsc/internal/printer/utilities.go`.
//!
//! Text is the source file's bytes. Runes are decoded with Go's
//! `utf8.DecodeRuneInString` (`tsr_jsstring::wtf8::decode_utf8`), as upstream
//! does: malformed input advances one byte and is never whitespace.

use crate::text_writer::get_default_indent_size;
use tsr_ast::SyntaxKind as K;
use tsr_jsstring::classify::is_white_space_single_line;
use tsr_jsstring::wtf8::decode_utf8;
use tsr_scanner::CommentRange;

fn index(pos: i64) -> usize {
    usize::try_from(pos).expect("comment positions are not negative")
}

// port: tsc/internal/printer/utilities.go:skipWhiteSpaceSingleLine
fn skip_white_space_single_line(text: &[u8], pos: &mut usize) {
    while *pos < text.len() {
        let (ch, size) = decode_utf8(&text[*pos..]);
        if !is_white_space_single_line(ch) {
            break;
        }
        *pos += size;
    }
}

// port: tsc/internal/printer/utilities.go:matchWhiteSpaceSingleLine
fn match_white_space_single_line(text: &[u8], pos: &mut usize) -> bool {
    let start_pos = *pos;
    skip_white_space_single_line(text, pos);
    *pos != start_pos
}

// port: tsc/internal/printer/utilities.go:matchRune
fn match_rune(text: &[u8], pos: &mut usize, expected: char) -> bool {
    let (ch, size) = decode_utf8(&text[*pos..]);
    if ch == expected as i32 {
        *pos += size;
        return true;
    }
    false
}

// port: tsc/internal/printer/utilities.go:matchString
fn match_string(text: &[u8], pos: &mut usize, expected: &str) -> bool {
    let mut text_pos = *pos;
    for expected_rune in expected.chars() {
        if text_pos >= text.len() {
            return false;
        }
        if !match_rune(text, &mut text_pos, expected_rune) {
            return false;
        }
    }
    *pos = text_pos;
    true
}

// port: tsc/internal/printer/utilities.go:matchQuotedString
fn match_quoted_string(text: &[u8], pos: &mut usize) -> bool {
    let mut text_pos = *pos;
    let quote_char = if match_rune(text, &mut text_pos, '\'') {
        i32::from(b'\'')
    } else if match_rune(text, &mut text_pos, '"') {
        i32::from(b'"')
    } else {
        return false;
    };
    while text_pos < text.len() {
        let (ch, size) = decode_utf8(&text[text_pos..]);
        text_pos += size;
        if ch == quote_char {
            *pos = text_pos;
            return true;
        }
    }
    false
}

/// `/// <reference path="..." />`, `types`, `lib`, `no-default-lib`,
/// `/// <amd-dependency path="..." />` and `/// <amd-module />`.
// port: tsc/internal/printer/utilities.go:IsRecognizedTripleSlashComment
pub fn is_recognized_triple_slash_comment(text: &[u8], comment: &CommentRange) -> bool {
    let (comment_pos, comment_end) = (comment.loc.pos(), comment.loc.end());
    if comment.kind == K::SingleLineCommentTrivia
        && comment.loc.len() > 2
        && text[index(comment_pos + 1)] == b'/'
        && text[index(comment_pos + 2)] == b'/'
    {
        let text = &text[index(comment_pos + 3)..index(comment_end)];
        let mut pos = 0;
        skip_white_space_single_line(text, &mut pos);
        if !match_rune(text, &mut pos, '<') {
            return false;
        }
        if match_string(text, &mut pos, "reference") {
            if !match_white_space_single_line(text, &mut pos) {
                return false;
            }
            if !match_string(text, &mut pos, "path")
                && !match_string(text, &mut pos, "types")
                && !match_string(text, &mut pos, "lib")
                && !match_string(text, &mut pos, "no-default-lib")
            {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_rune(text, &mut pos, '=') {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_quoted_string(text, &mut pos) {
                return false;
            }
        } else if match_string(text, &mut pos, "amd-dependency") {
            if !match_white_space_single_line(text, &mut pos) {
                return false;
            }
            if !match_string(text, &mut pos, "path") {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_rune(text, &mut pos, '=') {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_quoted_string(text, &mut pos) {
                return false;
            }
        } else if match_string(text, &mut pos, "amd-module") {
            skip_white_space_single_line(text, &mut pos);
        } else {
            return false;
        }
        return text[pos..].windows(2).any(|pair| pair == b"/>");
    }
    false
}

// port: tsc/internal/printer/utilities.go:isJSDocLikeText
pub(crate) fn is_jsdoc_like_text(text: &[u8], comment: &CommentRange) -> bool {
    comment.kind == K::MultiLineCommentTrivia
        && comment.loc.len() >= 5
        && text[index(comment.loc.pos() + 2)] == b'*'
        && text[index(comment.loc.pos() + 3)] != b'/'
}

// port: tsc/internal/printer/utilities.go:IsPinnedComment
pub fn is_pinned_comment(text: &[u8], comment: &CommentRange) -> bool {
    comment.kind == K::MultiLineCommentTrivia
        && comment.loc.len() > 5
        && text[index(comment.loc.pos() + 2)] == b'!'
}

/// The column width of the whitespace at `pos`, with tabs advancing to the
/// next multiple of the default indent size.
// port: tsc/internal/printer/utilities.go:calculateIndent
pub(crate) fn calculate_indent(text: &[u8], mut pos: i64, end: i64) -> i64 {
    let mut current_line_indent = 0;
    let indent_size = get_default_indent_size() as i64;
    while pos < end {
        let (ch, size) = decode_utf8(&text[index(pos)..]);
        if !is_white_space_single_line(ch) {
            break;
        }
        if ch == i32::from(b'\t') {
            // Tabs = TabSize = indent size and go to next tabStop
            current_line_indent += indent_size - (current_line_indent % indent_size);
        } else {
            // Single space
            current_line_indent += 1;
        }
        pos += size as i64;
    }
    current_line_indent
}

// port: tsc/internal/printer/utilities.go:canHaveDecorators
pub(crate) fn can_have_decorators(kind: tsr_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::Parameter
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassExpression
                | K::ClassDeclaration
        )
    )
}
