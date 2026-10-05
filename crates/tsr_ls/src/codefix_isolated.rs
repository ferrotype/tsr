use crate::{
    change_nodes::NodeTracker,
    code_actions::{localized, Fix, Provider},
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{
    modifier_flags as mf, AstView, Factory, FactoryMethods, JsString, NodeId, SyntaxKind as K,
};
use tsr_checker::{type_flags as tf, BuilderRequest, Operation, SymbolRef, TypeRef};
use tsr_core::TextRange;
use tsr_nodebuilder::{flags as nf, internal_flags as inf};
use tsr_printer::EmitTextWriter;
pub(super) const DECLARATION_FLAGS: u32 = nf::MULTILINE_OBJECT_LITERALS
    | nf::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
    | nf::USE_TYPE_OF_FUNCTION
    | nf::USE_STRUCTURAL_FALLBACK
    | nf::ALLOW_EMPTY_TUPLE
    | nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS
    | nf::NO_TRUNCATION;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Full,
    Relative,
    Widened,
}
pub(super) struct Fixer<'s, 'p, 'o> {
    pub service: &'s mut LanguageService<'p>,
    pub checker: &'s mut Operation<'o>,
    pub syntax: Syntax<'p>,
    pub tracker: NodeTracker<'p>,
    pub locale: &'s tsr_locale::Locale,
    pub fixed: HashSet<NodeId>,
    pub mode: Mode,
    pub symbols: Vec<SymbolRef>,
    pub mutated: bool,
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:getIsolatedDeclarationsCodeActions
    pub(crate) fn isolated_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        span: TextRange,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Vec<Fix>> {
        let mut fixes = Vec::new();
        for (inline, mode) in [
            (false, Mode::Full),
            (false, Mode::Relative),
            (false, Mode::Widened),
            (true, Mode::Full),
            (true, Mode::Relative),
            (true, Mode::Widened),
        ] {
            let mut fixer = Fixer::new(self, c, source, options, locale, mode)?;
            let title = if inline {
                fixer.inline(span)?
            } else {
                fixer.annotate(span)?
            };
            if !title.is_empty() {
                if let Some(fix) = fixer.finish(title)? {
                    fixes.push(fix);
                }
            }
        }
        let mut fixer = Fixer::new(self, c, source, options, locale, Mode::Full)?;
        let title = fixer.extract(span)?;
        if !title.is_empty() {
            if let Some(fix) = fixer.finish(title)? {
                fixes.push(fix);
            }
        }
        Ok(fixes)
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:getAllIsolatedDeclarationsCodeActions
    pub(crate) fn all_isolated_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<Fix>> {
        let diagnostics = self.action_diagnostics(c, source)?;
        let mut fixer = Fixer::new(self, c, source, options, locale, Mode::Full)?;
        let codes = Provider::Isolated.codes();
        for d in diagnostics {
            if d.source.is_empty() && codes.contains(&d.code) {
                fixer.annotate(d.loc)?;
            }
        }
        fixer.finish(localized(
            tsr_diagnostics::Add_all_missing_type_annotations,
            locale,
            &[],
        ))
    }
}
impl<'s, 'p, 'o> Fixer<'s, 'p, 'o> {
    fn new(
        service: &'s mut LanguageService<'p>,
        checker: &'s mut Operation<'o>,
        source: NodeId,
        options: &'s CompletionOptions,
        locale: &'s tsr_locale::Locale,
        mode: Mode,
    ) -> Result<Self> {
        let syntax = Syntax::new(service.view(source)?, source)?;
        let tracker = NodeTracker::new(service.program, &options.format);
        Ok(Self {
            service,
            checker,
            syntax,
            tracker,
            locale,
            fixed: HashSet::new(),
            mode,
            symbols: Vec::new(),
            mutated: false,
        })
    }
    fn finish(mut self, title: String) -> Result<Option<Fix>> {
        for symbol in std::mem::take(&mut self.symbols) {
            self.add_existing_import(symbol)?;
        }
        let changes = self.tracker.finish(self.service)?;
        let uri = tsr_lsproto::DocumentUri::from_file_name(
            self.syntax.file.original_file_name()?.as_bytes(),
        );
        let edits = changes.edits.get(&uri).cloned().unwrap_or_default();
        Ok((!edits.is_empty()).then_some(Fix { title, edits }))
    }
    pub fn id(&mut self, text: &[u8]) -> NodeId {
        self.tracker.ast.new_identifier(JsString::from_bytes(text))
    }
    pub fn clone_node(&mut self, node: NodeId) -> NodeId {
        tsr_ast::deep_clone_node(&mut self.tracker.ast, Some(node)).expect("cloned fix syntax")
    }
    pub fn list(&mut self, nodes: &[NodeId]) -> Result<tsr_ast::NodeListId> {
        crate::completion_snippets::list(
            &mut self.tracker.ast,
            &nodes.iter().map(|&n| Some(n)).collect::<Vec<_>>(),
        )
    }
    pub fn unique(&mut self, text: &[u8], optimistic: bool) -> NodeId {
        self.tracker.emit.new_unique_name_ex(
            &mut self.tracker.ast,
            JsString::from_bytes(text),
            tsr_printer::AutoGenerateOptions {
                flags: if optimistic {
                    tsr_printer::generated_identifier_flags::OPTIMISTIC
                } else {
                    0
                },
                ..Default::default()
            },
        )
    }
    pub fn variable(
        &mut self,
        name: NodeId,
        ty: Option<NodeId>,
        init: Option<NodeId>,
        export: bool,
    ) -> Result<NodeId> {
        let node = self
            .tracker
            .ast
            .new_variable_declaration(Some(name), None, ty, init);
        let nodes = self.list(&[node])?;
        let declarations = self
            .tracker
            .ast
            .new_variable_declaration_list(Some(nodes), tsr_ast::node_flags::CONST);
        let modifiers = if export {
            let m = self.tracker.ast.new_modifier(K::ExportKeyword.into());
            Some(self.list(&[m])?)
        } else {
            None
        };
        Ok(self
            .tracker
            .ast
            .new_variable_statement(modifiers, Some(declarations)))
    }
    pub fn ancestor(
        &self,
        node: NodeId,
        predicate: impl Fn(&tsr_ast::NodeRead<'_>) -> bool,
    ) -> Result<Option<NodeId>> {
        Ok(tsr_ast::utilities::find_ancestor(
            self.syntax.view,
            Some(node),
            predicate,
        )?)
    }
    pub fn statement(&self, node: NodeId) -> Result<Option<NodeId>> {
        let mut at = Some(node);
        while let Some(n) = at {
            if tsr_ast::utilities::is_statement(self.syntax.view, n)? {
                return Ok(Some(n));
            }
            at = self.syntax.view.node(n)?.parent();
        }
        Ok(None)
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.addTypeAnnotation
    fn annotate(&mut self, span: TextRange) -> Result<String> {
        self.service.check_canceled()?;
        let token = self.syntax.nav().get_token_at_position(span.pos())?;
        if let Some(expando) = self.expando(token)? {
            return if self.syntax.view.node(expando)?.kind() == K::FunctionDeclaration {
                self.expando_namespace(expando)
            } else {
                self.fix_node(expando)
            };
        }
        let mut current = Some(token);
        while let Some(node) = current {
            let n = self.syntax.view.node(node)?;
            if matches!(
                n.kind().known(),
                Some(
                    K::GetAccessor
                        | K::MethodDeclaration
                        | K::PropertyDeclaration
                        | K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                        | K::VariableDeclaration
                        | K::Parameter
                        | K::ExportAssignment
                        | K::ClassDeclaration
                )
            ) || matches!(
                n.kind().known(),
                Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
            ) && n.parent().is_some_and(|p| {
                self.syntax
                    .view
                    .node(p)
                    .is_ok_and(|p| p.kind() == K::VariableDeclaration)
            }) {
                return self.fix_node(node);
            }
            current = n.parent();
        }
        Ok(String::new())
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.fixIsolatedDeclarationError
    fn fix_node(&mut self, node: NodeId) -> Result<String> {
        if !self.fixed.insert(node) {
            return Ok(String::new());
        }
        let view = self.syntax.view;
        let read = view.node(node)?;
        let source = self.syntax.source;
        match read.kind().known() {
            Some(K::Parameter | K::PropertyDeclaration | K::VariableDeclaration) => {
                let Some(ty) = self.infer(node, None)? else {
                    return Ok(String::new());
                };
                if let Some(old) = read.type_node() {
                    self.tracker.replace_node(source, old, ty, None)?;
                } else {
                    self.tracker.insert_type_annotation(source, node, ty)?;
                    if read.kind() == K::Parameter {
                        if let Some(parent) = read
                            .parent()
                            .filter(|&p| view.node(p).is_ok_and(|p| p.kind() == K::ArrowFunction))
                        {
                            self.tracker.parenthesize_arrow_parameters(source, parent)?;
                        }
                    }
                }
                let text = self.display(ty)?;
                Ok(localized(
                    tsr_diagnostics::Add_annotation_of_type_0,
                    self.locale,
                    &[&text],
                ))
            }
            Some(
                K::ArrowFunction
                | K::FunctionExpression
                | K::FunctionDeclaration
                | K::MethodDeclaration
                | K::GetAccessor,
            ) => {
                if read.type_node().is_some() {
                    return Ok(String::new());
                }
                let Some(ty) = self.infer(node, None)? else {
                    return Ok(String::new());
                };
                self.tracker.insert_type_annotation(source, node, ty)?;
                let text = self.display(ty)?;
                Ok(localized(
                    tsr_diagnostics::Add_return_type_0,
                    self.locale,
                    &[&text],
                ))
            }
            Some(K::ExportAssignment) => self.export_assignment(node),
            Some(K::ClassDeclaration) => self.extends_clause(node),
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern) => self.destructure(node),
            _ => Ok(String::new()),
        }
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:findBestFittingNode
    pub(super) fn best(&mut self, span: TextRange) -> Result<NodeId> {
        let view = self.syntax.view;
        let mut node = self.syntax.nav().get_token_at_position(span.pos())?;
        while i64::from(view.node(node)?.end()) < span.end() {
            let Some(p) = view.node(node)?.parent() else {
                break;
            };
            node = p;
        }
        while let Some(parent) = view.node(node)?.parent() {
            if view.node(parent)?.range() != view.node(node)?.range() {
                break;
            }
            node = parent;
        }
        if view.node(node)?.kind() == K::Identifier {
            if let Some(parent) = view.node(node)?.parent() {
                let p = view.node(parent)?;
                if let Some(init) = tsr_ast::utilities_middle::has_initializer(&p)
                    .then(|| p.initializer())
                    .flatten()
                {
                    return Ok(init);
                }
                if p.kind() == K::ShorthandPropertyAssignment {
                    return Ok(parent);
                }
            }
        }
        Ok(node)
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:findExpandoFunction
    pub(super) fn expando(&mut self, node: NodeId) -> Result<Option<NodeId>> {
        let view = self.syntax.view;
        let mut at = Some(node);
        let mut declaration = None;
        while let Some(n) = at {
            if tsr_ast::utilities::is_statement(view, n)? {
                break;
            }
            let r = view.node(n)?;
            if matches!(
                r.kind().known(),
                Some(
                    K::PropertyAccessExpression | K::ElementAccessExpression | K::BinaryExpression
                )
            ) {
                declaration = Some(n);
                break;
            }
            at = r.parent();
        }
        let Some(declaration) = declaration else {
            return Ok(None);
        };
        let mut target = declaration;
        if view.node(target)?.kind() == K::BinaryExpression {
            target = view
                .node(target)?
                .data_source()
                .as_binary_expression()
                .and_then(|d| d.left())
                .expect("binary left");
        }
        if !matches!(
            view.node(target)?.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return Ok(None);
        }
        let Some(expression) = view.node(target)?.expression() else {
            return Ok(None);
        };
        let ty = self.checker.get_type_at_location(expression)?;
        let parent = view.node(declaration)?.parent();
        let mut found = false;
        for property in self.checker.get_properties_of_type(ty)? {
            let value = self.checker.symbol(property)?.value_declaration();
            if value == Some(declaration) || value == parent {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(None);
        }
        let Some(symbol) = self.checker.type_symbol(ty)? else {
            return Ok(None);
        };
        let symbol = self.checker.symbol_ref(symbol)?;
        let Some(node) = self.checker.symbol(symbol)?.value_declaration() else {
            return Ok(None);
        };
        let r = view.node(node)?;
        if matches!(
            r.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) {
            if let Some(p) = r.parent().filter(|&p| {
                view.node(p)
                    .is_ok_and(|p| p.kind() == K::VariableDeclaration)
            }) {
                return Ok(Some(p));
            }
        }
        Ok((r.kind() == K::FunctionDeclaration).then_some(node))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.inferType
    pub fn infer(
        &mut self,
        node: NodeId,
        variable_type: Option<TypeRef>,
    ) -> Result<Option<NodeId>> {
        self.mutated = false;
        if self.mode == Mode::Relative {
            return self.relative(node);
        }
        let view = self.syntax.view;
        let read = view.node(node)?;
        let enclosing = self
            .ancestor(node, |n| tsr_ast::is_declaration(n))?
            .unwrap_or(self.syntax.source);
        let mut ty = if value_signature(read.kind()) {
            let signature = self.checker.get_signature_from_declaration(node)?;
            if let Some(predicate) = self.checker.get_type_predicate_of_signature(signature)? {
                let Some(ty) = self.checker.type_predicate_parts(predicate)?.r#type else {
                    return Ok(None);
                };
                let flags = DECLARATION_FLAGS
                    | if self.checker.type_flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0 {
                        nf::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                    } else {
                        0
                    };
                let mut builder = self.checker.node_builder_with_emit(&self.tracker.emit);
                let node = builder.type_predicate_to_type_predicate_node(
                    predicate,
                    BuilderRequest {
                        enclosing: Some(enclosing),
                        flags,
                        ..Default::default()
                    },
                )?;
                let nodes = builder.into_syntax();
                self.tracker
                    .retain_generated(nodes, &node.into_iter().collect::<Vec<_>>())?;
                return Ok(node);
            }
            self.checker.get_return_type_of_signature(signature)?
        } else {
            self.checker.get_type_at_location(node)?
        };
        if self.mode == Mode::Widened {
            ty = variable_type.unwrap_or(ty);
            let widened = self.checker.get_widened_literal_type(ty)?;
            if self.checker.is_type_assignable_to(widened, ty)? {
                return Ok(None);
            }
            ty = widened;
        }
        let unique = self.checker.type_flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0
            && (read.kind() == K::VariableDeclaration
                || read.kind() == K::PropertyDeclaration
                    && tsr_ast::utilities::has_syntactic_modifier(
                        view,
                        node,
                        mf::STATIC | mf::READONLY,
                    )?);
        if read.kind() == K::Parameter && self.checker.requires_adding_implicit_undefined(node)? {
            ty = self.checker.union_type_with(
                &[self.checker.get_undefined_type(), ty],
                tsr_checker::UnionReduction::None,
            )?;
        }
        self.minimized(
            ty,
            enclosing,
            DECLARATION_FLAGS
                | if unique {
                    nf::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                } else {
                    0
                },
        )
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.typeToMinimizedReferenceType
    pub fn minimized(
        &mut self,
        ty: TypeRef,
        enclosing: NodeId,
        flags: u32,
    ) -> Result<Option<NodeId>> {
        let cutoff = self.checker.required_reference_arguments(ty)?;
        let mut builder = self.checker.node_builder_with_emit(&self.tracker.emit);
        let root =
            builder.type_to_type_node(ty, Some(enclosing), flags, inf::WRITE_COMPUTED_PROPS)?;
        let Some(mut root) = root else {
            return Ok(None);
        };
        let mut nodes = builder.into_syntax();
        if let Some(cutoff) = cutoff {
            let read = nodes.ast.view().node(root)?;
            let data = read.data_source();
            if let Some(reference) = data.as_type_reference_node() {
                let name = reference.type_name();
                let args = reference.type_arguments();
                let args = args
                    .map(|l| {
                        nodes.ast.view().list(l).and_then(|l| {
                            nodes
                                .ast
                                .view()
                                .node_slice(l.nodes())
                                .map(|s| s.iter().collect::<Vec<_>>())
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                if !args.is_empty() && cutoff < args.len() {
                    let list = crate::completion_snippets::list(&mut nodes.ast, &args[..cutoff])?;
                    root = nodes.ast.update_type_reference_node(root, name, Some(list));
                }
            }
        }
        let mut roots = [root];
        self.symbols
            .extend(tsr_autoimport::type_nodes::importable_references(
                &mut nodes,
                &mut roots,
                self.checker,
                self.service.program,
            )?);
        self.tracker.retain_generated(nodes, &roots)?;
        Ok(Some(roots[0]))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:typeToStringForDiag
    pub fn display(&mut self, node: NodeId) -> Result<Vec<u8>> {
        let saved = self.tracker.emit.emit_flags(node);
        self.tracker
            .emit
            .set_emit_flags(node, saved | tsr_printer::emit_flags::SINGLE_LINE);
        let mut writer = tsr_printer::SingleLineStringWriter::new();
        let printer = tsr_printer::Printer::new(
            tsr_printer::PrinterOptions {
                new_line: tsr_core::NewLineKind::LF,
                ..Default::default()
            },
            &self.tracker.emit,
        );
        let result = printer.write(
            self.tracker.ast.view(),
            node,
            Some(self.syntax.source),
            &mut writer,
            None,
        );
        drop(printer);
        self.tracker.emit.set_emit_flags(node, saved);
        result?;
        let mut text = writer.text().to_vec();
        if text.len() > 160 {
            text.truncate(157);
            text.extend_from_slice(b"...");
        }
        Ok(text)
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.addSymbolToExistingImport
    fn add_existing_import(&mut self, symbol: SymbolRef) -> Result<()> {
        let Some(parent) = self.checker.symbol(symbol)?.parent() else {
            return Ok(());
        };
        let parent = self.checker.symbol_ref(parent)?;
        let module = self.checker.get_merged_symbol(parent)?;
        let name = self.checker.symbol(symbol)?.name_bytes().to_vec();
        let view = self.syntax.view;
        let statements: Vec<_> = view
            .node_slice(view.node(self.syntax.source)?.statements(view)?)?
            .iter()
            .flatten()
            .collect();
        for statement in statements {
            let read = view.node(statement)?;
            let data = read.data_source();
            let Some(import) = data.as_import_declaration() else {
                continue;
            };
            let Some(clause) = import.import_clause() else {
                continue;
            };
            let Some(specifier) = import.module_specifier() else {
                continue;
            };
            let Some(import_module) = self.checker.get_symbol_at_location(specifier)? else {
                continue;
            };
            if self.checker.get_merged_symbol(import_module)? != module {
                continue;
            }
            let clause_read = view.node(clause)?;
            let data = clause_read.data_source();
            let clause_data = data.as_import_clause().unwrap();
            if let Some(bindings) = clause_data
                .named_bindings()
                .filter(|&b| view.node(b).is_ok_and(|b| b.kind() == K::NamedImports))
            {
                let mut elements: Vec<_> = view
                    .node_slice(view.node(bindings)?.elements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                let id = self.id(&name);
                elements.push(self.tracker.ast.new_import_specifier(false, None, Some(id)));
                let list = self.list(&elements)?;
                let bindings = self.tracker.ast.new_named_imports(Some(list));
                let new_clause = self.tracker.ast.update_import_clause(
                    clause,
                    clause_data.phase_modifier(),
                    clause_data.name(),
                    Some(bindings),
                );
                let new = self.tracker.ast.update_import_declaration(
                    statement,
                    read.modifiers(),
                    Some(new_clause),
                    import.module_specifier(),
                    import.attributes(),
                );
                self.tracker
                    .replace_node(self.syntax.source, statement, new, None)?;
            }
            return Ok(());
        }
        Ok(())
    }
}
pub(super) fn value_signature(kind: tsr_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionDeclaration
                | K::Constructor
        )
    )
}
pub(super) fn named_declaration(kind: tsr_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::ArrowFunction
                | K::BindingElement
                | K::ClassDeclaration
                | K::ClassExpression
                | K::ClassStaticBlockDeclaration
                | K::Constructor
                | K::EnumDeclaration
                | K::EnumMember
                | K::ExportSpecifier
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::GetAccessor
                | K::ImportClause
                | K::ImportEqualsDeclaration
                | K::ImportSpecifier
                | K::InterfaceDeclaration
                | K::JsxAttribute
                | K::MethodDeclaration
                | K::MethodSignature
                | K::ModuleDeclaration
                | K::NamespaceExportDeclaration
                | K::NamespaceImport
                | K::NamespaceExport
                | K::Parameter
                | K::PropertyAssignment
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::SetAccessor
                | K::ShorthandPropertyAssignment
                | K::TypeAliasDeclaration
                | K::TypeParameter
                | K::VariableDeclaration
                | K::JSDocTypedefTag
                | K::JSDocCallbackTag
                | K::JSDocPropertyTag
                | K::NamedTupleMember
        )
    )
}
pub(super) fn const_assertion(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    Ok(matches!(
        n.kind().known(),
        Some(K::AsExpression | K::TypeAssertionExpression)
    ) && n.type_node().is_some_and(|n| {
        view.node(n).is_ok_and(|n| {
            tsr_ast::utilities_middle::is_const_type_reference(view, &n).unwrap_or(false)
        })
    }))
}
