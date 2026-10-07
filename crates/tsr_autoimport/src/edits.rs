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
    pub specifiers: &'a SpecifierPreferences,
}

/// A configured organize-imports string comparer.
pub type StringComparer = std::sync::Arc<dyn Fn(&[u8], &[u8]) -> std::cmp::Ordering + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeOrder {
    Last,
    Inline,
    First,
}
impl TypeOrder {
    fn index(self) -> usize {
        match self {
            Self::Last => 0,
            Self::Inline => 1,
            Self::First => 2,
        }
    }
}

/// The organize-imports preferences that select a named-specifier comparer.
/// The language service derives them from the user's settings.
// port: tsc/internal/ls/lsutil/organizeimports.go:GetDetectionLists
#[derive(Clone)]
pub struct SpecifierPreferences {
    /// `comparersToTest`; the first is the comparer used without detection.
    pub comparers: Vec<StringComparer>,
    /// `typeOrdersToTest`.
    pub type_orders: Vec<TypeOrder>,
    /// The configured type order, or `None` for `auto`.
    pub type_order: Option<TypeOrder>,
    /// Whether the resolved sort or the type order is `auto`.
    pub detect: bool,
}
impl Default for SpecifierPreferences {
    fn default() -> Self {
        Self {
            comparers: vec![
                std::sync::Arc::new(tsr_jsstring::compare::compare_case_insensitive_eslint),
                std::sync::Arc::new(tsr_jsstring::compare::compare_case_sensitive),
            ],
            type_orders: vec![TypeOrder::Last, TypeOrder::Inline, TypeOrder::First],
            type_order: None,
            detect: true,
        }
    }
}

/// A named import or export specifier: its local name and type-only marker.
pub(crate) type Specifier = (Vec<u8>, bool);

#[derive(Clone)]
pub(crate) struct SpecifierOrder {
    compare: StringComparer,
    types: TypeOrder,
}
impl SpecifierOrder {
    // port: tsc/internal/ls/lsutil/organizeimports.go:compareImportOrExportSpecifiers
    pub(crate) fn compare(&self, a: (&[u8], bool), b: (&[u8], bool)) -> std::cmp::Ordering {
        let types = match self.types {
            TypeOrder::First => b.1.cmp(&a.1),
            TypeOrder::Inline => std::cmp::Ordering::Equal,
            TypeOrder::Last => a.1.cmp(&b.1),
        };
        types.then_with(|| (self.compare)(a.0, b.0))
    }
    fn unsorted_pairs(&self, list: &[Specifier]) -> usize {
        list.windows(2)
            .filter(|w| self.compare((&w[0].0, w[0].1), (&w[1].0, w[1].1)).is_gt())
            .count()
    }
}

// port: tsc/internal/ls/lsutil/organizeimports.go:detectNamedImportOrganizationBySort
fn detect_named(
    lists: &[Vec<Specifier>],
    p: &SpecifierPreferences,
) -> Option<(SpecifierOrder, bool)> {
    let lists: Vec<_> = lists.iter().filter(|list| !list.is_empty()).collect();
    if lists.is_empty() {
        return None;
    }
    let mixed = lists
        .iter()
        .any(|list| list.iter().any(|s| s.1) && list.iter().any(|s| !s.1));
    if !mixed || p.type_orders.is_empty() {
        // detectCaseSensitivityBySort over the names alone.
        let mut best: Option<(usize, &StringComparer)> = None;
        for comparer in &p.comparers {
            let diff = lists
                .iter()
                .filter(|list| list.len() > 1)
                .map(|list| {
                    list.windows(2)
                        .filter(|w| comparer(&w[0].0, &w[1].0).is_gt())
                        .count()
                })
                .sum();
            if best.is_none_or(|(old, _)| diff < old) {
                best = Some((diff, comparer));
            }
        }
        let (diff, comparer) = best?;
        let types = if p.type_orders.len() == 1 {
            p.type_orders[0]
        } else {
            TypeOrder::Last
        };
        return Some((
            SpecifierOrder {
                compare: comparer.clone(),
                types,
            },
            diff == 0,
        ));
    }
    let mut best_diff = [usize::MAX; 3];
    let mut best_comparer = [
        p.comparers[0].clone(),
        p.comparers[0].clone(),
        p.comparers[0].clone(),
    ];
    for comparer in &p.comparers {
        let mut current = [0; 3];
        for list in &lists {
            for &types in &p.type_orders {
                let order = SpecifierOrder {
                    compare: comparer.clone(),
                    types,
                };
                current[types.index()] += order.unsorted_pairs(list);
            }
        }
        for &types in &p.type_orders {
            if current[types.index()] < best_diff[types.index()] {
                best_diff[types.index()] = current[types.index()];
                best_comparer[types.index()] = comparer.clone();
            }
        }
    }
    let chosen = p
        .type_orders
        .iter()
        .copied()
        .find(|best| {
            p.type_orders
                .iter()
                .all(|test| best_diff[test.index()] >= best_diff[best.index()])
        })
        .unwrap_or(TypeOrder::Last);
    Some((
        SpecifierOrder {
            compare: best_comparer[chosen.index()].clone(),
            types: chosen,
        },
        best_diff[chosen.index()] == 0,
    ))
}

/// The comparer for new named specifiers and whether the existing ones are
/// sorted by it (`None` when no detection ran). `declaration` is `None` when
/// the clause does not belong to an import declaration.
// port: tsc/internal/ls/lsutil/organizeimports.go:GetNamedImportSpecifierComparerWithDetection
pub(crate) fn specifier_order(
    p: &SpecifierPreferences,
    declaration: Option<&[Specifier]>,
    file: impl FnOnce() -> Result<Vec<Vec<Specifier>>, Error>,
) -> Result<(SpecifierOrder, Option<bool>), Error> {
    let initial = SpecifierOrder {
        compare: p.comparers[0].clone(),
        types: p.type_order.unwrap_or(TypeOrder::Last),
    };
    if p.detect {
        if let Some(declaration) = declaration {
            let detected = match detect_named(&[declaration.to_vec()], p) {
                Some(detected) => Some(detected),
                None => detect_named(&file()?, p),
            };
            if let Some((order, sorted)) = detected {
                return Ok((order, Some(sorted)));
            }
        }
    }
    Ok((initial, None))
}

// port: tsc/internal/ls/lsutil/organizeimports.go:GetImportSpecifierInsertionIndex
pub(crate) fn specifier_insertion_index(
    sorted: &[Specifier],
    new: (&[u8], bool),
    order: &SpecifierOrder,
) -> usize {
    // core.BinarySearchUniqueFunc: an equal element yields its own index.
    let (mut low, mut high) = (0_isize, sorted.len() as isize - 1);
    while low <= high {
        let middle = low + ((high - low) >> 1);
        let value = &sorted[middle as usize];
        match order.compare((&value.0, value.1), new) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle - 1,
            std::cmp::Ordering::Equal => return middle as usize,
        }
    }
    low as usize
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
    // Only a shebang line is followed past its line break; a file that starts
    // with an empty line keeps the insertion at offset zero.
    let shebang = tsr_scanner::get_shebang(text).len();
    let pos = if shebang == 0 { 0 } else { advance(shebang) };
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
