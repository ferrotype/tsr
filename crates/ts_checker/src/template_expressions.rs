//! Runtime template expressions reuse the pinned constant evaluator and type
//! template interner; contextual templates retain substitution identities.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkTemplateExpression
    pub(crate) fn check_template_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_template_expression()
            .ok_or(Error::MissingLink("template expression"))?;
        let head = data.head().ok_or(Error::MissingLink("template head"))?;
        let spans = self.source_list(node, data.template_spans())?;
        let mut texts = Vec::with_capacity(spans.len() + 1);
        let mut types = Vec::with_capacity(spans.len());
        texts.push(self.ast(head)?.node_text(head)?.into_js_string());
        for span in spans {
            let read = self.ast(span)?.node(span)?;
            let data = read
                .data_source()
                .as_template_span()
                .ok_or(Error::MissingLink("template span"))?;
            let expression = data
                .expression()
                .ok_or(Error::MissingLink("template substitution"))?;
            let literal = data
                .literal()
                .ok_or(Error::MissingLink("template literal tail"))?;
            let ty = self.check_expression(expression)?;
            if self.maybe_type_with_constraint(ty, tf::ES_SYMBOL_LIKE)? {
                self.error_at(Some(expression),ts_diagnostics::Implicit_conversion_of_a_symbol_to_a_string_will_fail_at_runtime_Consider_wrapping_this_expression_in_String,vec![])?;
            }
            texts.push(self.ast(literal)?.node_text(literal)?.into_js_string());
            types.push(
                if self.is_type_related_to(
                    ty,
                    self.builtins.template_constraint_type,
                    RelationKind::Assignable,
                )? {
                    ty
                } else {
                    self.builtins.string_type
                },
            );
        }
        let parent = self.ast(node)?.node(node)?.parent();
        let tagged = parent
            .map(|parent| {
                self.ast(parent)?
                    .node(parent)
                    .map(|read| read.kind() == K::TaggedTemplateExpression)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        if !tagged {
            if let Some(crate::enums::EnumValue::String(value)) =
                self.evaluate_enum_expression(node, Some(node))?.value
            {
                let ty = self.get_string_literal_type(value)?;
                return self.get_fresh_type_of_literal_type(ty);
            }
        }
        let mut in_type_context = false;
        let mut location = node;
        while let Some(parent) = self.ast(location)?.node(location)?.parent() {
            let read = self.ast(parent)?.node(parent)?;
            if read.kind() == K::ParenthesizedExpression {
                location = parent;
                continue;
            }
            in_type_context = read.kind() == K::ElementAccessExpression
                && read
                    .data_source()
                    .as_element_access_expression()
                    .ok_or(Error::MissingLink("template access"))?
                    .argument_expression()
                    == Some(location);
            break;
        }
        if self.is_const_context(node)? || in_type_context {
            return self.get_template_literal_type(&texts, &types);
        }
        if let Some(context) = self.contextual_expression_type(node)? {
            if self.any_type(context, &mut |c, t| {
                let flags = c.types.flags(t)?;
                if flags & (tf::STRING_LITERAL | tf::TEMPLATE_LITERAL) != 0 {
                    return Ok(true);
                }
                if flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
                    let constraint = c
                        .base_constraint_of_type(t)?
                        .unwrap_or(c.builtins.unknown_type);
                    return c.maybe_type_of_kind(constraint, tf::STRING_LIKE);
                }
                Ok(false)
            })? {
                return self.get_template_literal_type(&texts, &types);
            }
        }
        Ok(self.builtins.string_type)
    }
}
