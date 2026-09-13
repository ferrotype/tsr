//! Contextual `this` for object methods follows enclosing object literals and
//! the native inference mapper; assignment functions derive it from the target.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:getContainingObjectLiteral
    pub(crate) fn containing_object_literal(
        &self,
        function: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let read = self.ast(function)?.node(function)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let parent_read = self.ast(parent)?.node(parent)?;
        match read.kind().known() {
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
                if parent_read.kind() == K::ObjectLiteralExpression =>
            {
                Ok(Some(parent))
            }
            Some(K::FunctionExpression) if parent_read.kind() == K::PropertyAssignment => {
                Ok(parent_read.parent())
            }
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getThisTypeArgument
    fn contextual_this_type_argument(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        let record = self.types.get(ty)?;
        if record.object_flags & of::REFERENCE != 0
            && Some(self.types.target(ty)?) == self.query.global_types.get("ThisType").copied()
        {
            return self
                .get_type_arguments(ty)?
                .first()
                .copied()
                .ok_or(Error::MissingLink("ThisType type argument"))
                .map(Some);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getThisTypeFromContextualType
    fn this_type_from_contextual_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        self.map_type(ty, &mut |checker, part| {
            let candidates = if checker.types.flags(part)? & tf::INTERSECTION != 0 {
                checker.types.compound_types(part)?.clone()
            } else {
                vec![part].into()
            };
            for &candidate in candidates.iter() {
                if let Some(this) = checker.contextual_this_type_argument(candidate)? {
                    return Ok(Some(this));
                }
            }
            Ok(None)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getThisTypeOfObjectLiteralFromContextualType
    pub(crate) fn this_type_of_object_context(
        &mut self,
        mut literal: NodeId,
        mut context: Option<TypeId>,
    ) -> Result<Option<TypeId>, Error> {
        while let Some(ty) = context {
            if let Some(this) = self.this_type_from_contextual_type(ty)? {
                return Ok(Some(this));
            }
            let parent = self
                .ast(literal)?
                .node(literal)?
                .parent()
                .ok_or(Error::MissingLink("object context parent"))?;
            if self.ast(parent)?.node(parent)?.kind() != K::PropertyAssignment {
                break;
            }
            literal = self
                .ast(parent)?
                .node(parent)?
                .parent()
                .ok_or(Error::MissingLink("enclosing object literal"))?;
            context = self.apparent_contextual_expression_type_ex(literal, 0)?;
        }
        Ok(None)
    }

    // The latter half of getContextualThisParameterType, after the contextual
    // signature's explicit this parameter has been considered.
    // port: tsc/internal/checker/checker.go:Checker.getContextualThisParameterType
    pub(crate) fn contextual_this_from_object_or_assignment(
        &mut self,
        function: NodeId,
        javascript: bool,
    ) -> Result<Option<TypeId>, Error> {
        if let Some(literal) = self.containing_object_literal(function)? {
            let context = self.apparent_contextual_expression_type_ex(literal, 0)?;
            if let Some(this) = self.this_type_of_object_context(literal, context)? {
                let mapper = self
                    .call_inference_at_node(literal)?
                    .map(|inference| {
                        self.inference_context(inference)
                            .map(|context| context.mapper)
                    })
                    .transpose()?;
                return self.instantiate_type(this, mapper).map(Some);
            }
            let this = match context {
                Some(context) => self.non_nullable_type(context)?,
                None => self.check_expression_cached(literal)?,
            };
            return self.widened_type(this).map(Some);
        }
        let Some(mut parent) = self.ast(function)?.node(function)?.parent() else {
            return Ok(None);
        };
        while self.ast(parent)?.node(parent)?.kind() == K::ParenthesizedExpression {
            parent = self
                .ast(parent)?
                .node(parent)?
                .parent()
                .ok_or(Error::MissingLink("parenthesized function parent"))?;
        }
        if ts_ast::is_assignment_expression(self.ast(parent)?, parent, false)? {
            let parent_read = self.ast(parent)?.node(parent)?;
            let target = parent_read
                .data_source()
                .as_binary_expression()
                .and_then(|data| data.left())
                .ok_or(Error::MissingLink("function assignment target"))?;
            if matches!(
                self.ast(target)?.node(target)?.kind().known(),
                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
            ) {
                let expression = self
                    .ast(target)?
                    .node(target)?
                    .expression()
                    .ok_or(Error::MissingLink("function assignment receiver"))?;
                if javascript && self.ast(expression)?.node(expression)?.kind() == K::Identifier {
                    let source = ts_ast::utilities::get_source_file_of_node(
                        self.ast(parent)?,
                        Some(parent),
                    )?
                    .ok_or(Error::MissingLink("function assignment source"))?;
                    if self
                        .ast(source)?
                        .source_file(source)?
                        .common_js_module_indicator()
                        .is_some()
                    {
                        let symbol = self.resolved_value_symbol(expression)?;
                        if self.symbol(symbol)?.flags() & sf::MODULE_EXPORTS != 0 {
                            return Ok(None);
                        }
                    }
                }
                let ty = self.check_expression_cached(expression)?;
                return self.widened_type(ty).map(Some);
            }
        }
        Ok(None)
    }
}
