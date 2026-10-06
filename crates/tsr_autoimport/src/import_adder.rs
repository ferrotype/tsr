//! Request-local accumulation of import fixes. Names and type-only requirements
//! are coalesced before edits are placed against the original source.
use crate::edits::{self, Edit, Options};
use std::collections::BTreeMap;
use tsr_ast::{AstView, NodeId, SyntaxKind as K};
use tsr_checker::Error;
use tsr_lsproto as lsp;

#[derive(Default)]
pub struct ImportAdder {
    individual: Vec<lsp::AutoImportFix>,
    existing: BTreeMap<i32, Collection>,
    new: BTreeMap<(String, bool), Collection>,
}
#[derive(Default)]
struct Collection {
    default: Option<lsp::AutoImportFix>,
    named: BTreeMap<String, lsp::AutoImportFix>,
    namespace: Option<lsp::AutoImportFix>,
    require: bool,
}
fn merge(old: &mut Option<lsp::AutoImportFix>, mut new: lsp::AutoImportFix) {
    if let Some(old) = old {
        assert_eq!(old.name, new.name, "conflicting default/namespace imports");
        new.add_as_type_only.0 = new.add_as_type_only.0.max(old.add_as_type_only.0);
    }
    *old = Some(new);
}
impl Collection {
    fn add(&mut self, mut fix: lsp::AutoImportFix, verbatim: bool) {
        match fix.import_kind {
            lsp::ImportKind::DEFAULT => merge(&mut self.default, fix),
            lsp::ImportKind::NAMED | lsp::ImportKind::COMMON_JS
                if fix.import_kind == lsp::ImportKind::NAMED || verbatim =>
            {
                if let Some(old) = self.named.get(&fix.name) {
                    fix.add_as_type_only.0 = fix.add_as_type_only.0.max(old.add_as_type_only.0);
                }
                self.named.insert(fix.name.clone(), fix);
            }
            _ => self.namespace = Some(fix),
        }
    }
    fn bindings(&self) -> impl Iterator<Item = &lsp::AutoImportFix> {
        self.default.iter().chain(self.named.values())
    }
}
impl ImportAdder {
    // port: tsc/internal/ls/autoimport/import_adder.go:importAdder.HasFixes
    pub fn has_fixes(&self) -> bool {
        !self.individual.is_empty() || !self.existing.is_empty() || !self.new.is_empty()
    }
    // port: tsc/internal/ls/autoimport/import_adder.go:importAdder.AddImportFix
    pub fn add(&mut self, fix: lsp::AutoImportFix, verbatim: bool) {
        match fix.kind {
            lsp::AutoImportFixKind::USE_NAMESPACE | lsp::AutoImportFixKind::JSDOC_TYPE_IMPORT => {
                self.individual.push(fix);
            }
            lsp::AutoImportFixKind::ADD_TO_EXISTING => self
                .existing
                .entry(fix.import_index)
                .or_default()
                .add(fix, verbatim),
            lsp::AutoImportFixKind::ADD_NEW => {
                // port: tsc/internal/ls/autoimport/import_adder.go:importAdder.getNewImportEntry
                let typed = (fix.import_kind == lsp::ImportKind::DEFAULT
                    && fix.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED)
                    || (fix.add_as_type_only == lsp::AddAsTypeOnly::ALLOWED
                        && self.new.contains_key(&(fix.module_specifier.clone(), true)));
                let entry = self
                    .new
                    .entry((fix.module_specifier.clone(), typed))
                    .or_insert_with(|| Collection {
                        require: fix.use_require,
                        ..Default::default()
                    });
                assert_eq!(
                    entry.require, fix.use_require,
                    "mixed require/import fixes for one module"
                );
                entry.add(fix, verbatim);
            }
            lsp::AutoImportFixKind::PROMOTE_TYPE_ONLY => {} // Excluded from fix-all at the pin.
            _ => panic!("unexpected auto-import fix kind"),
        }
    }
    // port: tsc/internal/ls/autoimport/import_adder.go:importAdder.Edits
    pub fn edits(
        &self,
        view: AstView<'_>,
        source: NodeId,
        options: &Options<'_>,
    ) -> Result<Vec<Edit>, Error> {
        let mut result = Vec::new();
        for fix in &self.individual {
            result.extend(edits::edits(view, source, fix, options)?.0);
        }
        let mut statements = Vec::new();
        for ((module, _), collection) in &self.new {
            for (text, order) in new_statements(module, collection, options) {
                statements.push((
                    module.as_str(),
                    format_statement(&text, options)?,
                    collection.require,
                    order,
                ));
            }
        }
        if !statements.is_empty() {
            result.extend(insert_statements(view, source, statements, options)?);
        }
        result.sort_by_key(|e| (e.start, e.end));
        // ChangeTracker joins coincident insertions into one edit, in addition
        // order. Replacements are never silently combined or overlapped.
        let mut joined: Vec<Edit> = Vec::new();
        for edit in result {
            if let Some(last) = joined
                .last_mut()
                .filter(|e| e.start == e.end && edit.start == edit.end && e.start == edit.start)
            {
                last.text.push_str(&edit.text);
            } else {
                joined.push(edit);
            }
        }
        // Replacements at the same offset remain distinct, as in Tracker.
        // In particular a multi-line list adds a comma and then a new line.
        for (index, collection) in &self.existing {
            joined.extend(existing(view, source, *index, collection, options)?);
        }
        joined.sort_by_key(|e| (e.start, e.end));
        Ok(joined)
    }
}

