//! Variable-like source checks for parameter and binding declarations. Child
//! binding elements are checked before the enclosing initializer is related.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    pub(crate) fn check_binding_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_binding_element_grammar(node)?;
        self.check_binding_variable(node)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarBindingElement
    fn check_binding_element_grammar(&mut self, node: NodeId) -> Result<(), Error> {
        if !self.binding_is_rest(node)? {
            return Ok(());
        }
        let read = self.ast(node)?.node(node)?;
        let parent = read.parent().ok_or(Error::MissingLink("rest pattern"))?;
        let property = read.property_name();
        let initializer = read.initializer();
        let name = read.name().ok_or(Error::MissingLink("rest binding name"))?;
        let list = self
            .ast(parent)?
            .node(parent)?
            .element_list()
            .ok_or(Error::MissingLink("rest elements"))?;
        let elements = self.source_list(parent, Some(list))?;
        if elements.last() != Some(&node) {
            self.grammar_error_node(
                node,
                d::A_rest_element_must_be_last_in_a_destructuring_pattern,
                vec![],
            )?;
            return Ok(());
        }
        if self.ast(parent)?.list_has_trailing_comma(list)? {
            let end = self.ast(parent)?.list(list)?.loc().end();
            self.grammar_error_range(
                parent,
                end - 1,
                end,
                d::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma,
            )?;
        }
        if property.is_some() {
            self.grammar_error_node(name, d::A_rest_element_cannot_have_a_property_name, vec![])?;
            return Ok(());
        }
        if let Some(initializer) = initializer {
            let pos = i64::from(self.ast(initializer)?.node(initializer)?.pos());
            self.grammar_error_range(
                node,
                pos - 1,
                pos,
                d::A_rest_element_cannot_have_an_initializer,
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkVariableLikeDeclaration
    pub(crate) fn check_binding_variable(&mut self, node: NodeId) -> Result<(), Error> {
        for modifier in self.source_list(node, self.ast(node)?.node(node)?.modifiers())? {
            if self.ast(modifier)?.node(modifier)?.kind() == K::Decorator {
                return Err(Error::Unsupported("checkDecorators: parameter or binding"));
            }
        }
        let read = self.ast(node)?.node(node)?;
        let Some(name) = read.name() else {
            return Ok(());
        };
        let binding_element = read.kind() == K::BindingElement;
        let initializer = read.initializer();
        let annotation = read.type_node();
        let pattern = self.is_binding_pattern(name)?;
        let root = self.root_binding_declaration(node)?;
        let parameter = self.ast(root)?.node(root)?.kind() == K::Parameter;
        if !binding_element {
            if let Some(annotation) = annotation {
                self.check_source_element(annotation)?;
            }
        }
        if binding_element {
            if let Some(property) = self.ast(node)?.node(node)?.property_name() {
                if self.ast(property)?.node(property)?.kind() == K::PrivateIdentifier {
                    self.grammar_error_node(
                        property,
                        d::Private_identifiers_cannot_be_used_in_destructuring_patterns,
                        vec![],
                    )?;
                }
                if self.ast(name)?.node(name)?.kind() == K::Identifier
                    && parameter
                    && !self.parameter_function_body_present(root)?
                {
                    // type F = ({a: string}) => void;
                    //               ^^^^^^
                    // variable renaming in function type notation is confusing,
                    // so we forbid it even if noUnusedLocals is not enabled
                    self.query.renamed_binding_elements_in_types.push(node);
                    return Ok(());
                }
                if self.ast(property)?.node(property)?.kind() == K::ComputedPropertyName {
                    self.check_computed_property_name(property)?;
                }
            }
            let parent = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("binding pattern"))?;
            if self.ast(parent)?.node(parent)?.kind() == K::ObjectBindingPattern
                && self.binding_is_rest(node)?
                && self.program()?.host.options().emit_script_target()
                    < ts_core::ScriptTarget::ES2018
            {
                self.check_external_emit_helpers(node, crate::external_emit_helpers::REST)?;
            }
            let declaration = self
                .ast(parent)?
                .node(parent)?
                .parent()
                .ok_or(Error::MissingLink("parent binding declaration"))?;
            let mode = if self.binding_is_rest(node)? { 32 } else { 0 };
            if let Some(parent_type) = self.type_for_binding_element_parent(declaration, mode)? {
                let property = self.ast(node)?.node(node)?.property_name().unwrap_or(name);
                if !self.is_binding_pattern(property)? {
                    let literal = self.literal_type_from_property_name(property)?;
                    if let Some(text) = self.index_property_name(literal)? {
                        if let Some(symbol) =
                            self.constituent_property(parent_type, text.as_bytes(), false)?
                        {
                            self.mark_property_as_referenced(symbol, None, false)?;
                            // A destructuring is never a write-only reference.
                            let is_super = self
                                .ast(declaration)?
                                .node(declaration)?
                                .initializer()
                                .map(|initializer| {
                                    Ok::<_, Error>(
                                        self.ast(initializer)?.node(initializer)?.kind()
                                            == K::SuperKeyword,
                                    )
                                })
                                .transpose()?
                                .unwrap_or(false);
                            let error_node = self.ast(node)?.node(node)?.property_name_or_name();
                            self.check_access_property_accessibility(
                                node,
                                is_super,
                                false,
                                parent_type,
                                symbol,
                                error_node,
                            )?;
                        }
                    }
                }
            }
        }
        if pattern {
            for element in self.source_list(name, self.ast(name)?.node(name)?.element_list())? {
                self.check_source_element(element)?;
            }
        }
        if initializer.is_some() && parameter && !self.parameter_function_body_present(root)? {
            self.error_at(Some(node),d::A_parameter_initializer_is_only_allowed_in_a_function_or_constructor_implementation,vec![])?;
            return Ok(());
        }
        if pattern {
            if self.binding_ambient_or_type(node)? {
                return Ok(());
            }
            let parent = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("binding owner parent"))?;
            let for_in = match self.ast(parent)?.node(parent)?.parent() {
                Some(parent) => self.ast(parent)?.node(parent)?.kind() == K::ForInStatement,
                None => false,
            };
            let check_initial = initializer.is_some() && !for_in;
            let mut empty = true;
            for element in self.source_list(name, self.ast(name)?.node(name)?.element_list())? {
                empty &= self.ast(element)?.node(element)?.name().is_none();
            }
            if check_initial || empty {
                let raw = self.type_for_variable_like_raw(node, true, 0)?;
                let widened = self.widen_type_for_variable_like(node, raw, false)?;
                if let Some(initializer) = initializer.filter(|_| check_initial) {
                    let initial = self.check_expression_cached(initializer)?;
                    if self.options.strict_null_checks && empty {
                        self.check_binding_non_null_non_void(initial, node)?;
                    } else {
                        let raw = self.type_for_variable_like_raw(node, true, 0)?;
                        let target = self.widen_type_for_variable_like(node, raw, false)?;
                        self.check_expression_related_with_elaboration(
                            initial,
                            target,
                            RelationKind::Assignable,
                            Some(node),
                            Some(initializer),
                            None,
                        )?;
                    }
                }
                if empty {
                    if self.ast(name)?.node(name)?.kind() == K::ArrayBindingPattern {
                        self.check_iterated_type_or_element_type(
                            crate::iteration::ALLOW_SYNC
                                | crate::iteration::ALLOW_STRING
                                | crate::iteration::DESTRUCTURING_FLAG,
                            widened,
                            self.builtins.undefined_type,
                            Some(node),
                        )?;
                    } else if self.options.strict_null_checks {
                        self.check_binding_non_null_non_void(widened, node)?;
                    }
                }
            }
            return Ok(());
        }
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("binding variable symbol"))?;
        let target = self.get_type_of_symbol(symbol)?;
        let target = if target == self.builtins.auto_type {
            self.builtins.any_type
        } else if self.query.global_types.get("autoArrayType") == Some(&target) {
            *self
                .query
                .global_types
                .get("anyArrayType")
                .ok_or(Error::MissingLink("any array type"))?
        } else {
            target
        };
        if self.symbol(symbol)?.value_declaration() == Some(node) {
            if let Some(initializer) = initializer {
                let initial = self.check_expression_cached(initializer)?;
                self.check_expression_related_with_elaboration(
                    initial,
                    target,
                    RelationKind::Assignable,
                    Some(node),
                    Some(initializer),
                    None,
                )?;
            }
            self.check_variable_declaration_flags(node, symbol, true)?;
        } else {
            self.check_secondary_variable(node, symbol, target)?;
        }
        self.check_exports_on_merged_declarations(node)?;
        if matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::VariableDeclaration | K::BindingElement)
        ) {
            self.check_var_names_not_shadowed(node)?;
        }
        self.check_function_name_collision_boundary(node)
    }

    fn parameter_function_body_present(&self, parameter: NodeId) -> Result<bool, Error> {
        let function = self
            .containing_body_function(parameter)?
            .ok_or(Error::MissingLink("parameter function"))?;
        match self.ast(function)?.node(function)?.body() {
            Some(body) => {
                let read = self.ast(body)?.node(body)?;
                Ok(read.pos() != read.end())
            }
            None => Ok(false),
        }
    }

    // port: tsc/internal/checker/utilities.go:isInAmbientOrTypeNode
    fn binding_ambient_or_type(&self, node: NodeId) -> Result<bool, Error> {
        if self.ast(node)?.node(node)?.flags() & nf::AMBIENT != 0 {
            return Ok(true);
        }
        let mut current = Some(node);
        while let Some(node) = current {
            let read = self.ast(node)?.node(node)?;
            if matches!(
                read.kind().known(),
                Some(K::InterfaceDeclaration | K::TypeAliasDeclaration | K::TypeLiteral)
            ) {
                return Ok(true);
            }
            current = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNonNullNonVoidType
    fn check_binding_non_null_non_void(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let ty = self.check_non_null_type_with_reporter(ty, node, false)?;
        if self.types.flags(ty)? & tf::VOID != 0 {
            if ts_ast::is_entity_name_expression(self.ast(node)?, node)? {
                let text = self.entity_name_text(node)?;
                if self.ast(node)?.node(node)?.kind() == K::Identifier
                    && text.as_bytes() == b"undefined"
                {
                    self.error_at(Some(node), d::The_value_0_cannot_be_used_here, vec![text])?;
                    return Ok(ty);
                }
                if text.len() < 100 {
                    self.error_at(Some(node), d::X_0_is_possibly_undefined, vec![text])?;
                    return Ok(ty);
                }
            }
            self.error_at(Some(node), d::Object_is_possibly_undefined, vec![])?;
        }
        Ok(ty)
    }
}
