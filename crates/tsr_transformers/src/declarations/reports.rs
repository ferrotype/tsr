//! The `SymbolTrackerImpl` reports that write diagnostics. A resolver call
//! holds the tracker, so its reports are applied here, in order, once the call
//! returns (`apply_tracker_reports`).
use super::{
    diagnostics::{create_diagnostic_for_node, create_get_isolated_declaration_errors, Failure},
    tracker::Pending,
    transform::{Transformer, NIL},
};
use tsr_ast::{JsString, NodeId, SymbolId, SyntaxKind as K};
use tsr_diagnostics as d;
use tsr_printer::emit_resolver::{DeclarationEmitResolver, DeclarationTrackerEvent as Event};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    /// Applies the reports a resolver call made, in the order they were made.
    pub fn apply_tracker_reports(&mut self) -> Result<(), R::Error> {
        for pending in std::mem::take(&mut self.tracker.pending) {
            match pending {
                Pending::SelectorFailure(Failure::Arena(error)) => return Err(error.into()),
                Pending::SelectorFailure(Failure::Panic(message)) => panic!("{message}"),
                Pending::Accessibility(info, result) => {
                    let diag_node = result.error_node.or(info.error_node).expect(NIL);
                    let mut args = Vec::new();
                    if let Some(type_name) = info.type_name {
                        args.push(tsr_scanner::get_text_of_node(
                            self.resolver.ast(type_name)?,
                            type_name,
                        )?);
                    }
                    args.extend([result.error_symbol_name, result.error_module_name]);
                    self.add_diagnostic_for_node(diag_node, info.diagnostic_message, args)?;
                }
                Pending::Report(Event::PushErrorFallbackNode(node)) => {
                    self.tracker.push_error_fallback_node(node);
                }
                Pending::Report(Event::PopErrorFallbackNode) => {
                    self.tracker.pop_error_fallback_node();
                }
                Pending::Report(Event::InferenceFallback(node)) => {
                    self.report_inference_fallback(node)?;
                }
                Pending::Report(Event::NonlocalAugmentation {
                    containing_file,
                    parent_symbol,
                    augmenting_symbol,
                }) => {
                    self.report_nonlocal_augmentation(
                        containing_file,
                        parent_symbol,
                        augmenting_symbol,
                    )?;
                }
                Pending::Report(Event::CyclicStructure) => self.report_cyclic_structure_error()?,
                Pending::Report(Event::InaccessibleThis) => {
                    self.report_inaccessible_this_error()?;
                }
                Pending::Report(Event::InaccessibleUniqueSymbol) => {
                    self.report_inaccessible_unique_symbol_error()?;
                }
                Pending::Report(Event::LikelyUnsafeImportRequired {
                    specifier,
                    symbol_name,
                }) => self.report_likely_unsafe_import_required_error(specifier, symbol_name)?,
                Pending::Report(Event::Truncation) => self.report_truncation_error()?,
                Pending::Report(Event::NonSerializableProperty(property_name)) => {
                    self.report_non_serializable_property(property_name)?;
                }
                Pending::Report(Event::PrivateInBaseOfClassExpression(property_name)) => {
                    self.report_private_in_base_of_class_expression(property_name)?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportCyclicStructureError
    fn report_cyclic_structure_error(&mut self) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            let name = self.error_declaration_name_with_fallback()?;
            self.add_diagnostic_for_node(
                location,
                d::The_inferred_type_of_0_references_a_type_with_a_cyclic_structure_which_cannot_be_trivially_serialized_A_type_annotation_is_necessary,
                vec![name],
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportInaccessibleThisError
    fn report_inaccessible_this_error(&mut self) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            let name = self.error_declaration_name_with_fallback()?;
            self.add_diagnostic_for_node(
                location,
                d::The_inferred_type_of_0_references_an_inaccessible_1_type_A_type_annotation_is_necessary,
                vec![name, JsString::from_bytes(b"this".as_slice())],
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportInaccessibleUniqueSymbolError
    fn report_inaccessible_unique_symbol_error(&mut self) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            let name = self.error_declaration_name_with_fallback()?;
            self.add_diagnostic_for_node(
                location,
                d::The_inferred_type_of_0_references_an_inaccessible_1_type_A_type_annotation_is_necessary,
                vec![name, JsString::from_bytes(b"unique symbol".as_slice())],
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.isBoundExpando
    fn is_bound_expando(&mut self, node: NodeId) -> Result<bool, R::Error> {
        let view = self.resolver.ast(node)?;
        let read = view.node(node)?;
        if !(tsr_ast::utilities_tail::is_expando_property_declaration(Some(&read))
            && view
                .node(
                    read.as_binary_expression()
                        .expect("binary expression payload")
                        .left()
                        .expect(NIL),
                )?
                .kind()
                == K::PropertyAccessExpression)
        {
            return Ok(false);
        }
        let left = read
            .as_binary_expression()
            .expect("binary expression payload")
            .left()
            .expect(NIL);
        let leftmost = tsr_ast::precedence::get_leftmost_expression(view, left, true)?;
        let Some(reference) = self
            .resolver
            .referenced_value_declaration_unsafe(leftmost)?
        else {
            return Ok(false);
        };
        self.resolver.expando_function_declaration_unsafe(reference)
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.isChildOfBoundExpando
    fn is_child_of_bound_expando(&mut self, node: NodeId) -> Result<bool, R::Error> {
        // ast.FindAncestorOrQuit(node, ...)
        let mut current = Some(node);
        while let Some(n) = current {
            let kind = self.resolver.ast(n)?.node(n)?.kind();
            if kind == K::SourceFile || kind == K::Block {
                return Ok(false);
            }
            if self.is_bound_expando(n)? {
                return Ok(true);
            }
            current = self.resolver.ast(n)?.node(n)?.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportInferenceFallback
    pub fn report_inference_fallback(&mut self, node: NodeId) -> Result<(), R::Error> {
        if !self.isolated_declarations {
            return Ok(());
        }
        let source =
            tsr_ast::utilities::get_source_file_of_node(self.resolver.ast(node)?, Some(node))?;
        if source != Some(self.current_source_file) {
            return Ok(()); // Nested error on a declaration in another file - ignore, will be reemitted if file is in the output file set
        }
        // within a node builder call that should already lock the checker, use the unsafe call
        if self.resolver.expando_function_declaration_unsafe(node)? {
            self.report_expando_function_errors(node)?;
        }
        // expando props get an error when their host is visited by the above, this prevents a follow-on error on a non-inferrable expression
        if !self.is_child_of_bound_expando(node)? {
            let diagnostic = create_get_isolated_declaration_errors(self.resolver, node)?;
            self.add_diagnostic(diagnostic);
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportLikelyUnsafeImportRequiredError
    fn report_likely_unsafe_import_required_error(
        &mut self,
        specifier: JsString,
        symbol_name: JsString,
    ) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            let name = self.error_declaration_name_with_fallback()?;
            if symbol_name.is_empty() {
                self.add_diagnostic_for_node(
                    location,
                    d::The_inferred_type_of_0_cannot_be_named_without_a_reference_to_1_This_is_likely_not_portable_A_type_annotation_is_necessary,
                    vec![name, specifier],
                )?;
            } else {
                self.add_diagnostic_for_node(
                    location,
                    d::The_inferred_type_of_0_cannot_be_named_without_a_reference_to_2_from_1_This_is_likely_not_portable_A_type_annotation_is_necessary,
                    vec![name, specifier, symbol_name],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportNonSerializableProperty
    fn report_non_serializable_property(
        &mut self,
        property_name: JsString,
    ) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            self.add_diagnostic_for_node(
                location,
                d::The_type_of_this_node_cannot_be_serialized_because_its_property_0_cannot_be_serialized,
                vec![property_name],
            )?;
        }
        Ok(())
    }

    fn source_file_of(&self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        Ok(tsr_ast::utilities::get_source_file_of_node(
            self.resolver.ast(node)?,
            Some(node),
        )?)
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportNonlocalAugmentation
    fn report_nonlocal_augmentation(
        &mut self,
        containing_file: NodeId,
        parent_symbol: SymbolId,
        augmenting_symbol: SymbolId,
    ) -> Result<(), R::Error> {
        let mut primary_declaration = None;
        for declaration in self.resolver.symbol_declarations(parent_symbol)? {
            if self.source_file_of(declaration)? == Some(containing_file) {
                primary_declaration = Some(declaration);
                break;
            }
        }
        let mut augmenting_declarations = Vec::new();
        for declaration in self.resolver.symbol_declarations(augmenting_symbol)? {
            if self.source_file_of(declaration)? != Some(containing_file) {
                augmenting_declarations.push(declaration);
            }
        }
        if let Some(primary_declaration) = primary_declaration {
            for augmentations in augmenting_declarations {
                let mut diag = create_diagnostic_for_node(
                    self.resolver.ast(augmentations)?,
                    augmentations,
                    d::Declaration_augments_declaration_in_another_file_This_cannot_be_serialized,
                    vec![],
                )?;
                let related = create_diagnostic_for_node(
                    self.resolver.ast(primary_declaration)?,
                    primary_declaration,
                    d::This_is_the_declaration_being_augmented_Consider_moving_the_augmenting_declaration_into_the_same_file,
                    vec![],
                )?;
                diag.related_information.push(std::sync::Arc::new(related));
                self.add_diagnostic(diag);
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportPrivateInBaseOfClassExpression
    fn report_private_in_base_of_class_expression(
        &mut self,
        property_name: JsString,
    ) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            let view = self.resolver.ast(location)?;
            let mut diag = create_diagnostic_for_node(
                view,
                location,
                d::Property_0_of_exported_anonymous_class_type_may_not_be_private_or_protected,
                vec![property_name],
            )?;
            let parent = view.node(location)?.parent().expect(NIL);
            if view.node(parent)?.kind() == K::VariableDeclaration {
                let name = self.error_declaration_name_with_fallback()?;
                let related = create_diagnostic_for_node(
                    self.resolver.ast(location)?,
                    location,
                    d::Add_a_type_annotation_to_the_variable_0,
                    vec![name],
                )?;
                diag.related_information.push(std::sync::Arc::new(related));
            }
            self.add_diagnostic(diag);
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.ReportTruncationError
    fn report_truncation_error(&mut self) -> Result<(), R::Error> {
        if let Some(location) = self.tracker.error_location() {
            self.add_diagnostic_for_node(
                location,
                d::The_inferred_type_of_this_node_exceeds_the_maximum_length_the_compiler_will_serialize_An_explicit_type_annotation_is_needed,
                vec![],
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/tracker.go:SymbolTrackerImpl.errorDeclarationNameWithFallback
    fn error_declaration_name_with_fallback(&self) -> Result<JsString, R::Error> {
        if let Some(name) = self.tracker.error_name {
            return Ok(tsr_scanner::declaration_name_to_string(
                self.resolver.ast(name)?,
                Some(name),
            )?);
        }
        if let Some(fallback) = self.tracker.error_fallback_node() {
            let view = self.resolver.ast(fallback)?;
            if let Some(name) = tsr_ast::get_name_of_declaration(view, Some(fallback))? {
                return Ok(tsr_scanner::declaration_name_to_string(
                    self.resolver.ast(name)?,
                    Some(name),
                )?);
            }
            if let Some(data) = view.node(fallback)?.as_export_assignment() {
                return Ok(JsString::from_bytes(if data.is_export_equals() {
                    b"export=".as_slice()
                } else {
                    b"default".as_slice()
                }));
            }
        }
        Ok(JsString::from_bytes(b"(Missing)".as_slice())) // same fallback declarationNameToString uses when node is zero-width (ie, nameless)
    }
}
