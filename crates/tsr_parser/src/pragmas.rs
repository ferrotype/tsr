use crate::Parser;
use std::collections::BTreeMap;
use ts_ast::{
    AstBuilder, CheckJsDirective, CommentRange, FileReference, JsString, NodeId, Pragma,
    PragmaArgument, SyntaxKind as K,
};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

/// port: tsc/internal/parser/parser.go:getCommentPragmas
pub(crate) fn get_comment_pragmas(text: &[u8]) -> Vec<Pragma> {
    let mut pragmas = vec![];
    for comment in ts_scanner::get_leading_comment_ranges(text, 0) {
        let range = CommentRange {
            loc: comment.loc,
            kind: comment.kind.into(),
            has_trailing_new_line: comment.has_trailing_new_line,
        };
        pragmas.extend(extract_pragmas(
            range,
            &text[range.loc.pos() as usize..range.loc.end() as usize],
        ));
    }
    pragmas
}

/// port: tsc/internal/parser/parser.go:extractPragmas
fn extract_pragmas(range: CommentRange, mut text: &[u8]) -> Vec<Pragma> {
    if range.kind == K::SingleLineCommentTrivia {
        let mut pos = 2;
        let triple_slash = matches_text(text, pos, b"/");
        if triple_slash {
            pos += 1;
        }
        pos = skip_blanks(text, pos);
        if triple_slash && matches_text(text, pos, b"<") {
            if extract_name(text, pos + 1) != b"reference" {
                return vec![];
            }
            pos += 10;
            let mut args = BTreeMap::new();
            loop {
                pos = skip_blanks(text, pos);
                if matches_text(text, pos, b"/>") {
                    break;
                }
                let name = extract_name(text, pos);
                if name.is_empty() {
                    break;
                }
                pos = skip_blanks(text, pos + name.len());
                if !matches_text(text, pos, b"=") {
                    break;
                }
                pos = skip_blanks(text, pos + 1);
                let Some(value) = extract_quoted_string(text, pos) else {
                    break;
                };
                let name = JsString::from_bytes(name);
                let start = range.loc.pos() + pos as i64 + 1;
                args.insert(
                    name.clone(),
                    PragmaArgument {
                        loc: TextRange::new(start, start + value.len() as i64),
                        name,
                        value: JsString::from_bytes(value),
                    },
                );
                pos += value.len() + 2;
            }
            return vec![Pragma {
                range,
                name: JsString::from_bytes(b"reference".as_slice()),
                args,
            }];
        }
        if matches_text(text, pos, b"@") {
            let name = extract_name(text, pos + 1);
            if name != b"ts-check" && name != b"ts-nocheck" {
                return vec![];
            }
            return vec![Pragma {
                range,
                name: JsString::from_bytes(name),
                args: BTreeMap::new(),
            }];
        }
    }
    if range.kind == K::MultiLineCommentTrivia {
        if let Some(stripped) = text.strip_suffix(b"*/") {
            text = stripped;
        }
        let mut pos = 2;
        let mut pragmas = vec![];
        while let Some(at) = skip_to(text, pos, b"@") {
            pos = at;
            let name_pos = pos + 1;
            let name_end = skip_non_blanks(text, name_pos);
            if name_end == name_pos {
                pos += 1;
                continue;
            }
            let line_end = line_end_pos(text, pos);
            let name = ts_jsstring::helpers::to_lower_go(&text[name_pos..name_end]);
            if matches!(
                name.as_slice(),
                b"jsx" | b"jsxfrag" | b"jsximportsource" | b"jsxruntime"
            ) {
                let start = skip_blanks(text, name_end);
                let end = skip_non_blanks(text, start);
                if end != start {
                    let mut args = BTreeMap::new();
                    let key = JsString::from_bytes(b"factory".as_slice());
                    args.insert(
                        key.clone(),
                        PragmaArgument {
                            loc: TextRange::new(
                                range.loc.pos() + start as i64,
                                range.loc.pos() + end as i64,
                            ),
                            name: key,
                            value: JsString::from_bytes(&text[start..end]),
                        },
                    );
                    pragmas.push(Pragma {
                        range,
                        name: JsString::from_bytes(name),
                        args,
                    });
                }
            }
            pos = line_end;
        }
        return pragmas;
    }
    vec![]
}

