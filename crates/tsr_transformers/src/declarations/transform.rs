use super::tracker::{Pending, Selector, Tracker};
use std::collections::HashMap;
use tsr_ast::{
    AstBuilder, Diagnostic, Factory, JsString, NodeId, NodeListId, RuntimeFactory, SyntaxKind as K,
    VisitorMethods,
};
use tsr_printer::{
    emit_resolver::{DeclarationEmitResolver, DeclarationTrackerEvent as Event},
    EmitContext,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct DeclarationOptions {
    pub isolated_declarations: bool,
    pub strip_internal: bool,
}

/// The returned root belongs to `output`; original references were explicitly
/// retained before construction. The caller completes that owner before release.
pub struct DeclarationTransform {
    pub root: NodeId,
    pub diagnostics: Vec<Diagnostic>,
}

// port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitSourceFile
pub fn transform_declarations<R: DeclarationEmitResolver>(
    resolver: &mut R,
    output: &mut AstBuilder,
    emit: &mut EmitContext,
    source: NodeId,
    options: DeclarationOptions,
) -> Result<DeclarationTransform, R::Error> {
    resolver.retain_source(source, output)?;
    if resolver
        .ast(source)?
        .source_file(source)?
        .is_declaration_file
    {
        return Ok(DeclarationTransform {
            root: source,
            diagnostics: Vec::new(),
        });
    }
    resolver.precalculate_declaration_emit_visibility(source)?;
    let mut tx = Transformer {
        resolver,
        output,
        emit,
        source,
        options,
        tracker: Tracker::default(),
        diagnostics: Vec::new(),
        enclosing: source,
        suppress_context: false,
        in_class_expression_declaration: false,
        needs_declare: true,
        needs_scope_marker: false,
        has_scope_marker: false,
        external_indicator: false,
        replacements: HashMap::new(),
        visitor_error: None,
        cjs: super::common_js::CommonJsState::default(),
    };
    let root = tx.source_file(source)?;
    Ok(DeclarationTransform {
        root,
        diagnostics: tx.diagnostics,
    })
}

pub(super) struct Transformer<'a, R: DeclarationEmitResolver> {
    pub resolver: &'a mut R,
    pub output: &'a mut AstBuilder,
    pub emit: &'a mut EmitContext,
    pub source: NodeId,
    pub options: DeclarationOptions,
    pub tracker: Tracker,
    pub diagnostics: Vec<Diagnostic>,
    pub enclosing: NodeId,
    pub suppress_context: bool,
    pub in_class_expression_declaration: bool,
    pub needs_declare: bool,
    pub needs_scope_marker: bool,
    pub has_scope_marker: bool,
    pub external_indicator: bool,
    pub replacements: HashMap<NodeId, Option<NodeId>>,
    pub visitor_error: Option<R::Error>,
    pub cjs: super::common_js::CommonJsState,
}

pub(super) const BUILDER_FLAGS: tsr_nodebuilder::Flags =
    tsr_nodebuilder::flags::MULTILINE_OBJECT_LITERALS
        | tsr_nodebuilder::flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
        | tsr_nodebuilder::flags::USE_TYPE_OF_FUNCTION
        | tsr_nodebuilder::flags::USE_STRUCTURAL_FALLBACK
        | tsr_nodebuilder::flags::ALLOW_EMPTY_TUPLE
        | tsr_nodebuilder::flags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS
        | tsr_nodebuilder::flags::NO_TRUNCATION;