// New imports have no original source trivia. Format a private parsed fragment
// with the same production formatter used for generated completion nodes. It
// never replaces or reformats text belonging to an existing import.
fn format_statement(text: &str, options: &Options<'_>) -> Result<String, Error> {
    let file = tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        tsr_ast::SourceFileParseOptions {
            file_name: tsr_jsstring::JsString::from_bytes(b"/autoimport.ts".as_slice()),
            path: tsr_jsstring::JsString::from_bytes(b"/autoimport.ts".as_slice()),
            ..Default::default()
        },
    )
    .publish_unbound();
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut input = tsr_format::FormatFile {
        view: file.view(),
        source: file
            .root()
            .ok_or(Error::MissingLink("formatted import source"))?,
        jsdoc: &mut provider,
    };
    let edits = tsr_format::format_document(
        &mut input,
        &tsr_format::FormatContext::new(options.format.clone(), options.newline.as_bytes()),
    )
    .map_err(format_error)?;
    let output = tsr_core::apply_bulk_edits(text.as_bytes(), &edits)
        .expect("formatter returns non-overlapping edits of its own input");
    Ok(String::from_utf8(output).expect("formatting preserves Unicode source"))
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ImportOrder {
    SideEffect,
    TypeOnly,
    Namespace,
    Default,
    Named,
    ImportEquals,
    Require,
}

// Same module names are ordered by import syntax at the pin; named imports
// precede import-equals declarations even when they refer to the same module.
fn import_order(view: AstView<'_>, id: NodeId) -> Result<ImportOrder, Error> {
    let read = view.node(id)?;
    if read.kind() == K::ImportEqualsDeclaration {
        return Ok(ImportOrder::ImportEquals);
    }
    let Some(clause) = read.import_clause() else {
        return Ok(ImportOrder::SideEffect);
    };
    let clause = view.node(clause)?;
    let data = clause.data_source();
    let clause_data = data
        .as_import_clause()
        .ok_or(Error::MissingLink("import clause"))?;
    Ok(if clause.is_type_only() {
        ImportOrder::TypeOnly
    } else if clause_data
        .named_bindings()
        .map(|id| view.node(id).map(|n| n.kind() == K::NamespaceImport))
        .transpose()?
        .unwrap_or(false)
    {
        ImportOrder::Namespace
    } else if clause.name().is_some() {
        ImportOrder::Default
    } else {
        ImportOrder::Named
    })
}

