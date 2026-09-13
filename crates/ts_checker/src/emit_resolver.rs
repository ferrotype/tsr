//! The declaration resolver borrows the current exclusive checker operation.
//! No callback takes another owner lock, and output syntax belongs to its caller.
use crate::{type_flags as tf, CheckerState, Error, Operation};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, symbol_flags as sf, AstBuilder, AstView,
    FactoryMethods, SyntaxKind as K,
};
use ts_printer::emit_resolver::{ConstantValue, DeclarationEmitResolver, EnumMemberValue};

impl DeclarationEmitResolver for Operation<'_> {
    fn create_expando_namespace_scope(
        &mut self,
        parent: NodeId,
        name: ts_ast::JsString,
        host: SymbolId,
        local_name: ts_ast::JsString,
        symbol: SymbolId,
    ) -> Result<NodeId, Error> {
        self.state_mut().create_emit_scope(
            parent,
            K::ModuleDeclaration,
            Some(name),
            Some(host),
            [(local_name, Some(symbol))].into(),
            None,
        )
    }
    fn symbol_flags(&mut self, symbol: SymbolId) -> Result<ts_ast::SymbolFlags, Error> {
        Ok(self.state().symbol(symbol)?.flags())
    }
    fn symbol_export(&mut self, symbol: SymbolId, name: &[u8]) -> Result<Option<SymbolId>, Error> {
        let Some(table) = self.state().symbol(symbol)?.exports() else {
            return Ok(None);
        };
        Ok(self.state().table(table)?.get(name).flatten())
    }
    type Error = Error;

    fn referenced_value_declaration(&mut self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if !self.state().emit_parse_node(node)? {
            return Ok(None);
        }
        self.state_mut().emit_referenced_value_declaration(node)
    }
    fn element_access_expression_name(&mut self, node: NodeId) -> Result<ts_ast::JsString, Error> {
        self.state_mut().emit_element_access_expression_name(node)
    }
    fn referenced_name_declaration(
        &mut self,
        name: ts_ast::JsString,
        parent: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.state_mut()
            .emit_referenced_name_declaration(name, parent)
    }
    fn referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.state_mut()
            .emit_referenced_member_value_declaration(node)
    }
    fn referenced_value_declaration_unsafe(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.state_mut().emit_referenced_value_declaration(node)
    }
    fn redundant_this_property_assignment(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut()
            .emit_redundant_this_property_assignment(node)
    }
    fn definitely_reference_to_global_symbol_object(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        self.state_mut().emit_definitely_global_symbol_object(node)
    }

    fn unsupported(operation: &'static str) -> Error {
        Error::Unsupported(operation)
    }
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        self.state().ast(node)
    }
    fn retain_source(&self, source: NodeId, output: &mut AstBuilder) -> Result<(), Error> {
        let program = self.state().program()?;
        for index in 0..program.host.source_file_count() {
            let file = program.host.source_file(index);
            if file.source() == source {
                output.retain_completed(file);
                return Ok(());
            }
        }
        Err(ts_arena::Error::WrongOwner.into())
    }
    fn bound_symbol_of_declaration(&self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        self.state().raw_declaration_symbol(node)
    }
    fn symbol_of_declaration(&mut self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        self.state_mut().get_symbol_of_declaration(node)
    }
    fn symbol_value_declaration(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        Ok(self.state().symbol(symbol)?.value_declaration())
    }
    fn symbol_declarations(&self, symbol: SymbolId) -> Result<Vec<NodeId>, Error> {
        Ok(self
            .state()
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect())
    }
    fn precalculate_declaration_emit_visibility(&mut self, source: NodeId) -> Result<(), Error> {
        self.state_mut().emit_precalculate_visibility(source)
    }
    fn is_declaration_visible(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_declaration_visible(Some(node))
    }
    fn effective_declaration_flags(&mut self, node: NodeId, flags: u32) -> Result<u32, Error> {
        self.state().effective_declaration_flags(node, flags)
    }
    fn implementation_of_overload(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_implementation_of_overload(node)
    }
    fn optional_parameter(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().is_optional_parameter(node)
    }
    fn literal_const_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_literal_const_declaration(node)
    }
    fn symbol_accessible(
        &mut self,
        symbol: Option<SymbolId>,
        enclosing: Option<NodeId>,
        meaning: u32,
        compute_aliases: bool,
    ) -> Result<ts_printer::emit_resolver::SymbolAccessibilityResult, Error> {
        self.state_mut()
            .emit_symbol_accessible(symbol, enclosing, meaning, compute_aliases, true)
    }
    fn entity_name_visible(
        &mut self,
        node: NodeId,
        enclosing: NodeId,
    ) -> Result<ts_printer::emit_resolver::SymbolAccessibilityResult, Error> {
        self.state_mut().emit_entity_visible(node, enclosing)
    }
    fn late_bound(&mut self, node: NodeId) -> Result<bool, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(false);
        }
        Ok(match state.get_symbol_of_declaration(node)? {
            Some(s) => state.symbol(s)?.check_flags() & ts_ast::check_flags::LATE != 0,
            None => false,
        })
    }
    fn enum_member_value(&mut self, node: NodeId) -> Result<EnumMemberValue, Error> {
        let state = self.state_mut();
        if !state.emit_parse_node(node)? {
            return Ok(EnumMemberValue::default());
        }
        let parent = state
            .emit_parent(node, 1)?
            .ok_or(Error::MissingLink("enum member parent"))?;
        state.compute_enum_member_values(parent)?;
        let Some(value) = state.enums.values.try_get(node) else {
            return Ok(EnumMemberValue::default());
        };
        Ok(EnumMemberValue {
            value: value.value.as_ref().map(|v| match v {
                crate::enums::EnumValue::Number(n) => ConstantValue::Number(*n),
                crate::enums::EnumValue::String(s) => ConstantValue::String(s.clone()),
            }),
            is_syntactically_string: value.is_syntactically_string,
            resolved_other_files: value.resolved_other_files,
            has_external_references: value.has_external_references,
        })
    }
    fn requires_adding_implicit_undefined(
        &mut self,
        node: NodeId,
        symbol: Option<SymbolId>,
        enclosing: Option<NodeId>,
    ) -> Result<bool, Error> {
        self.state_mut()
            .emit_requires_undefined(node, symbol, enclosing)
    }
    fn requires_adding_implicit_undefined_unsafe(
        &mut self,
        node: NodeId,
        symbol: Option<SymbolId>,
        enclosing: Option<NodeId>,
    ) -> Result<bool, Error> {
        self.state_mut()
            .emit_requires_undefined(node, symbol, enclosing)
    }
    fn properties_of_container_function(&mut self, node: NodeId) -> Result<Vec<SymbolId>, Error> {
        self.state_mut().emit_container_function_properties(node)
    }
    fn expando_function_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_expando_function(node)
    }
    fn expando_function_declaration_unsafe(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_expando_function(node)
    }
    fn create_late_bound_index_signatures(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Vec<NodeId>, Error> {
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags,
            internal_flags,
            tracker,
            |builder| builder.declaration_late_indexes(node),
        )
    }
    fn name_resolvable(&mut self, node: NodeId, name: &[u8]) -> Result<bool, Error> {
        Ok(self
            .state_mut()
            .resolve_name(
                Some(node),
                name,
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
                None,
                false,
            )?
            .is_some())
    }
    fn create_type_of_declaration(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Error> {
        let original = emit.most_original(node);
        if output.view().node(original)?.flags() & nf::SYNTHESIZED != 0 {
            return Ok(Some(output.new_keyword_type_node(K::AnyKeyword.into())));
        }
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags | ts_nodebuilder::flags::MULTILINE_OBJECT_LITERALS,
            internal_flags,
            tracker,
            |builder| {
                let result =
                    builder.serialize_declaration_type(Some(original), None, None, true)?;
                Ok((!builder.encountered_error).then_some(result))
            },
        )
    }
    fn create_return_type_of_signature(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Error> {
        let original = emit.most_original(node);
        if output.view().node(original)?.flags() & nf::SYNTHESIZED != 0 {
            return Ok(Some(output.new_keyword_type_node(K::AnyKeyword.into())));
        }
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags,
            internal_flags,
            tracker,
            |builder| {
                let signature = builder.checker.signature_from_declaration(original)?;
                let result = builder.with_signature_scope(signature, |b| {
                    b.serialize_signature_return(signature, true)
                })?;
                Ok(if builder.encountered_error {
                    None
                } else {
                    result
                })
            },
        )
    }
    fn create_type_of_expression(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Error> {
        let original = emit.most_original(node);
        if output.view().node(original)?.flags() & nf::SYNTHESIZED != 0 {
            return Ok(Some(output.new_keyword_type_node(K::AnyKeyword.into())));
        }
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags | ts_nodebuilder::flags::MULTILINE_OBJECT_LITERALS,
            internal_flags,
            tracker,
            |builder| {
                let result = builder.serialize_expression_type(original)?;
                Ok((!builder.encountered_error).then_some(result))
            },
        )
    }
    fn create_type_parameters_of_signature(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Vec<NodeId>, Error> {
        let node = emit.most_original(node);
        if output.view().node(node)?.flags() & nf::SYNTHESIZED != 0 {
            return Ok(vec![]);
        }
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags,
            internal_flags,
            tracker,
            |builder| {
                let nodes = builder.declaration_type_parameters(node)?;
                Ok(if builder.encountered_error {
                    vec![]
                } else {
                    nodes
                })
            },
        )
    }
    fn create_literal_const_value(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Error> {
        let node = emit.most_original(node);
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            node,
            0,
            0,
            tracker,
            |builder| builder.declaration_literal_value(node),
        )
    }
    fn try_js_type_node_to_type_node(
        &mut self,
        output: &mut AstBuilder,
        emit: &mut ts_printer::EmitContext,
        node: NodeId,
        enclosing: NodeId,
        flags: ts_nodebuilder::Flags,
        internal_flags: ts_nodebuilder::InternalFlags,
        tracker: &mut dyn ts_printer::emit_resolver::DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Error> {
        let node = emit.most_original(node);
        if output.view().node(node)?.flags() & nf::SYNTHESIZED != 0 {
            return Err(Error::MissingLink("JS annotation parse node"));
        }
        crate::node_builder::NodeBuilder::with_output(
            self.state_mut(),
            output,
            emit,
            enclosing,
            flags,
            internal_flags,
            tracker,
            |builder| {
                let result = builder.try_js_type_node_to_type_node(node)?;
                Ok(if builder.encountered_error {
                    None
                } else {
                    result
                })
            },
        )
    }
    fn external_module_file_from_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        self.state_mut().emit_external_module_file(node)
    }
    fn import_required_by_augmentation(&mut self, node: NodeId) -> Result<bool, Error> {
        self.state_mut().emit_import_required_by_augmentation(node)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsImplementationOfOverload
    fn emit_implementation_of_overload(&mut self, node: NodeId) -> Result<bool, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        let read = self.ast(node)?.node(node)?;
        if matches!(read.kind().known(), Some(K::GetAccessor | K::SetAccessor)) {
            return Ok(false);
        }
        let Some(body) = read.body() else {
            return Ok(false);
        };
        if !ts_ast::node_is_present(Some(&self.ast(body)?.node(body)?)) {
            return Ok(false);
        }
        let symbol = self.get_symbol_of_declaration(node)?;
        let signatures = self.signatures_of_symbol(symbol)?;
        if signatures.len() > 1 {
            return Ok(true);
        }
        if let Some(&signature) = signatures.first() {
            if Some(signature) == self.signature_of_full_signature(node)? {
                return Ok(false);
            }
            if let Some(declaration) = self.signatures.get(signature)?.declaration {
                return Ok(declaration != node
                    && self.ast(declaration)?.node(declaration)?.flags() & nf::JS_DOC == 0);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsLiteralConstDeclaration
    fn emit_literal_const_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let parameter_property = match read.parent() {
            Some(parent) => {
                ts_ast::utilities::is_parameter_property_declaration(view, node, parent)?
            }
            None => false,
        };
        let readonly = ts_ast::utilities::get_combined_modifier_flags(view, node)? & mf::READONLY
            != 0
            && !parameter_property;
        if readonly
            || read.kind() == K::VariableDeclaration && ts_ast::utilities::is_var_const(view, node)?
        {
            if let Some(symbol) = self.get_symbol_of_declaration(node)? {
                let ty = self.get_type_of_symbol(symbol)?;
                return self.is_fresh_literal_type(ty);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.requiresAddingImplicitUndefined
    pub(crate) fn emit_requires_undefined(
        &mut self,
        node: NodeId,
        symbol: Option<SymbolId>,
        enclosing: Option<NodeId>,
    ) -> Result<bool, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::PropertyDeclaration | K::PropertySignature | K::JSDocPropertyTag) => {
                let symbol = match symbol {
                    Some(s) => s,
                    None => self
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("implicit undefined property symbol"))?,
                };
                let ty = self.get_type_of_symbol(symbol)?;
                let flags = self.symbol(symbol)?.flags();
                if flags & sf::PROPERTY == 0
                    || flags & sf::OPTIONAL == 0
                    || self
                        .ast(node)?
                        .node(node)?
                        .question_token(self.ast(node)?)?
                        .is_none()
                {
                    return Ok(false);
                }
                // Reverse-mapped symbol links retain the source mapped type.
                if self.reverse_symbol_parts(symbol).is_none() {
                    return Ok(false);
                }
                let candidate = if self.types.flags(ty)? & tf::UNION != 0 {
                    self.types.union(ty)?.types[0]
                } else {
                    ty
                };
                Ok(self.types.flags(candidate)? & tf::UNDEFINED != 0
                    && candidate != self.builtins.missing_type)
            }
            Some(K::Parameter | K::JSDocParameterTag) => {
                let initialized = read.initializer().is_some();
                let annotation = read.type_node();
                if !self.options.strict_null_checks {
                    return Ok(false);
                }
                let optional = self.is_optional_parameter(node)?;
                let property = ts_ast::utilities::has_syntactic_modifier(
                    self.ast(node)?,
                    node,
                    mf::PARAMETER_PROPERTY_MODIFIER,
                )?;
                let enclosing_function = match enclosing {
                    Some(n) => ts_ast::utilities::is_function_like(Some(&self.ast(n)?.node(n)?)),
                    None => false,
                };
                let requires = (!optional && initialized && (!property || enclosing_function))
                    || optional && !initialized && property;
                if !requires {
                    return Ok(false);
                }
                if let Some(annotation) = annotation {
                    let ty = self.get_type_from_type_node(annotation)?;
                    let first = if self.types.flags(ty)? & tf::UNION != 0 {
                        self.types.union(ty)?.types[0]
                    } else {
                        ty
                    };
                    if self.is_error_type(ty)? || self.types.flags(first)? & tf::UNDEFINED != 0 {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => Err(Error::MissingLink("implicit undefined declaration kind")),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.GetPropertiesOfContainerFunction
    fn emit_container_function_properties(&mut self, node: NodeId) -> Result<Vec<SymbolId>, Error> {
        let Some(symbol) = self.get_symbol_of_declaration(node)? else {
            return Ok(vec![]);
        };
        let ty = self.get_type_of_symbol(symbol)?;
        self.get_properties_of_type(ty)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsExpandoFunctionDeclarationUnsafe
    fn emit_expando_function(&mut self, node: NodeId) -> Result<bool, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        for property in self.emit_container_function_properties(node)? {
            if let Some(value) = self.symbol(property)?.value_declaration() {
                if ts_ast::utilities_tail::is_expando_property_declaration(Some(
                    &self.ast(value)?.node(value)?,
                )) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getExternalModuleFileFromDeclaration
    pub(crate) fn emit_external_module_file(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(None);
        }
        let read = self.ast(node)?.node(node)?;
        let specifier = if read.kind() == K::ModuleDeclaration {
            match read.name() {
                Some(name) if self.ast(name)?.node(name)?.kind() == K::StringLiteral => Some(name),
                _ => None,
            }
        } else {
            self.module_specifier(node)?
        };
        let Some(specifier) = specifier else {
            return Ok(None);
        };
        self.prepare_module_attributes(specifier)?;
        let Some(symbol) =
            self.resolve_external_module_name_with_error(specifier, specifier, false, None, false)?
        else {
            return Ok(None);
        };
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            if self.ast(node)?.node(node)?.kind() == K::SourceFile {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.IsImportRequiredByAugmentation
    fn emit_import_required_by_augmentation(&mut self, node: NodeId) -> Result<bool, Error> {
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("augmentation import source"))?;
        let Some(file_symbol) = self.raw_declaration_symbol(source)? else {
            return Ok(false);
        };
        let Some(target) = self.emit_external_module_file(node)? else {
            return Ok(false);
        };
        if target == source {
            return Ok(false);
        }
        let Some(exports) = self.module_exports_of_symbol(file_symbol)? else {
            return Ok(false);
        };
        let symbols: Vec<_> = self
            .table(exports)?
            .iter()
            .filter_map(|(_, symbol)| symbol)
            .collect();
        for symbol in symbols {
            let merged = self.get_merged_symbol(symbol);
            if merged != symbol {
                for declaration in self.symbol_declarations(merged)?.iter().flatten() {
                    if ts_ast::utilities::get_source_file_of_node(
                        self.ast(declaration)?,
                        Some(declaration),
                    )? == Some(target)
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}
