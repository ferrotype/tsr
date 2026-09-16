//! Destructuring writes use indexed reads and flow narrowing before relating each target.
use crate::{access_flags as af, type_facts as facts, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{Factory, SyntaxKind as K};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.hasDefaultValue
    pub(crate) fn assignment_has_default(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(K::BindingElement) => read.initializer().is_some(),
            Some(K::PropertyAssignment) => self.assignment_has_default(required(
                read.initializer(),
                "assignment property initializer",
            )?)?,
            Some(K::ShorthandPropertyAssignment) => read
                .data_source()
                .as_shorthand_property_assignment()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .object_assignment_initializer()
                .is_some(),
            Some(K::BinaryExpression) => {
                let token = required(
                    read.data_source()
                        .as_binary_expression()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .operator_token(),
                    "assignment default operator",
                )?;
                self.ast(token)?.node(token)?.kind() == K::EqualsToken
            }
            _ => false,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.checkDestructuringAssignment
    pub(crate) fn check_destructuring_assignment(
        &mut self,
        node: NodeId,
        mut source: TypeId,
        mode: u32,
        right_is_this: bool,
    ) -> Result<TypeId, Error> {
        let mut target = node;
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::ShorthandPropertyAssignment {
            target = required(read.name(), "assignment shorthand name")?;
            let initializer = read
                .data_source()
                .as_shorthand_property_assignment()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .object_assignment_initializer();
            if let Some(initializer) = initializer {
                if self.options.strict_null_checks {
                    let value = self.check_expression(initializer)?;
                    if self.type_facts(value, facts::IS_UNDEFINED)? == 0 {
                        source = self.type_with_facts(source, facts::NE_UNDEFINED)?;
                    }
                }
                let left = self.check_expression_ex(target, mode)?;
                let right = self.check_expression_ex(initializer, mode)?;
                self.assignment_operator(target, initializer, K::EqualsToken.into(), left, right)?;
            }
        }
        if self.ast(target)?.node(target)?.kind() == K::BinaryExpression
            && self.assignment_has_default(target)?
        {
            self.check_expression_ex(target, mode)?;
            target = required(
                self.ast(target)?
                    .node(target)?
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .left(),
                "assignment default target",
            )?;
            if self.options.strict_null_checks {
                source = self.type_with_facts(source, facts::NE_UNDEFINED)?;
            }
        }
        match self.ast(target)?.node(target)?.kind().known() {
            Some(K::ObjectLiteralExpression) => {
                self.check_object_assignment(target, source, right_is_this)
            }
            Some(K::ArrayLiteralExpression) => self.check_array_assignment(target, source, mode),
            _ => self.check_reference_assignment(target, source, mode),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.checkObjectLiteralAssignment
    fn check_object_assignment(
        &mut self,
        node: NodeId,
        source: TypeId,
        right_is_this: bool,
    ) -> Result<TypeId, Error> {
        let list = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_object_literal_expression()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .properties();
        let properties = self.source_list(node, list)?;
        if self.options.strict_null_checks && properties.is_empty() {
            return self.check_non_null_type(source, node);
        }
        for (index, &property) in properties.iter().enumerate() {
            self.check_object_assignment_property(
                property,
                source,
                &properties,
                index,
                list,
                right_is_this,
            )?;
        }
        Ok(source)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkObjectLiteralDestructuringPropertyAssignment
    fn check_object_assignment_property(
        &mut self,
        property: NodeId,
        source: TypeId,
        properties: &[NodeId],
        index: usize,
        list: Option<ts_ast::NodeListId>,
        right_is_this: bool,
    ) -> Result<(), Error> {
        let read = self.ast(property)?.node(property)?;
        match read.kind().known() {
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment) => {
                let name = required(read.name(), "assignment property name")?;
                let target = if read.kind() == K::PropertyAssignment {
                    required(read.initializer(), "assignment property value")?
                } else {
                    property
                };
                if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
                    self.grammar_error_node(
                        name,
                        d::Private_identifiers_cannot_be_used_in_destructuring_patterns,
                        vec![],
                    )?;
                }
                let key = self.literal_type_from_property_name(name)?;
                if let Some(text) = self.index_property_name(key)? {
                    if let Some(symbol) =
                        self.constituent_property(source, text.as_bytes(), false)?
                    {
                        self.mark_property_as_referenced(symbol, Some(property), right_is_this)?;
                        // checkPropertyAccessibilityEx reports on node.Name() for these kinds.
                        self.check_access_property_accessibility(
                            property,
                            false,
                            true,
                            source,
                            symbol,
                            Some(name),
                        )?;
                    }
                }
                let flags = af::EXPRESSION_POSITION
                    | if self.assignment_has_default(property)? {
                        af::ALLOW_MISSING
                    } else {
                        0
                    };
                let element = self.get_indexed_access_type(source, key, flags, Some(name), None)?;
                let flow = self.flow_type_of_destructuring(property, element)?;
                self.check_destructuring_assignment(target, flow, 0, false)?;
            }
            Some(K::SpreadAssignment) => {
                let target = required(read.expression(), "assignment object rest")?;
                if index + 1 < properties.len() {
                    self.error_at(
                        Some(property),
                        d::A_rest_element_must_be_last_in_a_destructuring_pattern,
                        vec![],
                    )?;
                    return Ok(());
                }
                if self.program()?.host.options().emit_script_target()
                    < ts_core::ScriptTarget::ES2018
                {
                    self.check_external_emit_helpers(property, crate::external_emit_helpers::REST)?;
                }
                let mut names = Vec::new();
                for &other in properties {
                    let read = self.ast(other)?.node(other)?;
                    if read.kind() != K::SpreadAssignment {
                        names.push(required(read.name(), "non-rest assignment name")?);
                    }
                }
                let rest = self.object_rest_type(source, &names, self.types.get(source)?.symbol)?;
                if let Some(list) = list {
                    self.check_grammar_trailing_comma(
                        property,
                        list,
                        d::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma,
                    )?;
                }
                self.check_destructuring_assignment(target, rest, 0, false)?;
            }
            _ => {
                self.error_at(Some(property), d::Property_assignment_expected, vec![])?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkArrayLiteralAssignment
    fn check_array_assignment(
        &mut self,
        node: NodeId,
        source: TypeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let list = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_array_literal_expression()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .elements();
        let elements = self.source_list(node, list)?;
        let use_ = crate::iteration::ALLOW_SYNC | crate::iteration::DESTRUCTURING_FLAG;
        let out = self.check_iterated_type_or_element_type(
            use_ | crate::iteration::POSSIBLY_OUT_OF_BOUNDS,
            source,
            self.builtins.undefined_type,
            Some(node),
        )?;
        let mut in_bounds = if self.program()?.host.options().no_unchecked_indexed_access
            == ts_core::Tristate::TRUE
        {
            None
        } else {
            Some(out)
        };
        for (index, &element) in elements.iter().enumerate() {
            let spread = self.ast(element)?.node(element)?.kind() == K::SpreadElement;
            let element_type = if spread {
                if in_bounds.is_none() {
                    in_bounds = Some(self.check_iterated_type_or_element_type(
                        use_,
                        source,
                        self.builtins.undefined_type,
                        Some(node),
                    )?);
                }
                in_bounds.unwrap()
            } else {
                out
            };
            self.check_array_assignment_element(
                element,
                source,
                index,
                element_type,
                mode,
                &elements,
                list,
            )?;
        }
        Ok(source)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkArrayLiteralDestructuringElementAssignment
    #[allow(
        clippy::too_many_arguments,
        reason = "Parameters preserve the upstream operation and its independently selected checking modes"
    )]
    fn check_array_assignment_element(
        &mut self,
        element: NodeId,
        source: TypeId,
        index: usize,
        mut element_type: TypeId,
        mode: u32,
        elements: &[NodeId],
        list: Option<ts_ast::NodeListId>,
    ) -> Result<(), Error> {
        let read = self.ast(element)?.node(element)?;
        if read.kind() == K::OmittedExpression {
            return Ok(());
        }
        if read.kind() != K::SpreadElement {
            let key = self.get_number_literal_type(ts_jsnum::Number::new(index as f64))?;
            if self.is_array_like_type(source)? {
                let default = self.assignment_has_default(element)?;
                let flags = af::EXPRESSION_POSITION | if default { af::ALLOW_MISSING } else { 0 };
                let access = self.synthetic_expression_at(element, key)?;
                element_type = self
                    .indexed_access_or_undefined(source, key, flags, Some(access), None)?
                    .unwrap_or(self.builtins.error_type);
                if default {
                    element_type = self.type_with_facts(element_type, facts::NE_UNDEFINED)?;
                }
                element_type = self.flow_type_of_destructuring(element, element_type)?;
            }
            self.check_destructuring_assignment(element, element_type, mode, false)?;
        } else if index + 1 < elements.len() {
            self.error_at(
                Some(element),
                d::A_rest_element_must_be_last_in_a_destructuring_pattern,
                vec![],
            )?;
        } else {
            let target = required(read.expression(), "array assignment rest")?;
            if self.ast(target)?.node(target)?.kind() == K::BinaryExpression
                && self.assignment_has_default(target)?
            {
                let token = required(
                    self.ast(target)?
                        .node(target)?
                        .data_source()
                        .as_binary_expression()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .operator_token(),
                    "rest default operator",
                )?;
                self.error_at(
                    Some(token),
                    d::A_rest_element_cannot_have_an_initializer,
                    vec![],
                )?;
            } else {
                if let Some(list) = list {
                    self.check_grammar_trailing_comma(
                        element,
                        list,
                        d::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma,
                    )?;
                }
                let mut tuples = true;
                for part in self.distributed_types(source)? {
                    if !self.is_tuple_type(part)? {
                        tuples = false;
                        break;
                    }
                }
                let rest = if tuples {
                    self.map_type(source, &mut |c, t| c.slice_tuple(t, index, 0).map(Some))?
                        .ok_or(Error::MissingLink("assignment tuple rest"))?
                } else {
                    self.create_array_type(element_type, false)?
                };
                self.check_destructuring_assignment(target, rest, mode, false)?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkReferenceAssignment
    fn check_reference_assignment(
        &mut self,
        target: NodeId,
        source: TypeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let target_type = self.check_expression_ex(target, mode)?;
        let parent = self.ast(target)?.node(target)?.parent();
        let spread = parent
            .map(|parent| {
                self.ast(parent)?
                    .node(parent)
                    .map(|n| n.kind() == K::SpreadAssignment)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        let invalid = if spread {
            d::The_target_of_an_object_rest_assignment_must_be_a_variable_or_a_property_access
        } else {
            d::The_left_hand_side_of_an_assignment_expression_must_be_a_variable_or_a_property_access
        };
        let optional = if spread {
            d::The_target_of_an_object_rest_assignment_may_not_be_an_optional_property_access
        } else {
            d::The_left_hand_side_of_an_assignment_expression_may_not_be_an_optional_property_access
        };
        if self.check_reference_expression(target, invalid, optional)? {
            self.check_expression_related_with_elaboration(
                source,
                target_type,
                crate::RelationKind::Assignable,
                Some(target),
                Some(target),
                None,
            )?;
        }
        Ok(source)
    }
    // port: tsc/internal/checker/checker.go:Checker.createSyntheticExpression
    fn synthetic_expression_at(&mut self, location: NodeId, ty: TypeId) -> Result<NodeId, Error> {
        self.retain_flow_source(location)?;
        let read = self.ast(location)?.node(location)?;
        let range = read.range();
        let parent = read.parent();
        let node = self.new_synthetic_expression(ty, false, None)?;
        self.factory.set_node_range(node, range);
        self.factory.set_node_parent(node, parent);
        Ok(node)
    }
}
