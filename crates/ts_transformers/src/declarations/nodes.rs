use super::{
    transform::{Transformer, BUILDER_FLAGS, INTERNAL_FLAGS},
    util,
};
use ts_ast::{
    modifier_flags as mf, Factory, FactoryMethods, JsString, NodeId, NodeListId, RuntimeFactory,
    SyntaxKind as K,
};
use ts_printer::emit_resolver::{
    DeclarationEmitResolver, DeclarationSymbolTracker, DeclarationTrackerEvent,
};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitDeclarationSubtree
    pub fn subtree(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        if self.strip_internal(node)? || self.declaration_not_visible(node)? {
            return Ok(None);
        }
        let kind = self.node(node).kind().known();
        if kind == Some(K::SemicolonClassElement) {
            return Ok(None);
        }
        if ts_ast::utilities::is_function_like(Some(&self.node(node)))
            && self.resolver.implementation_of_overload(node)?
        {
            return Ok(None);
        }
        let dynamic = ts_ast::has_dynamic_name(self.output.view(), Some(node))?;
        if dynamic {
            let name = Self::required(self.node(node).name())?;
            let expression = Self::required(self.node(name).expression())?;
            let entity = ts_ast::is_entity_name_expression(self.output.view(), expression)?;
            if self.options.isolated_declarations {
                if !self
                    .resolver
                    .definitely_reference_to_global_symbol_object(expression)?
                {
                    let parent = Self::required(self.node(node).parent())?;
                    if matches!(
                        self.node(parent).kind().known(),
                        Some(K::ClassDeclaration | K::ObjectLiteralExpression)
                    ) {
                        self.diagnostic(node, ts_diagnostics::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations, vec![])?;
                        return Ok(None);
                    }
                    if matches!(
                        self.node(parent).kind().known(),
                        Some(K::InterfaceDeclaration | K::TypeLiteral)
                    ) && !entity
                    {
                        self.diagnostic(node, ts_diagnostics::Computed_properties_must_be_number_or_string_literals_variables_or_dotted_expressions_with_isolatedDeclarations, vec![])?;
                        return Ok(None);
                    }
                }
            } else if !self.resolver.late_bound(node)? || !entity {
                return Ok(None);
            }
        }
        let old_enclosing = self.enclosing;
        let old_selector = self.tracker.selector.clone();
        let old_name = self.tracker.error_name;
        let old_suppress = self.suppress_context;
        if util::is_enclosing_declaration(&self.node(node)) {
            self.enclosing = node;
        }
        let produces = util::can_produce_diagnostics(&self.node(node));
        if produces && !self.suppress_context {
            self.select_context(node, false)?;
        }
        if matches!(kind, Some(K::TypeLiteral | K::MappedType))
            && !self.node(node).parent().is_some_and(|p| {
                matches!(
                    self.node(p).kind().known(),
                    Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
                )
            })
        {
            self.suppress_context = true;
        }
        let result = self.subtree_worker(node);
        let result = match result {
            Ok(result) => {
                if result.is_some() && produces && dynamic {
                    if !self.suppress_context {
                        self.select_context(node, true)?;
                    }
                    self.tracker.error_name = self.node(node).name();
                    let name = Self::required(self.node(node).name())?;
                    let expression = Self::required(self.node(name).expression())?;
                    self.entity_visible(expression)?;
                }
                Ok(result)
            }
            Err(error) => Err(error),
        };
        self.enclosing = old_enclosing;
        self.tracker.selector = old_selector;
        self.tracker.error_name = old_name;
        self.suppress_context = old_suppress;
        result
    }
    #[allow(
        clippy::match_same_arms,
        reason = "Keep the pinned upstream per-kind dispatch auditable when individual syntax cases change"
    )]
    fn subtree_worker(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        match self.node(node).kind().known() {
            Some(K::VariableDeclaration) => {
                if self
                    .output
                    .read_source_file(self.source)?
                    .common_js_module_indicator()
                    .is_some()
                    && ts_ast::is_variable_declaration_initialized_to_require(
                        self.output.view(),
                        node,
                    )?
                {
                    return self.cjs_require_variable(node);
                }
                self.suppress_context = true;
                let name = self.node(node).name();
                if name.is_some_and(|n| {
                    matches!(
                        self.node(n).kind().known(),
                        Some(K::ArrayBindingPattern | K::ObjectBindingPattern)
                    )
                }) && self.has_binding_initializer(Self::required(name)?)?
                {
                    return self.recreate_binding(Self::required(name)?);
                }
                let name = self.binding_name(name)?;
                let ty = self.ensure_type(node, false)?;
                let init = self.ensure_initializer(node)?;
                Ok(Some(
                    self.output
                        .update_variable_declaration(node, name, None, ty, init),
                ))
            }
            Some(K::Parameter) => self.parameter(node).map(Some),
            Some(K::PropertyDeclaration | K::PropertySignature) => {
                let name = self.node(node).name();
                if name.is_some_and(|name| self.node(name).kind() == K::PrivateIdentifier) {
                    return Ok(None);
                }
                let mut postfix = self.node(node).postfix_token();
                if self.node(node).kind() == K::PropertyDeclaration
                    && postfix.is_some_and(|p| self.node(p).kind() == K::ExclamationToken)
                {
                    postfix = None;
                }
                let modifiers = self.modifiers(node)?;
                let ty = self.ensure_type(node, false)?;
                let init = self.ensure_initializer(node)?;
                Ok(Some(if self.node(node).kind() == K::PropertySignature {
                    self.output.update_property_signature_declaration(
                        node, modifiers, name, postfix, ty, init,
                    )
                } else {
                    self.output
                        .update_property_declaration(node, modifiers, name, postfix, ty, init)
                }))
            }
            Some(
                K::CallSignature
                | K::ConstructSignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::IndexSignature
                | K::FunctionType
                | K::ConstructorType,
            ) => self.signature(node),
            Some(K::TypeReference) => {
                let name = Self::required(
                    self.node(node)
                        .as_type_reference_node()
                        .unwrap()
                        .type_name(),
                )?;
                self.entity_visible(name)?;
                self.children(node).map(Some)
            }
            Some(K::TypeQuery) => {
                let name =
                    Self::required(self.node(node).as_type_query_node().unwrap().expr_name())?;
                self.entity_visible(name)?;
                self.children(node).map(Some)
            }
            Some(K::ExpressionWithTypeArguments) => {
                let expression = Self::required(self.node(node).expression())?;
                if ts_ast::is_entity_name_expression(self.output.view(), expression)? {
                    self.entity_visible(expression)?;
                }
                self.children(node).map(Some)
            }
            Some(K::ConditionalType) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_conditional_type_node()
                    .unwrap()
                    .to_owned();
                let check = self.visit(data.check_type)?;
                let extends = self.visit(data.extends_type)?;
                let old = self.enclosing;
                self.enclosing = Self::required(data.true_type)?;
                let yes = self.visit(data.true_type);
                self.enclosing = old;
                let yes = yes?;
                let no = self.visit(data.false_type)?;
                Ok(Some(self.output.update_conditional_type_node(
                    node, check, extends, yes, no,
                )))
            }
            Some(K::MappedType) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_mapped_type_node()
                    .unwrap()
                    .to_owned();
                let ty = match data.r#type {
                    Some(ty) => self.visit(Some(ty))?,
                    None => Some(self.output.new_keyword_type_node(K::AnyKeyword.into())),
                };
                let parameter = self.visit(data.type_parameter)?;
                let name = self.visit(data.name_type)?;
                Ok(Some(self.output.update_mapped_type_node(
                    node,
                    data.readonly_token,
                    parameter,
                    name,
                    data.question_token,
                    ty,
                    None,
                )))
            }
            Some(K::HeritageClause) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_heritage_clause()
                    .unwrap()
                    .to_owned();
                let mut types = Vec::new();
                for element in self.list_nodes(data.types) {
                    let name = if self.node(element).kind() == K::ExpressionWithTypeArguments {
                        Self::required(self.node(element).expression())?
                    } else {
                        Self::required(
                            self.node(element)
                                .as_type_reference_node()
                                .unwrap()
                                .type_name(),
                        )?
                    };
                    if ts_ast::is_entity_name_expression(self.output.view(), name)?
                        || matches!(self.node(name).kind().known(), Some(K::QualifiedName))
                        || (data.token == K::ExtendsKeyword
                            && self.node(name).kind() == K::NullKeyword)
                    {
                        if let Some(element) = self.visit(Some(element))? {
                            types.push(element);
                        }
                    }
                }
                if types.is_empty() {
                    return Ok(None);
                }
                let list = self.new_list(types);
                Ok(Some(self.output.update_heritage_clause(
                    node,
                    data.token,
                    Some(list),
                )))
            }
            Some(K::TypeParameter) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_type_parameter_declaration()
                    .unwrap()
                    .to_owned();
                let private_method = self.node(node).parent().is_some_and(|parent| {
                    matches!(self.node(parent).kind().known(), Some(K::MethodDeclaration))
                }) && self.resolver.effective_declaration_flags(
                    Self::required(self.node(node).parent())?,
                    mf::PRIVATE,
                )? != 0;
                if private_method && (data.constraint.is_some() || data.default_type.is_some()) {
                    return Ok(Some(self.output.update_type_parameter_declaration(
                        node,
                        data.modifiers,
                        data.name,
                        None,
                        data.expression,
                        None,
                    )));
                }
                self.children(node).map(Some)
            }
            Some(K::QualifiedName) => {
                let right = Self::required(self.node(node).as_qualified_name().unwrap().right())?;
                if self.node(right).kind() == K::PrivateIdentifier {
                    self.diagnostic(node, ts_diagnostics::Declaration_emit_elides_private_members_but_0_refers_to_a_private_member_Write_an_explicit_type_here, vec![self.output.view().node_text(right)?.into_js_string()])?;
                }
                self.children(node).map(Some)
            }
            Some(K::ImportType) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_import_type_node()
                    .unwrap()
                    .to_owned();
                if data
                    .argument
                    .is_none_or(|arg| self.node(arg).kind() != K::LiteralType)
                {
                    return Ok(Some(node));
                }
                let arguments = self.visit_list_result(data.type_arguments)?;
                Ok(Some(self.output.update_import_type_node(
                    node,
                    data.is_type_of,
                    data.argument,
                    data.attributes,
                    data.qualifier,
                    arguments,
                )))
            }
            Some(K::JSDocTypeExpression) => self.visit(self.node(node).type_node()),
            Some(K::JSDocNonNullableType) => self.visit(self.node(node).type_node()),
            Some(
                K::JSDocAllType
                | K::JSDocNullableType
                | K::JSDocOptionalType
                | K::JSDocVariadicType
                | K::JSDocTypeLiteral
                | K::JSDocPropertyTag,
            ) => self.jsdoc_type(node).map(Some),
            _ => self.children(node).map(Some),
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocTypeLiteral
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocPropertyTag
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocAllType
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocNullableType
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocOptionalType
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformJSDocVariadicType
    fn jsdoc_type(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let result = match self.node(node).kind().known() {
            Some(K::JSDocAllType) => self.output.new_keyword_type_node(K::AnyKeyword.into()),
            Some(K::JSDocTypeLiteral) => {
                let slice = self
                    .node(node)
                    .as_js_doc_type_literal()
                    .unwrap()
                    .js_doc_property_tags();
                let tags: Vec<_> = self.output.read_nodes(slice).iter().flatten().collect();
                let mut members = Vec::new();
                for tag in tags {
                    if let Some(member) = self.visit(Some(tag))? {
                        members.push(member);
                    }
                }
                let list = self.new_list(members);
                self.output.new_type_literal_node(Some(list))
            }
            Some(K::JSDocPropertyTag) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_js_doc_parameter_or_property_tag()
                    .unwrap()
                    .to_owned();
                let name = self.visit(data.tag_name)?;
                let ty = self.visit(data.type_expression)?;
                self.output
                    .new_property_signature_declaration(None, name, None, ty, None)
            }
            Some(K::JSDocVariadicType) => {
                let operand = self
                    .node(node)
                    .data_source()
                    .as_js_doc_variadic_type()
                    .and_then(|data| data.r#type());
                let ty = self.visit(operand)?;
                self.output.new_array_type_node(ty)
            }
            Some(K::JSDocNullableType | K::JSDocOptionalType) => {
                let ty = self.visit(self.node(node).type_node())?;
                let extra = if self.node(node).kind() == K::JSDocNullableType {
                    let null = self.output.new_keyword_expression(K::NullKeyword.into());
                    self.output.new_literal_type_node(Some(null))
                } else {
                    self.output
                        .new_keyword_type_node(K::UndefinedKeyword.into())
                };
                let nodes = self.output.alloc_nodes(vec![ty, Some(extra)]);
                let list = self
                    .output
                    .alloc_list(ts_core::TextRange::new(-1, -1), nodes);
                self.output.new_union_type_node(Some(list))
            }
            _ => return Err(ts_arena::Error::InvalidGraph.into()),
        };
        self.emit.set_original(result, node);
        Ok(result)
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureType
    pub fn ensure_type(
        &mut self,
        node: NodeId,
        ignore_private: bool,
    ) -> Result<Option<NodeId>, R::Error> {
        let flags = self.builder_flags();
        if !ignore_private
            && self
                .resolver
                .effective_declaration_flags(node, mf::PRIVATE)?
                != 0
        {
            return Ok(None);
        }
        if self.should_initializer(node)? {
            return Ok(None);
        }
        let kind = self.node(node).kind();
        let ty = if matches!(kind.known(), Some(K::ExportAssignment | K::BindingElement)) {
            None
        } else {
            self.node(node).type_node()
        };
        if let Some(ty) = ty {
            if kind != K::Parameter
                || !self.resolver.requires_adding_implicit_undefined(
                    node,
                    None,
                    Some(self.enclosing),
                )?
            {
                if !self.output.read_source_file(self.source)?.is_js() {
                    return self.visit(Some(ty));
                }
                let result = self.resolver.try_js_type_node_to_type_node(
                    self.output,
                    self.emit,
                    ty,
                    self.enclosing,
                    flags,
                    INTERNAL_FLAGS,
                    &mut self.tracker,
                )?;
                self.flush_reports()?;
                if result.is_some() {
                    return Ok(result);
                }
            }
        }
        let old_name = self.tracker.error_name;
        let old_selector = self.tracker.selector.clone();
        self.tracker.error_name = self.node(node).name();
        if !self.suppress_context && util::can_produce_diagnostics(&self.node(node)) {
            self.select_context(node, false)?;
        }
        let result = if ts_ast::utilities_tail::has_inferred_type(&self.node(node)) {
            self.resolver.create_type_of_declaration(
                self.output,
                self.emit,
                node,
                self.enclosing,
                flags,
                INTERNAL_FLAGS,
                &mut self.tracker,
            )
        } else if ts_ast::utilities::is_function_like(Some(&self.node(node))) {
            self.resolver.create_return_type_of_signature(
                self.output,
                self.emit,
                node,
                self.enclosing,
                flags,
                INTERNAL_FLAGS,
                &mut self.tracker,
            )
        } else {
            return Self::unsupported("declaration emit: ensureType node kind");
        };
        let flushed = self.flush_reports();
        self.tracker.error_name = old_name;
        self.tracker.selector = old_selector;
        flushed?;
        result.map(|node| {
            node.or_else(|| Some(self.output.new_keyword_type_node(K::AnyKeyword.into())))
        })
    }
    pub fn should_initializer(&mut self, node: NodeId) -> Result<bool, R::Error> {
        let flags = self
            .resolver
            .effective_declaration_flags(node, mf::PRIVATE)?;
        if !util::can_have_literal_initializer(&self.node(node), flags) {
            return Ok(false);
        }
        Ok(self.node(node).initializer().is_some()
            && self.resolver.literal_const_declaration(node)?)
    }
    pub fn ensure_initializer(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        if !self.should_initializer(node)? {
            return Ok(None);
        }
        let initializer = Self::required(self.node(node).initializer())?;
        let unwrapped = util::unwrap_parenthesized_expression(self.output.view(), initializer)?;
        if !ts_ast::utilities_tail::is_primitive_literal_value(
            self.output.view(),
            &self.node(unwrapped),
            true,
        )? {
            self.tracker
                .report(DeclarationTrackerEvent::InferenceFallback(node));
        }
        let result = self.resolver.create_literal_const_value(
            self.output,
            self.emit,
            node,
            &mut self.tracker,
        )?;
        self.flush_reports()?;
        Ok(result)
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureParameter
    pub fn parameter(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let old = self.tracker.selector.clone();
        if !self.suppress_context {
            self.select_context(node, false)?;
        }
        let data = self
            .node(node)
            .data_source()
            .as_parameter_declaration()
            .unwrap()
            .to_owned();
        let question = if self.resolver.optional_parameter(node)? {
            data.question_token
                .or_else(|| Some(self.output.new_token(K::QuestionToken.into())))
        } else {
            None
        };
        let name = self.binding_name(data.name)?;
        let ty = self.ensure_type(node, true)?;
        let init = self.ensure_initializer(node)?;
        let result = self.output.update_parameter_declaration(
            node,
            None,
            data.dot_dot_dot_token,
            name,
            question,
            ty,
            init,
        );
        self.tracker.selector = old;
        Ok(result)
    }
    pub fn parameters(&mut self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        let params = self.node(node).parameter_list();
        let mut result = Vec::new();
        if self
            .resolver
            .effective_declaration_flags(node, mf::PRIVATE)?
            == 0
        {
            for parameter in self.list_nodes(params) {
                result.push(self.parameter(parameter)?);
            }
        }
        Ok(Some(self.new_list(result)))
    }
    pub fn type_parameters(&mut self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        if self
            .resolver
            .effective_declaration_flags(node, mf::PRIVATE)?
            != 0
        {
            return Ok(None);
        }
        let params = self.visit_list_result(self.node(node).type_parameter_list())?;
        if params.is_some() {
            return Ok(params);
        }
        let read = self.node(node);
        let data = read.data_source();
        let full = match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .and_then(|d| d.full_signature()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .and_then(|d| d.full_signature()),
            Some(K::Constructor) => data
                .as_constructor_declaration()
                .and_then(|d| d.full_signature()),
            Some(K::GetAccessor) => data
                .as_get_accessor_declaration()
                .and_then(|d| d.full_signature()),
            Some(K::SetAccessor) => data
                .as_set_accessor_declaration()
                .and_then(|d| d.full_signature()),
            _ => None,
        };
        drop(read);
        if full.is_none() {
            return Ok(None);
        }
        let old_name = self.tracker.error_name;
        let old_selector = self.tracker.selector.clone();
        self.tracker.error_name = self.node(node).name();
        if !self.suppress_context && util::can_produce_diagnostics(&self.node(node)) {
            self.select_context(node, false)?;
        }
        let result = self.resolver.create_type_parameters_of_signature(
            self.output,
            self.emit,
            node,
            self.enclosing,
            BUILDER_FLAGS,
            INTERNAL_FLAGS,
            &mut self.tracker,
        )?;
        self.flush_reports()?;
        self.tracker.error_name = old_name;
        self.tracker.selector = old_selector;
        Ok((!result.is_empty()).then(|| self.new_list(result)))
    }
    pub fn signature(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        let kind = self.node(node).kind();
        let name = self.node(node).name();
        if name.is_some_and(|name| self.node(name).kind() == K::PrivateIdentifier) {
            return Ok(None);
        }
        let modifiers = self.modifiers(node)?;
        if matches!(
            kind.known(),
            Some(K::MethodDeclaration | K::MethodSignature)
        ) && self
            .resolver
            .effective_declaration_flags(node, mf::PRIVATE)?
            != 0
        {
            if let Some(symbol) = self.resolver.symbol_of_declaration(node)? {
                if self
                    .resolver
                    .symbol_declarations(symbol)?
                    .first()
                    .is_some_and(|first| *first != node)
                {
                    return Ok(None);
                }
            }
            return Ok(Some(
                self.output
                    .new_property_declaration(modifiers, name, None, None, None),
            ));
        }
        let type_params = if matches!(
            kind.known(),
            Some(K::Constructor | K::GetAccessor | K::SetAccessor | K::IndexSignature)
        ) {
            None
        } else {
            self.type_parameters(node)?
        };
        let parameters = if matches!(kind.known(), Some(K::GetAccessor | K::SetAccessor)) {
            self.accessor_parameters(node)?
        } else {
            self.parameters(node)?
        };
        let ty =
            match kind.known() {
                Some(K::SetAccessor | K::Constructor) => None,
                Some(K::FunctionType | K::ConstructorType | K::IndexSignature) => {
                    let ty = self.visit(self.node(node).type_node())?;
                    if kind == K::IndexSignature {
                        Some(ty.unwrap_or_else(|| {
                            self.output.new_keyword_type_node(K::AnyKeyword.into())
                        }))
                    } else {
                        ty
                    }
                }
                _ => self.ensure_type(node, false)?,
            };
        let postfix = if matches!(
            kind.known(),
            Some(K::MethodDeclaration | K::MethodSignature)
        ) {
            self.node(node).postfix_token()
        } else {
            None
        };
        let result = match kind.known() {
            Some(K::CallSignature) => {
                self.output
                    .update_call_signature_declaration(node, type_params, parameters, ty)
            }
            Some(K::ConstructSignature) => self.output.update_construct_signature_declaration(
                node,
                type_params,
                parameters,
                ty,
            ),
            Some(K::MethodSignature) => self.output.update_method_signature_declaration(
                node,
                modifiers,
                name,
                postfix,
                type_params,
                parameters,
                ty,
            ),
            Some(K::MethodDeclaration) => self.output.update_method_declaration(
                node,
                modifiers,
                None,
                name,
                postfix,
                type_params,
                parameters,
                ty,
                None,
                None,
            ),
            Some(K::Constructor) => self.output.update_constructor_declaration(
                node, modifiers, None, parameters, None, None, None,
            ),
            Some(K::GetAccessor) => self.output.update_get_accessor_declaration(
                node, modifiers, name, None, parameters, ty, None, None,
            ),
            Some(K::SetAccessor) => self.output.update_set_accessor_declaration(
                node, modifiers, name, None, parameters, None, None, None,
            ),
            Some(K::IndexSignature) => self
                .output
                .update_index_signature_declaration(node, modifiers, parameters, ty),
            Some(K::FunctionType) => {
                self.output
                    .update_function_type_node(node, type_params, parameters, ty)
            }
            Some(K::ConstructorType) => self.output.update_constructor_type_node(
                node,
                modifiers,
                type_params,
                parameters,
                ty,
            ),
            _ => return Err(ts_arena::Error::InvalidGraph.into()),
        };
        Ok(Some(result))
    }
    fn accessor_parameters(&mut self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        let private = self
            .resolver
            .effective_declaration_flags(node, mf::PRIVATE)?
            != 0;
        let parameters = self.list_nodes(self.node(node).parameter_list());
        let mut result = Vec::new();
        if !private {
            if let Some(first) = parameters.first() {
                if self.node(*first).name().is_some_and(|name| {
                    self.node(name).kind() == K::Identifier
                        && self.node(name).as_identifier().unwrap().text() == b"this"
                }) {
                    result.push(self.parameter(*first)?);
                }
            }
        }
        if self.node(node).kind() == K::SetAccessor {
            let value = if private {
                None
            } else {
                parameters
                    .get(result.len())
                    .copied()
                    .map(|p| self.parameter(p))
                    .transpose()?
            };
            let value = if let Some(value) = value {
                value
            } else {
                let ty =
                    (!private).then(|| self.output.new_keyword_type_node(K::AnyKeyword.into()));
                let name = self
                    .output
                    .new_identifier(JsString::from_bytes(b"value".as_slice()));
                self.output
                    .new_parameter_declaration(None, None, Some(name), None, ty, None)
            };
            result.push(value);
        }
        Ok(Some(self.new_list(result)))
    }
    pub fn binding_name(&mut self, node: Option<NodeId>) -> Result<Option<NodeId>, R::Error> {
        let Some(node) = node else { return Ok(None) };
        match self.node(node).kind().known() {
            Some(K::ArrayBindingPattern | K::ObjectBindingPattern) => {
                let elements = self.list_nodes(self.node(node).element_list());
                let mut result = Vec::new();
                for e in elements {
                    if let Some(e) = self.binding_name(Some(e))? {
                        result.push(e);
                    }
                }
                let list = self.new_list(result);
                Ok(Some(self.output.update_binding_pattern(node, Some(list))))
            }
            Some(K::BindingElement) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_binding_element()
                    .unwrap()
                    .to_owned();
                if let Some(property) = data.property_name {
                    if self.node(property).kind() == K::ComputedPropertyName {
                        let expression = Self::required(self.node(property).expression())?;
                        if ts_ast::is_entity_name_expression(self.output.view(), expression)? {
                            self.entity_visible(expression)?;
                        }
                    }
                }
                let name = self.binding_name(data.name)?;
                Ok(Some(self.output.update_binding_element(
                    node,
                    data.dot_dot_dot_token,
                    data.property_name,
                    name,
                    None,
                )))
            }
            _ => Ok(Some(node)),
        }
    }
    fn has_binding_initializer(&self, pattern: NodeId) -> Result<bool, R::Error> {
        for element in self.list_nodes(self.node(pattern).element_list()) {
            if self.node(element).kind() != K::BindingElement {
                continue;
            }
            if self.node(element).initializer().is_some() {
                return Ok(true);
            }
            if let Some(name) = self.node(element).name() {
                if matches!(
                    self.node(name).kind().known(),
                    Some(K::ArrayBindingPattern | K::ObjectBindingPattern)
                ) && self.has_binding_initializer(name)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn recreate_binding(&mut self, pattern: NodeId) -> Result<Option<NodeId>, R::Error> {
        let mut result = Vec::new();
        for element in self.list_nodes(self.node(pattern).element_list()) {
            let Some(name) = self.node(element).name() else {
                continue;
            };
            if !self.binding_visible(element)? {
                continue;
            }
            if matches!(
                self.node(name).kind().known(),
                Some(K::ArrayBindingPattern | K::ObjectBindingPattern)
            ) {
                if let Some(nested) = self.recreate_binding(name)? {
                    self.append_flat(nested, &mut result);
                }
            } else {
                let ty = self.ensure_type(element, false)?;
                result.push(
                    self.output
                        .new_variable_declaration(Some(name), None, ty, None),
                );
            }
        }
        Ok(match result.len() {
            0 => None,
            1 => result.first().copied(),
            _ => {
                let nodes = self
                    .output
                    .alloc_nodes(result.into_iter().map(Some).collect());
                Some(self.output.new_syntax_list(nodes))
            }
        })
    }
}
