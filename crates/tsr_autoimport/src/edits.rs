//! Import-specific text changes. Ranges refer to the retained source; the
//! language service projects every edit back to the client's document.
use tsr_ast::{AstView, NodeId, SyntaxKind as K};
use tsr_checker::Error;
use tsr_lsproto as lsp;

pub struct Options<'a> {
    pub format: &'a tsr_format::FormatCodeSettings,
    pub locale: &'a tsr_locale::Locale,
    pub single_quote: bool,
    pub semicolons: bool,
    pub prefer_type_only: bool,
    pub verbatim: bool,
    pub newline: &'a str,
    pub usage: Option<i64>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: i64,
    pub end: i64,
    pub text: String,
}
impl Edit {
    fn insert(at: i64, text: String) -> Self {
        Self {
            start: at,
            end: at,
            text,
        }
    }
}
pub(crate) fn start(view: AstView<'_>, source: NodeId, id: NodeId) -> Result<i64, Error> {
    Ok(tsr_scanner::get_token_pos_of_node(
        view,
        source,
        id,
        false,
        &mut tsr_parser::ParserJsDocProvider::default(),
    )?)
}
pub(crate) fn end(view: AstView<'_>, id: NodeId) -> Result<i64, Error> {
    Ok(i64::from(view.node(id)?.end()))
}
pub fn edits(
    view: AstView<'_>,
    source: NodeId,
    fix: &lsp::AutoImportFix,
    options: &Options<'_>,
) -> Result<(Vec<Edit>, String), Error> {
    let module = quote_module(&fix.module_specifier, options.single_quote);
    let description = |message, args: &[&[u8]]| {
        String::from_utf8_lossy(&tsr_diagnostics::localize(
            options.locale,
            Some(message),
            b"",
            args,
        ))
        .into_owned()
    };
    match fix.kind {
        lsp::AutoImportFixKind::USE_NAMESPACE | lsp::AutoImportFixKind::JSDOC_TYPE_IMPORT => {
            let usage = options
                .usage
                .ok_or(Error::MissingLink("auto-import usage position"))?;
            let prefix = if fix.kind == lsp::AutoImportFixKind::USE_NAMESPACE {
                format!("{}.", fix.namespace_prefix)
            } else {
                format!("import({module}).")
            };
            let message = description(
                tsr_diagnostics::Change_0_to_1,
                &[
                    fix.name.as_bytes(),
                    format!("{prefix}{}", fix.name).as_bytes(),
                ],
            );
            Ok((vec![Edit::insert(usage, prefix)], message))
        }
        lsp::AutoImportFixKind::ADD_NEW => {
            let mut adder = crate::ImportAdder::default();
            adder.add(fix.clone(), options.verbatim);
            Ok((
                adder.edits(view, source, options)?,
                description(
                    tsr_diagnostics::Add_import_from_0,
                    &[fix.module_specifier.as_bytes()],
                ),
            ))
        }
        lsp::AutoImportFixKind::ADD_TO_EXISTING => {
            let default = (fix.import_kind == lsp::ImportKind::DEFAULT).then_some(fix);
            let named = if fix.import_kind == lsp::ImportKind::NAMED {
                vec![fix]
            } else {
                Vec::new()
            };
            Ok((
                crate::import_adder::add_existing(
                    view,
                    source,
                    fix.import_index,
                    default,
                    &named,
                    options,
                )?,
                description(
                    tsr_diagnostics::Update_import_from_0,
                    &[fix.module_specifier.as_bytes()],
                ),
            ))
        }
        _ => Err(Error::MissingLink("auto-import edit kind")),
    }
}
// Source: change.Tracker.getAdjustedEndPosition with TrailingTriviaOptionNone.
pub(crate) fn after_statement(text: &[u8], pos: usize) -> usize {
    let new_end = tsr_scanner::skip_trivia_ex(
        text,
        pos as i64,
        Some(&tsr_scanner::SkipTriviaOptions {
            stop_after_line_break: true,
            ..Default::default()
        }),
    ) as usize;
    if new_end > pos && matches!(text.get(new_end - 1), Some(b'\n' | b'\r')) {
        new_end
    } else {
        pos
    }
}
// port: tsc/internal/ls/change/trackerimpl.go:Tracker.getInsertionPositionAtSourceFileTop
pub(crate) fn top_position(view: AstView<'_>, source: NodeId) -> Result<i64, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let advance = |mut pos: usize| {
        if text.get(pos) == Some(&b'\r') {
            pos += 1;
            if text.get(pos) == Some(&b'\n') {
                pos += 1;
            }
        } else if text.get(pos) == Some(&b'\n') {
            pos += 1;
        }
        pos
    };
    let statements = view.node_slice(view.node(source)?.statements(view)?)?;
    let mut last = None;
    for node in statements.iter().flatten() {
        let n = view.node(node)?;
        if n.kind() == K::ExpressionStatement
            && n.expression()
                .is_some_and(|e| view.node(e).is_ok_and(|e| e.kind() == K::StringLiteral))
        {
            last = Some(n.end() as usize);
        } else {
            break;
        }
    }
    if let Some(last) = last {
        return Ok(advance(last) as i64);
    }
    let pos = advance(tsr_scanner::get_shebang(text).len());
    let line = |pos: i64| {
        tsr_jsstring::scanner_positions::compute_line_of_position(
            file.ecma_line_map(),
            pos as isize,
        )
    };
    let first = statements
        .iter()
        .flatten()
        .next()
        .map(|n| start(view, source, n))
        .transpose()?;
    let mut last: Option<tsr_scanner::CommentRange> = None;
    let mut pinned = false;
    for comment in tsr_scanner::get_leading_comment_ranges(text, pos as i64) {
        if tsr_printer::is_pinned_comment(text, &comment)
            || tsr_printer::is_recognized_triple_slash_comment(text, &comment)
        {
            last = Some(comment);
            pinned = true;
            continue;
        }
        if let Some(last) = &last {
            if pinned || line(comment.loc.pos()) >= line(last.loc.end()) + 2 {
                break;
            }
        }
        if first.is_some_and(|first| line(first) < line(comment.loc.end()) + 2) {
            break;
        }
        last = Some(comment);
        pinned = false;
    }
    Ok(last.map_or(pos, |last| advance(last.loc.end() as usize)) as i64)
}

