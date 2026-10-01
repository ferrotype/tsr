//! `transformers/tstransforms/importelision.go`: elides the import and export
//! declarations, and the parts of them, that the checker reports as
//! unreferenced or type-only.
use crate::transformer::{Error, SharedEmitResolver, TransformOptions, Transformer};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::{
    utilities::{is_external_module, is_in_js_file},
    FactoryMethods, NodeId, NodeListId, NodeVisitor, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::CompilerOptions;
use tsr_printer::EmitContext;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `ImportElisionTransformer`. The embedded `Transformer` is the one the
/// constructor returns; its emit context is kept here for the visit.
struct ImportElisionTransformer<'a> {
    compiler_options: Arc<CompilerOptions>,
    current_source_file: Cell<Option<NodeId>>,
    emit_resolver: SharedEmitResolver<'a>,
    emit_context: EmitContext,
}

/// # Panics
///
/// With `verbatimModuleSyntax`, as upstream.
// port: tsc/internal/transformers/tstransforms/importelision.go:NewImportElisionTransformer
pub fn new_import_elision_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let compiler_options = opts.compiler_options.clone();
    let emit_context = opts.context.clone();
    assert!(
        !compiler_options.verbatim_module_syntax.is_true(),
        "ImportElisionTransformer should not be used with VerbatimModuleSyntax"
    );
    let tx = Rc::new(ImportElisionTransformer {
        compiler_options,
        current_source_file: Cell::new(None),
        emit_resolver: opts.emit_resolver.clone(),
        emit_context: emit_context.clone(),
    });
    let failure = opts.failure.clone();
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            if failure.is_set() {
                return node;
            }
            match tx.visit(visitor, node) {
                Ok(visited) => visited,
                Err(error) => {
                    failure.record(error);
                    node
                }
            }
        },
        Some(emit_context),
        opts.failure.clone(),
    ))
}

