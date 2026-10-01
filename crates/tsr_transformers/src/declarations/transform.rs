use super::{diagnostics, host::DeclarationEmitHost, tracker::Selector, tracker::Tracker, util};
use std::collections::{HashMap, HashSet};
use tsr_ast::{
    node_flags as nf, AstBuilder, Diagnostic, Factory, FactoryMethods, FileReference, JsString,
    NodeId, NodeListId, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::{
    emit_flags,
    emit_resolver::{DeclarationEmitResolver, SymbolAccessibilityResult},
    EmitContext,
};

pub(super) const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// The compiler options the declaration transformer reads, and the path of
/// the declaration file it produces (`declarationFilePath`).
#[derive(Clone, Debug, Default)]
pub struct DeclarationOptions {
    pub isolated_declarations: bool,
    pub strip_internal: bool,
    /// The output declaration file; preserved triple-slash path references
    /// are made relative to its directory.
    pub declaration_file_path: JsString,
}

/// The returned root belongs to `output`; original references were explicitly
/// retained before construction. The caller completes that owner before release.
/// `diagnostics` is the transformer's `GetDiagnostics`.
pub struct DeclarationTransform {
    pub root: NodeId,
    pub diagnostics: Vec<Diagnostic>,
}

/// Runs the declaration transformer over one source file
/// (`DeclarationTransformer.TransformSourceFile`).
pub fn transform_declarations<R: DeclarationEmitResolver>(
    resolver: &mut R,
    host: &dyn DeclarationEmitHost,
    output: &mut AstBuilder,
    emit: &mut EmitContext,
    source: NodeId,
    options: &DeclarationOptions,
) -> Result<DeclarationTransform, R::Error> {
    resolver.retain_source(source, output)?;
    let mut tx = Transformer::new(resolver, host, output, emit, source, options);
    let root = tx.visit(Some(source))?.expect(NIL);
    Ok(DeclarationTransform {
        root,
        diagnostics: tx.get_diagnostics(),
    })
}

/// A key of `seenProperties` (Go's `thisPropertyAssignmentKey`).
#[derive(Clone, Hash, PartialEq, Eq)]
pub(super) struct ThisPropertyAssignmentKey {
    pub name: JsString,
    pub node: Option<NodeId>,
    pub is_static: bool,
    pub is_private: bool,
}

/// `DeclarationTransformer` with its `SymbolTrackerSharedState`. The tracker
/// half that a resolver call holds is `tracker`; the diagnostics the reports
/// write are `diagnostics`.
pub(super) struct Transformer<'a, R: DeclarationEmitResolver> {
    pub resolver: &'a mut R,
    pub host: &'a dyn DeclarationEmitHost,
    pub output: &'a mut AstBuilder,
    pub emit: &'a mut EmitContext,
    pub isolated_declarations: bool,
    pub strip_internal: bool,
    pub declaration_file_path: JsString,
    pub tracker: Tracker,
    pub diagnostics: Vec<Diagnostic>,
    /// `state.currentSourceFile`.
    pub current_source_file: NodeId,

    pub needs_declare: bool,
    pub needs_scope_fix_marker: bool,
    pub result_has_scope_marker: bool,
    pub enclosing_declaration: NodeId,
    pub result_has_external_module_indicator: bool,
    pub suppress_new_diagnostic_contexts: bool,
    pub witnessed_cjs_exports: HashSet<JsString>,
    pub late_statement_replacement_map: HashMap<NodeId, Option<NodeId>>,
    pub expando_hosts: HashMap<NodeId, Option<NodeId>>,
    pub expando_members: HashMap<NodeId, Vec<NodeId>>,
    pub deferred_expando_assignments: HashMap<NodeId, Vec<NodeId>>,
    pub seen_properties: HashSet<ThisPropertyAssignmentKey>,
    pub this_property_assignments_collected: Vec<NodeId>,
    pub raw_referenced_files: Vec<(NodeId, FileReference)>,
    pub raw_type_reference_directives: Vec<FileReference>,
    pub raw_lib_reference_directives: Vec<FileReference>,

    pub cjs_export_assignment: Option<NodeId>,
    pub cjs_export_members: Vec<NodeId>,
    pub cjs_export_assignment_name: Option<NodeId>,
    pub in_class_expression_declaration: bool,
    /// The first resolver error inside a generated child visit.
    pub visitor_error: Option<R::Error>,
}

pub(super) const DECLARATION_EMIT_NODE_BUILDER_FLAGS: tsr_nodebuilder::Flags =
    tsr_nodebuilder::flags::MULTILINE_OBJECT_LITERALS
        | tsr_nodebuilder::flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
        | tsr_nodebuilder::flags::USE_TYPE_OF_FUNCTION
        | tsr_nodebuilder::flags::USE_STRUCTURAL_FALLBACK
        | tsr_nodebuilder::flags::ALLOW_EMPTY_TUPLE
        | tsr_nodebuilder::flags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS
        | tsr_nodebuilder::flags::NO_TRUNCATION;
pub(super) const DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS: tsr_nodebuilder::InternalFlags =
    tsr_nodebuilder::internal_flags::ALLOW_UNRESOLVED_NAMES;

/// The diagnostic context `setupDiagnosticContext` saves; its cleanup closure
/// restores it.
pub(super) struct DiagnosticContext {
    selector: Selector,
    error_name: Option<NodeId>,
    suppress: bool,
}

