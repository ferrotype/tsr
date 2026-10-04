//! Import-specific text changes. Ranges refer to the retained source; the
//! language service projects every edit back to the client's document.
use tsr_ast::{AstView, NodeId, SyntaxKind as K};
use tsr_checker::Error;
use tsr_lsproto as lsp;

pub struct Options<'a> {
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
fn start(view: AstView<'_>, source: NodeId, id: NodeId) -> Result<i64, Error> {
    Ok(tsr_scanner::get_token_pos_of_node(
        view,
        source,
        id,
        false,
        &mut tsr_parser::ParserJsDocProvider::default(),
    )?)
}
fn end(view: AstView<'_>, id: NodeId) -> Result<i64, Error> {
    Ok(i64::from(view.node(id)?.end()))
}
fn type_only(fix: &lsp::AutoImportFix, options: &Options<'_>) -> bool {
    fix.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED
        || fix.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED
            && (options.prefer_type_only || options.verbatim)
}
pub fn edits(
    view: AstView<'_>,
    source: NodeId,
    fix: &lsp::AutoImportFix,
    options: &Options<'_>,
) -> Result<(Vec<Edit>, String), Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let quote = if options.single_quote { '\'' } else { '"' };
    let module = format!("{quote}{}{quote}", fix.module_specifier);
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
            let message = format!("Change '{}' to '{prefix}{}'", fix.name, fix.name);
            Ok((vec![Edit::insert(usage, prefix)], message))
        }
        lsp::AutoImportFixKind::ADD_NEW => {
            let as_type = if type_only(fix, options) { "type " } else { "" };
            let mut statement = if fix.use_require {
                match fix.import_kind {
                    lsp::ImportKind::NAMED => {
                        format!("const {{ {} }} = require({module})", fix.name)
                    }
                    lsp::ImportKind::DEFAULT => {
                        format!("const {{ default: {} }} = require({module})", fix.name)
                    }
                    _ => format!("const {} = require({module})", fix.name),
                }
            } else {
                match fix.import_kind {
                    lsp::ImportKind::NAMED => {
                        format!("import {as_type}{{ {} }} from {module}", fix.name)
                    }
                    lsp::ImportKind::DEFAULT => {
                        format!("import {as_type}{} from {module}", fix.name)
                    }
                    lsp::ImportKind::NAMESPACE => {
                        format!("import {as_type}* as {} from {module}", fix.name)
                    }
                    lsp::ImportKind::COMMON_JS => {
                        format!("import {as_type}{} = require({module})", fix.name)
                    }
                    _ => return Err(Error::MissingLink("auto-import kind")),
                }
            };
            // The pinned formatter defaults to semicolons, then detects their
            // absence from statement terminators in the current file.
            if options.semicolons {
                statement.push(';');
            }
            let mut imports = Vec::new();
            for id in view
                .node_slice(view.node(source)?.statements(view)?)?
                .iter()
                .flatten()
            {
                let read = view.node(id)?;
                if matches!(
                    read.kind().known(),
                    Some(K::ImportDeclaration | K::ImportEqualsDeclaration)
                ) {
                    let literal = read.module_specifier().or_else(|| {
                        read.data_source()
                            .as_import_equals_declaration()
                            .and_then(|d| d.module_reference())
                            .and_then(|n| view.node(n).ok()?.expression())
                    });
                    if let Some(literal) = literal {
                        imports.push((id, view.node_text(literal)?.as_bytes().to_vec()));
                    }
                }
            }
            let edit = if imports.is_empty() {
                let pos = top_position(view, source)?;
                let prefix = if pos != 0 { options.newline } else { "" };
                let extra = if text
                    .get(pos as usize)
                    .is_some_and(|b| matches!(b, b'\n' | b'\r'))
                {
                    ""
                } else {
                    options.newline
                };
                Edit::insert(
                    pos,
                    format!("{prefix}{statement}{extra}{}", options.newline),
                )
            } else {
                let compare =
                    |a: &[u8], b: &[u8]| tsr_jsstring::compare::compare_case_insensitive(a, b);
                let sorted = imports
                    .windows(2)
                    .all(|w| !compare(&w[0].1, &w[1].1).is_gt());
                let next = if sorted {
                    imports.iter().position(|(_, name)| {
                        compare(fix.module_specifier.as_bytes(), name).is_lt()
                    })
                } else {
                    None
                };
                if let Some(next) = next {
                    let pos = start(view, source, imports[next].0)?;
                    Edit::insert(pos, format!("{statement}{}", options.newline))
                } else {
                    let last = imports.last().unwrap().0;
                    let pos = after_statement(text, end(view, last)? as usize);
                    let prefix = if pos > 0
                        && text
                            .get(pos - 1)
                            .is_some_and(|b| matches!(b, b'\n' | b'\r'))
                    {
                        ""
                    } else {
                        options.newline
                    };
                    Edit::insert(
                        pos as i64,
                        format!("{prefix}{statement}{}", options.newline),
                    )
                }
            };
            Ok((
                vec![edit],
                format!("Add import from \"{}\"", fix.module_specifier),
            ))
        }
        lsp::AutoImportFixKind::ADD_TO_EXISTING => {
            let literal = file
                .imports()?
                .get(fix.import_index as usize)
                .copied()
                .flatten()
                .ok_or(Error::MissingLink("auto-import import index"))?;
            let declaration =
                tsr_ast::utilities_modules::try_get_import_from_module_specifier(view, literal)?
                    .ok_or(Error::MissingLink("auto-import declaration"))?;
            let read = view.node(declaration)?;
            let clause = read
                .import_clause()
                .ok_or(Error::MissingLink("auto-import clause"))?;
            let clause_read = view.node(clause)?;
            let data = clause_read
                .data_source()
                .as_import_clause()
                .ok_or(Error::MissingLink("auto-import clause payload"))?;
            let binding = data.named_bindings();
            let promote = (data.phase_modifier() == K::TypeKeyword)
                && fix.add_as_type_only == lsp::AddAsTypeOnly::NOT_ALLOWED;
            let mut edits = Vec::new();
            if fix.import_kind == lsp::ImportKind::DEFAULT {
                edits.push(Edit::insert(
                    start(view, source, clause)?,
                    format!("{}, ", fix.name),
                ));
            } else {
                let typed = (data.phase_modifier() != K::TypeKeyword || promote)
                    && (fix.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED
                        || fix.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED
                            && options.prefer_type_only);
                let name = format!("{}{}", if typed { "type " } else { "" }, fix.name);
                if let Some(binding) = binding {
                    let read = view.node(binding)?;
                    let elements: Vec<_> = view
                        .node_slice(read.elements(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    if elements.is_empty() {
                        edits.push(Edit {
                            start: start(view, source, binding)?,
                            end: end(view, binding)?,
                            text: format!("{{ {name} }}"),
                        });
                    } else {
                        let mut names: Vec<_> = elements
                            .iter()
                            .map(|&e| {
                                let read = view.node(e)?;
                                Ok((
                                    view.node_text(
                                        read.name().ok_or(tsr_arena::Error::InvalidGraph)?,
                                    )?
                                    .as_bytes()
                                    .to_vec(),
                                    read.data_source()
                                        .as_import_specifier()
                                        .is_some_and(|d| d.is_type_only()),
                                ))
                            })
                            .collect::<Result<_, tsr_arena::Error>>()?;
                        let (order, sorted) = named_order(&names);
                        if promote {
                            for name in &mut names {
                                name.1 = true;
                            }
                        }
                        let sorted = sorted
                            && names.windows(2).all(|w| {
                                !order.compare((&w[0].0, w[0].1), (&w[1].0, w[1].1)).is_gt()
                            });
                        if let Some(index) = sorted
                            .then(|| {
                                names.iter().position(|n| {
                                    order
                                        .compare((fix.name.as_bytes(), typed), (&n.0, n.1))
                                        .is_lt()
                                })
                            })
                            .flatten()
                        {
                            let pos = start(view, source, elements[index])?;
                            edits.push(Edit::insert(pos, format!("{name}, ")));
                        } else {
                            edits.push(Edit::insert(
                                end(view, *elements.last().unwrap())?,
                                format!(", {name}"),
                            ));
                        }
                        if promote {
                            for &element in &elements {
                                if !view
                                    .node(element)?
                                    .data_source()
                                    .as_import_specifier()
                                    .is_some_and(|d| d.is_type_only())
                                {
                                    edits.push(Edit::insert(
                                        start(view, source, element)?,
                                        "type ".into(),
                                    ));
                                }
                            }
                        }
                    }
                } else {
                    let default = clause_read
                        .name()
                        .ok_or(Error::MissingLink("auto-import default binding"))?;
                    edits.push(Edit::insert(end(view, default)?, format!(", {{ {name} }}")));
                }
            }
            if promote {
                let pos = start(view, source, clause)?;
                let finish = tsr_scanner::skip_trivia(text, pos + 4);
                edits.push(Edit {
                    start: pos,
                    end: finish,
                    text: String::new(),
                });
            }
            edits.sort_by_key(|e| e.start);
            Ok((
                edits,
                format!("Update import from \"{}\"", fix.module_specifier),
            ))
        }
        _ => Err(Error::MissingLink("auto-import edit kind")),
    }
}
fn after_statement(text: &[u8], mut pos: usize) -> usize {
    while pos < text.len() && matches!(text[pos], b' ' | b'\t') {
        pos += 1;
    }
    if text.get(pos..pos + 2) == Some(b"//") {
        while pos < text.len() && !matches!(text[pos], b'\n' | b'\r') {
            pos += 1;
        }
    }
    if text.get(pos) == Some(&b'\r') {
        pos += 1;
    }
    if text.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    pos
}
fn top_position(view: AstView<'_>, source: NodeId) -> Result<i64, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let mut pos = 0;
    if text.starts_with(b"#!") {
        pos = text
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .unwrap_or(text.len());
    }
    for node in view
        .node_slice(view.node(source)?.statements(view)?)?
        .iter()
        .flatten()
    {
        let n = view.node(node)?;
        if n.kind() == K::ExpressionStatement
            && n.expression()
                .is_some_and(|e| view.node(e).is_ok_and(|e| e.kind() == K::StringLiteral))
        {
            pos = n.end() as usize;
        } else {
            break;
        }
    }
    Ok(pos as i64)
}

#[derive(Clone, Copy)]
struct NamedOrder {
    ignore_case: bool,
    types: u8,
}
impl NamedOrder {
    fn compare(self, a: (&[u8], bool), b: (&[u8], bool)) -> std::cmp::Ordering {
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
fn named_order(names: &[(Vec<u8>, bool)]) -> (NamedOrder, bool) {
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
