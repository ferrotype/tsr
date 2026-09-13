//! Parameter checks keep the variable-like semantic checks before the
//! parameter-specific property, `this`, and rest diagnostics.
use crate::{CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:Checker.isOptionalParameter
    pub(crate) fn is_optional_parameter(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::Parameter {
            return Ok(false);
        }
        if read.question_token(self.ast(node)?)?.is_some() {
            return Ok(true);
        }
        let initializer = read.initializer().is_some();
        let annotation = read.type_node().is_some();
        let rest = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingLink("optional parameter"))?
            .dot_dot_dot_token()
            .is_some();
        let function = read
            .parent()
            .ok_or(Error::MissingLink("optional parameter parent"))?;
        if initializer {
            let signature = self.signature_from_declaration(function)?;
            let parameters = self.source_list(
                function,
                self.ast(function)?.node(function)?.parameter_list(),
            )?;
            let index = parameters
                .iter()
                .position(|&parameter| parameter == node)
                .ok_or(Error::MissingLink("optional parameter position"))?;
            return Ok(index >= self.min_argument_count_ex(signature, 1 | 2)?);
        }
        if let Some(call) =
            ts_ast::get_immediately_invoked_function_expression(self.ast(function)?, function)?
        {
            let parameters = self.source_list(
                function,
                self.ast(function)?.node(function)?.parameter_list(),
            )?;
            let index = parameters
                .iter()
                .position(|&parameter| parameter == node)
                .ok_or(Error::MissingLink("IIFE optional parameter position"))?;
            return Ok(!annotation && !rest && index >= self.effective_call_arguments(call)?.len());
        }
        Ok(false)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.requiresAddingImplicitUndefinedWorker
    pub(crate) fn parameter_requires_implicit_undefined(
        &mut self,
        parameter: NodeId,
        enclosing: Option<NodeId>,
    ) -> Result<bool, Error> {
        if !self.options.strict_null_checks
            || self.ast(parameter)?.node(parameter)?.kind() != K::Parameter
        {
            return Ok(false);
        }
        let read = self.ast(parameter)?.node(parameter)?;
        let initializer = read.initializer();
        let annotation = read.type_node();
        let property =
            read.modifier_flags(self.ast(parameter)?)? & mf::PARAMETER_PROPERTY_MODIFIER != 0;
        let optional = self.is_optional_source_parameter(parameter)?;
        let needed = if optional {
            initializer.is_none() && property
        } else if initializer.is_some() {
            !property
                || enclosing
                    .map(|node| {
                        self.ast(node)?
                            .node(node)
                            .map(|read| ts_ast::utilities::is_function_like(Some(&read)))
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false)
        } else {
            false
        };
        if !needed {
            return Ok(false);
        }
        if let Some(annotation) = annotation {
            let ty = self.get_type_from_type_node(annotation)?;
            if self.is_error_type(ty)? {
                return Ok(false);
            }
            if self.class_type_contains_undefined(ty)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn is_optional_source_parameter(
        &mut self,
        parameter: NodeId,
    ) -> Result<bool, Error> {
        self.is_optional_parameter(parameter)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkParameter
    pub(crate) fn check_parameter(&mut self, parameter: NodeId) -> Result<(), Error> {
        self.check_grammar_modifiers(parameter)?;
        self.check_binding_variable(parameter)?;
        let read = self.ast(parameter)?.node(parameter)?;
        let function = self
            .containing_body_function(parameter)?
            .ok_or(Error::MissingLink("parameter containing function"))?;
        let function_read = self.ast(function)?.node(function)?;
        let kind = function_read.kind();
        let body = function_read.body();
        let parameters = self.source_list(function, function_read.parameter_list())?;
        let name = read.name().ok_or(Error::MissingLink("parameter name"))?;
        let identifier = self.ast(name)?.node(name)?.kind() == K::Identifier;
        let name_text = if identifier {
            self.ast(name)?.node_text(name)?.into_js_string()
        } else {
            JsString::from_bytes(b"".as_slice())
        };
        let property =
            read.modifier_flags(self.ast(parameter)?)? & mf::PARAMETER_PROPERTY_MODIFIER != 0;
        let initializer = read.initializer();
        let optional = read.question_token(self.ast(parameter)?)?.is_some();
        let rest = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingLink("parameter declaration"))?
            .dot_dot_dot_token()
            .is_some();
        if property {
            if self
                .program()?
                .host
                .options()
                .erasable_syntax_only
                .is_true()
                && self.ast(parameter)?.node(parameter)?.flags() & nf::JAVA_SCRIPT_FILE == 0
            {
                self.error_at(
                    Some(parameter),
                    d::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                    vec![],
                )?;
            }
            let present = body
                .map(|body| {
                    self.ast(body)?
                        .node(body)
                        .map(|read| read.pos() != read.end())
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
            if kind != K::Constructor || !present {
                self.error_at(
                    Some(parameter),
                    d::A_parameter_property_is_only_allowed_in_a_constructor_implementation,
                    vec![],
                )?;
            }
            if kind == K::Constructor && name_text.as_bytes() == b"constructor" {
                self.error_at(
                    Some(name),
                    d::X_constructor_cannot_be_used_as_a_parameter_property_name,
                    vec![],
                )?;
            }
        }
        if initializer.is_none() && optional && !identifier && body.is_some() {
            self.error_at(
                Some(parameter),
                d::A_binding_pattern_parameter_cannot_be_optional_in_an_implementation_signature,
                vec![],
            )?;
        }
        if matches!(name_text.as_bytes(), b"this" | b"new") {
            if parameters.first() != Some(&parameter) {
                self.error_at(
                    Some(parameter),
                    d::A_0_parameter_must_be_the_first_parameter,
                    vec![name_text],
                )?;
            }
            if matches!(
                kind.known(),
                Some(K::Constructor | K::ConstructSignature | K::ConstructorType)
            ) {
                self.error_at(
                    Some(parameter),
                    d::A_constructor_cannot_have_a_this_parameter,
                    vec![],
                )?;
            }
            if kind == K::ArrowFunction {
                self.error_at(
                    Some(parameter),
                    d::An_arrow_function_cannot_have_a_this_parameter,
                    vec![],
                )?;
            }
            if matches!(kind.known(), Some(K::GetAccessor | K::SetAccessor)) {
                self.error_at(
                    Some(parameter),
                    d::X_get_and_set_accessors_cannot_declare_this_parameters,
                    vec![],
                )?;
            }
        }
        if rest && identifier {
            let symbol = self
                .get_symbol_of_declaration(parameter)?
                .ok_or(Error::MissingLink("rest parameter symbol"))?;
            let ty = self.get_type_of_symbol(symbol)?;
            let ty = self.get_reduced_type(ty)?;
            let array = *self
                .query
                .global_types
                .get("anyReadonlyArrayType")
                .ok_or(Error::MissingLink("any readonly array type"))?;
            if !self.is_type_related_to(ty, array, RelationKind::Assignable)? {
                self.error_at(
                    Some(parameter),
                    d::A_rest_parameter_must_be_of_an_array_type,
                    vec![],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextuallyTypedParameterType
    pub(crate) fn contextually_typed_parameter_type(
        &mut self,
        parameter: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(parameter)?.node(parameter)?;
        let function = read
            .parent()
            .ok_or(Error::MissingLink("contextual parameter function"))?;
        let function_read = self.ast(function)?.node(function)?;
        let function_kind = function_read.kind();
        let is_method = function_kind == K::MethodDeclaration
            && function_read
                .parent()
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|read| read.kind() == K::ObjectLiteralExpression)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
        if !matches!(
            function_kind.known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) && !is_method
            || !self.expression_is_context_sensitive(function)?
        {
            return Ok(None);
        }
        let parameters = self.source_list(function, function_read.parameter_list())?;
        let index = parameters
            .iter()
            .position(|&node| node == parameter)
            .ok_or(Error::MissingLink("contextual parameter index"))?;
        let rest = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingLink("contextual parameter"))?
            .dot_dot_dot_token()
            .is_some();
        if let Some(call) =
            ts_ast::get_immediately_invoked_function_expression(self.ast(function)?, function)?
        {
            if rest {
                let args = self.effective_call_arguments(call)?;
                return self
                    .spread_argument_type(&args, index, args.len(), self.builtins.any_type, None, 0)
                    .map(Some);
            }
            return self.iife_parameter_argument_type(call, index, read.initializer().is_some());
        }
        let Some(signature) = self.contextual_body_signature(function)? else {
            return Ok(None);
        };
        let first_this = match parameters.first().copied() {
            Some(first) => match self.ast(first)?.node(first)?.name() {
                Some(name) => {
                    self.ast(name)?.node(name)?.kind() == K::Identifier
                        && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
                }
                None => false,
            },
            None => false,
        };
        let Some(index) = index.checked_sub(usize::from(first_this)) else {
            return Ok(None);
        };
        if rest && parameters.last() == Some(&parameter) {
            return self.rest_type_at(signature, index).map(Some);
        }
        self.parameter_type_at(signature, index)
    }
}
