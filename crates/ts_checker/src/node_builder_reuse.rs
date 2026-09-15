//! Native annotation reuse, including recovery-boundary diagnostics and symbol
//! tracking. Source syntax is retained before a transformed child is published.
use super::NodeBuilder;
use crate::{object_flags as of, type_flags as tf, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    node_flags as af, symbol_flags as sf, Factory, FactoryMethods, JsString, NodeListId,
    NodeVisitor, NodeVisitorHooks, SyntaxKind as K,
};
use ts_core::TextRange;
use ts_nodebuilder::{flags as nf, internal_flags as inf};
use ts_printer::{
    emit_flags as ef,
    emit_resolver::{DeclarationTrackerEvent as Event, SymbolAccessibility},
};

// Source: tsc/internal/checker/nodecopy.go:recoveryBoundary
pub(super) struct RecoveryBoundary {
    had_error: bool,
    reports: Vec<Event>,
    symbols: Vec<(SymbolId, Option<NodeId>, u32)>,
    approximate_length: usize,
    encountered_error: bool,
}
#[derive(Clone, Copy)]
struct RecoveryScope {
    had_error: bool,
    reports: usize,
    symbols: usize,
}

impl NodeBuilder<'_> {
    pub(super) fn defer_reuse_report(&mut self, event: &Event) -> bool {
        let Some(bound) = self.reuse_boundaries.last_mut() else {
            return false;
        };
        if matches!(
            event,
            Event::CyclicStructure
                | Event::InaccessibleThis
                | Event::InaccessibleUniqueSymbol
                | Event::LikelyUnsafeImportRequired { .. }
                | Event::NonSerializableProperty(_)
                | Event::PrivateInBaseOfClassExpression(_)
        ) {
            bound.had_error = true;
            bound.reports.push(event.clone());
            true
        } else {
            false
        }
    }
    pub(super) fn defer_reuse_symbol(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
    ) -> bool {
        if let Some(bound) = self.reuse_boundaries.last_mut() {
            bound.symbols.push((symbol, enclosing, meaning));
            true
        } else {
            false
        }
    }
    fn reuse_mark_error(&mut self) -> Result<(), Error> {
        self.reuse_boundaries
            .last_mut()
            .ok_or(Error::MissingLink("annotation recovery boundary"))?
            .had_error = true;
        Ok(())
    }
    fn reuse_had_error(&self) -> bool {
        self.reuse_boundaries.last().is_some_and(|b| b.had_error)
    }
    fn reuse_start_scope(&self) -> Result<RecoveryScope, Error> {
        let b = self
            .reuse_boundaries
            .last()
            .ok_or(Error::MissingLink("annotation recovery boundary"))?;
        Ok(RecoveryScope {
            had_error: b.had_error,
            reports: b.reports.len(),
            symbols: b.symbols.len(),
        })
    }
    fn reuse_end_scope(&mut self, state: RecoveryScope) -> Result<(), Error> {
        let b = self
            .reuse_boundaries
            .last_mut()
            .ok_or(Error::MissingLink("annotation recovery boundary"))?;
        b.had_error = state.had_error;
        b.reports.truncate(state.reports);
        b.symbols.truncate(state.symbols);
        Ok(())
    }
    fn reuse_retain(&mut self, node: NodeId) -> Result<(), Error> {
        if self.ast.view().node(node).is_err() {
            self.retain_source_node(node)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/nodecopy.go:NodeBuilderImpl.tryReuseExistingNodeHelper
    pub(super) fn reuse_node(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        self.reuse_retain(node)?;
        self.reuse_boundaries.push(RecoveryBoundary {
            had_error: false,
            reports: vec![],
            symbols: vec![],
            approximate_length: self.approximate_length,
            encountered_error: self.encountered_error,
        });
        let result = self.reuse_visit(node);
        let bound = self
            .reuse_boundaries
            .pop()
            .ok_or(Error::MissingLink("annotation recovery boundary"))?;
        self.approximate_length = bound.approximate_length;
        self.encountered_error = bound.encountered_error;
        let result = result?;
        for report in bound.reports {
            self.report(report);
        }
        if bound.had_error {
            return Ok(None);
        }
        for (symbol, enclosing, meaning) in bound.symbols {
            let old = self.enclosing;
            self.enclosing = enclosing;
            let tracked = self.track_symbol(symbol, meaning);
            self.enclosing = old;
            tracked?;
        }
        let read = self.ast.view().node(node)?;
        // Go's byte positions are signed. A source subtree has a nonnegative
        // range; synthetic zero-length roots do not contribute display length.
        self.approximate_length += usize::try_from(read.end() - read.pos())
            .map_err(|_| Error::MissingLink("annotation byte range"))?;
        Ok(result)
    }
    // port: tsc/internal/checker/nodecopy.go:NodeBuilderImpl.tryJSTypeNodeToTypeNode
    pub(crate) fn try_js_type_node_to_type_node(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.reuse_node(node)
    }

    // port: tsc/internal/checker/nodecopy.go:NodeBuilderImpl.reuseTypeNode
    pub(super) fn reuse_type_node(&mut self, node: NodeId) -> Result<NodeId, Error> {
        if let Some(result) = self.reuse_node(node)? {
            return Ok(result);
        }
        self.report(Event::InferenceFallback(node));
        let ty = self
            .reuse_type_from_node(node, false)?
            .ok_or(Error::MissingLink("reused annotation type"))?;
        self.type_node(ty)
    }
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getTypeFromTypeNode
    pub(super) fn reuse_type_from_node(
        &mut self,
        node: NodeId,
        no_mapped_types: bool,
    ) -> Result<Option<TypeId>, Error> {
        if self.checker.ast(node)?.node(node)?.parent().is_none() {
            return Ok(Some(self.checker.builtins.error_type));
        }
        let original = self.checker.get_type_from_type_node(node)?;
        let ty = self.checker.instantiate_type(original, self.mapper)?;
        Ok((!no_mapped_types || original == ty).then_some(ty))
    }
    fn reused_symbol_from_type_node(&mut self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        if self.checker.ast(node)?.node(node)?.parent().is_none() {
            return Ok(None);
        }
        self.checker.get_type_from_type_node(node)?;
        Ok(self
            .checker
            .query
            .resolved_symbols
            .try_get(node)
            .copied()
            .flatten())
    }
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.canReuseExistingJSTypeNode
    pub(super) fn can_reuse_existing_js_type_node(
        &mut self,
        node: NodeId,
        ty: TypeId,
    ) -> Result<bool, Error> {
        if self.checker.intended_jsdoc_type(node)?.is_some() {
            return Ok(false);
        }
        if self.checker.types.object_flags(ty)? & of::REFERENCE == 0
            || self.checker.ast(node)?.node(node)?.kind() != K::TypeReference
        {
            return Ok(true);
        }
        let Some(symbol) = self.reused_symbol_from_type_node(node)? else {
            return Ok(true);
        };
        let declared = self.checker.get_declared_type_of_symbol(symbol)?;
        let target = self.checker.types.target(ty)?;
        if declared != target {
            return Ok(true);
        }
        let parameters = self
            .checker
            .types
            .interface(target)?
            .type_parameters()
            .to_vec();
        let min = self.checker.min_type_argument_count(&parameters)?;
        let count = self
            .checker
            .source_list(
                node,
                self.checker.ast(node)?.node(node)?.type_argument_list(),
            )?
            .len();
        Ok(count >= min)
    }
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeToTypeNodeHelperWithPossibleReusableTypeNode
    pub(super) fn type_node_with_reusable_annotation(
        &mut self,
        ty: TypeId,
        annotation: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        // Declaration and display contexts have no active hover expansion.
        if let Some(annotation) = annotation {
            if self.reuse_type_from_node(annotation, false)? == Some(ty) {
                if let Some(node) = self.reuse_node(annotation)? {
                    return Ok(node);
                }
            }
        }
        self.type_node(ty)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.tryReuseExistingNonParameterTypeNode
    pub(super) fn try_reuse_existing_type_node(
        &mut self,
        node: NodeId,
        ty: TypeId,
        host: Option<NodeId>,
        annotation_type: Option<TypeId>,
    ) -> Result<Option<NodeId>, Error> {
        let annotation = if let Some(ty) = annotation_type {
            Some(ty)
        } else {
            self.reuse_type_from_node(node, true)?
        };
        let Some(annotation) = annotation else {
            return Ok(None);
        };
        let equivalent = if annotation == ty {
            true
        } else if let Some(host) = host.or(self.enclosing) {
            ts_ast::utilities_middle::has_question_token(
                self.checker.ast(host)?,
                &self.checker.ast(host)?.node(host)?,
            )? && self
                .checker
                .type_with_facts(ty, crate::type_facts::NE_UNDEFINED)?
                == annotation
        } else {
            false
        };
        if equivalent && self.can_reuse_existing_js_type_node(node, ty)? {
            self.reuse_node(node)
        } else {
            Ok(None)
        }
    }
    // port: tsc/internal/checker/nodecopy.go:NodeBuilderImpl.reuseName
    pub(super) fn reuse_name(
        &mut self,
        node: NodeId,
        is_method: bool,
    ) -> Result<Option<NodeId>, Error> {
        let Some(result) = self.reuse_node(node)? else {
            return Ok(None);
        };
        let Some(text) = self.reuse_property_name_text(result)? else {
            return Ok(Some(result));
        };
        let kind = self.ast.view().node(result)?.kind();
        let identifier = !(is_method && text.as_bytes() == b"new")
            && ts_scanner::is_identifier_text(text.as_bytes(), ts_core::LanguageVariant::STANDARD);
        let string = !identifier
            && (kind == K::StringLiteral
                || ts_jsnum::from_string(text.as_bytes())
                    .to_string()
                    .as_bytes()
                    != text.as_bytes()
                || ts_jsnum::from_string(text.as_bytes()).value() < 0.0);
        let renamed = if identifier && kind != K::Identifier {
            Some(self.ast.new_identifier(text))
        } else if string && kind != K::StringLiteral {
            Some(self.ast.new_string_literal(text, 0))
        } else {
            None
        };
        if let Some(renamed) = renamed {
            self.emit.set_original(renamed, result);
            self.set_reused_text_range(renamed, result).map(Some)
        } else {
            Ok(Some(result))
        }
    }
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.setTextRange
    pub(super) fn set_reused_text_range(
        &mut self,
        mut result: NodeId,
        location: NodeId,
    ) -> Result<NodeId, Error> {
        self.reuse_retain(result)?;
        self.reuse_retain(location)?;
        let enclosing_file = self
            .enclosing
            .map(|n| self.reuse_source_file(n))
            .transpose()?
            .flatten();
        let original = self.emit.most_original(result);
        let same_file =
            enclosing_file.is_some() && self.reuse_source_file(original)? == enclosing_file;
        let read = self.ast.view().node(result)?;
        if !ts_ast::utilities::node_is_synthesized(&read)
            || read.flags() & af::SYNTHESIZED == 0
            || !same_file
        {
            let previous = result;
            result = ts_ast::clone_node(&mut self.ast, result);
            self.ast.set_node_range(result, TextRange::new(-1, -1));
            if let Some(&symbol) = self.identifier_symbol(previous) {
                self.id_to_symbol.insert(result, symbol);
            }
        }
        if result == location {
            return Ok(result);
        }
        let mut original = self.emit.original(result);
        while let Some(node) = original {
            if node == location {
                break;
            }
            original = self.emit.original(node);
        }
        if original.is_none() {
            self.emit.set_original_ex(result, location, true);
        }
        let loc = if enclosing_file.is_some()
            && self.reuse_source_file(self.emit.most_original(location))? == enclosing_file
        {
            let read = self.ast.view().node(location)?;
            TextRange::new(i64::from(read.pos()), i64::from(read.end()))
        } else {
            TextRange::new(-1, -1)
        };
        self.ast.set_node_range(result, loc);
        Ok(result)
    }
    fn reuse_source_file(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let view = if self.ast.view().node(node).is_ok() {
            self.ast.view()
        } else {
            self.checker.ast(node)?
        };
        Ok(ts_ast::utilities::get_source_file_of_node(
            view,
            Some(node),
        )?)
    }
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.parameterToParameterDeclarationName
    pub(super) fn parameter_declaration_name(
        &mut self,
        symbol: Option<SymbolId>,
        declaration: NodeId,
    ) -> Result<NodeId, Error> {
        self.reuse_retain(declaration)?;
        let name = self.ast.view().node(declaration)?.name();
        let Some(mut name) = name else {
            let symbol = symbol.ok_or(Error::MissingLink("unnamed pseudo parameter symbol"))?;
            let text = self.checker.symbol(symbol)?.name_to_owned();
            let result = self.ast.new_identifier(text);
            self.id_to_symbol.insert(result, Some(symbol));
            return Ok(result);
        };
        let kind = self.ast.view().node(name)?.kind();
        if kind == K::QualifiedName {
            name = self
                .ast
                .view()
                .node(name)?
                .data_source()
                .as_qualified_name()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .right()
                .ok_or(Error::MissingLink("qualified parameter name"))?;
        }
        if matches!(kind.known(), Some(K::Identifier | K::QualifiedName)) {
            let result = ts_ast::deep_clone_node(&mut self.ast, Some(name))
                .ok_or(Error::MissingLink("parameter name clone"))?;
            self.emit.set_emit_flags(result, ef::NO_ASCII_ESCAPING);
            self.id_to_symbol.insert(result, symbol);
            Ok(result)
        } else {
            self.clone_binding_name_native(name)
        }
    }
    pub(super) fn clone_binding_name_native(&mut self, node: NodeId) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.clone_binding_name_native_worker(node)
        })
    }
    fn clone_binding_name_native_worker(&mut self, node: NodeId) -> Result<NodeId, Error> {
        self.reuse_retain(node)?;
        if self.ast.view().node(node)?.kind() == K::ComputedPropertyName
            && self.reuse_late_bindable_name(node)?
        {
            let expression = self
                .ast
                .view()
                .node(node)?
                .expression()
                .ok_or(Error::MissingLink("binding computed name"))?;
            self.reuse_track_computed_name(expression)?;
        }
        let children = self.reuse_children(node)?;
        let mut replacements = std::collections::HashMap::new();
        for child in children {
            replacements.insert(child, Some(self.clone_binding_name_native(child)?));
        }
        let mut result = self.reuse_replace_children(node, &replacements)?;
        if self.ast.view().node(result)?.kind() == K::BindingElement {
            let d = self
                .ast
                .view()
                .node(result)?
                .data_source()
                .as_binding_element()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            result = self.ast.update_binding_element(
                result,
                d.dot_dot_dot_token,
                d.property_name,
                d.name,
                None,
            );
        }
        if !ts_ast::utilities::node_is_synthesized(&self.ast.view().node(result)?) {
            result = ts_ast::deep_clone_node(&mut self.ast, Some(result))
                .ok_or(Error::MissingLink("binding name clone"))?;
        }
        self.emit
            .set_emit_flags(result, ef::SINGLE_LINE | ef::NO_ASCII_ESCAPING);
        Ok(result)
    }
}