impl ImportElisionTransformer<'_> {
    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.visit
    #[allow(clippy::match_same_arms)] // Keep upstream's case order and per-case comments.
    fn visit(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let id = node.expect(NIL);
        let kind = visitor.factory().node(id).kind();
        if kind == K::SourceFile {
            let source_file = self.emit_context.most_original(id);
            self.emit_resolver
                .borrow_mut()
                .mark_linked_references_recursively(source_file)?;
        }

        match kind.known() {
            Some(K::ImportEqualsDeclaration) => {
                if is_external_module_import_equals_declaration(visitor.factory(), id) {
                    if !self.should_emit_alias_declaration(visitor.factory(), id)? {
                        return Ok(None);
                    }
                } else if !self.should_emit_import_equals_declaration(visitor.factory(), id)? {
                    return Ok(None);
                }
                Ok(visitor.visit_each_child(node))
            }
            Some(K::ImportDeclaration) => {
                // Do not elide a side-effect only import declaration.
                //  import "foo";
                let (modifiers, import_clause, module_specifier, attributes) = {
                    let read = visitor.factory().node(id);
                    let n = read
                        .data_source()
                        .as_import_declaration()
                        .expect("ImportDeclaration payload");
                    (
                        n.modifiers(),
                        n.import_clause(),
                        n.module_specifier(),
                        n.attributes(),
                    )
                };
                if import_clause.is_some() {
                    let Some(import_clause) = visitor.visit_node(import_clause) else {
                        return Ok(None);
                    };
                    let attributes = visitor.visit_node(attributes);
                    return Ok(Some(visitor.factory_mut().update_import_declaration(
                        id,
                        modifiers,
                        Some(import_clause),
                        module_specifier,
                        attributes,
                    )));
                }
                Ok(visitor.visit_each_child(node))
            }
            Some(K::ImportClause) => {
                let (phase_modifier, name, named_bindings) = {
                    let read = visitor.factory().node(id);
                    let n = read
                        .data_source()
                        .as_import_clause()
                        .expect("ImportClause payload");
                    (n.phase_modifier(), n.name(), n.named_bindings())
                };
                let name = if self.should_emit_alias_declaration(visitor.factory(), id)? {
                    name
                } else {
                    None
                };
                let named_bindings = visitor.visit_node(named_bindings);
                if name.is_none() && named_bindings.is_none() {
                    // all import bindings were elided
                    return Ok(None);
                }
                Ok(Some(visitor.factory_mut().update_import_clause(
                    id,
                    phase_modifier,
                    name,
                    named_bindings,
                )))
            }
            Some(K::NamespaceImport) => {
                if !self.should_emit_alias_declaration(visitor.factory(), id)? {
                    // elide unused imports
                    return Ok(None);
                }
                Ok(node)
            }
            Some(K::NamedImports) => {
                let elements = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_named_imports()
                    .expect("NamedImports payload")
                    .elements();
                let elements = visitor.visit_nodes(elements);
                if list_is_empty(visitor.factory(), elements) {
                    // all import specifiers were elided
                    return Ok(None);
                }
                Ok(Some(
                    visitor.factory_mut().update_named_imports(id, elements),
                ))
            }
            Some(K::ImportSpecifier) => {
                if !self.should_emit_alias_declaration(visitor.factory(), id)? {
                    // elide type-only or unused imports
                    return Ok(None);
                }
                Ok(node)
            }
            Some(K::ExportAssignment) => {
                if !self.compiler_options.verbatim_module_syntax.is_true()
                    && !self.is_value_alias_declaration(visitor.factory(), id)?
                {
                    // elide unused import
                    return Ok(None);
                }
                Ok(visitor.visit_each_child(node))
            }
            Some(K::ExportDeclaration) => {
                let (export_clause, module_specifier, attributes) = {
                    let read = visitor.factory().node(id);
                    let n = read
                        .data_source()
                        .as_export_declaration()
                        .expect("ExportDeclaration payload");
                    (n.export_clause(), n.module_specifier(), n.attributes())
                };
                let mut visited_clause = None;
                if export_clause.is_some() {
                    visited_clause = visitor.visit_node(export_clause);
                    if visited_clause.is_none() {
                        // all export bindings were elided
                        return Ok(None);
                    }
                }
                let module_specifier = visitor.visit_node(module_specifier);
                let attributes = visitor.visit_node(attributes);
                Ok(Some(visitor.factory_mut().update_export_declaration(
                    id,
                    None,  /*modifiers*/
                    false, /*isTypeOnly*/
                    visited_clause,
                    module_specifier,
                    attributes,
                )))
            }
            Some(K::NamedExports) => {
                let elements = visitor
                    .factory()
                    .node(id)
                    .data_source()
                    .as_named_exports()
                    .expect("NamedExports payload")
                    .elements();
                let elements = visitor.visit_nodes(elements);
                if list_is_empty(visitor.factory(), elements) {
                    // all export specifiers were elided
                    return Ok(None);
                }
                Ok(Some(
                    visitor.factory_mut().update_named_exports(id, elements),
                ))
            }
            Some(K::ExportSpecifier) => {
                if !self.is_value_alias_declaration(visitor.factory(), id)? {
                    // elide unused export
                    return Ok(None);
                }
                Ok(node)
            }
            Some(K::SourceFile) => {
                let saved_current_source_file = self.current_source_file.get();
                self.current_source_file.set(Some(id));
                let node = visitor.visit_each_child(node);
                self.current_source_file.set(saved_current_source_file);
                Ok(node)
            }
            Some(K::ModuleDeclaration | K::ModuleBlock) => Ok(visitor.visit_each_child(node)),
            _ => Ok(node),
        }
    }

    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.shouldEmitAliasDeclaration
    fn should_emit_alias_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        Ok(is_in_js_file(Some(&factory.node(node)))
            || self.is_referenced_alias_declaration(factory, node)?)
    }

    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.shouldEmitImportEqualsDeclaration
    fn should_emit_import_equals_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        // preserve old compiler's behavior: emit import declaration (even if we do not consider them referenced) when
        // - current file is not external module
        // - import declaration is top level and target is value imported by entity name
        if self.should_emit_alias_declaration(factory, node)? {
            return Ok(true);
        }
        let current_source_file = self.current_source_file.get().expect(NIL);
        let current_source_file = factory.read_source_file(current_source_file)?;
        Ok(!is_external_module(&current_source_file)
            && self.is_top_level_value_import_equals_with_entity_name(factory, node)?)
    }

    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.isReferencedAliasDeclaration
    fn is_referenced_alias_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        let Some(node) = self.emit_context.parse_node(factory, node) else {
            return Ok(true);
        };
        Ok(self
            .emit_resolver
            .borrow_mut()
            .is_referenced_alias_declaration(node)?)
    }

    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.isValueAliasDeclaration
    fn is_value_alias_declaration(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        let Some(node) = self.emit_context.parse_node(factory, node) else {
            return Ok(true);
        };
        Ok(self
            .emit_resolver
            .borrow_mut()
            .is_value_alias_declaration(node)?)
    }

    // port: tsc/internal/transformers/tstransforms/importelision.go:ImportElisionTransformer.isTopLevelValueImportEqualsWithEntityName
    fn is_top_level_value_import_equals_with_entity_name(
        &self,
        factory: &dyn RuntimeFactory,
        node: NodeId,
    ) -> Result<bool, Error> {
        let Some(node) = self.emit_context.parse_node(factory, node) else {
            return Ok(false);
        };
        Ok(self
            .emit_resolver
            .borrow_mut()
            .is_top_level_value_import_equals_with_entity_name(node)?)
    }
}

/// `ast.IsExternalModuleImportEqualsDeclaration` over the transformer's
/// factory: the ported predicate reads an `AstView`, which a transformer does
/// not hold.
fn is_external_module_import_equals_declaration(
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    let read = factory.node(node);
    if read.kind() != K::ImportEqualsDeclaration {
        return false;
    }
    let module_reference = read
        .data_source()
        .as_import_equals_declaration()
        .expect("ImportEqualsDeclaration payload")
        .module_reference()
        .expect(NIL);
    factory.node(module_reference).kind() == K::ExternalModuleReference
}

/// `len(list.Nodes) == 0`; a nil list is a nil dereference, as upstream.
fn list_is_empty(factory: &dyn RuntimeFactory, list: Option<NodeListId>) -> bool {
    factory.read_list(list.expect(NIL)).nodes().is_empty()
}

#[cfg(test)]
mod tests {
    use crate::transformer_tests::options;
    use std::sync::Arc;
    use tsr_core::{CompilerOptions, Tristate};
    use tsr_printer::EmitContext;

    /// The pin panics in the constructor (the probe of `verbatimModuleSyntax`
    /// records the panic; the probe suite cannot run a panicking chain).
    #[test]
    #[should_panic(
        expected = "ImportElisionTransformer should not be used with VerbatimModuleSyntax"
    )]
    fn verbatim_module_syntax_is_refused() {
        let context = EmitContext::new();
        let mut opts = options(&context);
        opts.compiler_options = Arc::new(CompilerOptions {
            verbatim_module_syntax: Tristate::TRUE,
            ..CompilerOptions::default()
        });
        let _ = super::new_import_elision_transformer(&opts);
    }
}
