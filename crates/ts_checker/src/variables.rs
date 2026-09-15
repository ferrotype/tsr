//! Raw declaration inference precedes the separate widening and diagnostic
//! step. Binding parents must consume the raw type, including its optionality.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    pub(crate) fn type_of_variable_like(&mut self, declaration: NodeId) -> Result<TypeId, Error> {
        let ty = self.type_for_variable_like_raw(declaration, true, 0)?;
        let report = !self.parameter_of_context_sensitive_signature(declaration)?;
        self.widen_type_for_variable_like(declaration, ty, report)
    }

    // port: tsc/internal/checker/checker.go:Checker.isParameterOfContextSensitiveSignature
    pub(crate) fn parameter_of_context_sensitive_signature(
        &self,
        mut declaration: NodeId,
    ) -> Result<bool, Error> {
        while self.ast(declaration)?.node(declaration)?.kind() == K::BindingElement {
            let pattern = self
                .ast(declaration)?
                .node(declaration)?
                .parent()
                .ok_or(Error::MissingLink("binding pattern"))?;
            declaration = self
                .ast(pattern)?
                .node(pattern)?
                .parent()
                .ok_or(Error::MissingLink("binding declaration"))?;
        }
        let read = self.ast(declaration)?.node(declaration)?;
        if read.kind() != K::Parameter {
            return Ok(false);
        }
        let function = read
            .parent()
            .ok_or(Error::MissingLink("parameter function"))?;
        let read = self.ast(function)?.node(function)?;
        let method = read.kind() == K::MethodDeclaration
            && read
                .parent()
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|read| read.kind() == K::ObjectLiteralExpression)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
        Ok((matches!(
            read.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) || method)
            && self.expression_is_context_sensitive(function)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeForVariableLikeDeclaration
    pub(crate) fn type_for_variable_like_raw(
        &mut self,
        declaration: NodeId,
        include_optionality: bool,
        mode: u32,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(declaration)?.node(declaration)?;
        let kind = read.kind();
        if kind == K::BindingElement {
            return self.type_for_binding_element(declaration);
        }
        if kind == K::PropertyDeclaration {
            return self.type_for_class_property_raw(declaration, include_optionality, mode);
        }
        if !matches!(
            kind.known(),
            Some(K::VariableDeclaration | K::PropertySignature | K::Parameter)
        ) {
            return Err(Error::Unsupported(
                "getTypeForVariableLikeDeclaration: declaration family",
            ));
        }
        let parent = read.parent().ok_or(Error::MissingLink("variable parent"))?;
        if kind == K::VariableDeclaration {
            if let Some(grandparent) = self.ast(parent)?.node(parent)?.parent() {
                let read = self.ast(grandparent)?.node(grandparent)?;
                if read.kind() == K::ForInStatement {
                    let expression = read
                        .expression()
                        .ok_or(Error::MissingLink("for-in expression"))?;
                    let ty = self.check_expression_ex(expression, mode)?;
                    let ty = self.non_nullable_type_if_needed(ty)?;
                    let index = self.get_index_type(ty, 0)?;
                    return if self.types.flags(index)? & (tf::TYPE_PARAMETER | tf::INDEX) != 0 {
                        self.extract_string_type(index).map(Some)
                    } else {
                        Ok(Some(self.builtins.string_type))
                    };
                }
                if read.kind() == K::ForOfStatement {
                    return self.check_right_hand_side_of_for_of(grandparent).map(Some);
                }
            }
        }
        let read = self.ast(declaration)?.node(declaration)?;
        let optional =
            include_optionality && read.question_token(self.ast(declaration)?)?.is_some();
        let annotation = read.type_node();
        let initializer = read.initializer();
        let name = read.name().ok_or(Error::MissingLink("declaration name"))?;
        let declared = annotation
            .map(|node| self.get_type_from_type_node(node))
            .transpose()?;
        if kind == K::VariableDeclaration
            && self.ast(parent)?.node(parent)?.kind() == K::CatchClause
        {
            if let Some(ty) = declared {
                return Ok(Some(if self.types.flags(ty)? & tf::ANY_OR_UNKNOWN != 0 {
                    ty
                } else {
                    self.builtins.error_type
                }));
            }
            let options = self.program()?.host.options();
            return Ok(Some(
                if options.strict_option_value(options.use_unknown_in_catch_variables) {
                    self.builtins.unknown_type
                } else {
                    self.builtins.any_type
                },
            ));
        }
        if let Some(ty) = declared {
            return self
                .add_type_optionality(ty, kind == K::PropertySignature, optional)
                .map(Some);
        }
        let binding = matches!(
            self.ast(name)?.node(name)?.kind().known(),
            Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
        );
        let no_implicit = self
            .program()?
            .host
            .options()
            .strict_option_value(self.program()?.host.options().no_implicit_any);
        if no_implicit
            && kind == K::VariableDeclaration
            && !binding
            && ts_ast::utilities::get_combined_modifier_flags(self.ast(declaration)?, declaration)?
                & mf::EXPORT
                == 0
            && self.ast(declaration)?.node(declaration)?.flags() & nf::AMBIENT == 0
        {
            let constant =
                ts_ast::utilities::get_combined_node_flags(self.ast(declaration)?, declaration)?
                    & nf::CONSTANT
                    != 0;
            if !constant
                && match initializer {
                    Some(node) => self.null_or_undefined_expression(node)?,
                    None => true,
                }
            {
                return Ok(Some(self.builtins.auto_type));
            }
            if let Some(node) = initializer {
                let read = self.ast(node)?.node(node)?;
                if read.kind() == K::ArrayLiteralExpression
                    && self.source_list(node, read.element_list())?.is_empty()
                {
                    return self.auto_array_type().map(Some);
                }
            }
        }
        if kind == K::Parameter {
            if self.ast(parent)?.node(parent)?.kind() == K::SetAccessor {
                if let Some(symbol) = self.get_symbol_of_declaration(parent)? {
                    if let Some(getter) = self.declaration_of_kind(symbol, K::GetAccessor)? {
                        let signature = self.signature_from_declaration(getter)?;
                        if self.set_accessor_value_parameter(parent)? != Some(declaration) {
                            let this = self
                                .signatures
                                .get(signature)?
                                .this_parameter
                                .ok_or(Error::MissingLink("getter this parameter"))?;
                            return self.get_type_of_symbol(this).map(Some);
                        }
                        return self.return_type_of_signature(signature).map(Some);
                    }
                }
            }
            if let Some(ty) = self.parameter_type_of_full_signature(parent, declaration)? {
                return Ok(Some(ty));
            }
            let contextual = if self.ast(name)?.node(name)?.kind() == K::Identifier
                && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
            {
                self.contextual_this_parameter_type(parent)?
            } else {
                self.contextually_typed_parameter_type(declaration)?
            };
            if let Some(ty) = contextual {
                return self.add_type_optionality(ty, false, optional).map(Some);
            }
        }
        if initializer.is_some() {
            let ty = self.check_declaration_initializer(declaration, mode, None)?;
            let ty = self.widen_type_inferred_from_initializer(declaration, ty)?;
            return self
                .add_type_optionality(ty, kind == K::PropertySignature, optional)
                .map(Some);
        }
        if binding {
            return self.type_from_binding_pattern(name, false, true).map(Some);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkDeclarationInitializer
    pub(crate) fn check_declaration_initializer(
        &mut self,
        declaration: NodeId,
        mode: u32,
        contextual: Option<TypeId>,
    ) -> Result<TypeId, Error> {
        let initializer = self
            .ast(declaration)?
            .node(declaration)?
            .initializer()
            .ok_or(Error::MissingLink("declaration initializer"))?;
        let ty = if let Some(ty) = self.quick_type_of_expression(initializer)? {
            ty
        } else if let Some(ty) = contextual {
            self.calls.contexts.push(crate::calls::ArgumentContext {
                node: initializer,
                ty,
                inference: None,
            });
            let result = self.check_expression_ex(initializer, mode | 1);
            self.calls.contexts.pop();
            result?
        } else if mode == 0 {
            self.check_expression_cached(initializer)?
        } else {
            self.check_expression_ex(initializer, mode)?
        };
        let mut root = declaration;
        while self.ast(root)?.node(root)?.kind() == K::BindingElement {
            let pattern = self
                .ast(root)?
                .node(root)?
                .parent()
                .ok_or(Error::MissingLink("initializer pattern"))?;
            root = self
                .ast(pattern)?
                .node(pattern)?
                .parent()
                .ok_or(Error::MissingLink("initializer root"))?;
        }
        if self.ast(root)?.node(root)?.kind() == K::Parameter {
            let name = self
                .ast(declaration)?
                .node(declaration)?
                .name()
                .ok_or(Error::MissingLink("initializer name"))?;
            let kind = self.ast(name)?.node(name)?.kind();
            if kind == K::ObjectBindingPattern
                && self.types.get(ty)?.object_flags & of::OBJECT_LITERAL != 0
            {
                return self.pad_binding_object_literal_type(ty, name);
            }
            if kind == K::ArrayBindingPattern && self.is_tuple_type(ty)? {
                return self.pad_binding_tuple_type(ty, name);
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.widenTypeInferredFromInitializer
    pub(crate) fn widen_type_inferred_from_initializer(
        &mut self,
        declaration: NodeId,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        let constant =
            ts_ast::utilities::get_combined_node_flags(self.ast(declaration)?, declaration)?
                & nf::CONSTANT
                != 0;
        let readonly =
            ts_ast::utilities::get_combined_modifier_flags(self.ast(declaration)?, declaration)?
                & mf::READONLY
                != 0
                && !self
                    .ast(declaration)?
                    .node(declaration)?
                    .parent()
                    .map(|parent| {
                        ts_ast::utilities::is_parameter_property_declaration(
                            self.ast(declaration)?,
                            declaration,
                            parent,
                        )
                        .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
        let ty = if constant || readonly {
            ty
        } else {
            self.widen_literal_type(ty)?
        };
        if self.ast(declaration)?.node(declaration)?.flags() & nf::JAVA_SCRIPT_FILE != 0 {
            let empty = if self.options.strict_null_checks {
                self.builtins.implicit_never_type
            } else {
                self.builtins.undefined_widening_type
            };
            if ty == empty {
                self.report_implicit_any(declaration, self.builtins.any_type)?;
                return Ok(self.builtins.any_type);
            }
            if self.is_array_type(ty)? && self.get_type_arguments(ty)?.first() == Some(&empty) {
                let any_array = *self
                    .query
                    .global_types
                    .get("anyArrayType")
                    .ok_or(Error::MissingLink("JS empty array global"))?;
                self.report_implicit_any(declaration, any_array)?;
                return Ok(any_array);
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.widenTypeForVariableLikeDeclaration
    pub(crate) fn widen_type_for_variable_like(
        &mut self,
        declaration: NodeId,
        ty: Option<TypeId>,
        report: bool,
    ) -> Result<TypeId, Error> {
        if let Some(mut ty) = ty {
            if self.types.flags(ty)? & tf::ES_SYMBOL != 0 {
                if let Some(parent) = self.ast(declaration)?.node(declaration)?.parent() {
                    let global =
                        self.resolve_name(None, b"SymbolConstructor", sf::TYPE, None, false)?;
                    if global.is_some() && self.get_symbol_of_declaration(parent)? == global {
                        ty = self.es_symbol_like_type_for_node(declaration)?;
                    }
                }
            }
            if report
                && self
                    .program()?
                    .host
                    .options()
                    .strict_option_value(self.program()?.host.options().no_implicit_any)
                && self.types.get(ty)?.object_flags & of::CONTAINS_WIDENING_TYPE != 0
                && !self.report_widening_errors_in_type(ty)?
            {
                self.report_implicit_any(declaration, ty)?;
            }
            if self.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0
                && (self.ast(declaration)?.node(declaration)?.kind() == K::BindingElement
                    || self
                        .ast(declaration)?
                        .node(declaration)?
                        .type_node()
                        .is_none())
            {
                let symbol = self.types.get(ty)?.symbol;
                if symbol != self.get_symbol_of_declaration(declaration)? {
                    ty = self.builtins.es_symbol_type;
                }
            }
            return self.widened_type(ty);
        }
        let read = self.ast(declaration)?.node(declaration)?;
        let rest = read.kind() == K::Parameter
            && read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("rest parameter"))?
                .dot_dot_dot_token()
                .is_some();
        let ty = if rest {
            *self
                .query
                .global_types
                .get("anyArrayType")
                .ok_or(Error::MissingLink("rest any array"))?
        } else {
            self.builtins.any_type
        };
        if report && !self.binding_private_ambient(declaration)? {
            self.report_implicit_any(declaration, ty)?;
        }
        Ok(ty)
    }
}
