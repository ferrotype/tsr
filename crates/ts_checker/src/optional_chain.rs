//! Optional-chain markers stay distinct from user-visible `undefined` until the
//! outermost chain completes. Property, element, call and assertion paths share
//! the same marker operations.

use crate::{CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::node_flags as nf;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getOptionalExpressionType
    pub(crate) fn optional_expression_type(
        &mut self,
        ty: TypeId,
        expression: NodeId,
    ) -> Result<TypeId, Error> {
        if ts_ast::utilities::is_expression_of_optional_chain_root(
            self.ast(expression)?,
            expression,
        )? {
            self.non_nullable_type(ty)
        } else if self.ast(expression)?.node(expression)?.flags() & nf::OPTIONAL_CHAIN != 0 {
            self.remove_optional_type_marker(ty)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.removeOptionalTypeMarker
    pub(crate) fn remove_optional_type_marker(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.options.strict_null_checks {
            let marker = self.builtins.optional_type;
            self.filter_type(ty, &mut |_, part| Ok(part != marker))
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.addOptionalTypeMarker
    pub(crate) fn add_optional_type_marker(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.options.strict_null_checks {
            self.get_union_type(&[ty, self.builtins.optional_type])
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.propagateOptionalTypeMarker
    pub(crate) fn propagate_optional_type_marker(
        &mut self,
        ty: TypeId,
        node: NodeId,
        was_optional: bool,
    ) -> Result<TypeId, Error> {
        if !was_optional {
            return Ok(ty);
        }
        if ts_ast::utilities::is_outermost_optional_chain(self.ast(node)?, node)? {
            self.add_type_optionality(ty, false, true)
        } else {
            self.add_optional_type_marker(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNonNullAssertion
    // port: tsc/internal/checker/checker.go:Checker.checkNonNullChain
    pub(crate) fn check_non_null_assertion(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let chain = read.flags() & nf::OPTIONAL_CHAIN != 0;
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("non-null assertion expression"))?;
        let ty = self.check_expression(expression)?;
        if chain {
            let non_optional = self.optional_expression_type(ty, expression)?;
            let result = self.non_nullable_type(non_optional)?;
            self.propagate_optional_type_marker(result, node, non_optional != ty)
        } else {
            self.non_nullable_type(ty)
        }
    }
}
