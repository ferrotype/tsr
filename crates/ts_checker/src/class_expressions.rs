//! Value-level class, `this`, and `super` operations. The checker resolves the
//! lexical container before applying the existing control-flow query.

use crate::{CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkClassExpression
    pub(crate) fn check_class_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_class_declaration(node)?;
        self.defer_checker_node(node)?;
        self.check_class_expression_external_helpers(node)?;
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "class expression symbol",
        )?;
        self.get_type_of_symbol(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkClassExpressionDeferred
    pub(crate) fn check_class_expression_deferred(&mut self, node: NodeId) -> Result<(), Error> {
        for member in self.source_list(node, self.node(node)?.member_list())? {
            self.check_source_element(member)?;
        }
        self.register_for_unused_identifiers_check(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkClassExpressionExternalHelpers
    // port: tsc/internal/checker/checker.go:Checker.getFirstTransformableStaticClassElement
    fn check_class_expression_external_helpers(&mut self, node: NodeId) -> Result<(), Error> {
        if self.node(node)?.name().is_some() {
            return Ok(());
        }
        // walkUpOuterExpressions(OEKAll): assignments and comma expressions are
        // not outer wrappers in this native operation.
        let mut parent = self.node(node)?.parent();
        while let Some(outer) = parent {
            if !matches!(
                self.node(outer)?.kind().known(),
                Some(
                    K::ParenthesizedExpression
                        | K::TypeAssertionExpression
                        | K::AsExpression
                        | K::SatisfiesExpression
                        | K::ExpressionWithTypeArguments
                        | K::NonNullExpression
                        | K::PartiallyEmittedExpression
                )
            ) {
                break;
            }
            parent = self.node(outer)?.parent();
        }
        let Some(parent) = parent else {
            return Ok(());
        };
        if !ts_ast::utilities_tail::is_named_evaluation_source(
            self.ast(parent)?,
            &self.node(parent)?,
        )? {
            return Ok(());
        }
        // Class/decorator checks run first and explicitly reject the excluded
        // decorator family. The undecorated static-element branch remains live.
        if self.program()?.host.options().emit_script_target() >= ts_core::ScriptTarget::ESNEXT {
            return Ok(());
        }
        let transform_initializers = !self.program()?.host.options().emit_standard_class_fields();
        let mut location = None;
        for member in self.source_list(node, self.node(node)?.member_list())? {
            let read = self.node(member)?;
            if read.kind() == K::ClassStaticBlockDeclaration
                || read.modifier_flags(self.ast(member)?)? & mf::STATIC != 0
                    && (ts_ast::utilities::is_private_identifier_class_element_declaration(
                        self.ast(member)?,
                        member,
                    )? || transform_initializers
                        && read.kind() == K::PropertyDeclaration
                        && read.initializer().is_some())
            {
                location = Some(member);
                break;
            }
        }
        if let Some(location) = location {
            self.check_external_emit_helpers(
                location,
                crate::external_emit_helpers::SET_FUNCTION_NAME,
            )?;
            let read = self.node(parent)?;
            if matches!(
                read.kind().known(),
                Some(K::PropertyAssignment | K::PropertyDeclaration | K::BindingElement)
            ) {
                if let Some(name) = read.name() {
                    if self.node(name)?.kind() == K::ComputedPropertyName {
                        self.check_external_emit_helpers(
                            location,
                            crate::external_emit_helpers::PROP_KEY,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkThisExpression
    pub(crate) fn check_this_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let mut container = ts_ast::get_this_container(self.ast(node)?, node, true, true)?;
        let mut captured = false;
        let mut computed = false;
        if self.node(container)?.kind() == K::Constructor {
            self.check_this_before_super(node, container, d::X_super_must_be_called_before_accessing_this_in_the_constructor_of_a_derived_class)?;
        }
        loop {
            if self.node(container)?.kind() == K::ArrowFunction {
                container =
                    ts_ast::get_this_container(self.ast(container)?, container, false, !computed)?;
                captured = true;
            }
            if self.node(container)?.kind() == K::ComputedPropertyName {
                container =
                    ts_ast::get_this_container(self.ast(container)?, container, !captured, false)?;
                computed = true;
                continue;
            }
            break;
        }
        self.check_this_static_decorated_initializer(node, container)?;
        if computed {
            self.error_at(
                Some(node),
                d::X_this_cannot_be_referenced_in_a_computed_property_name,
                vec![],
            )?;
        } else {
            match self.node(container)?.kind().known() {
                Some(K::ModuleDeclaration) => {
                    self.error_at(
                        Some(node),
                        d::X_this_cannot_be_referenced_in_a_module_or_namespace_body,
                        vec![],
                    )?;
                }
                Some(K::EnumDeclaration) => {
                    self.error_at(
                        Some(node),
                        d::X_this_cannot_be_referenced_in_current_location,
                        vec![],
                    )?;
                }
                _ => {}
            }
        }
        let ty = self.try_this_type_at(node, true, Some(container))?;
        let options = self.program()?.host.options();
        if options.strict_option_value(options.no_implicit_this) {
            let global = self.get_type_of_symbol(self.builtins.global_this_symbol)?;
            if ty == Some(global) && captured {
                self.error_at(
                    Some(node),
                    d::The_containing_arrow_function_captures_the_global_value_of_this,
                    vec![],
                )?;
            } else if ty.is_none() {
                let diagnostic = self.error_at(
                    Some(node),
                    d::X_this_implicitly_has_type_any_because_it_does_not_have_a_type_annotation,
                    vec![],
                )?;
                if self.node(container)?.kind() != K::SourceFile {
                    if let Some(outside) = self.try_this_type_at(container, true, None)? {
                        if outside != global {
                            let related = self.diagnostic_for_node(
                                Some(container),
                                d::An_outer_value_of_this_is_shadowed_by_this_container,
                                vec![],
                            )?;
                            if let Some(diagnostic) = diagnostic {
                                self.add_related_diagnostic(diagnostic, related)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(ty.unwrap_or(self.builtins.any_type))
    }

    // port: tsc/internal/checker/checker.go:Checker.tryGetThisTypeAtEx
    pub(crate) fn try_this_type_at(
        &mut self,
        node: NodeId,
        include_global: bool,
        container: Option<NodeId>,
    ) -> Result<Option<TypeId>, Error> {
        let container = match container {
            Some(container) => container,
            None => ts_ast::get_this_container(self.ast(node)?, node, false, false)?,
        };
        let read = self.node(container)?;
        if ts_ast::utilities::is_function_like(Some(&read)) {
            let first = self
                .source_list(container, read.parameter_list())?
                .first()
                .copied();
            let explicit = match first {
                Some(parameter) => match self.node(parameter)?.name() {
                    Some(name) => {
                        self.node(name)?.kind() == K::Identifier
                            && self.node_text(name)?.as_bytes() == b"this"
                    }
                    None => false,
                },
                None => false,
            };
            if !self.in_parameter_initializer_before_function(node)? || explicit {
                let signature = self.signature_from_declaration(container)?;
                let parameter = self.signatures.get(signature)?.this_parameter;
                let mut ty = parameter
                    .map(|parameter| self.get_type_of_symbol(parameter))
                    .transpose()?;
                if ty.is_none() {
                    ty = self.contextual_this_parameter_type(container)?;
                }
                if let Some(ty) = ty {
                    return self.flow_type_of_this_reference(node, ty).map(Some);
                }
            }
        }
        if let Some(parent) = self.node(container)?.parent() {
            if ts_ast::utilities::is_class_like(&self.node(parent)?) {
                let symbol =
                    required(self.get_symbol_of_declaration(parent)?, "this class symbol")?;
                let ty = if ts_ast::utilities::is_static(self.ast(container)?, container)? {
                    self.get_type_of_symbol(symbol)?
                } else {
                    let class = self.get_declared_type_of_symbol(symbol)?;
                    required(self.types.interface(class)?.this_type, "class this type")?
                };
                return self.flow_type_of_this_reference(node, ty).map(Some);
            }
        }
        if self.node(container)?.kind() == K::SourceFile {
            if ts_ast::utilities::is_external_module(&*self.source_file_read(container)?) {
                return Ok(Some(self.builtins.undefined_type));
            }
            if include_global {
                return self
                    .get_type_of_symbol(self.builtins.global_this_symbol)
                    .map(Some);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualThisParameterType
    pub(crate) fn contextual_this_parameter_type(
        &mut self,
        function: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.node(function)?;
        let kind = read.kind();
        let javascript = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        if kind == K::ArrowFunction {
            return Ok(None);
        }
        if matches!(
            kind.known(),
            Some(K::FunctionExpression | K::MethodDeclaration)
        ) && self.expression_is_context_sensitive(function)?
        {
            if let Some(signature) = self.contextual_body_signature(function)? {
                if let Some(parameter) = self.signatures.get(signature)?.this_parameter {
                    return self.get_type_of_symbol(parameter).map(Some);
                }
            }
        }
        let options = self.program()?.host.options();
        if javascript || options.strict_option_value(options.no_implicit_this) {
            return self.contextual_this_from_object_or_assignment(function, javascript);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.isInParameterInitializerBeforeContainingFunction
    pub(crate) fn in_parameter_initializer_before_function(
        &self,
        mut node: NodeId,
    ) -> Result<bool, Error> {
        let mut in_binding = false;
        while let Some(parent) = self.node(node)?.parent() {
            let read = self.node(parent)?;
            if ts_ast::utilities::is_function_like(Some(&read)) {
                break;
            }
            if read.kind() == K::Parameter && (in_binding || read.initializer() == Some(node)) {
                return Ok(true);
            }
            if read.kind() == K::BindingElement && read.initializer() == Some(node) {
                in_binding = true;
            }
            node = parent;
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkThisInStaticClassFieldInitializerInDecoratedClass
    fn check_this_static_decorated_initializer(
        &mut self,
        node: NodeId,
        container: NodeId,
    ) -> Result<(), Error> {
        let read = self.node(container)?;
        if read.kind() != K::PropertyDeclaration
            || read.modifier_flags(self.ast(container)?)? & mf::STATIC == 0
            || !self
                .program()?
                .host
                .options()
                .experimental_decorators
                .is_true()
        {
            return Ok(());
        }
        let Some(initializer) = read.initializer() else {
            return Ok(());
        };
        let position = self.node(node)?.pos();
        let initial = self.node(initializer)?;
        if position < initial.pos() || position > initial.end() {
            return Ok(());
        }
        let class = required(self.node(container)?.parent(), "static property class")?;
        for modifier in self.source_list(class, self.node(class)?.modifiers())? {
            if self.node(modifier)?.kind() == K::Decorator {
                self.error_at(
                    Some(node),
                    d::Cannot_use_this_in_a_static_property_initializer_of_a_decorated_class,
                    vec![],
                )?;
                break;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.classDeclarationExtendsNull
    pub(crate) fn class_extends_null(&mut self, class: NodeId) -> Result<bool, Error> {
        let symbol = required(
            self.get_symbol_of_declaration(class)?,
            "class extends-null symbol",
        )?;
        let ty = self.get_declared_type_of_symbol(symbol)?;
        Ok(self.class_base_constructor_type(ty)? == self.builtins.null_widening_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkThisBeforeSuper
    pub(crate) fn check_this_before_super(
        &mut self,
        node: NodeId,
        constructor: NodeId,
        message: &'static d::Message,
    ) -> Result<(), Error> {
        let class = required(self.node(constructor)?.parent(), "constructor class")?;
        if !self
            .class_heritage_nodes(class, K::ExtendsKeyword)?
            .is_empty()
            && !self.class_extends_null(class)?
        {
            if let Some(flow) = self.node_flow(node)? {
                if !self.is_post_super_flow_node(node, flow)? {
                    self.error_at(Some(node), message, vec![])?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/utilities.go:getSuperContainer
    pub(crate) fn super_container(
        &self,
        mut node: NodeId,
        stop_functions: bool,
    ) -> Result<Option<NodeId>, Error> {
        loop {
            let Some(parent) = self.node(node)?.parent() else {
                return Ok(None);
            };
            node = parent;
            match self.node(node)?.kind().known() {
                Some(K::ComputedPropertyName) => {
                    node = required(self.node(node)?.parent(), "computed name parent")?;
                }
                Some(K::FunctionDeclaration | K::FunctionExpression | K::ArrowFunction)
                    if stop_functions =>
                {
                    return Ok(Some(node))
                }
                Some(
                    K::PropertyDeclaration
                    | K::PropertySignature
                    | K::MethodDeclaration
                    | K::MethodSignature
                    | K::Constructor
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::ClassStaticBlockDeclaration,
                ) => return Ok(Some(node)),
                Some(K::Decorator) => {
                    let parent = required(self.node(node)?.parent(), "decorator parent")?;
                    let grand = self.node(parent)?.parent();
                    if self.node(parent)?.kind() == K::Parameter
                        && grand
                            .map(|grand| {
                                self.ast(grand)?
                                    .node(grand)
                                    .map(|read| ts_ast::utilities::is_class_element(&read))
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(false)
                    {
                        node = required(grand, "decorated parameter owner")?;
                    } else if ts_ast::utilities::is_class_element(&self.node(parent)?) {
                        node = parent;
                    }
                }
                _ => {}
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSuperExpression
    pub(crate) fn check_super_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let parent = required(self.node(node)?.parent(), "super expression parent")?;
        let read = self.node(parent)?;
        let is_call = read.kind() == K::CallExpression && read.expression() == Some(node);
        let immediate = self.super_container(node, true)?;
        let mut container = immediate;
        if !is_call {
            while let Some(current) = container {
                if self.node(current)?.kind() != K::ArrowFunction {
                    break;
                }
                container = self.super_container(current, true)?;
            }
        }
        let legal = match container {
            None => false,
            Some(container) if is_call => self.node(container)?.kind() == K::Constructor,
            Some(container) => {
                let read = self.node(container)?;
                if let Some(parent) = read.parent() {
                    if ts_ast::utilities::is_class_like(&self.node(parent)?)
                        || self.node(parent)?.kind() == K::ObjectLiteralExpression
                    {
                        if ts_ast::utilities::is_static(self.ast(container)?, container)? {
                            matches!(
                                read.kind().known(),
                                Some(
                                    K::MethodDeclaration
                                        | K::MethodSignature
                                        | K::GetAccessor
                                        | K::SetAccessor
                                        | K::PropertyDeclaration
                                        | K::ClassStaticBlockDeclaration
                                )
                            )
                        } else {
                            matches!(
                                read.kind().known(),
                                Some(
                                    K::MethodDeclaration
                                        | K::MethodSignature
                                        | K::GetAccessor
                                        | K::SetAccessor
                                        | K::PropertyDeclaration
                                        | K::PropertySignature
                                        | K::Constructor
                                )
                            )
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        };
        if !legal {
            let mut current = Some(node);
            let mut computed = false;
            while let Some(at) = current {
                if Some(at) == container {
                    break;
                }
                if self.node(at)?.kind() == K::ComputedPropertyName {
                    computed = true;
                    break;
                }
                current = self.node(at)?.parent();
            }
            let message = if computed {
                d::X_super_cannot_be_referenced_in_a_computed_property_name
            } else if is_call {
                d::Super_calls_are_not_permitted_outside_constructors_or_in_nested_functions_inside_constructors
            } else {
                let owner = container
                    .map(|container| {
                        self.ast(container)?
                            .node(container)
                            .map(|read| read.parent())
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .flatten();
                let member_owner = owner
                    .map(|owner| {
                        self.ast(owner)?
                            .node(owner)
                            .map(|read| {
                                ts_ast::utilities::is_class_like(&read)
                                    || read.kind() == K::ObjectLiteralExpression
                            })
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
                if member_owner {
                    d::X_super_property_access_is_permitted_only_in_a_constructor_member_function_or_member_accessor_of_a_derived_class
                } else {
                    d::X_super_can_only_be_referenced_in_members_of_derived_classes_or_object_literal_expressions
                }
            };
            self.error_at(Some(node), message, vec![])?;
            return Ok(self.builtins.error_type);
        }
        let container = required(container, "legal super container")?;
        let immediate_constructor = match immediate {
            Some(immediate) => self.node(immediate)?.kind() == K::Constructor,
            None => false,
        };
        if !is_call && immediate_constructor {
            self.check_this_before_super(node, container, d::X_super_must_be_called_before_accessing_a_property_of_super_in_the_constructor_of_a_derived_class)?;
        }
        let class = required(self.node(container)?.parent(), "super class")?;
        if self.node(class)?.kind() == K::ObjectLiteralExpression {
            return Ok(self.builtins.any_type);
        }
        if self
            .class_heritage_nodes(class, K::ExtendsKeyword)?
            .is_empty()
        {
            self.error_at(
                Some(node),
                d::X_super_can_only_be_referenced_in_a_derived_class,
                vec![],
            )?;
            return Ok(self.builtins.error_type);
        }
        if self.class_extends_null(class)? {
            return Ok(if is_call {
                self.builtins.error_type
            } else {
                self.builtins.null_widening_type
            });
        }
        let symbol = required(self.get_symbol_of_declaration(class)?, "super class symbol")?;
        let class_type = self.get_declared_type_of_symbol(symbol)?;
        let bases = self.interface_base_types(class_type)?;
        let Some(&base) = bases.first() else {
            return Ok(self.builtins.error_type);
        };
        if self.node(container)?.kind() == K::Constructor
            && self.in_constructor_argument_initializer(node, container)?
        {
            self.error_at(
                Some(node),
                d::X_super_cannot_be_referenced_in_constructor_arguments,
                vec![],
            )?;
            return Ok(self.builtins.error_type);
        }
        if is_call || ts_ast::utilities::is_static(self.ast(container)?, container)? {
            if !is_call
                && self.program()?.host.options().emit_script_target()
                    <= ts_core::ScriptTarget::ES2021
                && matches!(
                    self.node(container)?.kind().known(),
                    Some(K::PropertyDeclaration | K::ClassStaticBlockDeclaration)
                )
            {
                self.mark_super_static_initializer_scopes(node)?;
            }
            return self.class_base_constructor_type(class_type);
        }
        let this = required(
            self.types.interface(class_type)?.this_type,
            "super this type",
        )?;
        self.get_type_with_this_argument(base, this, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isInConstructorArgumentInitializer
    fn in_constructor_argument_initializer(
        &self,
        mut node: NodeId,
        constructor: NodeId,
    ) -> Result<bool, Error> {
        loop {
            let read = self.node(node)?;
            if ts_ast::utilities::is_function_like(Some(&read)) {
                return Ok(false);
            }
            if read.kind() == K::Parameter && read.parent() == Some(constructor) {
                return Ok(true);
            }
            let Some(parent) = read.parent() else {
                return Ok(false);
            };
            node = parent;
        }
    }
}
