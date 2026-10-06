use crate::{
    change_nodes::{Leading, NodeTracker},
    code_actions::{localized, Fix},
    syntax::Syntax,
    CompletionOptions, LanguageService, OrganizeOptions, Result,
};
use tsr_ast::{FactoryMethods, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
impl LanguageService<'_> {
    // port: tsc/internal/ls/autoimport/fix.go:promoteFromTypeOnly
    pub(crate) fn promote_import(
        &mut self,
        syntax: &mut Syntax<'_>,
        alias: NodeId,
        options: &CompletionOptions,
        preferences: &OrganizeOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<Fix>> {
        let source = syntax.source;
        let view = syntax.view;
        let read = view.node(alias)?;
        let mut tracker = NodeTracker::new(self.program, &options.format);
        let clause = match read.kind().known() {
            Some(K::ImportSpecifier) => {
                let parent = read.parent().expect("specifier parent");
                let clause = view.node(parent)?.parent().expect("named imports parent");
                if read.is_type_only() {
                    let elements: Vec<_> = view
                        .node_slice(view.node(parent)?.elements(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    let data = read.data_source();
                    let spec = data.as_import_specifier().unwrap();
                    let property = spec
                        .property_name()
                        .map(|p| {
                            view.node_text(p)
                                .map(|s| tsr_ast::JsString::from_bytes(s.as_bytes()))
                        })
                        .transpose()?
                        .map(|s| tracker.ast.new_identifier(s));
                    let name = tracker.ast.new_identifier(tsr_ast::JsString::from_bytes(
                        view.node_text(spec.name().unwrap())?.as_bytes(),
                    ));
                    let new = tracker
                        .ast
                        .new_import_specifier(false, property, Some(name));
                    let import = view.node(clause)?.parent().expect("clause parent");
                    let compare = preferences.detect(view, &[vec![import]])?;
                    let current = elements
                        .iter()
                        .position(|&e| e == alias)
                        .expect("specifier in list");
                    let index = elements.partition_point(|&e| {
                        compare.specifiers(tracker.ast.view(), e, new).is_lt()
                    });
                    if elements.len() > 1 && index != current {
                        tracker.delete(source, alias);
                        tracker.insert_import_specifier(source, parent, new, index)?;
                    } else {
                        let start = syntax.start(alias)?;
                        let end = syntax.start(spec.property_name().or(spec.name()).unwrap())?;
                        tracker
                            .raw
                            .replace_text(source, TextRange::new(start, end), String::new());
                    }
                    let module = view
                        .node(import)?
                        .module_specifier()
                        .map(|n| promoted_module_text(syntax, n))
                        .transpose()?
                        .unwrap_or_default();
                    // The native promotion fix leaves its protocol Name unset.
                    let title = localized(
                        tsr_diagnostics::Remove_type_from_import_of_0_from_1,
                        locale,
                        &[b"", &module],
                    );
                    let changes = tracker.finish(self)?;
                    if !changes.unmappable.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(Fix {
                        title,
                        edits: changes.edits.into_values().flatten().collect(),
                    }));
                }
                clause
            }
            Some(K::ImportClause) => alias,
            Some(K::NamespaceImport) => read.parent().expect("namespace clause"),
            Some(K::ImportEqualsDeclaration) => {
                let mut scan = tsr_scanner::Scanner::new();
                scan.set_text(syntax.file.text().as_bytes());
                scan.reset_token_state(i64::from(read.pos()));
                scan.scan();
                scan.scan();
                delete_type_keyword(&mut tracker, syntax, scan.token_start());
                drop(scan);
                let data = read.data_source();
                let reference = data
                    .as_import_equals_declaration()
                    .and_then(|d| d.module_reference())
                    .unwrap();
                let r = view.node(reference)?;
                let module = if r.kind() == K::ExternalModuleReference {
                    r.expression().unwrap_or(reference)
                } else {
                    reference
                };
                let name = promoted_module_text(syntax, module)?;
                let title = localized(
                    tsr_diagnostics::Remove_type_from_import_declaration_from_0,
                    locale,
                    &[&name],
                );
                let changes = tracker.finish(self)?;
                if !changes.unmappable.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(Fix {
                    title,
                    edits: changes.edits.into_values().flatten().collect(),
                }));
            }
            _ => panic!("unexpected type-only alias kind"),
        };
        let read = view.node(clause)?;
        let data = read.data_source();
        let data = data.as_import_clause().expect("import clause");
        if data.phase_modifier() == K::TypeKeyword {
            delete_type_keyword(&mut tracker, syntax, i64::from(read.pos()));
        }
        if self.program.options().verbatim_module_syntax.is_true() {
            if let Some(bindings) = data
                .named_bindings()
                .filter(|&n| view.node(n).is_ok_and(|n| n.kind() == K::NamedImports))
            {
                let elements: Vec<_> = view
                    .node_slice(view.node(bindings)?.elements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                if elements.len() > 1 {
                    let import = read.parent().expect("clause declaration");
                    let compare = preferences.detect(view, &[vec![import]])?;
                    let sorted = elements
                        .windows(2)
                        .all(|pair| !compare.specifiers(view, pair[0], pair[1]).is_gt());
                    if sorted && view.node(alias)?.kind() == K::ImportSpecifier {
                        if let Some(index) = elements.iter().position(|&e| e == alias) {
                            if index > 0 {
                                tracker.delete(source, alias);
                                tracker.insert_import_specifier(source, bindings, alias, 0)?;
                            }
                        }
                    }
                    for e in elements {
                        if e != alias && !view.node(e)?.is_type_only() {
                            tracker.insert_modifier_before(source, K::TypeKeyword, e)?;
                        }
                    }
                }
            }
        }
        let import = read.parent().expect("clause declaration");
        let module = view
            .node(import)?
            .module_specifier()
            .map(|n| promoted_module_text(syntax, n))
            .transpose()?
            .unwrap_or_default();
        let title = localized(
            tsr_diagnostics::Remove_type_from_import_declaration_from_0,
            locale,
            &[&module],
        );
        let changes = tracker.finish(self)?;
        if !changes.unmappable.is_empty() {
            return Ok(None);
        }
        Ok(Some(Fix {
            title,
            edits: changes.edits.into_values().flatten().collect(),
        }))
    }
}
// port: tsc/internal/ls/autoimport/fix.go:deleteTypeKeyword
fn delete_type_keyword(tracker: &mut NodeTracker<'_>, syntax: &Syntax<'_>, pos: i64) {
    let mut scanner = tsr_scanner::Scanner::new();
    scanner.set_text(syntax.file.text().as_bytes());
    scanner.reset_token_state(pos);
    if scanner.scan() != K::TypeKeyword {
        return;
    }
    let start = scanner.token_start();
    let mut end = scanner.token_end();
    let text = syntax.file.text().as_bytes();
    while matches!(text.get(end as usize), Some(b' ' | b'\t')) {
        end += 1;
    }
    tracker
        .raw
        .replace_text(syntax.source, TextRange::new(start, end), String::new());
}
impl NodeTracker<'_> {
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertImportSpecifierAtIndex
    pub fn insert_import_specifier(
        &mut self,
        source: NodeId,
        bindings: NodeId,
        new: NodeId,
        index: usize,
    ) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let view = syntax.view;
        let elements: Vec<_> = view
            .node_slice(view.node(bindings)?.elements(view)?)?
            .iter()
            .flatten()
            .collect();
        if let Some(&previous) = index.checked_sub(1).and_then(|i| elements.get(i)) {
            self.insert_in_list_after(source, previous, new)
        } else {
            let import = view
                .node(view.node(bindings)?.parent().expect("named clause"))?
                .parent()
                .expect("import declaration");
            let first = syntax.start(elements[0])?;
            let import = syntax.start(import)?;
            let blank = !syntax.same_line(first, import);
            self.insert_before(source, elements[0], new, blank, Leading::None)
        }
    }
}

// port: tsc/internal/ls/autoimport/fix.go:getModuleSpecifierText
fn promoted_module_text(syntax: &mut Syntax<'_>, node: NodeId) -> Result<Vec<u8>> {
    let read = syntax.view.node(node)?;
    if matches!(
        read.kind().known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    ) {
        return Ok(syntax.view.node_text(node)?.as_bytes().to_vec());
    }
    // Error-recovery imports can contain arbitrary expressions. Like
    // scanner.GetTextOfNode, retain their source spelling in the fix title.
    let end = read.end() as usize;
    let start = syntax.start(node)? as usize;
    Ok(syntax.file.text().as_bytes()[start..end].to_vec())
}
