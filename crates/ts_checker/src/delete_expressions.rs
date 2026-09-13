//! `delete` expressions (`checkDeleteExpression` and
//! `checkDeleteExpressionMustBeOptional` in `tsc/internal/checker/checker.go`).

use crate::{type_facts, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkDeleteExpression
    pub(crate) fn check_delete_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let operand = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("delete operand"))?;
        self.check_expression(operand)?;
        let expression = ts_ast::skip_parentheses(self.ast(operand)?, operand)?;
        let read = self.ast(expression)?.node(expression)?;
        if !ts_ast::utilities::is_access_expression(&read) {
            self.error_at(
                Some(expression),
                d::The_operand_of_a_delete_operator_must_be_a_property_reference,
                vec![],
            )?;
            return Ok(self.builtins.boolean_type);
        }
        if read.kind() == K::PropertyAccessExpression {
            let name = read
                .data_source()
                .as_property_access_expression()
                .and_then(|access| access.name())
                .ok_or(Error::MissingLink("property access name"))?;
            if ts_ast::is_private_identifier(&self.ast(name)?.node(name)?) {
                self.error_at(
                    Some(expression),
                    d::The_operand_of_a_delete_operator_cannot_be_a_private_identifier,
                    vec![],
                )?;
            }
        }
        // `getResolvedSymbolOrNil`: the symbol the access check resolved, if any.
        let resolved = self
            .query
            .resolved_symbols
            .try_get(expression)
            .copied()
            .flatten();
        if let Some(symbol) = resolved {
            let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol)?;
            if self.is_readonly_symbol(symbol)? {
                self.error_at(
                    Some(expression),
                    d::The_operand_of_a_delete_operator_cannot_be_a_read_only_property,
                    vec![],
                )?;
            } else {
                self.check_delete_expression_must_be_optional(expression, symbol)?;
            }
        }
        Ok(self.builtins.boolean_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkDeleteExpressionMustBeOptional
    fn check_delete_expression_must_be_optional(
        &mut self,
        expression: NodeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        let ty = self.get_type_of_symbol(symbol)?;
        if self.options.strict_null_checks
            && self.types.flags(ty)? & (tf::ANY_OR_UNKNOWN | tf::NEVER) == 0
        {
            let optional = if self.options.exact_optional_property_types {
                self.symbol(symbol)?.flags() & sf::OPTIONAL != 0
            } else {
                self.type_facts(ty, type_facts::IS_UNDEFINED)? & type_facts::IS_UNDEFINED != 0
            };
            if !optional {
                self.error_at(
                    Some(expression),
                    d::The_operand_of_a_delete_operator_must_be_optional,
                    vec![],
                )?;
            }
        }
        Ok(())
    }
}