impl NodeBuilder<'_> {
    fn reuse_children(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        use std::ops::ControlFlow;
        struct Collector<'a> {
            view: ts_ast::AstView<'a>,
            nodes: Vec<NodeId>,
            error: Option<ts_arena::Error>,
        }
        impl ts_ast::ChildVisitor for Collector<'_> {
            fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
                self.nodes.push(node);
                ControlFlow::Continue(())
            }
            fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
                match self.view.list(list) {
                    Ok(list) => self.visit_node_slice(list.nodes()),
                    Err(e) => {
                        self.error = Some(e);
                        ControlFlow::Break(())
                    }
                }
            }
            fn visit_node_slice(&mut self, nodes: ts_ast::NodeSlice) -> ControlFlow<()> {
                match self.view.node_slice(nodes) {
                    Ok(nodes) => {
                        self.nodes.extend(nodes.iter().flatten());
                        ControlFlow::Continue(())
                    }
                    Err(e) => {
                        self.error = Some(e);
                        ControlFlow::Break(())
                    }
                }
            }
        }
        let mut c = Collector {
            view: self.ast.view(),
            nodes: vec![],
            error: None,
        };
        let _ = c.view.node(node)?.for_each_child(&mut c);
        if let Some(e) = c.error {
            return Err(e.into());
        }
        Ok(c.nodes)
    }
    fn reuse_replace_children(
        &mut self,
        node: NodeId,
        replacements: &std::collections::HashMap<NodeId, Option<NodeId>>,
    ) -> Result<NodeId, Error> {
        let visit = |_: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            node.and_then(|n| replacements.get(&n).copied().unwrap_or(Some(n)))
        };
        let enclosing = self
            .enclosing
            .map(|n| self.reuse_source_file(n))
            .transpose()?
            .flatten();
        let nonlocal = enclosing.is_none()
            || enclosing != self.reuse_source_file(self.emit.most_original(node))?;
        let lists = |v: &mut NodeVisitor<'_>, list: Option<NodeListId>| {
            let visited = v.visit_nodes(list);
            if !nonlocal {
                return visited;
            }
            let result = visited?;
            // NodeVisitor.VisitNodes keeps the input list's Loc even for nonlocal
            // nodes; only the nodes themselves get synthetic positions.
            let result = if visited == list {
                v.factory_mut().clone_list_header(result)
            } else {
                result
            };
            Some(result)
        };
        let mut visitor = NodeVisitor::new(
            Some(&visit),
            Some(&mut self.ast),
            NodeVisitorHooks {
                visit_nodes: Some(&lists),
                ..Default::default()
            },
        );
        visitor
            .visit_each_child(Some(node))
            .ok_or(Error::MissingLink("reused parent node"))
    }
    fn reuse_visit_children(&mut self, node: NodeId) -> Result<NodeId, Error> {
        let children = self.reuse_children(node)?;
        let mut replacements = std::collections::HashMap::new();
        for child in children {
            replacements.insert(child, self.reuse_visit(child)?);
        }
        self.reuse_replace_children(node, &replacements)
    }
    fn reuse_visit_list(
        &mut self,
        owner: NodeId,
        list: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, Error> {
        let Some(list) = list else {
            return Ok(None);
        };
        let values = self.checker.source_list(owner, Some(list))?;
        let mut result = Vec::new();
        for node in values {
            if let Some(node) = self.reuse_visit(node)? {
                result.push(node);
            }
        }
        let output = self.list(result)?;
        let enclosing = self
            .enclosing
            .map(|n| self.reuse_source_file(n))
            .transpose()?
            .flatten();
        if enclosing.is_some()
            && enclosing == self.reuse_source_file(self.emit.most_original(owner))?
        {
            let loc = self.ast.view().list(list)?.loc();
            self.ast.set_list_location(output, loc)?;
        }
        Ok(Some(output))
    }
    fn reuse_optional_node(&mut self, node: Option<NodeId>) -> Result<Option<NodeId>, Error> {
        match node {
            Some(n) => self.reuse_visit(n),
            None => Ok(None),
        }
    }
    fn reuse_property_name_text(&self, node: NodeId) -> Result<Option<JsString>, Error> {
        let view = self.ast.view();
        let read = view.node(node)?;
        match read.kind().known() {
            Some(
                K::Identifier
                | K::PrivateIdentifier
                | K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral,
            ) => Ok(Some(view.node_text(node)?.into_js_string())),
            Some(K::ComputedPropertyName) => {
                let e = read
                    .expression()
                    .ok_or(Error::MissingLink("computed property expression"))?;
                Ok(matches!(
                    view.node(e)?.kind().known(),
                    Some(
                        K::StringLiteral
                            | K::NumericLiteral
                            | K::BigIntLiteral
                            | K::NoSubstitutionTemplateLiteral
                    )
                )
                .then(|| view.node_text(e).map(ts_ast::NodeText::into_js_string))
                .transpose()?)
            }
            Some(K::JsxNamespacedName) => {
                let d = read
                    .data_source()
                    .as_jsx_namespaced_name()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let ns = d.namespace().ok_or(Error::MissingLink("JSX namespace"))?;
                let name = d.name().ok_or(Error::MissingLink("JSX name"))?;
                let mut text = view.node_text(ns)?.as_bytes().to_vec();
                text.push(b':');
                text.extend_from_slice(view.node_text(name)?.as_bytes());
                Ok(Some(JsString::from_bytes(text)))
            }
            _ => Ok(None),
        }
    }
    pub(super) fn reuse_late_bindable_name(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.checker.ast(node)?.node(node)?;
        let expression = match read.kind().known() {
            Some(K::ComputedPropertyName) => read.expression(),
            Some(K::ElementAccessExpression) => read
                .data_source()
                .as_element_access_expression()
                .and_then(|d| d.argument_expression()),
            _ => None,
        };
        let Some(expression) = expression else {
            return Ok(false);
        };
        if !ts_ast::is_entity_name_expression(self.checker.ast(expression)?, expression)? {
            return Ok(false);
        }
        let ty = self.checker.late_name_type(node)?;
        Ok(
            self.checker.types.flags(ty)? & (tf::STRING_OR_NUMBER_LITERAL | tf::UNIQUE_ES_SYMBOL)
                != 0,
        )
    }
    fn reuse_declaration_name(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.checker.ast(node)?;
        let read = view.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        Ok(read.kind() != K::SourceFile
            && !ts_ast::utilities::is_binding_pattern(&read)
            && ts_ast::is_declaration(&view.node(parent)?)
            && view.node(parent)?.name() == Some(node))
    }
    pub(super) fn reuse_track_computed_name(&mut self, node: NodeId) -> Result<(), Error> {
        let first = ts_ast::utilities_middle::get_first_identifier(self.checker.ast(node)?, node)?;
        let text = self.checker.ast(first)?.node_text(first)?.into_js_string();
        let symbol = self.checker.resolve_name(
            self.enclosing,
            text.as_bytes(),
            sf::VALUE | sf::EXPORT_VALUE,
            None,
            true,
        )?;
        let symbol = if symbol.is_some() {
            symbol
        } else {
            self.checker.resolve_name(
                Some(first),
                text.as_bytes(),
                sf::VALUE | sf::EXPORT_VALUE,
                None,
                true,
            )?
        };
        if let Some(symbol) = symbol {
            self.track_symbol(symbol, sf::VALUE)?;
        }
        Ok(())
    }
    fn reuse_attach_symbol(
        &mut self,
        leftmost: NodeId,
        node: NodeId,
        symbol: Option<SymbolId>,
    ) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.reuse_attach_symbol_worker(leftmost, node, symbol)
        })
    }

    fn reuse_attach_symbol_worker(
        &mut self,
        leftmost: NodeId,
        node: NodeId,
        symbol: Option<SymbolId>,
    ) -> Result<NodeId, Error> {
        if node == leftmost {
            let mut result = None;
            if let Some(symbol) = symbol {
                let ty = self.checker.get_declared_type_of_symbol(symbol)?;
                if self.checker.symbol(symbol)?.flags() & sf::TYPE_PARAMETER != 0 {
                    result = Some(self.reuse_type_parameter_name(ty)?);
                }
            }
            let result = if let Some(result) = result {
                result
            } else {
                let text = self.checker.ast(node)?.node_text(node)?.into_js_string();
                let result = self.ast.new_identifier(text);
                self.id_to_symbol.insert(result, symbol);
                result
            };
            let result = self.set_reused_text_range(result, node)?;
            self.emit.add_emit_flags(result, ef::NO_ASCII_ESCAPING);
            return Ok(result);
        }
        let children = self.reuse_children(node)?;
        let mut replacements = std::collections::HashMap::new();
        for child in children {
            replacements.insert(
                child,
                Some(self.reuse_attach_symbol(leftmost, child, symbol)?),
            );
        }
        let result = self.reuse_replace_children(node, &replacements)?;
        self.set_reused_text_range(result, node)
    }
    fn reuse_type_parameter_name(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        self.type_parameter_name(ty)
    }
    fn reuse_track_entity(
        &mut self,
        node: NodeId,
        override_enclosing: Option<NodeId>,
    ) -> Result<(bool, NodeId), Error> {
        let enclosing = override_enclosing.or(self.enclosing);
        let leftmost =
            ts_ast::utilities_middle::get_first_identifier(self.checker.ast(node)?, node)?;
        let view = self.checker.ast(leftmost)?;
        let read = view.node(leftmost)?;
        let js = read.flags() & af::JAVA_SCRIPT_FILE != 0;
        let parent = read.parent();
        if js
            && (ts_ast::is_exports_identifier(view, leftmost)?
                || parent
                    .map(|p| ts_ast::is_module_exports_access_expression(view, p))
                    .transpose()?
                    .unwrap_or(false)
                || parent
                    .map(|p| {
                        let read = view.node(p)?;
                        let d = read.data_source();
                        let Some(d) = d.as_qualified_name() else {
                            return Ok(false);
                        };
                        let (Some(left), Some(right)) = (d.left(), d.right()) else {
                            return Ok(false);
                        };
                        Ok::<_, ts_arena::Error>(
                            view.node(left)?.kind() == K::Identifier
                                && view.node(right)?.kind() == K::Identifier
                                && view.node_text(left)?.as_bytes() == b"module"
                                && view.node_text(right)?.as_bytes() == b"exports",
                        )
                    })
                    .transpose()?
                    .unwrap_or(false))
        {
            let cloned = ts_ast::deep_clone_node(&mut self.ast, Some(node))
                .ok_or(Error::MissingLink("JS entity clone"))?;
            return Ok((true, self.set_reused_text_range(cloned, node)?));
        }
        let meaning = self.checker.emit_entity_meaning(node)?;
        if self.checker.ast(leftmost)?.node_text(leftmost)?.as_bytes() == b"this" {
            let container =
                ts_ast::get_this_container(self.checker.ast(leftmost)?, leftmost, false, false)?;
            let symbol = self.checker.get_symbol_of_declaration(container)?;
            let bad = self
                .checker
                .emit_symbol_accessible(symbol, Some(leftmost), meaning, false, true)?
                .accessibility
                != SymbolAccessibility::Accessible;
            if bad {
                self.report(Event::InaccessibleThis);
            }
            return Ok((bad, self.reuse_attach_symbol(leftmost, node, symbol)?));
        }
        let mut symbol = self
            .checker
            .resolve_entity_name_ex(leftmost, meaning, true, true)?;
        if self.enclosing.is_some()
            && !symbol
                .map(|s| {
                    self.checker
                        .symbol(s)
                        .map(|s| s.flags() & sf::TYPE_PARAMETER != 0)
                })
                .transpose()?
                .unwrap_or(false)
        {
            symbol = symbol
                .map(|s| {
                    self.checker
                        .get_export_symbol_of_value_symbol_if_exported(s)
                })
                .transpose()?;
            let at = self.checker.resolve_entity_name_at(
                leftmost,
                meaning,
                true,
                true,
                self.enclosing,
            )?;
            let mismatch = if let (Some(at), Some(original)) = (at, symbol) {
                let export = self
                    .checker
                    .get_export_symbol_of_value_symbol_if_exported(at)?;
                !self
                    .checker
                    .module_symbols_same_reference(export, original)?
            } else {
                false
            };
            if at == Some(self.checker.builtins.unknown_symbol)
                || at.is_none() && symbol.is_some()
                || mismatch
            {
                if at != Some(self.checker.builtins.unknown_symbol) {
                    self.report(Event::InferenceFallback(node));
                }
                let cloned = ts_ast::deep_clone_node(&mut self.ast, Some(node))
                    .ok_or(Error::MissingLink("entity mismatch clone"))?;
                return Ok((true, self.set_reused_text_range(cloned, node)?));
            }
            symbol = at;
        }
        let mut bad = false;
        if let Some(symbol) = symbol {
            let read = self.checker.symbol(symbol)?;
            if read.flags() & sf::FUNCTION_SCOPED_VARIABLE != 0 {
                if let Some(declaration) = read.value_declaration() {
                    if ts_ast::utilities::is_part_of_parameter_declaration(
                        self.checker.ast(declaration)?,
                        declaration,
                    )? || self.checker.ast(declaration)?.node(declaration)?.kind()
                        == K::JSDocParameterTag
                    {
                        return Ok((
                            false,
                            self.reuse_attach_symbol(leftmost, node, Some(symbol))?,
                        ));
                    }
                }
            }
            if self.checker.symbol(symbol)?.flags() & sf::TYPE_PARAMETER == 0
                && !self.reuse_declaration_name(node)?
                && self
                    .checker
                    .emit_symbol_accessible(Some(symbol), enclosing, meaning, false, true)?
                    .accessibility
                    != SymbolAccessibility::Accessible
            {
                self.report(Event::InferenceFallback(node));
                bad = true;
            } else {
                let old = self.enclosing;
                self.enclosing = enclosing;
                let tracked = self.track_symbol(symbol, meaning);
                self.enclosing = old;
                tracked?;
            }
            Ok((bad, self.reuse_attach_symbol(leftmost, node, Some(symbol))?))
        } else {
            let cloned = ts_ast::deep_clone_node(&mut self.ast, Some(node))
                .ok_or(Error::MissingLink("unresolved entity clone"))?;
            Ok((bad, self.set_reused_text_range(cloned, node)?))
        }
    }
}