pub(super) const INTERNAL_FLAGS: tsr_nodebuilder::InternalFlags =
    tsr_nodebuilder::internal_flags::ALLOW_UNRESOLVED_NAMES;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    pub fn builder_flags(&self) -> tsr_nodebuilder::Flags {
        if self.in_class_expression_declaration {
            BUILDER_FLAGS & !tsr_nodebuilder::flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
        } else {
            BUILDER_FLAGS
        }
    }
    pub fn unsupported<T>(name: &'static str) -> Result<T, R::Error> {
        Err(R::unsupported(name))
    }
    pub fn node(&self, node: NodeId) -> tsr_ast::NodeRead<'_> {
        Factory::node(&*self.output, node)
    }
    pub fn required(node: Option<NodeId>) -> Result<NodeId, R::Error> {
        node.ok_or_else(|| tsr_arena::Error::InvalidGraph.into())
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
    pub fn new_list(&mut self, nodes: Vec<NodeId>) -> NodeListId {
        let nodes = self
            .output
            .alloc_nodes(nodes.into_iter().map(Some).collect());
        self.output
            .alloc_list(tsr_core::TextRange::new(-1, -1), nodes)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visit
    pub fn visit(&mut self, input: Option<NodeId>) -> Result<Option<NodeId>, R::Error> {
        let Some(node) = input else { return Ok(None) };
        match self.node(node).kind().known() {
            Some(K::SourceFile) => self.source_file(node).map(Some),
            Some(
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
                | K::ExportAssignment,
            ) => self.statement(node),
            Some(
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
                | K::ExpressionStatement,
            ) => Ok(None),
            _ => self.subtree(node),
        }
    }
    pub fn children(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let result = self.visit_each_child_generated(node);
        match self.visitor_error.take() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }
    pub fn visit_list_result(
        &mut self,
        list: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, R::Error> {
        let Some(original) = list else {
            return Ok(None);
        };
        let mut nodes = Vec::new();
        for node in self.list_nodes(list) {
            if let Some(result) = self.visit(Some(node))? {
                self.append_flat(result, &mut nodes);
            }
        }
        if nodes == self.list_nodes(list) {
            return Ok(list);
        }
        let loc = self.output.read_list(original).loc();
        let list = self.new_list(nodes);
        self.output.set_list_location(list, loc)?;
        Ok(Some(list))
    }
    pub fn append_flat(&self, node: NodeId, nodes: &mut Vec<NodeId>) {
        if let Some(data) = self.node(node).as_syntax_list() {
            nodes.extend(self.output.read_nodes(data.children()).iter().flatten());
        } else {
            nodes.push(node);
        }
    }
    pub fn select_context(&mut self, node: NodeId, name: bool) -> Result<(), R::Error> {
        self.tracker.selector = Selector::new(self.resolver.ast(node)?, node, name);
        Ok(())
    }
    pub fn entity_visible(&mut self, node: NodeId) -> Result<(), R::Error> {
        let result = self.resolver.entity_name_visible(node, self.enclosing)?;
        self.tracker.accessibility(result);
        self.flush_reports()
    }
    pub fn diagnostic(
        &mut self,
        node: NodeId,
        message: &'static tsr_diagnostics::Message,
        args: Vec<JsString>,
    ) -> Result<(), R::Error> {
        let diagnostic = super::diagnostics::diagnostic_for_node(
            self.resolver.ast(node)?,
            Some(node),
            message,
            args,
        )?;
        self.diagnostics.push(diagnostic);
        Ok(())
    }
    pub fn name_text(&self, name: NodeId) -> Result<JsString, R::Error> {
        let view = self.resolver.ast(name)?;
        Ok(tsr_scanner::get_text_of_node(view, name)?)
    }
    pub fn fallback_name(&self) -> Result<JsString, R::Error> {
        if let Some(name) = self.tracker.error_name {
            return Ok(tsr_scanner::declaration_name_to_string(
                self.resolver.ast(name)?,
                Some(name),
            )?);
        }
        if let Some(Some(node)) = self.tracker.fallback.last() {
            let read = self.node(*node);
            if let Some(name) =
                tsr_ast::get_name_of_declaration(self.resolver.ast(*node)?, Some(*node))?
            {
                return Ok(tsr_scanner::declaration_name_to_string(
                    self.resolver.ast(name)?,
                    Some(name),
                )?);
            }
            if let Some(data) = read.as_export_assignment() {
                return Ok(JsString::from_bytes(if data.is_export_equals() {
                    b"export=".as_slice()
                } else {
                    b"default".as_slice()
                }));
            }
        }
        Ok(JsString::from_bytes(b"(Missing)".as_slice()))
    }
    // Source: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl
    pub fn flush_reports(&mut self) -> Result<(), R::Error> {
        use tsr_diagnostics as d;
        for pending in std::mem::take(&mut self.tracker.pending) {
            match pending {
                Pending::SelectorError(error) => return Err(error.into()),
                Pending::Accessibility(info, result) => {
                    let node = Self::required(result.error_node.or(info.error_node))?;
                    let mut args = Vec::new();
                    if let Some(name) = info.type_name {
                        args.push(self.name_text(name)?);
                    }
                    args.extend([result.error_symbol_name, result.error_module_name]);
                    self.diagnostic(node, info.diagnostic_message, args)?;
                }
                Pending::Report(Event::PushErrorFallbackNode(node)) => {
                    self.tracker.fallback.push(node);
                }
                Pending::Report(Event::PopErrorFallbackNode) => {
                    if self.tracker.fallback.pop().is_none() {
                        return Err(tsr_arena::Error::InvalidGraph.into());
                    }
                }
                Pending::Report(Event::InferenceFallback(node)) => self.inference_fallback(node)?,
                Pending::Report(Event::NonlocalAugmentation {
                    containing_file,
                    parent_symbol,
                    augmenting_symbol,
                }) => {
                    self.nonlocal_augmentation(containing_file, parent_symbol, augmenting_symbol)?;
                }
                Pending::Report(event) => {
                    let location =
                        self.tracker
                            .error_name
                            .or(self.tracker.fallback.last().copied().flatten());
                    let Some(location) = location else { continue };
                    let name = self.fallback_name()?;
                    let (message, args) = match event {
                        Event::CyclicStructure => (&d::The_inferred_type_of_0_references_a_type_with_a_cyclic_structure_which_cannot_be_trivially_serialized_A_type_annotation_is_necessary, vec![name]),
                        Event::InaccessibleThis => (&d::The_inferred_type_of_0_references_an_inaccessible_1_type_A_type_annotation_is_necessary, vec![name, JsString::from_bytes(b"this".as_slice())]),
                        Event::InaccessibleUniqueSymbol => (&d::The_inferred_type_of_0_references_an_inaccessible_1_type_A_type_annotation_is_necessary, vec![name, JsString::from_bytes(b"unique symbol".as_slice())]),
                        Event::LikelyUnsafeImportRequired { specifier, symbol_name } => if symbol_name.is_empty() { (&d::The_inferred_type_of_0_cannot_be_named_without_a_reference_to_1_This_is_likely_not_portable_A_type_annotation_is_necessary, vec![name, specifier]) } else { (&d::The_inferred_type_of_0_cannot_be_named_without_a_reference_to_2_from_1_This_is_likely_not_portable_A_type_annotation_is_necessary, vec![name, specifier, symbol_name]) },
                        Event::Truncation => (&d::The_inferred_type_of_this_node_exceeds_the_maximum_length_the_compiler_will_serialize_An_explicit_type_annotation_is_needed, vec![]),
                        Event::NonSerializableProperty(property) => (&d::The_type_of_this_node_cannot_be_serialized_because_its_property_0_cannot_be_serialized, vec![property]),
                        Event::PrivateInBaseOfClassExpression(property) => { self.diagnostic(location, d::Property_0_of_exported_anonymous_class_type_may_not_be_private_or_protected, vec![property])?; if self.node(location).parent().is_some_and(|p| self.node(p).kind() == K::VariableDeclaration) { let related = super::diagnostics::diagnostic_for_node(self.resolver.ast(location)?, Some(location), d::Add_a_type_annotation_to_the_variable_0, vec![name])?; self.diagnostics.last_mut().expect("just added diagnostic").related_information.push(std::sync::Arc::new(related)); } continue; }
                        _ => return Err(tsr_arena::Error::InvalidGraph.into()),
                    };
                    self.diagnostic(location, message, args)?;
                }
            }
        }
        Ok(())
    }
}
