//! Context is resolved from the native parent path. Explicit argument contexts
//! also act as temporary barriers while an expression is checked speculatively.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:someType
    pub(crate) fn any_type(
        &mut self,
        ty: TypeId,
        predicate: &mut dyn FnMut(&mut Self, TypeId) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        for part in self.distributed_types(ty)? {
            if predicate(self, part)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualType
    pub(crate) fn contextual_expression_type(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        self.contextual_expression_type_ex(node, 0)
    }

    pub(crate) fn contextual_expression_type_ex(
        &mut self,
        node: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::IN_WITH_STATEMENT != 0 {
            return Ok(None);
        }
        if let Some(context) = self.contextual_call_argument(node) {
            return Ok(Some(context.ty));
        }
        if matches!(
            read.kind().known(),
            Some(
                K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::ArrowFunction
                    | K::MethodDeclaration
            )
        ) {
            if let Some(annotation) = self.full_signature_type_node(node)? {
                return self.get_type_from_type_node(annotation).map(Some);
            }
        }
        let read = self.ast(node)?.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let read = self.ast(parent)?.node(parent)?;
        match read.kind().known() {
            Some(
                K::VariableDeclaration
                | K::Parameter
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::BindingElement,
            ) => {
                if read.initializer() != Some(node) {
                    return Ok(None);
                }
                if let Some(annotation) = read.type_node() {
                    return self.get_type_from_type_node(annotation).map(Some);
                }
                let kind = read.kind();
                if kind == K::BindingElement {
                    if let Some(ty) = self.contextual_binding_element_type(parent)? {
                        return Ok(Some(ty));
                    }
                }
                if kind == K::Parameter {
                    if let Some(ty) = self.contextually_typed_parameter_type(parent)? {
                        return Ok(Some(ty));
                    }
                }
                let read = self.ast(parent)?.node(parent)?;
                if read.kind() == K::PropertyDeclaration
                    && read.modifier_flags(self.ast(parent)?)? & ts_ast::modifier_flags::STATIC != 0
                {
                    let class = read
                        .parent()
                        .ok_or(Error::MissingLink("static property class"))?;
                    if self.ast(class)?.node(class)?.kind() == K::ClassExpression {
                        if let Some(context) =
                            self.contextual_expression_type_ex(class, context_flags)?
                        {
                            let symbol = self
                                .get_symbol_of_declaration(parent)?
                                .ok_or(Error::MissingLink("static property symbol"))?;
                            let name = self.symbol(symbol)?.name_to_owned();
                            return self
                                .type_of_property_of_contextual_type(context, name.as_bytes());
                        }
                    }
                }
                let read = self.ast(parent)?.node(parent)?;
                if let Some(name) = read.name() {
                    let name = self.ast(name)?.node(name)?;
                    if context_flags & 8 == 0
                        && matches!(
                            name.kind().known(),
                            Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                        )
                        && !self.source_list(parent, name.element_list())?.is_empty()
                    {
                        return self
                            .type_from_binding_pattern(
                                read.name()
                                    .ok_or(Error::MissingLink("binding initializer name"))?,
                                true,
                                false,
                            )
                            .map(Some);
                    }
                }
                Ok(None)
            }
            Some(K::ArrowFunction | K::ReturnStatement) => {
                self.contextual_type_for_return_expression_ex(node, context_flags)
            }
            Some(K::ParenthesizedExpression | K::NonNullExpression) => {
                self.contextual_expression_type_ex(parent, context_flags)
            }
            Some(K::AsExpression | K::TypeAssertionExpression) => {
                if ts_ast::utilities_middle::is_const_assertion(self.ast(parent)?, &read)? {
                    return self.contextual_expression_type_ex(parent, context_flags);
                }
                self.get_type_from_type_node(
                    read.type_node()
                        .ok_or(Error::MissingLink("assertion annotation"))?,
                )
                .map(Some)
            }
            Some(K::SatisfiesExpression) => self
                .get_type_from_type_node(
                    read.type_node()
                        .ok_or(Error::MissingLink("satisfies annotation"))?,
                )
                .map(Some),
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment | K::MethodDeclaration) => {
                self.contextual_property_type_with_flags(parent, context_flags)
            }
            Some(K::SpreadAssignment) => self.contextual_expression_type_ex(
                read.parent().ok_or(Error::MissingLink("spread parent"))?,
                context_flags,
            ),
            Some(K::ArrayLiteralExpression) => {
                self.contextual_array_element_ex(node, parent, context_flags)
            }
            Some(K::ConditionalExpression) => {
                let condition = read
                    .data_source()
                    .as_conditional_expression()
                    .ok_or(Error::MissingLink("conditional"))?
                    .condition();
                if condition == Some(node) {
                    Ok(None)
                } else {
                    self.contextual_expression_type_ex(parent, context_flags)
                }
            }
            Some(K::TemplateSpan) => {
                let template = read.parent().ok_or(Error::MissingLink("template parent"))?;
                if let Some(parent) = self.ast(template)?.node(template)?.parent() {
                    if self.ast(parent)?.node(parent)?.kind() == K::TaggedTemplateExpression {
                        let args = self.effective_call_arguments(parent)?;
                        let Some(index) = args.iter().position(|&argument| argument == node) else {
                            return Ok(None);
                        };
                        let signature = if let Some(signature) = self.cached_call_signature(parent)
                        {
                            signature
                        } else {
                            self.check_tagged_template_expression(parent)?;
                            self.cached_call_signature(parent)
                                .ok_or(Error::MissingLink("contextual tagged signature"))?
                        };
                        return self.parameter_type_at(signature, index);
                    }
                }
                Ok(None)
            }
            Some(K::CallExpression | K::NewExpression) => {
                let args = self.source_list(parent, read.argument_list())?;
                let Some(index) = args.iter().position(|&arg| arg == node) else {
                    return Ok(None);
                };
                if crate::external_resolution::is_import_call(self.ast(parent)?, &read)? {
                    return match index {
                        0 => Ok(Some(self.builtins.string_type)),
                        1 => self.global_import_call_options_type(false).map(Some),
                        _ => Ok(Some(self.builtins.any_type)),
                    };
                }
                let signature = if let Some(signature) = self.cached_call_signature(parent) {
                    signature
                } else {
                    self.check_call_expression(parent)?;
                    self.cached_call_signature(parent)
                        .ok_or(Error::MissingLink("contextual call signature"))?
                };
                self.parameter_type_at(signature, index)
            }
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(Error::MissingLink("binary context"))?;
                if let Some(annotation) = read.type_node() {
                    return self.get_type_from_type_node(annotation).map(Some);
                }
                let left = data.left().ok_or(Error::MissingLink("binary left"))?;
                let right = data.right();
                let token = data
                    .operator_token()
                    .ok_or(Error::MissingLink("binary operator"))?;
                match self.ast(token)?.node(token)?.kind().known() {
                    Some(
                        K::EqualsToken
                        | K::AmpersandAmpersandEqualsToken
                        | K::BarBarEqualsToken
                        | K::QuestionQuestionEqualsToken,
                    ) if right == Some(node) => {
                        let root = self.leftmost_context_expression(left)?;
                        if self.ast(root)?.node(root)?.kind() == K::Identifier {
                            let symbol = self.resolved_value_symbol(root)?;
                            if self.symbol(symbol)?.flags() & sf::MODULE_EXPORTS != 0 {
                                return Ok(None);
                            }
                        }
                        self.contextual_assignment_expression(parent, left)
                    }
                    Some(K::BarBarToken | K::QuestionQuestionToken) => {
                        let context = self.contextual_expression_type_ex(parent, context_flags)?;
                        if right == Some(node) && context.is_none() {
                            self.get_type_of_expression(left).map(Some)
                        } else {
                            Ok(context)
                        }
                    }
                    Some(K::AmpersandAmpersandToken | K::CommaToken) if right == Some(node) => {
                        self.contextual_expression_type_ex(parent, context_flags)
                    }
                    _ => Ok(None),
                }
            }
            Some(K::AwaitExpression) => {
                let Some(context) = self.contextual_expression_type_ex(parent, context_flags)?
                else {
                    return Ok(None);
                };
                let Some(awaited) = self.awaited_type_no_alias(context)? else {
                    return Ok(None);
                };
                let promise = self.create_promise_like_type(awaited)?;
                self.get_union_type(&[awaited, promise]).map(Some)
            }
            Some(K::ExportAssignment) => read
                .type_node()
                .map(|ty| self.get_type_from_type_node(ty))
                .transpose(),
            Some(K::YieldExpression) => {
                self.contextual_type_for_yield_operand(parent, context_flags)
            }
            Some(K::ImportAttribute) => self.contextual_import_attribute_type(parent),
            Some(
                K::Decorator
                | K::JsxExpression
                | K::JsxAttribute
                | K::JsxSpreadAttribute
                | K::JsxOpeningElement
                | K::JsxSelfClosingElement,
            ) => Err(Error::Unsupported(
                "getContextualType: decorator/JSX context",
            )),
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getApparentTypeOfContextualType
    pub(crate) fn apparent_contextual_expression_type(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        self.apparent_contextual_expression_type_ex(node, 0)
    }

    pub(crate) fn apparent_contextual_expression_type_ex(
        &mut self,
        node: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let object_method = read.kind() == K::MethodDeclaration
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
        let context = if object_method {
            self.contextual_property_type_with_flags(node, context_flags)?
        } else {
            self.contextual_expression_type_ex(node, context_flags)?
        };
        let Some(ty) = context else {
            return Ok(None);
        };
        let inference = self.call_inference_at_node(node)?;
        let ty = self.instantiate_call_contextual_type(ty, inference, context_flags & 1 != 0)?;
        if context_flags & 2 != 0 && self.types.flags(ty)? & tf::TYPE_VARIABLE != 0 {
            return Ok(None);
        }
        let ty = self
            .map_type_ex(
                ty,
                &mut |c, t| {
                    if c.types.get(t)?.object_flags & of::MAPPED != 0 {
                        Ok(Some(t))
                    } else {
                        c.apparent_type(t).map(Some)
                    }
                },
                true,
            )?
            .ok_or(Error::MissingLink("apparent contextual type"))?;
        if self.types.flags(ty)? & tf::UNION != 0
            && self.ast(node)?.node(node)?.kind() == K::ObjectLiteralExpression
        {
            return self.discriminate_object_context(node, ty).map(Some);
        }
        Ok(Some(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.isValidConstAssertionArgument
    pub(crate) fn valid_const_assertion_argument(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(
                K::StringLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::TrueKeyword
                | K::FalseKeyword
                | K::ArrayLiteralExpression
                | K::ObjectLiteralExpression
                | K::TemplateExpression,
            ) => Ok(true),
            Some(K::ParenthesizedExpression) => self.valid_const_assertion_argument(
                read.expression()
                    .ok_or(Error::MissingLink("const parentheses"))?,
            ),
            Some(K::PrefixUnaryExpression) => {
                let data = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(Error::MissingLink("const prefix"))?;
                let operand = data.operand().ok_or(Error::MissingLink("const operand"))?;
                let operator = data.operator();
                let kind = self.ast(operand)?.node(operand)?.kind();
                Ok(operator == K::MinusToken
                    && matches!(kind.known(), Some(K::NumericLiteral | K::BigIntLiteral))
                    || operator == K::PlusToken && kind == K::NumericLiteral)
            }
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                let mut expression = read
                    .expression()
                    .ok_or(Error::MissingLink("const access"))?;
                while self.ast(expression)?.node(expression)?.kind() == K::ParenthesizedExpression {
                    expression = self
                        .ast(expression)?
                        .node(expression)?
                        .expression()
                        .ok_or(Error::MissingLink("const access parentheses"))?;
                }
                if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
                    return Ok(false);
                }
                let symbol = self.resolve_entity_name(expression, sf::VALUE, true)?;
                Ok(symbol
                    .map(|symbol| {
                        self.symbol(symbol)
                            .map(|symbol| symbol.flags() & sf::ENUM != 0)
                    })
                    .transpose()?
                    .unwrap_or(false))
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.isConstContext
    pub(crate) fn is_const_context(&mut self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        if ts_ast::utilities_middle::is_const_assertion(self.ast(parent)?, &read)? {
            return Ok(true);
        }
        if self.is_inline_import_attributes(node)? {
            return Ok(true);
        }
        if self.valid_const_assertion_argument(node)? {
            if let Some(ty) = self.contextual_expression_type(node)? {
                if self.is_const_type_variable(ty, 0)? {
                    return Ok(true);
                }
            }
        }
        let read = self.ast(parent)?.node(parent)?;
        match read.kind().known() {
            Some(K::ParenthesizedExpression | K::ArrayLiteralExpression | K::SpreadElement) => {
                self.is_const_context(parent)
            }
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment | K::TemplateSpan) => self
                .is_const_context(
                    read.parent()
                        .ok_or(Error::MissingLink("const property parent"))?,
                ),
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExpressionForMutableLocation
    pub(crate) fn check_expression_for_mutable_location(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let mut ty = self.check_expression_ex(node, self.expression_mode)?;
        if self.is_const_context(node)? {
            return self.get_regular_type_of_literal_type(ty);
        }
        if matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::AsExpression | K::TypeAssertionExpression)
        ) {
            return Ok(ty);
        }
        let contextual = self.contextual_expression_type(node)?;
        let inference = self
            .contextual_call_argument(node)
            .and_then(|context| context.inference);
        let contextual = contextual
            .map(|t| self.instantiate_call_contextual_type(t, inference, false))
            .transpose()?;
        if !self.literal_of_context(ty, contextual)? {
            ty = self.widen_literal_type(ty)?;
            ty = self.widened_unique_es_symbol_type(ty)?;
        }
        self.get_regular_type_of_literal_type(ty)
    }

    /// A unique symbol keeps its identity only in a const-like location. Every
    /// other mutable location widens it to `symbol`, so a later declaration
    /// serialization never reaches an inaccessible unique symbol.
    // port: tsc/internal/checker/checker.go:Checker.getWidenedUniqueESSymbolType
    pub(crate) fn widened_unique_es_symbol_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::UNIQUE_ES_SYMBOL != 0 {
            return Ok(self.builtins.es_symbol_type);
        }
        if flags & tf::UNION != 0 {
            // The mapper is total, so upstream's `mapType` always yields a type.
            return self
                .map_type(ty, &mut |checker, part| {
                    checker.widened_unique_es_symbol_type(part).map(Some)
                })?
                .ok_or(Error::MissingLink("widened unique symbol union"));
        }
        Ok(ty)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getContextualImportAttributeType
    fn contextual_import_attribute_type(&mut self, node: NodeId) -> Result<Option<TypeId>, Error> {
        let name = self
            .ast(node)?
            .node(node)?
            .name()
            .ok_or(Error::MissingLink("import attribute name"))?;
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let global = self.global_import_attributes_type()?;
        self.type_of_property_of_contextual_type(global, text.as_bytes())
    }

    // port: tsc/internal/checker/checker.go:Checker.isInlineImportAttributes
    fn is_inline_import_attributes(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if read.kind() != K::ObjectLiteralExpression {
            return Ok(false);
        }
        let Some(property) = read.parent() else {
            return Ok(false);
        };
        let property_read = view.node(property)?;
        if property_read.kind() != K::PropertyAssignment
            || property_read.initializer() != Some(node)
        {
            return Ok(false);
        }
        let Some(name) = property_read.name() else {
            return Ok(false);
        };
        let name_read = view.node(name)?;
        if !(name_read.kind() == K::Identifier
            || ts_ast::utilities::is_string_literal_like(&name_read))
            || view.node_text(name)?.as_bytes() != b"with"
        {
            return Ok(false);
        }
        let Some(options) = property_read.parent() else {
            return Ok(false);
        };
        if view.node(options)?.kind() != K::ObjectLiteralExpression {
            return Ok(false);
        }
        let mut ancestor = Some(options);
        let mut import_call = None;
        while let Some(id) = ancestor {
            let read = view.node(id)?;
            if crate::external_resolution::is_import_call(view, &read)? {
                import_call = Some(id);
                break;
            }
            ancestor = read.parent();
        }
        let Some(import_call) = import_call else {
            return Ok(false);
        };
        let arguments = self.source_list(import_call, view.node(import_call)?.argument_list())?;
        Ok(arguments.len() > 1 && ts_ast::skip_parentheses(view, arguments[1])? == options)
    }
}