// port: tsc/internal/ls/autoimport/fix.go:getNewImports
// port: tsc/internal/ls/autoimport/fix.go:getNewRequires
fn new_statements(module: &str, c: &Collection, o: &Options<'_>) -> Vec<(String, ImportOrder)> {
    let module = edits::quote_module(module, o.single_quote);
    let mut result = Vec::new();
    if c.default.is_some() || !c.named.is_empty() {
        let typed = c
            .bindings()
            .all(|b| b.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED)
            || (o.verbatim || o.prefer_type_only)
                && c.bindings()
                    .all(|b| b.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED);
        if c.require {
            let names = c
                .default
                .iter()
                .map(|b| format!("default: {}", b.name))
                .chain(c.named.keys().cloned())
                .collect::<Vec<_>>()
                .join(", ");
            result.push((
                format!("const {{ {names} }} = require({module})"),
                ImportOrder::Require,
            ));
        } else {
            let mut names = String::new();
            if let Some(default) = &c.default {
                names.push_str(&default.name);
            }
            if !c.named.is_empty() {
                if !names.is_empty() {
                    names.push_str(", ");
                }
                let named = c
                    .named
                    .values()
                    .map(|b| {
                        format!(
                            "{}{}",
                            if !typed
                                && (b.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED
                                    || b.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED
                                        && o.prefer_type_only)
                            {
                                "type "
                            } else {
                                ""
                            },
                            b.name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                names.push_str(&format!("{{ {named} }}"));
            }
            result.push((
                format!(
                    "import {}{names} from {module}",
                    if typed { "type " } else { "" }
                ),
                if typed {
                    ImportOrder::TypeOnly
                } else if c.default.is_some() {
                    ImportOrder::Default
                } else {
                    ImportOrder::Named
                },
            ));
        }
    }
    if let Some(b) = &c.namespace {
        let typed = if b.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED
            || b.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED && o.prefer_type_only
        {
            "type "
        } else {
            ""
        };
        result.push(if c.require {
            (
                format!("const {} = require({module})", b.name),
                ImportOrder::Require,
            )
        } else if b.import_kind == lsp::ImportKind::COMMON_JS {
            (
                format!("import {typed}{} = require({module})", b.name),
                ImportOrder::ImportEquals,
            )
        } else {
            (
                format!("import {typed}* as {} from {module}", b.name),
                if typed.is_empty() {
                    ImportOrder::Namespace
                } else {
                    ImportOrder::TypeOnly
                },
            )
        });
    }
    // The pinned printer emits statement terminators before the tracker formats
    // the generated node. Auto-detected writing settings affect indentation;
    // only the original formatter preferences may remove this semicolon.
    for (statement, _) in &mut result {
        statement.push(';');
    }
    result
}
// port: tsc/internal/ls/autoimport/fix.go:insertImports
fn insert_statements(
    view: AstView<'_>,
    source: NodeId,
    mut new: Vec<(&str, String, bool, ImportOrder)>,
    o: &Options<'_>,
) -> Result<Vec<Edit>, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let mut old = Vec::new();
    for id in view
        .node_slice(view.node(source)?.statements(view)?)?
        .iter()
        .flatten()
    {
        let read = view.node(id)?;
        if new[0].2 {
            if !tsr_ast::utilities_modules::is_require_variable_statement(view, id)? {
                continue;
            }
            let list = read
                .data_source()
                .as_variable_statement()
                .and_then(|d| d.declaration_list())
                .ok_or(Error::MissingLink("require declarations"))?;
            let declarations = view
                .node(list)?
                .data_source()
                .as_variable_declaration_list()
                .and_then(|d| d.declarations())
                .ok_or(Error::MissingLink("require declarations"))?;
            let declaration = view
                .node_slice(view.list(declarations)?.nodes())?
                .iter()
                .flatten()
                .next();
            if let Some(declaration) = declaration {
                if let Some(initializer) = view.node(declaration)?.initializer() {
                    if let Some(arg) = view
                        .node_slice(view.node(initializer)?.arguments(view)?)?
                        .iter()
                        .flatten()
                        .next()
                    {
                        old.push((
                            id,
                            view.node_text(arg)?.as_bytes().to_vec(),
                            ImportOrder::Require,
                        ));
                    }
                }
            }
        } else if matches!(
            read.kind().known(),
            Some(K::ImportDeclaration | K::ImportEqualsDeclaration)
        ) {
            // ImportEqualsDeclaration stores its external module name under
            // ModuleReference, not ModuleSpecifier. The shared utility selects
            // the accessor by kind and skips internal aliases such as A = B.C.
            let literal = tsr_ast::utilities_modules::get_external_module_name(view, id)?;
            if let Some(literal) = literal {
                old.push((
                    id,
                    view.node_text(literal)?.as_bytes().to_vec(),
                    import_order(view, id)?,
                ));
            }
        }
    }
    let compare = |a: &[u8], b: &[u8]| tsr_jsstring::compare::compare_case_insensitive(a, b);
    new.sort_by(|a, b| compare(a.0.as_bytes(), b.0.as_bytes()).then(a.3.cmp(&b.3)));
    if old.is_empty() {
        let mut pos = edits::top_position(view, source)?;
        let mut original_pos = pos;
        // InsertAtTopOfFile advances beyond synthesized mapper headers to the
        // first writable span, using original coordinates for leading trivia.
        if let Some(map) = file.span_map() {
            for segment in map.segments() {
                if segment.kind != tsr_ast::span_map::KIND_VERBATIM
                    || i64::from(segment.virtual_end) <= pos
                {
                    continue;
                }
                pos = pos.max(i64::from(segment.virtual_start));
                original_pos =
                    i64::from(segment.original_start) + pos - i64::from(segment.virtual_start);
                break;
            }
        }
        let prefix = if original_pos != 0 { o.newline } else { "" };
        let extra = if text
            .get(pos as usize)
            .is_some_and(|b| matches!(b, b'\n' | b'\r'))
        {
            ""
        } else {
            o.newline
        };
        return Ok(vec![Edit {
            start: pos,
            end: pos,
            text: format!(
                "{prefix}{}{extra}{}",
                new.iter()
                    .map(|s| s.1.as_str())
                    .collect::<Vec<_>>()
                    .join(o.newline),
                o.newline
            ),
        }]);
    }
    let sorted = old
        .windows(2)
        .all(|w| !compare(&w[0].1, &w[1].1).then(w[0].2.cmp(&w[1].2)).is_gt());
    let mut result = Vec::new();
    for (module, statement, _, order) in new {
        let next = sorted
            .then(|| {
                old.iter().position(|(_, s, old_order)| {
                    compare(module.as_bytes(), s)
                        .then(order.cmp(old_order))
                        .is_lt()
                })
            })
            .flatten();
        let (pos, prefix) = if let Some(0) = next {
            (edits::start(view, source, old[0].0)?, "")
        } else {
            let prev = old[next.unwrap_or(old.len()) - 1].0;
            let end = edits::end(view, prev)? as usize;
            (
                edits::after_statement(text, end) as i64,
                if end == text.len() { o.newline } else { "" },
            )
        };
        result.push(Edit {
            start: pos,
            end: pos,
            text: format!("{prefix}{statement}{}", o.newline),
        });
    }
    Ok(result)
}

fn existing(
    view: AstView<'_>,
    source: NodeId,
    index: i32,
    c: &Collection,
    o: &Options<'_>,
) -> Result<Vec<Edit>, Error> {
    add_existing(
        view,
        source,
        index,
        c.default.as_ref(),
        &c.named.values().collect::<Vec<_>>(),
        o,
    )
}

// port: tsc/internal/ls/autoimport/fix.go:addToExistingImport
pub(crate) fn add_existing(
    view: AstView<'_>,
    source: NodeId,
    index: i32,
    default: Option<&lsp::AutoImportFix>,
    named: &[&lsp::AutoImportFix],
    o: &Options<'_>,
) -> Result<Vec<Edit>, Error> {
    let file = view.source_file(source)?;
    let literal = file
        .imports()?
        .get(index as usize)
        .copied()
        .flatten()
        .ok_or(Error::MissingLink("import adder index"))?;
    let declaration =
        tsr_ast::utilities_modules::try_get_import_from_module_specifier(view, literal)?
            .ok_or(Error::MissingLink("import adder declaration"))?;
    let read = view.node(declaration)?;
    let mut result = Vec::new();
    if read.kind() == K::CallExpression {
        let variable = read
            .parent()
            .ok_or(Error::MissingLink("require variable"))?;
        let pattern = view
            .node(variable)?
            .name()
            .ok_or(Error::MissingLink("require binding"))?;
        let read = view.node(pattern)?;
        assert_eq!(
            read.kind(),
            K::ObjectBindingPattern,
            "require auto-import needs object bindings"
        );
        let elements = view.node_slice(read.elements(view)?)?;
        let names = default
            .map(|d| format!("default: {}", d.name))
            .into_iter()
            .chain(named.iter().map(|n| n.name.clone()))
            .collect::<Vec<_>>();
        let elements: Vec<_> = elements.iter().flatten().collect();
        if elements.is_empty() {
            result.push(Edit {
                start: edits::start(view, source, pattern)?,
                end: edits::end(view, pattern)?,
                text: format_bindings(&names.join(", "), o)?,
            });
        } else {
            for name in names {
                result.extend(insert_named_at(
                    view,
                    source,
                    pattern,
                    &elements,
                    elements.len(),
                    &name,
                    o,
                )?);
            }
        }
        return Ok(result);
    }
    let clause = read
        .import_clause()
        .ok_or(Error::MissingLink("import adder clause"))?;
    let c = view.node(clause)?;
    let data = c
        .data_source()
        .as_import_clause()
        .ok_or(Error::MissingLink("import clause payload"))?;
    let binding = data.named_bindings();
    let typed_clause = c.is_type_only();
    let promote = typed_clause
        && default
            .into_iter()
            .chain(named.iter().copied())
            .any(|b| b.add_as_type_only == lsp::AddAsTypeOnly::NOT_ALLOWED);
    let elements = if let Some(b) =
        binding.filter(|&b| view.node(b).is_ok_and(|b| b.kind() == K::NamedImports))
    {
        view.node_slice(view.node(b)?.elements(view)?)?
            .iter()
            .flatten()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if let Some(default) = default {
        assert!(c.name().is_none(), "cannot add a second default import");
        let pos = edits::start(view, source, clause)?;
        result.push(Edit {
            start: pos,
            end: pos,
            text: format!("{}, ", default.name),
        });
    }
    if !named.is_empty() {
        let mut names = elements
            .iter()
            .map(|&id| {
                let e = view.node(id)?;
                Ok((
                    view.node_text(e.name().ok_or(tsr_arena::Error::InvalidGraph)?)?
                        .as_bytes()
                        .to_vec(),
                    e.is_type_only(),
                ))
            })
            .collect::<Result<Vec<_>, tsr_arena::Error>>()?;
        let (order, sorted) = edits::named_order(&names);
        if promote {
            for name in &mut names {
                name.1 = true;
            }
        }
        let sorted = sorted
            && names
                .windows(2)
                .all(|w| !order.compare((&w[0].0, w[0].1), (&w[1].0, w[1].1)).is_gt());
        let mut additions = named
            .iter()
            .map(|b| {
                (
                    b.name.as_str(),
                    (!typed_clause || promote)
                        && (b.add_as_type_only == lsp::AddAsTypeOnly::REQUIRED
                            || b.add_as_type_only != lsp::AddAsTypeOnly::NOT_ALLOWED
                                && o.prefer_type_only),
                )
            })
            .collect::<Vec<_>>();
        additions.sort_by(|a, b| order.compare((a.0.as_bytes(), a.1), (b.0.as_bytes(), b.1)));
        let render = |n: (&str, bool)| format!("{}{}", if n.1 { "type " } else { "" }, n.0);
        if elements.is_empty() {
            let text = format_bindings(
                &additions
                    .into_iter()
                    .map(render)
                    .collect::<Vec<_>>()
                    .join(", "),
                o,
            )?;
            if let Some(binding) = binding {
                result.push(Edit {
                    start: edits::start(view, source, binding)?,
                    end: edits::end(view, binding)?,
                    text,
                });
            } else {
                let pos = edits::end(view, c.name().ok_or(Error::MissingLink("import default"))?)?;
                result.push(Edit {
                    start: pos,
                    end: pos,
                    text: format!(", {text}"),
                });
            }
        } else {
            for name in additions {
                let index = if sorted {
                    names.iter().position(|n| {
                        order
                            .compare((name.0.as_bytes(), name.1), (&n.0, n.1))
                            .is_lt()
                    })
                } else {
                    None
                };
                result.extend(insert_named_at(
                    view,
                    source,
                    binding.expect("existing named imports"),
                    &elements,
                    index.unwrap_or(elements.len()),
                    &render(name),
                    o,
                )?);
            }
        }
    }
    if promote {
        let pos = edits::start(view, source, clause)?;
        let finish = tsr_scanner::skip_trivia(file.text().as_bytes(), pos + 4);
        result.push(Edit {
            start: pos,
            end: finish,
            text: String::new(),
        });
        for element in elements {
            if !view.node(element)?.is_type_only() {
                let pos = edits::start(view, source, element)?;
                result.push(Edit {
                    start: pos,
                    end: pos,
                    text: "type ".into(),
                });
            }
        }
    }
    result.sort_by_key(|e| (e.start, e.end));
    Ok(result)
}

// Import-specific use of Tracker.InsertImportSpecifierAtIndex and
// InsertNodeInListAfter: keep the original trivia and separate replacement
// records, including the pin's trailing-comment behavior.
fn insert_named_at(
    view: AstView<'_>,
    source: NodeId,
    binding: NodeId,
    elements: &[NodeId],
    index: usize,
    name: &str,
    o: &Options<'_>,
) -> Result<Vec<Edit>, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let line = |pos: i64| {
        tsr_jsstring::scanner_positions::compute_line_of_position(
            file.ecma_line_map(),
            pos as isize,
        )
    };
    let insertion = |at, text| Edit {
        start: at,
        end: at,
        text,
    };
    if index == 0 {
        let start = edits::start(view, source, elements[0])?;
        let import = view
            .node(binding)?
            .parent()
            .and_then(|p| view.node(p).ok()?.parent())
            .ok_or(Error::MissingLink("named import declaration"))?;
        let suffix = if line(start) == line(edits::start(view, source, import)?) {
            ", ".into()
        } else {
            let column_start = file.ecma_line_map()[line(start) as usize] as usize;
            format!(
                ",{}{}",
                o.newline,
                String::from_utf8_lossy(&text[column_start..start as usize])
            )
        };
        return Ok(vec![insertion(start, format!("{name}{suffix}"))]);
    }
    let after = elements[index - 1];
    let end = edits::end(view, after)?;
    if index < elements.len() {
        let comma = tsr_scanner::skip_trivia(text, end) as usize;
        if text.get(comma) != Some(&b',') {
            return Ok(Vec::new());
        }
        let start = tsr_scanner::skip_trivia_ex(
            text,
            i64::from(view.node(elements[index])?.pos()),
            Some(&tsr_scanner::SkipTriviaOptions {
                stop_after_line_break: false,
                stop_at_comments: true,
                ..Default::default()
            }),
        );
        return Ok(vec![insertion(
            start,
            format!(
                "{name},{}",
                String::from_utf8_lossy(&text[comma + 1..start as usize])
            ),
        )]);
    }
    let start = edits::start(view, source, after)?;
    let binding_read = view.node(binding)?;
    let data = binding_read.data_source();
    let list = data
        .as_named_imports()
        .and_then(|d| d.elements())
        .or_else(|| data.as_binding_pattern().and_then(|d| d.elements()))
        .ok_or(Error::MissingLink("import bindings list"))?;
    let loc = view.list(list)?.loc();
    let has_comment = tsr_scanner::get_trailing_comment_ranges(text, end)
        .next()
        .is_some();
    let multiline = has_comment
        || line(loc.pos()) != line(loc.end())
        || (index > 1 && line(start) != line(edits::start(view, source, elements[index - 2])?));
    if !multiline {
        return Ok(vec![insertion(end, format!(", {name}"))]);
    }
    let mut separator = String::from(",");
    // The pinned synthetic comma inherits trailing trivia at end+1.
    for comment in tsr_scanner::get_trailing_comment_ranges(text, end + 1) {
        separator.push(' ');
        separator.push_str(&String::from_utf8_lossy(
            &text[comment.loc.pos() as usize..comment.loc.end() as usize],
        ));
    }
    let mut at = tsr_scanner::skip_trivia_ex(
        text,
        end,
        Some(&tsr_scanner::SkipTriviaOptions {
            stop_after_line_break: true,
            ..Default::default()
        }),
    );
    while at > end && matches!(text[at as usize - 1], b'\n' | b'\r') {
        at -= 1;
    }
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let input = tsr_format::FormatFile {
        view,
        source,
        jsdoc: &mut provider,
    };
    let line_start = i64::from(file.ecma_line_map()[line(start) as usize]);
    let column = tsr_format::find_first_non_whitespace_column(&input, line_start, start, o.format)
        .map_err(format_error)?;
    let indent = tsr_format::get_indentation_string(column, o.format);
    Ok(vec![
        insertion(end, separator),
        insertion(
            at,
            format!(
                "{}{indent}{name}",
                o.newline,
                indent = String::from_utf8_lossy(&indent)
            ),
        ),
    ])
}
fn format_bindings(names: &str, o: &Options<'_>) -> Result<String, Error> {
    let formatted = format_statement(&format!("import {{ {names} }} from \"\";"), o)?;
    let start = formatted.find('{').expect("formatted named import");
    let end = formatted.rfind('}').expect("formatted named import");
    Ok(formatted[start..=end].into())
}
fn format_error(error: tsr_format::Error) -> Error {
    match error {
        tsr_format::Error::Storage(error) => Error::from(error),
        tsr_format::Error::Assertion(message) => panic!("{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_import(text: &str, module: &str, name: &str, kind: lsp::ImportKind) -> String {
        let file = tsr_parser::parse_source_file(
            tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
            tsr_core::ScriptKind::TS,
            tsr_ast::SourceFileParseOptions {
                file_name: tsr_jsstring::JsString::from_bytes(b"/main.ts".as_slice()),
                path: tsr_jsstring::JsString::from_bytes(b"/main.ts".as_slice()),
                ..Default::default()
            },
        )
        .publish_unbound();
        let mut adder = ImportAdder::default();
        adder.add(
            lsp::AutoImportFix {
                kind: lsp::AutoImportFixKind::ADD_NEW,
                import_kind: kind,
                module_specifier: module.into(),
                name: name.into(),
                ..Default::default()
            },
            false,
        );
        let changes = adder
            .edits(
                file.view(),
                file.root().unwrap(),
                &Options {
                    format: &tsr_format::FormatCodeSettings::default(),
                    locale: &tsr_locale::DEFAULT,
                    single_quote: true,
                    semicolons: true,
                    prefer_type_only: false,
                    verbatim: false,
                    newline: "\n",
                    usage: None,
                },
            )
            .unwrap();
        let mut result = text.to_owned();
        for change in changes.into_iter().rev() {
            result.replace_range(change.start as usize..change.end as usize, &change.text);
        }
        result
    }

    #[test]
    fn new_imports_select_external_import_equals_module_reference() {
        // Native TestImportNameCodeFixNewImportFileQuoteStyle2 insertion.
        assert_eq!(
            add_import(
                "import m2 = require('./module2');\n\nf1();",
                "./module1",
                "f1",
                lsp::ImportKind::NAMED,
            ),
            "import { f1 } from './module1';\nimport m2 = require('./module2');\n\nf1();",
        );
        // Native TestImportNameCodeFixNewImportExportEqualsCommonJSInteropOn:
        // both a pure import-equals list and one mixed with an ES import.
        for prefix in ["", "import es from 'es';\n"] {
            let text = format!("{prefix}import bar = require('bar');\n\nfoo");
            assert_eq!(
                add_import(&text, "foo", "foo", lsp::ImportKind::COMMON_JS),
                format!(
                    "{prefix}import bar = require('bar');\nimport foo = require('foo');\n\nfoo"
                ),
            );
        }
    }

    #[test]
    fn named_import_precedes_same_module_import_equals() {
        // The new-import alternative in TestImportNameCodeFixExistingImportEquals0
        // scans the existing import-equals declaration before placing the edit.
        let result = add_import(
            "import ns = require('ambient-module');\nvar x = v1 + 5;",
            "ambient-module",
            "v1",
            lsp::ImportKind::NAMED,
        );
        assert_eq!(
            result,
            "import { v1 } from 'ambient-module';\nimport ns = require('ambient-module');\nvar x = v1 + 5;"
        );
    }

    #[test]
    fn internal_import_equals_alias_has_no_external_module_name() {
        let text = "import Alias = Namespace.Member;\n\nvalue;";
        assert_eq!(
            add_import(text, "./dep", "value", lsp::ImportKind::NAMED),
            "import { value } from './dep';\n\nimport Alias = Namespace.Member;\n\nvalue;",
        );
    }
}

#[cfg(test)]
#[path = "import_semicolon_tests.rs"]
mod semicolon_tests;
