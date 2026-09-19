//! `instanceof` shares signature resolution with calls when the right operand
//! supplies a callable `Symbol.hasInstance` member.

use crate::{type_flags as tf, CheckerState, Error, RelationKind, SignatureId, TypeId};
use ts_arena::NodeId;
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkInstanceOfExpression
    pub(crate) fn check_instanceof_expression(
        &mut self,
        left: NodeId,
        right: NodeId,
        left_type: TypeId,
        right_type: TypeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        if left_type == self.builtins.silent_never_type
            || right_type == self.builtins.silent_never_type
        {
            return Ok(self.builtins.silent_never_type);
        }
        if self.types.flags(left_type)? & tf::ANY == 0
            && self.all_assignable_to_kind(left_type, tf::PRIMITIVE)?
        {
            self.error_at(Some(left), messages::The_left_hand_side_of_an_instanceof_expression_must_be_of_type_any_an_object_type_or_a_type_parameter, vec![])?;
        }
        let node = self
            .ast(left)?
            .node(left)?
            .parent()
            .ok_or(Error::MissingLink("instanceof expression parent"))?;
        let saved_mode = std::mem::replace(&mut self.expression_mode, mode);
        let signature = self.resolved_call_signature(node);
        self.expression_mode = saved_mode;
        let signature = signature?;
        if signature == self.builtins.resolving_signature {
            return Ok(self.builtins.silent_never_type);
        }
        let returned = self.return_type_of_signature(signature)?;
        let (_, diagnostic) = self.check_type_related_ex(returned, self.builtins.boolean_type, RelationKind::Assignable, Some(right), Some(messages::An_object_s_Symbol_hasInstance_method_must_return_a_boolean_value_for_it_to_be_used_on_the_right_hand_side_of_an_instanceof_expression))?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(self.builtins.boolean_type)
    }

    // port: tsc/internal/checker/flow.go:Checker.getSymbolHasInstanceMethodOfObjectType
    pub(crate) fn symbol_has_instance_method_of_object_type(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let name = self.property_name_for_known_symbol("hasInstance")?;
        if self.all_assignable_to_kind(ty, tf::NON_PRIMITIVE)? {
            if let Some(property) = self.constituent_property(ty, name.as_bytes(), false)? {
                let method = self.get_type_of_symbol(property)?;
                if !self.signatures_of_type(method, false)?.is_empty() {
                    return Ok(Some(method));
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveInstanceofExpression
    pub(crate) fn resolve_instanceof_expression(
        &mut self,
        node: NodeId,
    ) -> Result<SignatureId, Error> {
        let right = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_binary_expression()
            .ok_or(Error::MissingLink("instanceof binary expression"))?
            .right()
            .ok_or(Error::MissingLink("instanceof right operand"))?;
        let ty = self.check_expression(right)?;
        if self.types.flags(ty)? & tf::ANY == 0 {
            if let Some(method) = self.symbol_has_instance_method_of_object_type(ty)? {
                let apparent = self.apparent_type(method)?;
                if self.is_error_type(apparent)? {
                    self.resolve_untyped_call(node)?;
                    return Ok(self.builtins.unknown_signature);
                }
                let calls = self.signatures_of_type(apparent, false)?;
                let constructors = self.signatures_of_type(apparent, true)?;
                if self.instanceof_untyped_call(
                    method,
                    apparent,
                    calls.len(),
                    constructors.len(),
                )? {
                    return self.resolve_untyped_call(node);
                }
                if !calls.is_empty() {
                    return self.resolve_typed_call(node, &calls);
                }
            } else {
                let has_signatures = !self.signatures_of_type(ty, false)?.is_empty()
                    || !self.signatures_of_type(ty, true)?.is_empty();
                let function = *self
                    .query
                    .global_types
                    .get("Function")
                    .ok_or(Error::MissingLink("global Function type"))?;
                if !has_signatures
                    && !self.is_type_related_to(ty, function, RelationKind::Subtype)?
                {
                    self.error_at(Some(right), messages::The_right_hand_side_of_an_instanceof_expression_must_be_either_of_type_any_a_class_function_or_other_type_assignable_to_the_Function_interface_type_or_an_object_type_with_a_Symbol_hasInstance_method, vec![])?;
                    self.resolve_untyped_call(node)?;
                    return Ok(self.builtins.unknown_signature);
                }
            }
        }
        Ok(self.builtins.any_signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUntypedFunctionCall
    fn instanceof_untyped_call(
        &mut self,
        ty: TypeId,
        apparent: TypeId,
        calls: usize,
        constructors: usize,
    ) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::ANY != 0
            || self.types.flags(apparent)? & tf::ANY != 0
                && self.types.flags(ty)? & tf::TYPE_PARAMETER != 0
        {
            return Ok(true);
        }
        if calls != 0 || constructors != 0 || self.types.flags(apparent)? & tf::UNION != 0 {
            return Ok(false);
        }
        let reduced = self.get_reduced_type(apparent)?;
        if self.types.flags(reduced)? & tf::NEVER != 0 {
            return Ok(false);
        }
        let function = *self
            .query
            .global_types
            .get("Function")
            .ok_or(Error::MissingLink("global Function type"))?;
        self.is_type_related_to(ty, function, RelationKind::Assignable)
    }
}