impl NodeBuilder<'_> {
    fn reuse_serialize_type_name(
        &mut self,
        name: NodeId,
        is_type_of: bool,
        args: Option<NodeListId>,
    ) -> Result<Option<NodeId>, Error> {
        let meaning = if is_type_of { sf::VALUE } else { sf::TYPE };
        let Some(symbol) =
            self.checker
                .resolve_entity_name_at(name, meaning, true, false, Some(name))?
        else {
            return Ok(None);
        };
        let resolved = if self.checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            self.checker.resolve_alias(symbol)?
        } else {
            symbol
        };
        if self
            .checker
            .emit_symbol_accessible(Some(symbol), self.enclosing, meaning, false, true)?
            .accessibility
            != SymbolAccessibility::Accessible
        {
            return Ok(None);
        }
        self.track_symbol(resolved, meaning)?;
        self.symbol_type_node_from_chain(resolved, meaning, args)
            .map(Some)
    }
    fn reuse_simple_type(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let mut inner = node;
        while self.checker.ast(inner)?.node(inner)?.kind() == K::ParenthesizedExpression {
            inner = self
                .checker
                .ast(inner)?
                .node(inner)?
                .expression()
                .ok_or(Error::MissingLink("parenthesized reusable expression"))?;
        }
        let kind = self.checker.ast(inner)?.node(inner)?.kind();
        match kind.known() {
            Some(K::TypeReference) => self.reuse_type_reference(inner),
            Some(K::TypeQuery) => self.reuse_type_query(inner),
            Some(K::IndexedAccessType) => self.reuse_indexed_type(inner),
            Some(K::TypeOperator)
                if self
                    .checker
                    .ast(inner)?
                    .node(inner)?
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|d| d.operator() == K::KeyOfKeyword) =>
            {
                self.reuse_keyof_type(inner)
            }
            _ => self.reuse_visit(node),
        }
    }
    fn reuse_type_reference(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if ts_ast::utilities_middle::is_const_type_reference(
            self.checker.ast(node)?,
            &self.checker.ast(node)?.node(node)?,
        )? {
            return Ok(None);
        }
        let Some(symbol) = self.reused_symbol_from_type_node(node)? else {
            return Ok(None);
        };
        if self.checker.symbol(symbol)?.flags() & sf::TYPE_PARAMETER != 0 {
            let ty = self.checker.get_declared_type_of_symbol(symbol)?;
            if self.checker.instantiate_type(ty, self.mapper)? != ty {
                return Ok(None);
            }
        }
        let ty = self
            .reuse_type_from_node(node, false)?
            .ok_or(Error::MissingLink("reused type reference"))?;
        if !self.can_reuse_existing_js_type_node(node, ty)? {
            return Ok(None);
        }
        let data = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_type_reference_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let name = data
            .type_name
            .ok_or(Error::MissingLink("reused type reference name"))?;
        let (error, new_name) = self.reuse_track_entity(name, None)?;
        let args = self.reuse_visit_list(node, data.type_arguments)?;
        if error {
            return self
                .reuse_serialize_type_name(name, false, args)?
                .map(|n| self.set_reused_text_range(n, name))
                .transpose();
        }
        let result = self
            .ast
            .update_type_reference_node(node, Some(new_name), args);
        self.set_reused_text_range(result, node).map(Some)
    }
    fn reuse_type_query(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let data = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_type_query_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let name = data
            .expr_name
            .ok_or(Error::MissingLink("reused type query name"))?;
        let (error, new_name) = self.reuse_track_entity(name, None)?;
        let args = self.reuse_visit_list(node, data.type_arguments)?;
        if error {
            return self
                .reuse_serialize_type_name(name, true, args)?
                .map(|n| self.set_reused_text_range(n, name))
                .transpose();
        }
        let result = self.ast.update_type_query_node(node, Some(new_name), args);
        self.set_reused_text_range(result, node).map(Some)
    }
    fn reuse_indexed_type(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let data = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_indexed_access_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let object = data
            .object_type
            .ok_or(Error::MissingLink("reused indexed object"))?;
        let Some(object) = self.reuse_simple_type(object)? else {
            return Ok(None);
        };
        let index = self.reuse_optional_node(data.index_type)?;
        let result = self
            .ast
            .update_indexed_access_type_node(node, Some(object), index);
        self.set_reused_text_range(result, node).map(Some)
    }
    fn reuse_keyof_type(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let data = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_type_operator_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let Some(ty) = self.reuse_simple_type(
            data.r#type
                .ok_or(Error::MissingLink("reused keyof operand"))?,
        )?
        else {
            return Ok(None);
        };
        let result = self
            .ast
            .update_type_operator_node(node, data.operator, Some(ty));
        self.set_reused_text_range(result, node).map(Some)
    }
    fn reuse_visit(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.reuse_visit_scoped(node)
        })
    }
    fn reuse_visit_scoped(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if self.reuse_had_error() {
            return Ok(Some(node));
        }
        let state = self.reuse_start_scope()?;
        let read = self.checker.ast(node)?.node(node)?;
        let function = ts_ast::utilities::is_function_like(Some(&read));
        let result = if function {
            let signature = self.checker.signature_from_declaration(node)?;
            let signature = self.checker.signatures.get(signature)?.clone();
            self.with_serialization_scope(
                Some(node),
                signature.parameters.as_deref().unwrap_or_default(),
                signature.type_parameters.as_deref().unwrap_or_default(),
                &[],
                None,
                |b| b.reuse_visit_worker(node),
            )?
        } else if read.kind() == K::MappedType {
            let parameter = read
                .data_source()
                .as_mapped_type_node()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .type_parameter()
                .ok_or(Error::MissingLink("reused mapped parameter"))?;
            let symbol = self
                .checker
                .get_symbol_of_declaration(parameter)?
                .ok_or(Error::MissingLink("mapped parameter symbol"))?;
            let ty = self.checker.get_declared_type_of_symbol(symbol)?;
            self.with_serialization_scope(Some(node), &[], &[ty], &[], None, |b| {
                b.reuse_visit_worker(node)
            })?
        } else {
            self.reuse_visit_worker(node)?
        };
        let mut result = result;
        if result == Some(node)
            && !ts_ast::utilities::node_is_synthesized(&self.ast.view().node(node)?)
        {
            result = ts_ast::deep_clone_node(&mut self.ast, result);
        }
        result = result
            .map(|r| self.set_reused_text_range(r, node))
            .transpose()?;
        if self.reuse_had_error() {
            if ts_ast::utilities::is_type_node(&self.checker.ast(node)?.node(node)?)
                && self.checker.ast(node)?.node(node)?.kind() != K::TypePredicate
            {
                self.reuse_end_scope(state)?;
                let ty = self
                    .reuse_type_from_node(node, false)?
                    .ok_or(Error::MissingLink("recovery type"))?;
                return self.type_node(ty).map(Some);
            }
            let clone = ts_ast::clone_node(&mut self.ast, node);
            return self.set_reused_text_range(clone, node).map(Some);
        }
        Ok(result)
    }
    fn reuse_visit_worker(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.checker.ast(node)?.node(node)?;
        let kind = read.kind();
        match kind.known() {
            Some(K::JSDocTypeExpression | K::JSDocNonNullableType) => {
                return self.reuse_optional_node(read.type_node())
            }
            Some(K::JSDocAllType) => {
                return Ok(Some(self.ast.new_keyword_type_node(K::AnyKeyword.into())))
            }
            Some(K::JSDocNullableType | K::JSDocOptionalType) => {
                let inner = self
                    .reuse_optional_node(read.type_node())?
                    .ok_or(Error::MissingLink("reused documentation inner type"))?;
                let other = if kind == K::JSDocNullableType {
                    let null = self.ast.new_keyword_expression(K::NullKeyword.into());
                    self.ast.new_literal_type_node(Some(null))
                } else {
                    self.ast.new_keyword_type_node(K::UndefinedKeyword.into())
                };
                let members = self.list(vec![inner, other])?;
                return Ok(Some(self.ast.new_union_type_node(Some(members))));
            }
            Some(K::JSDocVariadicType) => {
                let operand = read
                    .data_source()
                    .as_js_doc_variadic_type()
                    .and_then(|data| data.r#type());
                let inner = self.reuse_optional_node(operand)?;
                return Ok(Some(self.ast.new_array_type_node(inner)));
            }
            Some(K::JSDocTypeLiteral) => return self.reuse_jsdoc_type_literal(node).map(Some),
            Some(K::ThisType) => return Ok(Some(node)),
            _ => {}
        }
        if kind == K::TypeReference {
            let name = read
                .data_source()
                .as_type_reference_node()
                .and_then(|d| d.type_name())
                .ok_or(Error::MissingLink("reused reference name"))?;
            if self.checker.ast(name)?.node(name)?.kind() == K::Identifier
                && self.checker.ast(name)?.node_text(name)?.is_empty()
            {
                let replacement = self.ast.new_keyword_type_node(K::AnyKeyword.into());
                self.emit.set_original(replacement, node);
                return Ok(Some(replacement));
            }
        }
        if kind == K::TypeParameter {
            let data = read
                .data_source()
                .as_type_parameter_declaration()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            let (_, name) = self.reuse_track_entity(
                data.name
                    .ok_or(Error::MissingLink("reused parameter name"))?,
                None,
            )?;
            let modifiers = self.reuse_visit_list(node, data.modifiers)?;
            let constraint = self.reuse_optional_node(data.constraint)?;
            let expression = self.reuse_optional_node(data.expression)?;
            let default = self.reuse_optional_node(data.default_type)?;
            return Ok(Some(self.ast.update_type_parameter_declaration(
                node,
                modifiers,
                Some(name),
                constraint,
                expression,
                default,
            )));
        }
        let simple = match kind.known() {
            Some(K::IndexedAccessType) => Some(self.reuse_indexed_type(node)?),
            Some(K::TypeReference) => Some(self.reuse_type_reference(node)?),
            Some(K::TypeQuery) => Some(self.reuse_type_query(node)?),
            _ => None,
        };
        if let Some(result) = simple {
            if result.is_none() {
                self.reuse_mark_error()?;
                return Ok(Some(node));
            }
            return Ok(result);
        }
        if kind == K::TypeOperator {
            let data = self
                .checker
                .ast(node)?
                .node(node)?
                .data_source()
                .as_type_operator_node()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            let inner = data
                .r#type
                .ok_or(Error::MissingLink("reused type operator operand"))?;
            if data.operator == K::UniqueKeyword
                && self.checker.ast(inner)?.node(inner)?.kind() == K::SymbolKeyword
            {
                let mut enclosing = self.enclosing;
                while let Some(enc) = enclosing {
                    if !self
                        .checker
                        .synthetic_scopes
                        .signature_kinds
                        .contains_key(&enc)
                    {
                        break;
                    }
                    enclosing = self.checker.ast(enc)?.node(enc)?.parent();
                }
                let mut current = Some(node);
                let mut same = false;
                while let Some(n) = current {
                    if Some(n) == enclosing {
                        same = true;
                        break;
                    }
                    current = self.checker.ast(n)?.node(n)?.parent();
                }
                if !same {
                    self.reuse_mark_error()?;
                    return Ok(Some(node));
                }
            } else if data.operator == K::KeyOfKeyword {
                let result = self.reuse_keyof_type(node)?;
                if result.is_none() {
                    self.reuse_mark_error()?;
                    return Ok(Some(node));
                }
                return Ok(result);
            }
        }
        if kind == K::ImportType && self.reuse_is_literal_import_type(node)? {
            return self.reuse_import_type(node);
        }
        let read = self.checker.ast(node)?.node(node)?;
        if let Some(name) = read.name() {
            if self.checker.ast(name)?.node(name)?.kind() == K::ComputedPropertyName
                && !self.reuse_late_bindable_name(name)?
            {
                if !ts_ast::has_dynamic_name(self.checker.ast(node)?, Some(node))? {
                    return self.reuse_visit_children(node).map(Some);
                }
                let expression = self
                    .checker
                    .ast(name)?
                    .node(name)?
                    .expression()
                    .ok_or(Error::MissingLink("dynamic computed name"))?;
                let keep = self.internal_flags & inf::ALLOW_UNRESOLVED_NAMES != 0
                    && ts_ast::is_entity_name_expression(
                        self.checker.ast(expression)?,
                        expression,
                    )?
                    && {
                        let ty = self.checker.check_computed_property_name(name)?;
                        self.checker.types.flags(ty)? & tf::ANY != 0
                    };
                if !keep {
                    return Ok(None);
                }
            }
        }
        let read = self.checker.ast(node)?.node(node)?;
        if (ts_ast::utilities::is_function_like(Some(&read)) && read.type_node().is_none())
            || matches!(
                kind.known(),
                Some(K::PropertyDeclaration | K::PropertySignature | K::Parameter)
            ) && read.type_node().is_none()
                && read.initializer().is_none()
        {
            let mut visited = self.reuse_visit_children(node)?;
            if visited == node {
                visited = ts_ast::clone_node(&mut self.ast, node);
                visited = self.set_reused_text_range(visited, node)?;
            }
            let ty = self.ast.new_keyword_type_node(K::AnyKeyword.into());
            if matches!(
                kind.known(),
                Some(
                    K::PropertyDeclaration
                        | K::PropertySignature
                        | K::Parameter
                        | K::MethodSignature
                        | K::CallSignature
                        | K::JSDocSignature
                        | K::ConstructSignature
                        | K::IndexSignature
                        | K::FunctionType
                        | K::ConstructorType
                )
            ) {
                self.ast.node_mut(visited)?.set_type_node(Some(ty));
                if matches!(
                    kind.known(),
                    Some(K::PropertyDeclaration | K::PropertySignature | K::Parameter)
                ) {
                    self.ast.node_mut(visited)?.set_initializer(None);
                }
                if kind == K::Parameter {
                    self.ast.node_mut(visited)?.set_modifiers(None);
                }
                return Ok(Some(visited));
            }
        }
        if kind == K::ComputedPropertyName {
            let expression = self
                .checker
                .ast(node)?
                .node(node)?
                .expression()
                .ok_or(Error::MissingLink("computed expression"))?;
            if ts_ast::is_entity_name_expression(self.checker.ast(expression)?, expression)? {
                let (error, expression) = self.reuse_track_entity(expression, None)?;
                if !error {
                    return Ok(Some(
                        self.ast
                            .update_computed_property_name(node, Some(expression)),
                    ));
                }
                self.reuse_mark_error()?;
                return self.reuse_visit_children(node).map(Some);
            }
        }
        if kind == K::TypePredicate {
            let data = self
                .checker
                .ast(node)?
                .node(node)?
                .data_source()
                .as_type_predicate_node()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            let name = data
                .parameter_name
                .ok_or(Error::MissingLink("reused predicate parameter"))?;
            let name = if self.checker.ast(name)?.node(name)?.kind() == K::Identifier {
                let (error, name) = self.reuse_track_entity(name, None)?;
                if error {
                    self.reuse_mark_error()?;
                }
                name
            } else {
                ts_ast::clone_node(&mut self.ast, name)
            };
            let asserts = self.reuse_optional_node(data.asserts_modifier)?;
            let ty = self.reuse_optional_node(data.r#type)?;
            return Ok(Some(self.ast.update_type_predicate_node(
                node,
                asserts,
                Some(name),
                ty,
            )));
        }
        if kind == K::ConditionalType {
            let data = self
                .checker
                .ast(node)?
                .node(node)?
                .data_source()
                .as_conditional_type_node()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            let check = self.reuse_optional_node(data.check_type)?;
            let parameters = self.checker.infer_type_parameters(node)?;
            let (extends, true_type) =
                self.with_serialization_scope(Some(node), &[], &parameters, &[], None, |b| {
                    Ok((
                        b.reuse_optional_node(data.extends_type)?,
                        b.reuse_optional_node(data.true_type)?,
                    ))
                })?;
            let false_type = self.reuse_optional_node(data.false_type)?;
            return Ok(Some(self.ast.update_conditional_type_node(
                node, check, extends, true_type, false_type,
            )));
        }
        if kind == K::TupleType
            || kind == K::TypeLiteral && self.flags & nf::MULTILINE_OBJECT_LITERALS == 0
            || kind == K::MappedType
        {
            let mut result = self.reuse_visit_children(node)?;
            if result == node {
                result = ts_ast::clone_node(&mut self.ast, node);
                result = self.set_reused_text_range(result, node)?;
            }
            self.emit.add_emit_flags(result, ef::SINGLE_LINE);
            return Ok(Some(result));
        }
        if matches!(
            kind.known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ) {
            let result = ts_ast::clone_node(&mut self.ast, node);
            if kind == K::StringLiteral
                && self.flags & nf::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE != 0
            {
                if let ts_ast::NodeData::StringLiteral(data) = self.ast.node_mut(result)?.data_mut()
                {
                    data.token_flags |= ts_ast::token_flags::SINGLE_QUOTE;
                }
            }
            self.emit.add_emit_flags(result, ef::NO_ASCII_ESCAPING);
            return Ok(Some(result));
        }
        self.reuse_visit_children(node).map(Some)
    }
}

impl NodeBuilder<'_> {
    fn reuse_jsdoc_type_literal(&mut self, node: NodeId) -> Result<NodeId, Error> {
        let slice = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_js_doc_type_literal()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .js_doc_property_tags();
        let tags: Vec<_> = self
            .checker
            .ast(node)?
            .node_slice(slice)?
            .iter()
            .flatten()
            .collect();
        let mut members = Vec::new();
        for tag in tags {
            let read = self.checker.ast(tag)?.node(tag)?;
            if !matches!(
                read.kind().known(),
                Some(K::JSDocPropertyTag | K::JSDocParameterTag)
            ) {
                continue;
            }
            let data = read
                .data_source()
                .as_js_doc_parameter_or_property_tag()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .to_owned();
            let mut name = data
                .name
                .ok_or(Error::MissingLink("documentation property name"))?;
            if self.checker.ast(name)?.node(name)?.kind() != K::Identifier {
                name = self
                    .checker
                    .ast(name)?
                    .node(name)?
                    .data_source()
                    .as_qualified_name()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .right()
                    .ok_or(Error::MissingLink("documentation qualified name"))?;
            }
            let name = self.reuse_visit(name)?;
            let optional = data.is_bracketed
                || data
                    .type_expression
                    .map(|n| {
                        self.checker
                            .ast(n)?
                            .node(n)
                            .map(|n| n.kind() == K::JSDocOptionalType)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
            let question = optional.then(|| self.ast.new_token(K::QuestionToken.into()));
            let ty = self.reuse_optional_node(data.type_expression)?;
            members.push(
                self.ast
                    .new_property_signature_declaration(None, name, question, ty, None),
            );
        }
        let members = self.list(members)?;
        Ok(self.ast.new_type_literal_node(Some(members)))
    }
    fn reuse_is_literal_import_type(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.checker.ast(node)?;
        let Some(argument) = view
            .node(node)?
            .data_source()
            .as_import_type_node()
            .and_then(|d| d.argument())
        else {
            return Ok(false);
        };
        let Some(literal) = view
            .node(argument)?
            .data_source()
            .as_literal_type_node()
            .and_then(|d| d.literal())
        else {
            return Ok(false);
        };
        Ok(view.node(literal)?.kind() == K::StringLiteral)
    }
    fn reuse_module_specifier_override(
        &mut self,
        parent: NodeId,
        original: NodeId,
    ) -> Result<Option<JsString>, Error> {
        let enclosing_file = self
            .enclosing
            .map(|n| self.reuse_source_file(n))
            .transpose()?
            .flatten();
        if enclosing_file == self.reuse_source_file(original)? {
            return Ok(None);
        }
        let data = self
            .checker
            .ast(parent)?
            .node(parent)?
            .data_source()
            .as_import_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let mode = ts_ast::utilities_middle::import_attributes_resolution_mode(
            self.checker.ast(parent)?,
            data.attributes,
        )?
        .unwrap_or(ts_core::ResolutionMode::NONE);
        let original_name = self
            .checker
            .ast(original)?
            .node_text(original)?
            .into_js_string();
        let mut name = original_name.clone();
        let node_symbol = self.reused_symbol_from_type_node(parent)?;
        let meaning = if data.is_type_of { sf::VALUE } else { sf::TYPE };
        let mut parent_symbol = None;
        if let Some(symbol) = node_symbol {
            if self
                .checker
                .emit_symbol_accessible(Some(symbol), self.enclosing, meaning, false, true)?
                .accessibility
                == SymbolAccessibility::Accessible
            {
                let chain = if let Some(enclosing) = self.enclosing {
                    self.accessibility_chain(symbol, enclosing, meaning)?
                } else {
                    vec![symbol]
                };
                parent_symbol = chain.first().copied();
            }
        }
        let external = match parent_symbol {
            Some(symbol) if self.name_external_module(symbol)? => Some(symbol),
            _ => None,
        };
        if let Some(symbol) = external {
            name = self.module_specifier_with_context_and_mode(symbol, self.enclosing, mode)?;
        } else if let Some(file) = self.checker.emit_external_module_file(parent)? {
            if let Some(symbol) = self.checker.raw_declaration_symbol(file)? {
                name = self.module_specifier_with_context_and_mode(symbol, self.enclosing, mode)?;
            }
        }
        if name
            .as_bytes()
            .windows(b"/node_modules/".len())
            .any(|w| w == b"/node_modules/")
        {
            self.encountered_error = true;
            self.report(Event::LikelyUnsafeImportRequired {
                specifier: name.clone(),
                symbol_name: JsString::default(),
            });
        }
        Ok((name != original_name).then_some(name))
    }
    fn reuse_import_type(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let data = self
            .checker
            .ast(node)?
            .node(node)?
            .data_source()
            .as_import_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        if let Some(attributes) = data.attributes {
            if self
                .checker
                .ast(attributes)?
                .node(attributes)?
                .data_source()
                .as_import_attributes()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .token()
                == K::AssertKeyword
            {
                self.reuse_mark_error()?;
                return Ok(Some(node));
            }
        }
        if self.reuse_type_from_node(node, true)?.is_none() {
            self.reuse_mark_error()?;
            return Ok(Some(node));
        }
        let mut argument = data
            .argument
            .ok_or(Error::MissingLink("reused import type argument"))?;
        let original = self
            .checker
            .ast(argument)?
            .node(argument)?
            .data_source()
            .as_literal_type_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .literal()
            .ok_or(Error::MissingLink("import type literal"))?;
        let specifier = if let Some(name) = self.reuse_module_specifier_override(node, original)? {
            let n = self.ast.new_string_literal(name, 0);
            self.emit.set_original(n, original);
            n
        } else {
            self.reuse_visit(original)?
                .ok_or(Error::MissingLink("reused import string"))?
        };
        if specifier != original {
            argument = self.ast.new_literal_type_node(Some(specifier));
        }
        let attributes = self.reuse_optional_node(data.attributes)?;
        let qualifier = self.reuse_optional_node(data.qualifier)?;
        let args = self.reuse_visit_list(node, data.type_arguments)?;
        Ok(Some(self.ast.update_import_type_node(
            node,
            data.is_type_of,
            Some(argument),
            attributes,
            qualifier,
            args,
        )))
    }
}
