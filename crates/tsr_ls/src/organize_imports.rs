use crate::{
    change_nodes::{Leading, NodeOptions, NodeTracker, Trailing},
    organize_coalesce,
    organize_compare::Comparers,
    syntax::Syntax,
    LanguageService, OrganizeOptions, Result,
};
use std::collections::BTreeMap;
use tsr_ast::{AstView, FactoryMethods, NodeId, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_lsproto as lsp;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrganizeMode {
    Organize,
    Sort,
    RemoveUnused,
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/organizeimports.go:LanguageService.OrganizeImports
    pub fn organize_imports(
        &mut self,
        checker: &mut Operation<'_>,
        source: NodeId,
        mode: OrganizeMode,
        settings: &tsr_format::FormatCodeSettings,
        preferences: &OrganizeOptions,
    ) -> Result<BTreeMap<lsp::DocumentUri, Vec<lsp::TextEdit>>> {
        self.check_canceled()?;
        let view = self.view(source)?;
        let syntax = Syntax::new(view, source)?;
        let statements: Vec<_> = view
            .node_slice(view.node(source)?.statements(view)?)?
            .iter()
            .flatten()
            .collect();
        let imports = filter_imports(view, &statements)?;
        let groups = group_imports(&syntax, &imports)?;
        let comparers = preferences.detect(view, &groups)?;
        let mut tracker = NodeTracker::new(self.program, settings);
        for group in groups {
            self.check_canceled()?;
            import_group(checker, &mut tracker, source, &group, mode, comparers)?;
        }
        if mode != OrganizeMode::RemoveUnused {
            for group in export_groups(&syntax, &statements)? {
                export_group(&mut tracker, source, &group, comparers)?;
            }
        }
        for stmt in statements {
            if !tsr_ast::is_ambient_module(view, stmt)? {
                continue;
            }
            let Some(body) = view.node(stmt)?.body() else {
                continue;
            };
            let statements: Vec<_> = view
                .node_slice(view.node(body)?.statements(view)?)?
                .iter()
                .flatten()
                .collect();
            for group in group_imports(&syntax, &filter_imports(view, &statements)?)? {
                self.check_canceled()?;
                import_group(checker, &mut tracker, source, &group, mode, comparers)?;
            }
            if mode != OrganizeMode::RemoveUnused {
                let exports: Vec<_> = statements
                    .into_iter()
                    .filter(|&n| view.node(n).is_ok_and(|n| n.kind() == K::ExportDeclaration))
                    .collect();
                export_group(&mut tracker, source, &exports, comparers)?;
            }
        }
        Ok(tracker.finish(self)?.edits)
    }
}
fn filter_imports(view: AstView<'_>, statements: &[NodeId]) -> Result<Vec<NodeId>> {
    let mut imports = Vec::new();
    for &node in statements {
        if view.node(node)?.kind() == K::ImportDeclaration {
            imports.push(node);
        }
    }
    Ok(imports)
}
// port: tsc/internal/ls/organizeimports.go:groupByNewlineContiguous
fn group_imports(syntax: &Syntax<'_>, nodes: &[NodeId]) -> Result<Vec<Vec<NodeId>>> {
    let mut result: Vec<Vec<NodeId>> = Vec::new();
    for &node in nodes {
        let start = i64::from(syntax.view.node(node)?.pos());
        let text = syntax.file.text().as_bytes();
        let mut new_group = result.is_empty();
        if !new_group && start >= 0 && (start as usize) < text.len() {
            let end = tsr_scanner::skip_trivia(text, start);
            if end > start {
                let mut scanner = tsr_scanner::Scanner::new();
                scanner.set_skip_trivia(false);
                scanner.set_text(&text[start as usize..end as usize]);
                let mut newlines = 0;
                loop {
                    let kind = scanner.scan();
                    if kind == K::EndOfFile {
                        break;
                    }
                    if kind == K::NewLineTrivia {
                        newlines += 1;
                        if newlines >= 2 {
                            new_group = true;
                            break;
                        }
                    }
                }
            }
        }
        if new_group {
            result.push(Vec::new());
        }
        result.last_mut().unwrap().push(node);
    }
    Ok(result)
}
fn grouped_by_module(view: AstView<'_>, nodes: &[NodeId]) -> Result<Vec<Vec<NodeId>>> {
    let mut groups: Vec<(Vec<u8>, Vec<NodeId>)> = Vec::new();
    let mut indices = std::collections::HashMap::new();
    for &id in nodes {
        let key = module_name(view, id)?;
        let index = *indices.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[index].1.push(id);
    }
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}
pub(crate) fn module_name(view: AstView<'_>, node: NodeId) -> Result<Vec<u8>> {
    Ok(view
        .node(node)?
        .module_specifier()
        .filter(|&n| {
            view.node(n)
                .is_ok_and(|n| tsr_ast::utilities::is_string_literal_like(&n))
        })
        .map(|n| view.node_text(n).map(|s| s.as_bytes().to_vec()))
        .transpose()?
        .unwrap_or_default())
}
// port: tsc/internal/ls/organizeimports.go:organizeImportsWorker
fn import_group(
    c: &mut Operation<'_>,
    tracker: &mut NodeTracker<'_>,
    source: NodeId,
    old: &[NodeId],
    mode: OrganizeMode,
    compare: Comparers,
) -> Result<()> {
    if old.is_empty() {
        return Ok(());
    }
    let processed = if mode == OrganizeMode::Sort {
        old.to_vec()
    } else {
        remove_unused(c, tracker, source, old)?
    };
    let new = if mode == OrganizeMode::RemoveUnused {
        processed
    } else {
        let mut groups = grouped_by_module(tracker.ast.view(), &processed)?;
        groups.sort_by(|a, b| {
            compare.modules.modules(
                &module_name(tracker.ast.view(), a[0]).unwrap(),
                &module_name(tracker.ast.view(), b[0]).unwrap(),
            )
        });
        let mut result = Vec::new();
        for group in groups {
            let mut combined = organize_coalesce::imports(tracker, source, &group, compare)?;
            combined.sort_by(|&a, &b| {
                compare
                    .modules
                    .modules(
                        &module_name(tracker.ast.view(), a).unwrap(),
                        &module_name(tracker.ast.view(), b).unwrap(),
                    )
                    .then_with(|| {
                        organize_coalesce::import_kind(tracker.ast.view(), a)
                            .cmp(&organize_coalesce::import_kind(tracker.ast.view(), b))
                    })
            });
            result.extend(combined);
        }
        result
    };
    replace_group(tracker, source, old, new, false)
}
fn replace_group(
    tracker: &mut NodeTracker<'_>,
    source: NodeId,
    old: &[NodeId],
    new: Vec<NodeId>,
    export: bool,
) -> Result<()> {
    if new.is_empty() {
        return tracker.delete_range(
            source,
            old[0],
            *old.last().unwrap(),
            Leading::Exclude,
            Trailing::Include,
        );
    }
    for &node in &new {
        if export {
            tracker
                .emit
                .add_emit_flags(node, tsr_printer::emit_flags::NO_LEADING_COMMENTS);
        } else {
            tracker
                .emit
                .set_emit_flags(node, tsr_printer::emit_flags::NO_LEADING_COMMENTS);
        }
    }
    tracker.replace_node_with_nodes(
        source,
        old[0],
        new,
        Some(NodeOptions {
            leading: Leading::Exclude,
            trailing: Trailing::Include,
            suffix: "\n".into(),
            ..Default::default()
        }),
    )?;
    for &old in &old[1..] {
        tracker.delete(source, old);
    }
    Ok(())
}
// port: tsc/internal/ls/organizeimports.go:removeUnusedImports
fn remove_unused(
    c: &mut Operation<'_>,
    t: &mut NodeTracker<'_>,
    source: NodeId,
    old: &[NodeId],
) -> Result<Vec<NodeId>> {
    let view = t.source_view(source)?;
    let file = view.source_file(source)?;
    let jsx = view.subtree_facts(source) & tsr_ast::subtree_flags::JSX != 0;
    let explicit = matches!(
        t.program.options().jsx,
        tsr_core::JsxEmit::REACT | tsr_core::JsxEmit::REACT_NATIVE
    );
    let mut result = Vec::new();
    for &id in old {
        let import = organize_coalesce::Import::read(t.ast.view(), id)?;
        let Some(clause) = import.clause else {
            result.push(id);
            continue;
        };
        let mut name = import.name;
        let mut bindings = import.bindings;
        if let Some(n) = name {
            if !c.is_declaration_used(source, n, jsx, explicit)? {
                name = None;
            }
        }
        if let Some(b) = bindings {
            match view.node(b)?.kind().known() {
                Some(K::NamespaceImport) => {
                    if !c.is_declaration_used(
                        source,
                        view.node(b)?.name().expect("namespace name"),
                        jsx,
                        explicit,
                    )? {
                        bindings = None;
                    }
                }
                Some(K::NamedImports) => {
                    let elements: Vec<_> = view
                        .node_slice(view.node(b)?.elements(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    let mut used = Vec::new();
                    for &element in &elements {
                        if c.is_declaration_used(
                            source,
                            view.node(element)?.name().expect("specifier name"),
                            jsx,
                            explicit,
                        )? {
                            used.push(element);
                        }
                    }
                    if used.is_empty() {
                        bindings = None;
                    } else if used.len() < elements.len() {
                        let list = organize_coalesce::list(&mut t.ast, &used)?;
                        bindings = Some(t.ast.update_named_imports(b, Some(list)));
                    }
                    if let Some(new) = bindings {
                        organize_coalesce::preserve_multiline(t, source, b, new)?;
                    }
                }
                _ => {}
            }
        }
        if name.is_some() || bindings.is_some() {
            let clause = t
                .ast
                .update_import_clause(clause, import.phase, name, bindings);
            result.push(import.update(t, Some(clause)));
        } else {
            let module = import
                .specifier
                .map(|n| view.node_text(n).map(|s| s.as_bytes().to_vec()))
                .transpose()?
                .unwrap_or_default();
            let mut augmented = false;
            for node in file.module_augmentations()?.iter().flatten() {
                if view.node(*node)?.kind() == K::StringLiteral
                    && view.node_text(*node)?.as_bytes() == module
                {
                    augmented = true;
                    break;
                }
            }
            if augmented {
                result.push(if file.is_declaration_file {
                    import.update(t, None)
                } else {
                    id
                });
            }
        }
    }
    Ok(result)
}
// port: tsc/internal/ls/organizeimports.go:getTopLevelExportGroups
fn export_groups(syntax: &Syntax<'_>, statements: &[NodeId]) -> Result<Vec<Vec<NodeId>>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    let mut i = 0;
    while i < statements.len() {
        let read = syntax.view.node(statements[i])?;
        if read.kind() == K::ExportDeclaration {
            if read.module_specifier().is_some() {
                current.push(statements[i]);
                i += 1;
            } else {
                while i < statements.len()
                    && syntax.view.node(statements[i])?.kind() == K::ExportDeclaration
                {
                    current.push(statements[i]);
                    i += 1;
                }
                result.extend(group_imports(syntax, &current)?);
                current.clear();
            }
        } else {
            i += 1;
            if !current.is_empty() {
                result.extend(group_imports(syntax, &current)?);
                current.clear();
            }
        }
    }
    result.extend(group_imports(syntax, &current)?);
    Ok(result)
}
// port: tsc/internal/ls/organizeimports.go:organizeExportsWorker
fn export_group(
    t: &mut NodeTracker<'_>,
    source: NodeId,
    old: &[NodeId],
    compare: Comparers,
) -> Result<()> {
    if old.is_empty() {
        return Ok(());
    }
    let new = organize_coalesce::exports(t, source, old, compare)?;
    replace_group(t, source, old, new, true)
}
