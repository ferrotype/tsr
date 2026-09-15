//! Identifier flow starts with the exported value's declaration, retaining the
//! local symbol for diagnostics and assignment tracking.
use crate::{
    flow_assignments::AssignmentKind, node_check_flags as nc, type_facts as f, type_flags as tf,
    CheckerState, Error, TypeId, TypeSystemEntity, TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, what: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(what))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkIdentifier
    pub(crate) fn check_value_identifier(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if self.flow_this_type_query(node)? {
            return self.check_this_expression(node);
        }
        let local = self.resolved_value_symbol(node)?;
        if local == self.builtins.unknown_symbol {
            return Ok(self.builtins.error_type);
        }
        if local == self.builtins.arguments_symbol {
            if self.in_property_initializer_or_static_block(node, true)? {
                self.error_at(Some(node),d::X_arguments_cannot_be_referenced_in_property_initializers_or_class_static_initialization_blocks,vec![])?;
                return Ok(self.builtins.error_type);
            }
            return self.get_type_of_symbol(local);
        }
        self.mark_value_identifier_alias(node, local)?;
        let symbol = self.get_export_symbol_of_value_symbol_if_exported(local)?;
        let flags = self.symbol(symbol)?.flags();
        if flags & sf::ALIAS != 0 {
            self.resolve_alias(symbol)?;
        }
        let immediate = self.symbol(symbol)?.value_declaration();
        if let Some(declaration) = immediate {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() == K::BindingElement {
                let pattern = required(read.parent(), "binding pattern")?;
                if self.bindings.contextual_patterns.contains(&pattern)
                    && self.reference_descendant(node, pattern)?
                {
                    return Ok(self.builtins.non_inferrable_any_type);
                }
            }
        }
        let mut ty = self.narrowed_type_of_symbol(symbol, node)?;
        let assignment = self.assignment_target_kind(node)?;
        if assignment != AssignmentKind::None {
            let js_module = self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0
                && flags & sf::VALUE_MODULE != 0;
            let error = if flags & sf::VARIABLE == 0 && !js_module {
                Some(if flags & sf::ENUM != 0 {
                    d::Cannot_assign_to_0_because_it_is_an_enum
                } else if flags & sf::CLASS != 0 {
                    d::Cannot_assign_to_0_because_it_is_a_class
                } else if flags & sf::MODULE != 0 {
                    d::Cannot_assign_to_0_because_it_is_a_namespace
                } else if flags & sf::FUNCTION != 0 {
                    d::Cannot_assign_to_0_because_it_is_a_function
                } else if flags & sf::ALIAS != 0 {
                    d::Cannot_assign_to_0_because_it_is_an_import
                } else {
                    d::Cannot_assign_to_0_because_it_is_not_a_variable
                })
            } else if self.is_readonly_symbol(symbol)? {
                Some(if flags & sf::VARIABLE != 0 {
                    d::Cannot_assign_to_0_because_it_is_a_constant
                } else {
                    d::Cannot_assign_to_0_because_it_is_a_read_only_property
                })
            } else {
                None
            };
            if let Some(error) = error {
                let name = self.symbol_to_string(local)?;
                self.error_at(Some(node), error, vec![name])?;
                return Ok(self.builtins.error_type);
            }
        }
        let alias = flags & sf::ALIAS != 0;
        let declaration = if flags & sf::VARIABLE != 0 {
            if assignment == AssignmentKind::Definite {
                return if self.compound_like_assignment(node)? {
                    self.base_literal_type(ty)
                } else {
                    Ok(ty)
                };
            }
            immediate
        } else if alias {
            Some(self.alias_declaration(local)?)
        } else {
            return Ok(ty);
        };
        let Some(declaration) = declaration else {
            return Ok(ty);
        };
        ty = self.narrowable_reference_type(ty, node, self.expression_mode)?;
        let root = ts_ast::utilities::get_root_declaration(self.ast(declaration)?, declaration)?;
        let parameter = self.ast(root)?.node(root)?.kind() == K::Parameter;
        let declaration_container = self.control_flow_container_or_none(declaration)?;
        let mut container = self.control_flow_container(node)?;
        let outer = Some(container) != declaration_container;
        let parent = required(self.ast(node)?.node(node)?.parent(), "identifier parent")?;
        let nonnull = self.ast(parent)?.node(parent)?.kind() == K::NonNullExpression;
        let automatic = ty == self.builtins.auto_type
            || self.query.global_types.get("autoArrayType") == Some(&ty);
        let auto_nonnull = automatic && nonnull;
        while Some(container) != declaration_container {
            let read = self.ast(container)?.node(container)?;
            if !matches!(
                read.kind().known(),
                Some(K::FunctionExpression | K::ArrowFunction)
            ) && !ts_ast::utilities::is_object_literal_or_class_expression_method_or_accessor(
                self.ast(container)?,
                container,
            )? {
                break;
            }
            if !(self.is_constant_flow_variable(symbol)?
                && self.query.global_types.get("autoArrayType") != Some(&ty)
                || self.is_parameter_or_mutable_local_variable(symbol)?
                    && self.is_past_last_assignment(symbol, Some(node))?)
            {
                break;
            }
            container = self.control_flow_container(container)?;
        }
        let never_initialized = if let Some(declaration) = immediate {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() == K::VariableDeclaration {
                let data = read
                    .data_source()
                    .as_variable_declaration()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let list = required(read.parent(), "uninitialized declaration list")?;
                let statement = required(
                    self.ast(list)?.node(list)?.parent(),
                    "uninitialized statement",
                )?;
                !matches!(
                    self.ast(statement)?.node(statement)?.kind().known(),
                    Some(K::ForInStatement | K::ForOfStatement)
                ) && data.initializer().is_none()
                    && data.exclamation_token().is_none()
                    && self.is_mutable_local_variable_declaration(declaration)?
                    && !self.is_symbol_assigned_definitely(local)?
            } else {
                false
            }
        } else {
            false
        };
        let read = self.ast(declaration)?.node(declaration)?;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let assertion = read
            .data_source()
            .as_variable_declaration()
            .is_some_and(|data| data.exclamation_token().is_some());
        let spread = self.ast(parent)?.node(parent)?.kind() == K::SpreadAssignment
            && self
                .ast(parent)?
                .node(parent)?
                .parent()
                .map(|parent| self.reference_destructuring_target(parent))
                .transpose()?
                .unwrap_or(false);
        let assume = parameter
            || alias
            || outer && !never_initialized
            || spread
            || self.symbol(local)?.flags() & sf::MODULE_EXPORTS != 0
            || self.same_scoped_binding_reference(node, declaration)?
            || !automatic
                && (!self.options.strict_null_checks
                    || self.types.flags(ty)? & (tf::ANY_OR_UNKNOWN | tf::VOID) != 0
                    || self.in_type_query(node)?
                    || self.in_ambient_or_type_node(node)?
                    || self.ast(parent)?.node(parent)?.kind() == K::ExportSpecifier)
            || nonnull
            || assertion
            || ambient;
        let initial = if auto_nonnull {
            self.builtins.undefined_type
        } else if assume && parameter {
            self.remove_parameter_reference_optionality(ty, declaration)?
        } else if assume {
            ty
        } else if automatic {
            self.builtins.undefined_type
        } else {
            self.add_type_optionality(ty, false, true)?
        };
        let mut flow = self.flow_type_of_reference(node, ty, initial, container)?;
        if auto_nonnull {
            flow = self.non_nullable_type(flow)?;
        }
        if !self.evolving_array_operation_target(node)? && automatic {
            if flow == self.builtins.auto_type
                || self.query.global_types.get("autoArrayType") == Some(&flow)
            {
                if self
                    .program()?
                    .host
                    .options()
                    .strict_option_value(self.program()?.host.options().no_implicit_any)
                {
                    let name = self.symbol_to_string(local)?;
                    let display = self.type_to_string(flow, crate::type_display::DEFAULT_FLAGS)?;
                    self.error_at(self.ast(declaration)?.node(declaration)?.name(),d::Variable_0_implicitly_has_type_1_in_some_locations_where_its_type_cannot_be_determined,vec![name.clone(),display.clone()])?;
                    self.error_at(
                        Some(node),
                        d::Variable_0_implicitly_has_an_1_type,
                        vec![name, display],
                    )?;
                }
                return if flow == self.builtins.auto_type {
                    Ok(self.builtins.any_type)
                } else {
                    self.any_array_type()
                };
            }
        } else if !assume
            && !self.maybe_type_of_kind(ty, tf::UNDEFINED)?
            && self.maybe_type_of_kind(flow, tf::UNDEFINED)?
        {
            let name = self.symbol_to_string(local)?;
            self.error_at(
                Some(node),
                d::Variable_0_is_used_before_being_assigned,
                vec![name],
            )?;
            return Ok(ty);
        }
        if assignment == AssignmentKind::None {
            Ok(flow)
        } else {
            self.base_literal_type(flow)
        }
    }

    fn reference_descendant(&self, mut node: NodeId, ancestor: NodeId) -> Result<bool, Error> {
        loop {
            if node == ancestor {
                return Ok(true);
            }
            let Some(parent) = self.ast(node)?.node(node)?.parent() else {
                return Ok(false);
            };
            node = parent;
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.isDestructuringAssignmentTarget
    pub(crate) fn reference_destructuring_target(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        if let Some(data) = read.data_source().as_binary_expression() {
            return Ok(data.left() == Some(node));
        }
        if read.kind() == K::ForOfStatement {
            return Ok(read.initializer() == Some(node));
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.isSameScopedBindingElement
    fn same_scoped_binding_reference(
        &self,
        node: NodeId,
        declaration: NodeId,
    ) -> Result<bool, Error> {
        if self.ast(declaration)?.node(declaration)?.kind() != K::BindingElement {
            return Ok(false);
        }
        let mut ancestor = Some(node);
        while let Some(node) = ancestor {
            if self.ast(node)?.node(node)?.kind() == K::BindingElement {
                return Ok(
                    ts_ast::utilities::get_root_declaration(self.ast(node)?, node)?
                        == ts_ast::utilities::get_root_declaration(
                            self.ast(declaration)?,
                            declaration,
                        )?,
                );
            }
            ancestor = self.ast(node)?.node(node)?.parent();
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.removeOptionalityFromDeclaredType
    fn remove_parameter_reference_optionality(
        &mut self,
        ty: TypeId,
        declaration: NodeId,
    ) -> Result<TypeId, Error> {
        let read = self.ast(declaration)?.node(declaration)?;
        if self.options.strict_null_checks
            && read.kind() == K::Parameter
            && read.initializer().is_some()
            && self.type_facts(ty, f::IS_UNDEFINED)? != 0
            && !self.parameter_initializer_contains_undefined(declaration)?
        {
            return self.type_with_facts(ty, f::NE_UNDEFINED);
        }
        Ok(ty)
    }

    /// The answer is cached on the declaration's node links, and the initializer
    /// is checked under a resolution entry: a parameter whose default refers back
    /// to the parameter reports a circularity once instead of recurring forever.
    // port: tsc/internal/checker/checker.go:Checker.parameterInitializerContainsUndefined
    fn parameter_initializer_contains_undefined(
        &mut self,
        declaration: NodeId,
    ) -> Result<bool, Error> {
        if self.initializer_node_flags(declaration) & nc::INITIALIZER_IS_UNDEFINED_COMPUTED == 0 {
            if !self.push_type_resolution(
                TypeSystemEntity::Node(declaration),
                TypeSystemPropertyName::InitializerIsUndefined,
            ) {
                self.report_declaration_circularity(declaration)?;
                return Ok(true);
            }
            let contains = self
                .check_declaration_initializer(declaration, 0, None)
                .and_then(|initializer| self.type_facts(initializer, f::IS_UNDEFINED));
            // Both initializer checking and fact computation can fail. Restore
            // the stack before propagating either error to the next query.
            let complete = self.resolution.pop();
            let contains = contains? != 0;
            if !complete {
                self.report_declaration_circularity(declaration)?;
                return Ok(true);
            }
            // Upstream re-tests the flag: checking the initializer can have set it.
            let flags = self.emit_checks.node_flags.get_or_default(declaration);
            if *flags & nc::INITIALIZER_IS_UNDEFINED_COMPUTED == 0 {
                *flags |= nc::INITIALIZER_IS_UNDEFINED_COMPUTED
                    | if contains {
                        nc::INITIALIZER_IS_UNDEFINED
                    } else {
                        0
                    };
            }
        }
        Ok(self.initializer_node_flags(declaration) & nc::INITIALIZER_IS_UNDEFINED != 0)
    }

    fn initializer_node_flags(&self, declaration: NodeId) -> u32 {
        self.emit_checks
            .node_flags
            .try_get(declaration)
            .copied()
            .unwrap_or_default()
    }

    /// `reportCircularityError` takes the declaration's symbol upstream; a
    /// declaration without one has nothing to name, so it reports nothing.
    fn report_declaration_circularity(&mut self, declaration: NodeId) -> Result<(), Error> {
        if let Some(symbol) = self.get_symbol_of_declaration(declaration)? {
            self.report_symbol_circularity(symbol)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.isInPropertyInitializerOrClassStaticBlock
    pub(crate) fn in_property_initializer_or_static_block(
        &self,
        mut node: NodeId,
        ignore_arrows: bool,
    ) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            let parent = read.parent();
            match read.kind().known() {
                Some(K::PropertyDeclaration | K::ClassStaticBlockDeclaration) => return Ok(true),
                Some(K::TypeQuery | K::JsxClosingElement) => return Ok(false),
                Some(K::ArrowFunction) if !ignore_arrows => return Ok(false),
                Some(K::Block) => {
                    if let Some(parent) = parent {
                        let read = self.ast(parent)?.node(parent)?;
                        if ts_ast::utilities::is_function_like(Some(&read))
                            && read.kind() != K::ArrowFunction
                        {
                            return Ok(false);
                        }
                    }
                }
                _ => {}
            }
            let Some(parent) = parent else {
                return Ok(false);
            };
            node = parent;
        }
    }

    fn mark_value_identifier_alias(&mut self, node: NodeId, symbol: SymbolId) -> Result<(), Error> {
        let parent = self.ast(node)?.node(node)?.parent();
        if let Some(parent) = parent {
            let read = self.ast(parent)?.node(parent)?;
            if read.kind() == K::PropertyAccessExpression && read.expression() == Some(node) {
                return Ok(());
            }
            if read
                .data_source()
                .as_export_specifier()
                .is_some_and(|data| data.is_type_only())
            {
                return Ok(());
            }
            if let Some(grandparent) = read.parent() {
                if let Some(great) = self.ast(grandparent)?.node(grandparent)?.parent() {
                    if self
                        .ast(great)?
                        .node(great)?
                        .data_source()
                        .as_export_declaration()
                        .is_some_and(|data| data.is_type_only())
                    {
                        return Ok(());
                    }
                }
            }
        }
        self.mark_alias_referenced_at(node, symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.markAliasReferenced
    pub(crate) fn mark_alias_referenced_at(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        if self
            .program()?
            .host
            .options()
            .verbatim_module_syntax
            .is_true()
            || self.in_type_query(node)?
            || !ts_ast::is_non_local_alias(Some(&self.symbol(symbol)?), sf::VALUE)
        {
            return Ok(());
        }
        let target = self.resolve_alias(symbol)?;
        if self.module_symbol_flags(symbol, true, false)? & (sf::VALUE | sf::EXPORT_VALUE) == 0 {
            return Ok(());
        }
        let target = self.get_export_symbol_of_value_symbol_if_exported(target)?;
        let flags = self.symbol(target)?.flags();
        let options = self.program()?.host.options();
        if options.isolated_modules()
            || flags & sf::CONST_ENUM == 0
                && !(flags & sf::VALUE_MODULE != 0 && flags & sf::CONST_ENUM_ONLY_MODULE != 0)
        {
            self.mark_module_alias_referenced(symbol)?;
        }
        Ok(())
    }
}