/// port: tsc/internal/parser/parser.go:match
fn matches_text(text: &[u8], pos: usize, needle: &[u8]) -> bool {
    text[pos..].starts_with(needle)
}
/// port: tsc/internal/parser/parser.go:skipBlanks
fn skip_blanks(text: &[u8], mut pos: usize) -> usize {
    while pos < text.len() && matches!(text[pos], b' ' | b'\t') {
        pos += 1;
    }
    pos
}
/// port: tsc/internal/parser/parser.go:skipNonBlanks
fn skip_non_blanks(text: &[u8], mut pos: usize) -> usize {
    while pos < text.len() && !matches!(text[pos], b' ' | b'\t' | b'\r' | b'\n') {
        pos += 1;
    }
    pos
}
/// port: tsc/internal/parser/parser.go:skipTo
fn skip_to(text: &[u8], pos: usize, needle: &[u8]) -> Option<usize> {
    if pos >= text.len() {
        return None;
    }
    if needle.is_empty() {
        return Some(pos);
    }
    text[pos..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|index| pos + index)
}
/// port: tsc/internal/parser/parser.go:lineEndPos
fn line_end_pos(text: &[u8], mut pos: usize) -> usize {
    while pos < text.len() {
        let (rune, size) = ts_jsstring::wtf8::decode_utf8(&text[pos..]);
        if matches!(rune, 10 | 13 | 0x2028 | 0x2029) {
            return pos;
        }
        pos += size;
    }
    text.len()
}
/// port: tsc/internal/parser/parser.go:extractName
fn extract_name(text: &[u8], mut pos: usize) -> Vec<u8> {
    let start = pos;
    while pos < text.len() && (text[pos].is_ascii_alphabetic() || text[pos] == b'-') {
        pos += 1;
    }
    text[start..pos].to_ascii_lowercase()
}
/// port: tsc/internal/parser/parser.go:extractQuotedString
fn extract_quoted_string(text: &[u8], mut pos: usize) -> Option<&[u8]> {
    if pos == text.len() {
        return None;
    }
    let quote = text[pos];
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    pos += 1;
    let start = pos;
    while pos < text.len() && text[pos] != quote {
        pos += 1;
    }
    (pos < text.len()).then(|| &text[start..pos])
}

impl Parser<'_, AstBuilder> {
    /// port: tsc/internal/parser/parser.go:Parser.processPragmasIntoFields
    pub(crate) fn process_pragmas_into_fields(&mut self, root: NodeId) {
        let pragmas = self
            .factory
            .view()
            .source_file(root)
            .expect("source metadata")
            .pragmas()
            .expect("owned pragmas")
            .to_vec();
        let mut check_js = None;
        let mut referenced = vec![];
        let mut types = vec![];
        let mut libs = vec![];
        for pragma in pragmas {
            match pragma.name.as_bytes() {
                b"reference" => {
                    let get = |key: &[u8]| pragma.args.get(&JsString::from_bytes(key));
                    let preserve =
                        get(b"preserve").is_some_and(|arg| arg.value.as_bytes() == b"true");
                    if get(b"no-default-lib").is_some_and(|arg| arg.value.as_bytes() == b"true") {
                        continue;
                    }
                    if let Some(arg) = get(b"types") {
                        let resolution_mode = get(b"resolution-mode").map_or(0, |mode| {
                            self.parse_resolution_mode(
                                mode.value.as_bytes(),
                                mode.loc.pos(),
                                mode.loc.end(),
                            )
                        });
                        types.push(FileReference {
                            loc: arg.loc,
                            file_name: arg.value.clone(),
                            resolution_mode,
                            preserve,
                        });
                    } else if let Some(arg) = get(b"lib") {
                        libs.push(FileReference {
                            loc: arg.loc,
                            file_name: arg.value.clone(),
                            resolution_mode: 0,
                            preserve,
                        });
                    } else if let Some(arg) = get(b"path") {
                        referenced.push(FileReference {
                            loc: arg.loc,
                            file_name: arg.value.clone(),
                            resolution_mode: 0,
                            preserve,
                        });
                    } else {
                        self.parse_error_at_range(
                            pragma.range.loc,
                            diagnostics::Invalid_reference_directive_syntax,
                            vec![],
                        );
                    }
                }
                b"ts-check" | b"ts-nocheck" => {
                    if check_js.is_none_or(|directive: CheckJsDirective| {
                        pragma.range.loc.pos() > directive.range.loc.pos()
                    }) {
                        check_js = Some(CheckJsDirective {
                            enabled: pragma.name.as_bytes() == b"ts-check",
                            range: pragma.range,
                        });
                    }
                }
                b"jsx" | b"jsxfrag" | b"jsximportsource" | b"jsxruntime" => {}
                _ => panic!(
                    "Unhandled pragma kind: {}",
                    String::from_utf8_lossy(pragma.name.as_bytes())
                ),
            }
        }
        let referenced = if referenced.is_empty() {
            ts_ast::ReferenceSlice::empty()
        } else {
            self.factory
                .source_references(referenced)
                .expect("owned references")
        };
        let types = if types.is_empty() {
            ts_ast::ReferenceSlice::empty()
        } else {
            self.factory
                .source_references(types)
                .expect("owned type references")
        };
        let libs = if libs.is_empty() {
            ts_ast::ReferenceSlice::empty()
        } else {
            self.factory
                .source_references(libs)
                .expect("owned library references")
        };
        let file = self.factory.source_file_mut(root).expect("source metadata");
        file.check_js_directive = check_js;
        file.referenced_files = referenced;
        file.type_reference_directives = types;
        file.lib_reference_directives = libs;
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseResolutionMode
    pub(crate) fn parse_resolution_mode(&mut self, mode: &[u8], pos: i64, end: i64) -> i64 {
        match mode {
            b"import" => 99,
            b"require" => 1,
            _ => {
                self.parse_error_at(
                    pos,
                    end,
                    diagnostics::X_resolution_mode_should_be_either_require_or_import,
                    vec![],
                );
                0
            }
        }
    }
}