#[derive(Clone, Copy)]
pub(crate) struct NamedOrder {
    ignore_case: bool,
    types: u8,
}
impl NamedOrder {
    pub(crate) fn compare(self, a: (&[u8], bool), b: (&[u8], bool)) -> std::cmp::Ordering {
        let types = match self.types {
            0 => a.1.cmp(&b.1),
            2 => b.1.cmp(&a.1),
            _ => std::cmp::Ordering::Equal,
        };
        types.then_with(|| {
            if self.ignore_case {
                tsr_jsstring::compare::compare_case_insensitive(a.0, b.0)
            } else {
                tsr_jsstring::compare::compare_case_sensitive(a.0, b.0)
            }
        })
    }
}
// Detection order from lsutil/organizeimports.go: type-last, inline, type-first;
// ties prefer case-insensitive. Detect on original specifiers before promotion.
pub(crate) fn named_order(names: &[(Vec<u8>, bool)]) -> (NamedOrder, bool) {
    let mixed = names.iter().any(|n| n.1) && names.iter().any(|n| !n.1);
    let mut best = (
        usize::MAX,
        NamedOrder {
            ignore_case: true,
            types: 0,
        },
    );
    for types in 0..(if mixed { 3 } else { 1 }) {
        for ignore_case in [true, false] {
            let order = NamedOrder { ignore_case, types };
            let diff = names
                .windows(2)
                .filter(|w| order.compare((&w[0].0, w[0].1), (&w[1].0, w[1].1)).is_gt())
                .count();
            if diff < best.0 {
                best = (diff, order);
            }
        }
    }
    (best.1, best.0 == 0)
}

pub fn quote_module(name: &str, single: bool) -> String {
    let quote = if single {
        tsr_jsstring::QuoteChar::Single
    } else {
        tsr_jsstring::QuoteChar::Double
    };
    let ch = if single { '\'' } else { '"' };
    format!(
        "{ch}{}{ch}",
        String::from_utf8_lossy(&tsr_jsstring::escape::escape_string(name.as_bytes(), quote))
    )
}
