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
        for (index, collection) in &self.existing {
            result.extend(existing(view, source, *index, collection, options)?);
        }
        let mut statements = Vec::new();
        for ((module, _), collection) in &self.new {
            for text in new_statements(module, collection, options) {
                statements.push((module.as_str(), text, collection.require));
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
        Ok(joined)
    }
}
// port: tsc/internal/ls/autoimport/fix.go:getNewImports
// port: tsc/internal/ls/autoimport/fix.go:getNewRequires
fn new_statements(module: &str, c: &Collection, o: &Options<'_>) -> Vec<String> {
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
            result.push(format!("const {{ {names} }} = require({module})"));
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
            result.push(format!(
                "import {}{names} from {module}",
                if typed { "type " } else { "" }
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
            format!("const {} = require({module})", b.name)
        } else if b.import_kind == lsp::ImportKind::COMMON_JS {
            format!("import {typed}{} = require({module})", b.name)
        } else {
            format!("import {typed}* as {} from {module}", b.name)
        });
    }
    if o.semicolons {
        for statement in &mut result {
            statement.push(';');
        }
    }
    result
}
// port: tsc/internal/ls/autoimport/fix.go:insertImports
fn insert_statements(
    view: AstView<'_>,
    source: NodeId,
    mut new: Vec<(&str, String, bool)>,
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
                        old.push((id, view.node_text(arg)?.as_bytes().to_vec()));
                    }
                }
            }
        } else if matches!(
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
                old.push((id, view.node_text(literal)?.as_bytes().to_vec()));
            }
        }
    }
    let compare = |a: &[u8], b: &[u8]| tsr_jsstring::compare::compare_case_insensitive(a, b);
    new.sort_by(|a, b| compare(a.0.as_bytes(), b.0.as_bytes()));
    if old.is_empty() {
        let pos = edits::top_position(view, source)?;
        let prefix = if pos != 0 { o.newline } else { "" };
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
    let sorted = old.windows(2).all(|w| !compare(&w[0].1, &w[1].1).is_gt());
    let mut result = Vec::new();
    for (module, statement, _) in new {
        let next = sorted
            .then(|| {
                old.iter()
                    .position(|(_, s)| compare(module.as_bytes(), s).is_lt())
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
        if let Some(last) = elements.iter().flatten().last() {
            result.push(Edit {
                start: edits::end(view, last)?,
                end: edits::end(view, last)?,
                text: format!(", {}", names.join(", ")),
            });
        } else {
            result.push(Edit {
                start: edits::start(view, source, pattern)?,
                end: edits::end(view, pattern)?,
                text: format!("{{ {} }}", names.join(", ")),
            });
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
            let text = format!(
                "{{ {} }}",
                additions
                    .into_iter()
                    .map(render)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
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
                let (pos, text) = if let Some(index) = index {
                    (
                        edits::start(view, source, elements[index])?,
                        format!("{}, ", render(name)),
                    )
                } else {
                    (
                        edits::end(view, *elements.last().unwrap())?,
                        format!(", {}", render(name)),
                    )
                };
                result.push(Edit {
                    start: pos,
                    end: pos,
                    text,
                });
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
