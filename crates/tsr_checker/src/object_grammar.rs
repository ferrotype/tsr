//! Object-member grammar preserves native early returns separately from semantic
//! member traversal. Parsed syntax diagnostics suppress grammar diagnostics.
use crate::{types::Map, CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarComputedPropertyName
    fn check_object_computed_grammar(&mut self, node: NodeId) -> Result<(), Error> {
        if self.node(node)?.kind() != K::ComputedPropertyName {
            return Ok(());
        }
        let expression = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("computed expression"))?;
        if self.node(expression)?.kind() == K::BinaryExpression {
            let operator = self
                .ast(expression)?
                .node(expression)?
                .data_source()
                .as_binary_expression()
                .ok_or(Error::MissingLink("computed binary"))?
                .operator_token()
                .ok_or(Error::MissingLink("computed operator"))?;
            if self.node(operator)?.kind() == K::CommaToken {
                self.grammar_error_node(
                    expression,
                    d::A_comma_expression_is_not_allowed_in_a_computed_property_name,
                    vec![],
                )?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarObjectLiteralExpression
    pub(crate) fn check_object_literal_grammar(
        &mut self,
        node: NodeId,
        destructuring: bool,
    ) -> Result<(), Error> {
        let mut seen = Map::default();
        for property in self.source_list(node, self.node(node)?.property_list())? {
            let read = self.node(property)?;
            let kind = read.kind();
            if kind == K::SpreadAssignment {
                if destructuring {
                    let expression = read
                        .expression()
                        .ok_or(Error::MissingLink("spread expression"))?;
                    let mut inner = expression;
                    while self.node(inner)?.kind() == K::ParenthesizedExpression {
                        inner = self
                            .ast(inner)?
                            .node(inner)?
                            .expression()
                            .ok_or(Error::MissingLink("spread parentheses"))?;
                    }
                    if matches!(
                        self.node(inner)?.kind().known(),
                        Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression)
                    ) {
                        self.grammar_error_node(
                            expression,
                            d::A_rest_element_cannot_contain_a_binding_pattern,
                            vec![],
                        )?;
                        return Ok(());
                    }
                }
                continue;
            }
            let name = read
                .name()
                .ok_or(Error::MissingLink("object grammar name"))?;
            self.check_object_computed_grammar(name)?;
            if kind == K::ShorthandPropertyAssignment && !destructuring {
                let initializer = self
                    .ast(property)?
                    .node(property)?
                    .data_source()
                    .as_shorthand_property_assignment()
                    .ok_or(Error::MissingLink("shorthand grammar"))?
                    .object_assignment_initializer();
                if let Some(initializer) = initializer {
                    let mut last = name;
                    for child in self.source_children(property)? {
                        if child == initializer {
                            break;
                        }
                        last = child;
                    }
                    self.grammar_error_first_token(last,d::Did_you_mean_to_use_a_Colon_An_can_only_follow_a_property_name_when_the_containing_object_literal_is_part_of_a_destructuring_pattern,vec![])?;
                }
            }
            if self.node(name)?.kind() == K::PrivateIdentifier {
                self.grammar_error_node(
                    name,
                    d::Private_identifiers_are_not_allowed_outside_class_bodies,
                    vec![],
                )?;
            }
            for modifier in self.source_list(property, self.node(property)?.modifiers())? {
                let kind_mod = self.node(modifier)?.kind();
                if kind_mod != K::Decorator
                    && (kind_mod != K::AsyncKeyword || kind != K::MethodDeclaration)
                {
                    let text = ts_scanner::get_text_of_node(self.ast(modifier)?, modifier)?;
                    self.grammar_error_node(
                        modifier,
                        d::X_0_modifier_cannot_be_used_here,
                        vec![text],
                    )?;
                }
            }
            let meaning = match kind.known() {
                Some(K::PropertyAssignment | K::ShorthandPropertyAssignment) => {
                    let read = self.node(property)?;
                    let optional = read.question_token(self.ast(property)?)?;
                    if let Some(token) = read.postfix_token().filter(|&token| {
                        self.ast(token)
                            .and_then(|view| {
                                view.node(token)
                                    .map(|read| read.kind() == K::ExclamationToken)
                                    .map_err(Error::from)
                            })
                            .unwrap_or(false)
                    }) {
                        self.grammar_error_node(
                            token,
                            d::A_definite_assignment_assertion_is_not_permitted_in_this_context,
                            vec![],
                        )?;
                    }
                    if let Some(token) = optional {
                        self.grammar_error_node(
                            token,
                            d::An_object_member_cannot_be_declared_optional,
                            vec![],
                        )?;
                    }
                    if self.node(name)?.kind() == K::NumericLiteral {
                        self.check_grammar_numeric_literal(name)?;
                    }
                    if self.node(name)?.kind() == K::BigIntLiteral {
                        self.error_at(
                            Some(name),
                            d::A_bigint_literal_cannot_be_used_as_a_property_name,
                            vec![],
                        )?;
                    }
                    1u8
                }
                Some(K::MethodDeclaration) => 2,
                Some(K::GetAccessor) => 4,
                Some(K::SetAccessor) => 8,
                _ => {
                    return Err(Error::Unsupported(
                        "checkGrammarObjectLiteralExpression: invalid member",
                    ))
                }
            };
            if destructuring {
                continue;
            }
            let effective = if self.node(name)?.kind() == K::ComputedPropertyName {
                let expression = self
                    .ast(name)?
                    .node(name)?
                    .expression()
                    .ok_or(Error::MissingLink("computed name"))?;
                let ty = self.get_type_of_expression(expression)?;
                self.index_property_name(ty)?
            } else if ts_ast::utilities::is_property_name_literal(&self.node(name)?) {
                Some(self.node_text(name)?.into_js_string())
            } else {
                None
            };
            let Some(effective) = effective else {
                continue;
            };
            let previous = seen.get(&effective).copied().unwrap_or(0);
            if previous == 0 {
                seen.insert(effective, meaning);
                continue;
            }
            if meaning & 2 != 0 && previous & 2 != 0 {
                let text = ts_scanner::get_text_of_node(self.ast(name)?, name)?;
                self.grammar_error_node(name, d::Duplicate_identifier_0, vec![text])?;
            } else if meaning & 1 != 0 && previous & 1 != 0 {
                let text = ts_scanner::get_text_of_node(self.ast(name)?, name)?;
                self.grammar_error_node(
                    name,
                    d::An_object_literal_cannot_have_multiple_properties_with_the_same_name,
                    vec![text],
                )?;
            } else if meaning & 0b1100 != 0 && previous & 0b1100 != 0 {
                if previous != 12 && previous != meaning {
                    seen.insert(effective, meaning | previous);
                } else {
                    self.grammar_error_node(name,d::An_object_literal_cannot_have_multiple_get_Slashset_accessors_with_the_same_name,vec![])?;
                    return Ok(());
                }
            } else {
                self.grammar_error_node(
                    name,
                    d::An_object_literal_cannot_have_property_and_accessor_with_the_same_name,
                    vec![],
                )?;
                return Ok(());
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarMethod
    pub(crate) fn check_object_method_grammar(&mut self, node: NodeId) -> Result<(), Error> {
        if self.check_grammar_function_like(node)? {
            return Ok(());
        }
        let modifiers = self.source_list(node, self.node(node)?.modifiers())?;
        if !(modifiers.is_empty()
            || modifiers.len() == 1
                && self.ast(modifiers[0])?.node(modifiers[0])?.kind() == K::AsyncKeyword)
        {
            self.grammar_error_first_token(node, d::Modifiers_cannot_appear_here, vec![])?;
            return Ok(());
        }
        let read = self.node(node)?;
        if let Some(token) = read.question_token(self.ast(node)?)? {
            self.grammar_error_node(
                token,
                d::An_object_member_cannot_be_declared_optional,
                vec![],
            )?;
            return Ok(());
        }
        if let Some(token) = read.postfix_token().filter(|&token| {
            self.ast(token)
                .and_then(|view| {
                    view.node(token)
                        .map(|read| read.kind() == K::ExclamationToken)
                        .map_err(Error::from)
                })
                .unwrap_or(false)
        }) {
            self.grammar_error_node(
                token,
                d::A_definite_assignment_assertion_is_not_permitted_in_this_context,
                vec![],
            )?;
            return Ok(());
        }
        if read.body().is_none() {
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("method source"))?;
            if self
                .ast(source)?
                .source_file(source)?
                .diagnostics()
                .is_empty()
            {
                let end = self.node(node)?.end();
                self.add_diagnostic(ts_ast::Diagnostic::new(
                    Some(source),
                    ts_core::TextRange::new(i64::from(end) - 1, i64::from(end)),
                    d::X_0_expected,
                    vec![JsString::from_bytes(b"{".as_slice())],
                ))?;
            }
            return Ok(());
        }
        self.check_grammar_generator(node)?;
        Ok(())
    }
}