impl<'a, R: DeclarationEmitResolver> Transformer<'a, R> {
    // port: tsc/internal/transformers/declarations/transform.go:NewDeclarationTransformer
    pub fn new(
        resolver: &'a mut R,
        host: &'a dyn DeclarationEmitHost,
        output: &'a mut AstBuilder,
        emit: &'a mut EmitContext,
        source: NodeId,
        options: &DeclarationOptions,
    ) -> Self {
        Self {
            resolver,
            host,
            output,
            emit,
            isolated_declarations: options.isolated_declarations,
            strip_internal: options.strip_internal,
            declaration_file_path: options.declaration_file_path.clone(),
            tracker: Tracker::new(),
            diagnostics: Vec::new(),
            current_source_file: source,
            needs_declare: false,
            needs_scope_fix_marker: false,
            result_has_scope_marker: false,
            enclosing_declaration: source,
            result_has_external_module_indicator: false,
            suppress_new_diagnostic_contexts: false,
            witnessed_cjs_exports: HashSet::new(),
            late_statement_replacement_map: HashMap::new(),
            expando_hosts: HashMap::new(),
            expando_members: HashMap::new(),
            deferred_expando_assignments: HashMap::new(),
            seen_properties: HashSet::new(),
            this_property_assignments_collected: Vec::new(),
            raw_referenced_files: Vec::new(),
            raw_type_reference_directives: Vec::new(),
            raw_lib_reference_directives: Vec::new(),
            cjs_export_assignment: None,
            cjs_export_members: Vec::new(),
            cjs_export_assignment_name: None,
            in_class_expression_declaration: false,
            visitor_error: None,
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.GetDiagnostics
    pub fn get_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    // ---- node access -------------------------------------------------------

    /// Makes a node the resolver answered with, possibly from another file,
    /// readable through the output builder, which then retains its file.
    pub fn retain_source_of(&mut self, node: NodeId) -> Result<(), R::Error> {
        if self.view().node(node).is_err() {
            let source =
                tsr_ast::utilities::get_source_file_of_node(self.resolver.ast(node)?, Some(node))?
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
            self.resolver.retain_source(source, self.output)?;
        }
        Ok(())
    }

    pub fn node(&self, node: NodeId) -> tsr_ast::NodeRead<'_> {
        Factory::node(&*self.output, node)
    }
    pub fn view(&self) -> tsr_ast::AstView<'_> {
        self.output.view()
    }
    pub fn kind(&self, node: NodeId) -> K {
        self.node(node).kind().known().unwrap_or(K::Unknown)
    }
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.node(node).parent()
    }
    /// `node.Parent.Kind`, a nil parent being the pin's dereference panic.
    pub fn parent_kind(&self, node: NodeId) -> K {
        self.kind(self.parent(node).expect(NIL))
    }
    pub fn list_nodes(&self, list: Option<NodeListId>) -> Vec<NodeId> {
        list.map(|list| {
            self.output
                .read_nodes(self.output.read_list(list).nodes())
                .iter()
                .flatten()
                .collect()
        })
        .unwrap_or_default()
    }
    /// `Factory().NewNodeList(nodes)`.
    pub fn new_node_list(&mut self, nodes: Vec<NodeId>) -> NodeListId {
        let nodes = self
            .output
            .alloc_nodes(nodes.into_iter().map(Some).collect());
        self.output
            .alloc_list(tsr_core::TextRange::new(-1, -1), nodes)
    }
    /// `Factory().NewModifierList(nodes)`.
    pub fn new_modifier_list(&mut self, nodes: Vec<NodeId>) -> NodeListId {
        let nodes = self
            .output
            .alloc_nodes(nodes.into_iter().map(Some).collect());
        self.output.new_modifier_list(nodes)
    }
    pub fn new_syntax_list(&mut self, nodes: Vec<NodeId>) -> NodeId {
        let nodes = self
            .output
            .alloc_nodes(nodes.into_iter().map(Some).collect());
        self.output.new_syntax_list(nodes)
    }
    pub fn node_text(&self, node: NodeId) -> Result<JsString, R::Error> {
        Ok(self.view().node_text(node)?.into_js_string())
    }
    pub fn most_original(&self, node: NodeId) -> NodeId {
        self.emit.most_original(node)
    }
    pub fn parse_node(&self, node: NodeId) -> Option<NodeId> {
        self.emit.parse_node(&*self.output, node)
    }
    /// `host.GetEffectiveDeclarationFlags(EmitContext().ParseNode(node), flags)`.
    pub fn effective_declaration_flags(
        &mut self,
        node: NodeId,
        flags: u32,
    ) -> Result<u32, R::Error> {
        let node = self.parse_node(node).expect(NIL);
        self.resolver.effective_declaration_flags(node, flags)
    }
    /// `state.currentSourceFile.IsJS()`.
    pub fn is_source_file_js(&self) -> Result<bool, R::Error> {
        Ok(self
            .output
            .read_source_file(self.current_source_file)?
            .is_js())
    }
    /// `Factory().NewUniqueNameEx(text, {Flags: Optimistic})`.
    pub fn new_unique_name(&mut self, text: &[u8]) -> NodeId {
        self.emit.new_unique_name_ex(
            self.output,
            JsString::from_bytes(text),
            tsr_printer::AutoGenerateOptions {
                flags: tsr_printer::generated_identifier_flags::OPTIMISTIC,
                ..Default::default()
            },
        )
    }
    pub fn new_modifier(&mut self, kind: K) -> NodeId {
        self.output.new_modifier(kind.into())
    }
    /// `ast.CreateModifiersFromModifierFlags(flags, Factory().NewModifier)`.
    pub fn create_modifiers_from_modifier_flags(&mut self, flags: u32) -> Vec<NodeId> {
        tsr_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
            Some(self.output.new_modifier(kind))
        })
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
    }
    /// `ast.HasSyntacticModifier(node, flags)`.
    pub fn has_syntactic_modifier(&self, node: NodeId, flags: u32) -> Result<bool, R::Error> {
        Ok(tsr_ast::utilities::has_syntactic_modifier(
            self.view(),
            node,
            flags,
        )?)
    }
    /// `ast.GetCombinedModifierFlags(node)`.
    pub fn combined_modifier_flags(&self, node: NodeId) -> Result<u32, R::Error> {
        Ok(tsr_ast::utilities::get_combined_modifier_flags(
            self.view(),
            node,
        )?)
    }
    /// `NewVariableStatement(modifiers, NewVariableDeclarationList([NewVariableDeclaration(name, nil, type, initializer)], flags))`.
    pub fn new_single_variable_statement(
        &mut self,
        modifiers: Option<NodeListId>,
        name: NodeId,
        ty: Option<NodeId>,
        initializer: Option<NodeId>,
        flags: u32,
    ) -> NodeId {
        let declaration = self
            .output
            .new_variable_declaration(Some(name), None, ty, initializer);
        let declarations = self.new_node_list(vec![declaration]);
        let list = self
            .output
            .new_variable_declaration_list(Some(declarations), flags);
        self.output.new_variable_statement(modifiers, Some(list))
    }
    /// `NewExportDeclaration(nil, false, NewNamedExports(NewNodeList([NewExportSpecifier(false, propertyName, name)])), nil, nil)`.
    pub fn new_named_export_declaration(
        &mut self,
        property_name: Option<NodeId>,
        name: NodeId,
    ) -> NodeId {
        let specifier = self
            .output
            .new_export_specifier(false, property_name, Some(name));
        let list = self.new_node_list(vec![specifier]);
        let named = self.output.new_named_exports(Some(list));
        self.output
            .new_export_declaration(None, false, Some(named), None, None)
    }

    // ---- diagnostic context ----------------------------------------------------

    /// Installs `createGetSymbolAccessibilityDiagnosticForNode(node)`.
    pub fn set_diagnostic_context_for_node(&mut self, node: NodeId) -> Result<(), R::Error> {
        let view = self.resolver.ast(node)?;
        let getter = diagnostics::create_get_symbol_accessibility_diagnostic_for_node(view, node)?;
        self.tracker.selector = Selector::new(view, getter);
        Ok(())
    }
    /// Installs `createGetSymbolAccessibilityDiagnosticForNodeName(node)`.
    pub fn set_diagnostic_context_for_node_name(&mut self, node: NodeId) -> Result<(), R::Error> {
        let view = self.resolver.ast(node)?;
        let getter =
            diagnostics::create_get_symbol_accessibility_diagnostic_for_node_name(view, node)?;
        self.tracker.selector = Selector::new(view, getter);
        Ok(())
    }
    /// Installs a closure returning one fixed diagnostic.
    pub fn set_fixed_diagnostic_context(
        &mut self,
        diagnostic_message: &'static tsr_diagnostics::Message,
        error_node: NodeId,
        type_name: Option<NodeId>,
    ) {
        self.tracker.selector = Selector::fixed(diagnostics::SymbolAccessibilityDiagnostic {
            diagnostic_message,
            error_node: Some(error_node),
            type_name,
        });
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.setupDiagnosticContext
    pub fn setup_diagnostic_context(
        &mut self,
        input: NodeId,
    ) -> Result<(bool, DiagnosticContext), R::Error> {
        let can_produce_diagnostic = util::can_produce_diagnostics(&self.node(input));
        let old_within_object_literal_type = self.suppress_new_diagnostic_contexts;
        let should_enter_suppress_new_diagnostics_context_context =
            matches!(self.kind(input), K::TypeLiteral | K::MappedType)
                && !matches!(
                    self.parent_kind(input),
                    K::TypeAliasDeclaration | K::JSTypeAliasDeclaration
                );
        let old_diag = self.tracker.selector.clone();
        if can_produce_diagnostic && !self.suppress_new_diagnostic_contexts {
            self.set_diagnostic_context_for_node(input)?;
        }
        let old_name = self.tracker.error_name;
        if should_enter_suppress_new_diagnostics_context_context {
            self.suppress_new_diagnostic_contexts = true;
        }
        Ok((
            can_produce_diagnostic,
            DiagnosticContext {
                selector: old_diag,
                error_name: old_name,
                suppress: old_within_object_literal_type,
            },
        ))
    }
    /// The cleanup closure of `setupDiagnosticContext`.
    pub fn cleanup_diagnostic_context(&mut self, context: DiagnosticContext) {
        self.tracker.selector = context.selector;
        self.tracker.error_name = context.error_name;
        self.suppress_new_diagnostic_contexts = context.suppress;
    }

    // ---- the visitor -------------------------------------------------------------

    /// Functions as both `visitDeclarationStatements` and `transformRoot`,
    /// utilizing `SyntaxList` nodes.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visit
    pub fn visit(&mut self, node: Option<NodeId>) -> Result<Option<NodeId>, R::Error> {
        let Some(node) = node else { return Ok(None) };
        match self.kind(node) {
            K::SourceFile => self.visit_source_file(node).map(Some),
            // statements we keep but do something to
            K::FunctionDeclaration
            | K::ModuleDeclaration
            | K::ImportEqualsDeclaration
            | K::InterfaceDeclaration
            | K::ClassDeclaration
            | K::JSTypeAliasDeclaration
            | K::TypeAliasDeclaration
            | K::EnumDeclaration
            | K::VariableStatement
            | K::ImportDeclaration
            | K::JSImportDeclaration
            | K::ExportDeclaration
            | K::ExportAssignment => self.visit_declaration_statements(node),
            // statements we elide
            K::BreakStatement
            | K::ContinueStatement
            | K::DebuggerStatement
            | K::DoStatement
            | K::EmptyStatement
            | K::ForInStatement
            | K::ForOfStatement
            | K::ForStatement
            | K::IfStatement
            | K::LabeledStatement
            | K::ReturnStatement
            | K::SwitchStatement
            | K::ThrowStatement
            | K::TryStatement
            | K::WhileStatement
            | K::WithStatement
            | K::NotEmittedStatement
            | K::Block
            | K::MissingDeclaration
            | K::ExpressionStatement => Ok(None),
            // parts of things, things we just visit children of
            _ => self.visit_declaration_subtree(node),
        }
    }
    /// `Visitor().VisitEachChild(node)`.
    pub fn visit_each_child(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let result = tsr_ast::VisitorMethods::visit_each_child_generated(self, node);
        match self.visitor_error.take() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }
    /// `Visitor().VisitSlice(nodes)`: the visited nodes, with syntax lists
    /// spliced and nil results dropped, and whether anything changed.
    pub fn visit_slice(&mut self, nodes: &[NodeId]) -> Result<(Vec<NodeId>, bool), R::Error> {
        let mut result = Vec::new();
        let mut changed = false;
        for &node in nodes {
            let visited = self.visit(Some(node))?;
            match visited {
                None => changed = true,
                Some(visited) if visited == node => result.push(visited),
                Some(visited) => {
                    changed = true;
                    result.extend(self.node_or_syntax_list_children(visited));
                }
            }
        }
        Ok((result, changed))
    }
    /// `Visitor().VisitNodes(list)`: the same list when nothing changed.
    pub fn visit_nodes(
        &mut self,
        list: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, R::Error> {
        let Some(original) = list else {
            return Ok(None);
        };
        let nodes = self.list_nodes(list);
        let (visited, changed) = self.visit_slice(&nodes)?;
        if !changed {
            return Ok(list);
        }
        let loc = self.output.read_list(original).loc();
        let result = self.new_node_list(visited);
        self.output.set_list_location(result, loc)?;
        Ok(Some(result))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitSourceFile
    fn visit_source_file(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        self.cjs_export_assignment_name = None;
        if self.output.read_source_file(node)?.is_declaration_file {
            return Ok(node);
        }
        self.needs_declare = true;
        self.needs_scope_fix_marker = false;
        self.result_has_scope_marker = false;
        self.enclosing_declaration = node;
        self.tracker.selector = Selector::throw();
        self.result_has_external_module_indicator = false;
        self.suppress_new_diagnostic_contexts = false;
        self.tracker.late_marked = Vec::new();
        self.late_statement_replacement_map = HashMap::new();
        self.expando_hosts = HashMap::new();
        self.expando_members = HashMap::new();
        self.deferred_expando_assignments = HashMap::new();
        self.raw_referenced_files = Vec::new();
        self.raw_type_reference_directives = Vec::new();
        self.raw_lib_reference_directives = Vec::new();
        self.witnessed_cjs_exports.clear();
        self.current_source_file = node;
        self.collect_file_references(node)?;
        let original = self.most_original(node);
        self.resolver
            .precalculate_declaration_emit_visibility(original)?;
        self.transform_source_file(node)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.collectFileReferences
    fn collect_file_references(&mut self, source_file: NodeId) -> Result<(), R::Error> {
        let file = self.output.read_source_file(source_file)?;
        let referenced: Vec<_> = file
            .referenced_files()?
            .iter()
            .map(|reference| (source_file, reference.clone()))
            .collect();
        let types = file.type_reference_directives()?.to_vec();
        let libs = file.lib_reference_directives()?.to_vec();
        self.raw_referenced_files.extend(referenced);
        self.raw_type_reference_directives.extend(types);
        self.raw_lib_reference_directives.extend(libs);
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:nodeOrSyntaxListChildren
    pub fn node_or_syntax_list_children(&self, node: NodeId) -> Vec<NodeId> {
        if let Some(data) = self.node(node).as_syntax_list() {
            return self
                .output
                .read_nodes(data.children())
                .iter()
                .flatten()
                .collect();
        }
        vec![node]
    }

    // port: tsc/internal/transformers/declarations/transform.go:flattenSyntaxLists
    fn flatten_syntax_lists(&self, nodes: &[NodeId]) -> Vec<NodeId> {
        nodes
            .iter()
            .flat_map(|&node| self.node_or_syntax_list_children(node))
            .collect()
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.appendCjsExports
    fn append_cjs_exports(&mut self, combined_statements: NodeListId) -> NodeListId {
        let mut result = Vec::new();
        if let Some(assignment) = self.cjs_export_assignment {
            result.push(assignment);
        }
        result.extend(self.cjs_export_members.iter().copied());
        let combined = self.list_nodes(Some(combined_statements));
        let combined_len = combined.len();
        result.extend(combined);
        let statement_nodes = self.flatten_syntax_lists(&result);
        if statement_nodes.len() != combined_len {
            return self.new_node_list(statement_nodes);
        }
        combined_statements
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformSourceFile
    fn transform_source_file(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        self.cjs_export_assignment = None;
        self.cjs_export_assignment_name = None;
        self.cjs_export_members = Vec::new();
        let result = self.transform_source_file_worker(node);
        self.cjs_export_assignment = None;
        self.cjs_export_assignment_name = None;
        self.cjs_export_members = Vec::new();
        result
    }
    fn transform_source_file_worker(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        // collect nested module.exports= assignments
        self.walk_expressions(node, Self::visit_cjs_export_assignments)?;
        // collect expando members (requires any export assignment be located in advance)
        self.walk_expressions(node, Self::visit_nested_expression)?;
        let (statements, end_of_file_token) = {
            let read = self.node(node);
            let data = read.as_source_file().expect("source file payload");
            (data.statements(), data.end_of_file_token())
        };
        let statements = self.visit_nodes(statements)?.expect(NIL);
        let combined_statements = self.transform_and_replace_late_painted_statements(statements)?;
        let mut combined_statements = self.append_cjs_exports(combined_statements);
        let statements_loc = self.output.read_list(statements).loc();
        self.output
            .set_list_location(combined_statements, statements_loc)?; // setTextRange
        let file = self.output.read_source_file(node)?;
        if tsr_ast::utilities::is_external_or_common_js_module(&file) {
            if tsr_ast::utilities::is_in_js_file(Some(&self.node(node))) {
                self.report_multiple_export_equals(node)?;
            }
            if !self.result_has_external_module_indicator
                || (self.needs_scope_fix_marker && !self.result_has_scope_marker)
            {
                let marker = self.create_empty_exports();
                let mut new_list = self.list_nodes(Some(combined_statements));
                new_list.push(marker);
                let loc = self.output.read_list(combined_statements).loc();
                let with_marker = self.new_node_list(new_list);
                self.output.set_list_location(with_marker, loc)?;
                combined_statements = with_marker;
            }
        }
        let output_file_path = tsr_tspath::directory(&tsr_tspath::normalize_slashes(
            self.declaration_file_path.as_bytes(),
        ));
        let result =
            self.output
                .update_source_file(node, Some(combined_statements), end_of_file_token);
        let lib_references = self.get_lib_references();
        let lib_references = self.output.source_references(lib_references)?;
        let type_references = self.get_type_references();
        let type_references = self.output.source_references(type_references)?;
        let referenced_files = self.get_referenced_files(&output_file_path)?;
        let referenced_files = self.output.source_references(referenced_files)?;
        let file = self.output.mut_source_file(result)?;
        file.lib_reference_directives = lib_references;
        file.type_reference_directives = type_references;
        file.is_declaration_file = true;
        file.referenced_files = referenced_files;
        Ok(result)
    }
    /// The `export=` declarations check of `transformSourceFile` for a JS
    /// module.
    fn report_multiple_export_equals(&mut self, node: NodeId) -> Result<(), R::Error> {
        let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? else {
            return Ok(());
        };
        let Some(export_equals) = self
            .resolver
            .symbol_export(symbol, tsr_ast::internal_symbol_names::EXPORT_EQUALS)?
        else {
            return Ok(());
        };
        let declarations = self.resolver.symbol_declarations(export_equals)?;
        if declarations.len() > 1 {
            for declaration in declarations {
                self.add_diagnostic_for_node(
                    declaration,
                    tsr_diagnostics::Multiple_module_exports_assignments_cannot_be_serialized_for_declaration_emit,
                    vec![],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:createEmptyExports
    pub fn create_empty_exports(&mut self) -> NodeId {
        let list = self.new_node_list(Vec::new());
        let exports = self.output.new_named_exports(Some(list));
        self.output
            .new_export_declaration(None, false, Some(exports), None, None)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformAndReplaceLatePaintedStatements
    pub fn transform_and_replace_late_painted_statements(
        &mut self,
        statements: NodeListId,
    ) -> Result<NodeListId, R::Error> {
        // This is a `while` loop because `handleSymbolAccessibilityError` can see additional import aliases marked as visible during
        // error handling which must now be included in the output and themselves checked for errors.
        while !self.tracker.late_marked.is_empty() {
            let next = self.tracker.late_marked.remove(0);
            // A late-marked declaration can come from a merged declaration in
            // another file; its syntax must outlive the replacement made here.
            self.retain_source_of(next)?;
            let save_needs_declare = self.needs_declare;
            self.needs_declare = self
                .parent(next)
                .is_some_and(|parent| self.kind(parent) == K::SourceFile);
            let result = self.transform_top_level_declaration(next);
            self.needs_declare = save_needs_declare;
            let result = result?;
            let original = self.most_original(next);
            self.late_statement_replacement_map.insert(original, result);
        }
        // And lastly, we need to get the final form of all those indetermine import declarations from before and add them to the output list
        // (and remove them from the set to examine for outter declarations)
        let mut results = Vec::new();
        for statement in self.list_nodes(Some(statements)) {
            if !tsr_ast::utilities_middle::is_late_visibility_painted_statement(
                &self.node(statement),
            ) {
                results.push(statement);
                continue;
            }
            let original = self.most_original(statement);
            let Some(&replacement) = self.late_statement_replacement_map.get(&original) else {
                results.push(statement);
                continue; // not replaced
            };
            let Some(replacement) = replacement else {
                continue; // deleted
            };
            let parent_is_source_file = self.parent_kind(statement) == K::SourceFile;
            if self.kind(replacement) == K::SyntaxList {
                let children = self.node_or_syntax_list_children(replacement);
                if !self.needs_scope_fix_marker || !self.result_has_external_module_indicator {
                    for &elem in &children {
                        if util::needs_scope_marker(self.view(), elem)? {
                            self.needs_scope_fix_marker = true;
                        }
                        if parent_is_source_file
                            && tsr_ast::utilities_modules::is_external_module_indicator(
                                self.view(),
                                elem,
                            )?
                        {
                            self.result_has_external_module_indicator = true;
                        }
                    }
                }
                results.extend(children);
            } else {
                if util::needs_scope_marker(self.view(), replacement)? {
                    self.needs_scope_fix_marker = true;
                }
                if parent_is_source_file
                    && tsr_ast::utilities_modules::is_external_module_indicator(
                        self.view(),
                        replacement,
                    )?
                {
                    self.result_has_external_module_indicator = true;
                }
                results.push(replacement);
            }
        }
        Ok(self.new_node_list(results))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getReferencedFiles
    fn get_referenced_files(
        &mut self,
        output_file_path: &[u8],
    ) -> Result<Vec<FileReference>, R::Error> {
        // Handle path rewrites for triple slash ref comments
        let mut results = Vec::new();
        for (source_file, reference) in self.raw_referenced_files.clone() {
            if !reference.preserve {
                continue;
            }
            let Some(file) = self
                .host
                .get_source_file_from_reference(source_file, &reference)
            else {
                continue;
            };
            let (is_declaration_file, file_name) = {
                let view = self.resolver.ast(file)?;
                let state = view.source_file(file)?;
                (state.is_declaration_file, state.file_name().to_vec())
            };
            let decl_file_name = if is_declaration_file {
                file_name
            } else {
                let paths = self.host.get_output_paths_for(file, true);
                // Try to use output path for referenced file, or output js path if that doesn't exist, or the input path if all else fails
                let mut decl_file_name = paths.declaration_file_path().to_vec();
                if decl_file_name.is_empty() {
                    decl_file_name = paths.js_file_path().to_vec();
                }
                if decl_file_name.is_empty() {
                    decl_file_name = file_name;
                }
                decl_file_name
            };
            // Should only be missing if the source file is missing a fileName (at which point we can't name a reference to it anyway)
            if decl_file_name.is_empty() {
                continue;
            }
            let file_name = tsr_tspath::relative_to_directory_or_url(
                output_file_path,
                &decl_file_name,
                false,
                self.host.get_current_directory(),
                self.host.use_case_sensitive_file_names(),
            );
            results.push(FileReference {
                loc: tsr_core::TextRange::new(-1, -1),
                file_name: JsString::from_bytes(file_name),
                resolution_mode: reference.resolution_mode,
                preserve: reference.preserve,
            });
        }
        Ok(results)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getLibReferences
    fn get_lib_references(&self) -> Vec<FileReference> {
        // clone retained references
        retained_references(&self.raw_lib_reference_directives)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getTypeReferences
    fn get_type_references(&self) -> Vec<FileReference> {
        // clone retained references
        retained_references(&self.raw_type_reference_directives)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitDeclarationStatements
    fn visit_declaration_statements(&mut self, input: NodeId) -> Result<Option<NodeId>, R::Error> {
        if self.should_strip_internal(Some(input))? {
            return Ok(None);
        }
        match self.kind(input) {
            K::ExportDeclaration => {
                if self.parent_kind(input) == K::SourceFile {
                    self.result_has_external_module_indicator = true;
                }
                self.result_has_scope_marker = true;
                // Rewrite external module names if necessary
                let (modifiers, is_type_only, export_clause, module_specifier, attributes) = {
                    let read = self.node(input);
                    let data = read
                        .as_export_declaration()
                        .expect("export declaration payload");
                    (
                        read.modifiers(),
                        data.is_type_only(),
                        data.export_clause(),
                        data.module_specifier(),
                        data.attributes(),
                    )
                };
                let module_specifier = self.rewrite_module_specifier(input, module_specifier);
                Ok(Some(self.output.update_export_declaration(
                    input,
                    modifiers,
                    is_type_only,
                    export_clause,
                    module_specifier,
                    attributes,
                )))
            }
            K::ExportAssignment => {
                let (expression, is_export_equals) = {
                    let read = self.node(input);
                    let data = read
                        .as_export_assignment()
                        .expect("export assignment payload");
                    (data.expression().expect(NIL), data.is_export_equals())
                };
                self.transform_export_assignment(input, input, expression, is_export_equals)
                    .map(Some)
            }
            _ => {
                let id = self.most_original(input);
                if self
                    .late_statement_replacement_map
                    .get(&id)
                    .copied()
                    .flatten()
                    .is_none()
                {
                    // Don't actually transform yet; just leave as original node - will be elided/swapped by late pass
                    let result = self.transform_top_level_declaration(input)?;
                    self.late_statement_replacement_map.insert(id, result);
                }
                Ok(Some(input))
            }
        }
    }

    /// Transforms the direct child of a source file into zero or more replacement statements
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformTopLevelDeclaration
    pub fn transform_top_level_declaration(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if !self.tracker.late_marked.is_empty() {
            // Remove duplicates of the current statement from the deferred work queue (this was done via orderedRemoveItem in strada - why? to ensure the same backing array? microop?)
            self.tracker.late_marked.retain(|node| *node != input);
        }
        if self.should_strip_internal(Some(input))? {
            return Ok(None);
        }
        if self.kind(input) == K::ImportEqualsDeclaration {
            return self.transform_import_equals_declaration(input);
        }
        if matches!(
            self.kind(input),
            K::ImportDeclaration | K::JSImportDeclaration
        ) {
            let res = self.transform_import_declaration(input)?;
            if let Some(res) = res {
                if self.kind(res) != K::ImportDeclaration {
                    let data = self.node(res).data().to_owned();
                    let clone = self.output.new_node(K::ImportDeclaration.into(), data);
                    return Ok(Some(self.output.finish_clone(clone, res)));
                }
            }
            return Ok(res);
        }
        if tsr_ast::is_declaration(&self.node(input))
            && util::is_declaration_and_not_visible(self, input)?
        {
            return Ok(None);
        }
        // Elide implementation signatures from overload sets
        if tsr_ast::utilities::is_function_like(Some(&self.node(input)))
            && self.resolver.implementation_of_overload(input)?
        {
            return Ok(None);
        }
        let original = self.most_original(input);
        let is_expando_host = self.expando_hosts.contains_key(&original);
        let has_deferred_expando_assignments =
            self.deferred_expando_assignments.contains_key(&original);
        if is_expando_host || has_deferred_expando_assignments {
            return self.create_full_expando_block(original);
        }
        let previous_enclosing_declaration = self.enclosing_declaration;
        if util::is_enclosing_declaration(&self.node(input)) {
            self.enclosing_declaration = input;
        }
        let can_produce_diagnostic = util::can_produce_diagnostics(&self.node(input));
        let old_diag = self.tracker.selector.clone();
        let old_name = self.tracker.error_name;
        let save_needs_declare = self.needs_declare;
        let result = self.transform_top_level_declaration_worker(input, can_produce_diagnostic);
        self.enclosing_declaration = previous_enclosing_declaration;
        self.tracker.selector = old_diag;
        self.needs_declare = save_needs_declare;
        self.tracker.error_name = old_name;
        result
    }
    fn transform_top_level_declaration_worker(
        &mut self,
        input: NodeId,
        can_produce_diagnostic: bool,
    ) -> Result<Option<NodeId>, R::Error> {
        if can_produce_diagnostic {
            self.set_diagnostic_context_for_node(input)?;
        }
        match self.kind(input) {
            K::TypeAliasDeclaration | K::JSTypeAliasDeclaration => {
                self.transform_type_alias_declaration(input).map(Some)
            }
            K::InterfaceDeclaration => self.transform_interface_declaration(input).map(Some),
            K::FunctionDeclaration => self.transform_function_declaration(input).map(Some),
            K::ModuleDeclaration => self.transform_module_declaration(input).map(Some),
            K::ClassDeclaration => self.transform_class_declaration(input).map(Some),
            K::VariableStatement => self.transform_variable_statement(input),
            K::EnumDeclaration => self.transform_enum_declaration(input).map(Some),
            // Anything left unhandled is an error, so this should be unreachable
            _ => panic!(
                "Unhandled top-level node in declaration emit: {:?}",
                diagnostics::kind_string(self.node(input).kind())
            ),
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.checkEntityNameVisibility
    pub fn check_entity_name_visibility(
        &mut self,
        entity_name: NodeId,
        enclosing_declaration: NodeId,
    ) -> Result<(), R::Error> {
        let visibility_result = self
            .resolver
            .entity_name_visible(entity_name, enclosing_declaration)?;
        self.handle_symbol_accessibility_error(visibility_result)
    }

    /// `tracker.handleSymbolAccessibilityError(result)` and the diagnostic it
    /// writes.
    pub fn handle_symbol_accessibility_error(
        &mut self,
        result: SymbolAccessibilityResult,
    ) -> Result<(), R::Error> {
        self.tracker.handle_symbol_accessibility_error(result);
        self.apply_tracker_reports()
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.rewriteModuleSpecifier
    pub fn rewrite_module_specifier(
        &mut self,
        parent: NodeId,
        input: Option<NodeId>,
    ) -> Option<NodeId> {
        let input = input?;
        self.result_has_external_module_indicator = self.result_has_external_module_indicator
            || !matches!(self.kind(parent), K::ModuleDeclaration | K::ImportType);
        Some(input)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.preserveJsDoc
    pub fn preserve_js_doc(&mut self, updated: NodeId, original: NodeId) {
        // Copy comment range from original to updated node so JSDoc comments are preserved
        self.emit
            .assign_comment_range(&*self.output, updated, original);
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.preservePartialJsDoc
    pub fn preserve_partial_js_doc(
        &mut self,
        updated: NodeId,
        original: NodeId,
    ) -> Result<(), R::Error> {
        if self.node(original).flags() & nf::REPARSED == 0 {
            return Ok(());
        }
        let view = self.resolver.ast(original)?;
        let source = tsr_ast::utilities::get_source_file_of_node(view, Some(original))?;
        let roots = match source {
            Some(source) => match view.source_eager_jsdoc(source, original)? {
                Some(roots) => Some(roots),
                None => view.eager_jsdoc(original)?,
            },
            None => view.eager_jsdoc(original)?,
        };
        let Some(jsdoc) = roots.and_then(|roots| roots.iter().next().copied()) else {
            return Ok(());
        };
        let comment = view
            .node(jsdoc)?
            .data_source()
            .as_js_doc()
            .expect("JSDoc payload")
            .comment();
        let description = tsr_scanner::get_text_of_jsdoc_comment(view, comment)?;
        if description.is_empty() {
            return Ok(());
        }
        let mut comment = b"*\n * ".to_vec();
        for &byte in description.as_bytes() {
            if byte == b'\n' {
                comment.extend_from_slice(b"\n * ");
            } else {
                comment.push(byte);
            }
        }
        comment.extend_from_slice(b"\n ");
        self.emit.add_synthetic_leading_comment(
            updated,
            K::MultiLineCommentTrivia,
            JsString::from_bytes(comment),
            true, /*hasTrailingNewLine*/
        );
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.removeAllComments
    pub fn remove_all_comments(&mut self, node: NodeId) {
        self.emit.add_emit_flags(node, emit_flags::NO_COMMENTS);
        // !!! TODO: Also remove synthetic trailing/leading comments added by transforms
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.shouldStripInternal
    pub fn should_strip_internal(&self, node: Option<NodeId>) -> Result<bool, R::Error> {
        Ok(self.strip_internal
            && node.is_some()
            && self.is_internal_declaration(node, self.current_source_file)?)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.isInternalDeclaration
    fn is_internal_declaration(
        &self,
        node: Option<NodeId>,
        source_file: NodeId,
    ) -> Result<bool, R::Error> {
        let Some(node) = node else {
            return Ok(false);
        };
        let parse_tree_node = self.most_original(node);
        if !tsr_ast::utilities_positions::is_parse_tree_node(&self.node(parse_tree_node)) {
            return Ok(false);
        }
        let file = self.output.read_source_file(source_file)?;
        let text = file.text().as_bytes();
        if self.kind(parse_tree_node) == K::Parameter {
            let parent = self.parent(parse_tree_node).expect(NIL);
            let params = self.list_nodes(self.node(parent).parameter_list());
            let param_idx = params.iter().position(|p| *p == parse_tree_node);
            let previous_sibling = match param_idx {
                Some(index) if index > 0 => Some(params[index - 1]),
                _ => None,
            };
            let mut comment_ranges = Vec::new();
            let stop = tsr_scanner::SkipTriviaOptions {
                stop_at_comments: true,
                ..Default::default()
            };
            if let Some(previous_sibling) = previous_sibling {
                // to handle
                // ... parameters, /** @internal */
                // public param: string
                let trailing_pos = tsr_scanner::skip_trivia_ex(
                    text,
                    i64::from(self.node(previous_sibling).end()) + 1,
                    Some(&stop),
                );
                comment_ranges.extend(tsr_scanner::get_trailing_comment_ranges(text, trailing_pos));
                comment_ranges.extend(tsr_scanner::get_leading_comment_ranges(
                    text,
                    i64::from(self.node(node).pos()),
                ));
            } else {
                let trailing_pos = tsr_scanner::skip_trivia_ex(
                    text,
                    i64::from(self.node(node).pos()),
                    Some(&stop),
                );
                comment_ranges.extend(tsr_scanner::get_trailing_comment_ranges(text, trailing_pos));
            }
            if let Some(last) = comment_ranges.last() {
                return Ok(has_internal_annotation(*last, text));
            }
            return Ok(false);
        }
        for comment_range in self.get_leading_comment_ranges_of_node(Some(parse_tree_node), text) {
            if has_internal_annotation(comment_range, text) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getLeadingCommentRangesOfNode
    fn get_leading_comment_ranges_of_node(
        &self,
        node: Option<NodeId>,
        text: &[u8],
    ) -> Vec<tsr_scanner::CommentRange> {
        let Some(node) = node else {
            return Vec::new();
        };
        if self.kind(node) == K::JsxText {
            return Vec::new();
        }
        tsr_scanner::get_leading_comment_ranges(text, i64::from(self.node(node).pos())).collect()
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerSharedState.addDiagnostic
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// `state.addDiagnostic(createDiagnosticForNode(node, message, args...))`.
    pub fn add_diagnostic_for_node(
        &mut self,
        node: NodeId,
        message: &'static tsr_diagnostics::Message,
        args: Vec<JsString>,
    ) -> Result<(), R::Error> {
        let diagnostic =
            diagnostics::create_diagnostic_for_node(self.resolver.ast(node)?, node, message, args)?;
        self.add_diagnostic(diagnostic);
        Ok(())
    }

    /// The builder flags of a serialization request: a class expression kept
    /// as a class declaration is not written as a type literal.
    pub fn builder_flags(&self) -> tsr_nodebuilder::Flags {
        if self.in_class_expression_declaration {
            DECLARATION_EMIT_NODE_BUILDER_FLAGS
                & !tsr_nodebuilder::flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
        } else {
            DECLARATION_EMIT_NODE_BUILDER_FLAGS
        }
    }

    /// The closure `NewDeclarationTransformer` installs as
    /// `state.reportExpandoFunctionErrors`.
    pub fn report_expando_function_errors(&mut self, node: NodeId) -> Result<(), R::Error> {
        if !self.isolated_declarations {
            return Ok(());
        }
        let props = self.resolver.properties_of_container_function(node)?;
        for p in props {
            let value_declaration = self.resolver.symbol_value_declaration(p)?;
            let Some(value_declaration) = value_declaration else {
                continue;
            };
            let view = self.resolver.ast(value_declaration)?;
            if tsr_ast::utilities_tail::is_expando_property_declaration(Some(
                &view.node(value_declaration)?,
            )) {
                let mut error_target = value_declaration;
                if view.node(error_target)?.kind() == K::BinaryExpression {
                    error_target = view
                        .node(error_target)?
                        .as_binary_expression()
                        .expect("binary expression payload")
                        .left()
                        .expect(NIL);
                }
                self.add_diagnostic_for_node(
                    error_target,
                    tsr_diagnostics::Assigning_properties_to_functions_without_declaring_them_is_not_supported_with_isolatedDeclarations_Add_an_explicit_declaration_for_the_properties_assigned_to_this_function,
                    vec![],
                )?;
            }
        }
        Ok(())
    }

    /// `[declare]` when the declaration needs `declare`; otherwise an empty
    /// modifier list, or none when `none_when_empty`.
    pub fn declare_modifier_list(&mut self, none_when_empty: bool) -> Option<NodeListId> {
        if self.needs_declare {
            let declare = self.new_modifier(K::DeclareKeyword);
            Some(self.new_modifier_list(vec![declare]))
        } else if none_when_empty {
            None
        } else {
            Some(self.new_modifier_list(Vec::new()))
        }
    }
}

/// The preserved references, cloned with no position.
fn retained_references(references: &[FileReference]) -> Vec<FileReference> {
    references
        .iter()
        .filter(|reference| reference.preserve)
        .map(|reference| FileReference {
            loc: tsr_core::TextRange::new(-1, -1),
            file_name: reference.file_name.clone(),
            resolution_mode: reference.resolution_mode,
            preserve: reference.preserve,
        })
        .collect()
}

// port: tsc/internal/transformers/declarations/transform.go:hasInternalAnnotation
fn has_internal_annotation(comment_range: tsr_scanner::CommentRange, text: &[u8]) -> bool {
    let start = usize::try_from(comment_range.loc.pos()).expect("comment start");
    let end = usize::try_from(comment_range.loc.end()).expect("comment end");
    let comment = &text[start..end];
    comment
        .windows(b"@internal".len())
        .any(|window| window == b"@internal")
}
