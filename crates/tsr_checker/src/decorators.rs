//! Decorators (docs/PHASE2-C4-plan.md, C4.6 and C4.7): the checks of a
//! decorated declaration, a decorator resolved as a call against the legacy or
//! ES decorator call signature, the ES decorator context types, the decorator
//! grammar, and the type references `emitDecoratorMetadata` marks referenced.
//! The checker records; the decorator and metadata transforms are Phase 3's.
use crate::{
    external_emit_helpers as eh, type_flags as tf, CheckerState, Error, SignatureId, TypeId,
};
use std::sync::Arc;
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{symbol_flags as sf, SyntaxKind as K};
use tsr_diagnostics as d;
use tsr_jsstring::JsString;

/// `CachedTypeKindDecoratorContext*`: the override object type of a member
/// decorator context is cached per name type and these two flags.
const CONTEXT_PRIVATE: u8 = 1;
const CONTEXT_STATIC: u8 = 2;

/// Inline `ast.CanHaveDecorators`.
pub(crate) fn can_have_decorators(kind: tsr_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::Parameter
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassExpression
                | K::ClassDeclaration
        )
    )
}

impl CheckerState {
    /// `c.legacyDecorators`: `experimentalDecorators` is true.
    pub(crate) fn legacy_decorators(&self) -> Result<bool, Error> {
        Ok(self
            .program()?
            .host
            .options()
            .experimental_decorators
            .is_true())
    }

    fn decorated_node_has_decorators(&self, node: NodeId) -> Result<bool, Error> {
        tsr_ast::utilities_middle::has_decorators(self.ast(node)?, &self.node(node)?)
            .map_err(Error::from)
    }

    fn first_decorator(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        for modifier in self.source_list(node, self.node(node)?.modifiers())? {
            if self.node(modifier)?.kind() == K::Decorator {
                return Ok(Some(modifier));
            }
        }
        Ok(None)
    }

    fn decorator_expression(&self, decorator: NodeId) -> Result<NodeId, Error> {
        self.node(decorator)?
            .expression()
            .ok_or(Error::MissingLink("decorator expression"))
    }

    fn decorated_node(&self, decorator: NodeId) -> Result<NodeId, Error> {
        self.node(decorator)?
            .parent()
            .ok_or(Error::MissingLink("decorator parent"))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkDecorators
    pub(crate) fn check_decorators(&mut self, node: NodeId) -> Result<(), Error> {
        // Nodes that cannot have decorators already had an error reported by
        // checkGrammarModifiers.
        let read = self.node(node)?;
        let kind = read.kind();
        let parent = read.parent();
        let grandparent = match parent {
            Some(parent) => self.node(parent)?.parent(),
            None => None,
        };
        let legacy = self.legacy_decorators()?;
        if !can_have_decorators(kind)
            || !self.decorated_node_has_decorators(node)?
            || !tsr_ast::utilities_class::node_can_be_decorated(
                self.ast(node)?,
                legacy,
                node,
                parent,
                grandparent,
            )?
        {
            return Ok(());
        }
        let Some(first) = self.first_decorator(node)? else {
            return Ok(());
        };
        if legacy {
            self.check_external_emit_helpers(first, eh::DECORATE)?;
            if kind == K::Parameter {
                self.check_external_emit_helpers(first, eh::PARAM)?;
            }
        } else if self.program()?.host.options().emit_script_target()
            < tsr_core::ScriptTarget::ESNEXT
        {
            // LanguageFeatureMinimumTarget.ClassAndClassElementDecorators is ESNext.
            self.check_external_emit_helpers(first, eh::ES_DECORATE_AND_RUN_INITIALIZERS)?;
            if kind == K::ClassDeclaration {
                if self.node(node)?.name().is_none()
                    || self
                        .first_transformable_static_class_element(node)?
                        .is_some()
                {
                    self.check_external_emit_helpers(first, eh::SET_FUNCTION_NAME)?;
                }
            } else if kind != K::ClassExpression {
                if let Some(name) = self.node(node)?.name() {
                    let name_kind = self.node(name)?.kind();
                    if name_kind == K::PrivateIdentifier
                        && (matches!(
                            kind.known(),
                            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
                        ) || tsr_ast::utilities::is_auto_accessor_property_declaration(
                            self.ast(node)?,
                            node,
                        )?)
                    {
                        self.check_external_emit_helpers(first, eh::SET_FUNCTION_NAME)?;
                    }
                    if name_kind == K::ComputedPropertyName {
                        self.check_external_emit_helpers(first, eh::PROP_KEY)?;
                    }
                }
            }
        }
        self.mark_linked_references(
            node,
            crate::linked_references::ReferenceHint::Decorator,
            None,
            None,
        )?;
        for modifier in self.source_list(node, self.node(node)?.modifiers())? {
            if self.node(modifier)?.kind() == K::Decorator {
                self.check_decorator(modifier)?;
            }
        }
        Ok(())
    }

    #[allow(
        clippy::match_same_arms,
        reason = "Keep the pinned switch and its property fallthrough auditable"
    )]
    // port: tsc/internal/checker/checker.go:Checker.checkDecorator
    fn check_decorator(&mut self, decorator: NodeId) -> Result<(), Error> {
        self.check_grammar_decorator(decorator)?;
        let signature = self.resolved_call_signature(decorator)?;
        self.check_deprecated_signature(signature, decorator)?;
        let return_type = self.return_type_of_signature(signature)?;
        if self.types.flags(return_type)? & tf::ANY != 0 {
            return Ok(());
        }
        // Without a signature and return type a grammar error was already reported.
        let Some(decorator_signature) = self.decorator_call_signature(decorator)? else {
            return Ok(());
        };
        let Some(expected) = self
            .signatures
            .get(decorator_signature)?
            .resolved_return_type
        else {
            return Ok(());
        };
        let parent = self.decorated_node(decorator)?;
        let head = match self.node(parent)?.kind().known() {
            Some(K::ClassDeclaration | K::ClassExpression) => {
                d::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            Some(K::PropertyDeclaration) if !self.legacy_decorators()? => {
                d::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            Some(K::PropertyDeclaration | K::Parameter) => {
                d::Decorator_function_return_type_is_0_but_is_expected_to_be_void_or_any
            }
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => {
                d::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            _ => {
                return Err(Error::MissingLink(
                    "checkDecorator: unhandled decorated node",
                ))
            }
        };
        let expression = self.decorator_expression(decorator)?;
        let (_, diagnostic) = self.check_type_related_ex(
            return_type,
            expected,
            crate::RelationKind::Assignable,
            Some(expression),
            Some(head),
        )?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveDecorator
    pub(crate) fn resolve_decorator(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        let parent = self.decorated_node(node)?;
        if !can_have_decorators(self.node(parent)?.kind()) {
            return self.resolve_error_call(node);
        }
        let expression = self.decorator_expression(node)?;
        let function_type = self.check_expression(expression)?;
        let apparent = self.apparent_type(function_type)?;
        if self.is_error_type(apparent)? {
            return self.resolve_error_call(node);
        }
        let calls = self.signatures_of_type(apparent, false)?;
        let construct_count = self.signatures_of_type(apparent, true)?.len();
        if self.is_untyped_function_call(function_type, apparent, calls.len(), construct_count)? {
            return self.resolve_untyped_call(node);
        }
        if self.is_potentially_uncalled_decorator(node, &calls)?
            && self.node(expression)?.kind() != K::ParenthesizedExpression
        {
            let text = self.node_text(expression)?.into_js_string();
            self.error_at(
                Some(node),
                d::X_0_accepts_too_few_arguments_to_be_used_as_a_decorator_here_Did_you_mean_to_call_it_first_and_write_0,
                vec![text],
            )?;
            return self.resolve_error_call(node);
        }
        let head = self.decorator_resolution_head_message(node)?;
        if calls.is_empty() {
            self.call_invocation_error_ex(node, apparent, false, Some(head))?;
            return self.resolve_error_call(node);
        }
        if self.decorator_call_signature(node)?.is_none() {
            return self.resolve_error_call(node);
        }
        self.resolve_typed_call(node, &calls)
    }

    /// `resolveErrorCall`: check the arguments untyped, then the unknown signature.
    pub(crate) fn resolve_error_call(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        self.resolve_untyped_call(node)?;
        Ok(self.builtins.unknown_signature)
    }

    /// A decorator that could accept zero arguments but receives more as a
    /// decorator: the user may have meant to call it first.
    // port: tsc/internal/checker/checker.go:Checker.isPotentiallyUncalledDecorator
    fn is_potentially_uncalled_decorator(
        &mut self,
        decorator: NodeId,
        signatures: &[SignatureId],
    ) -> Result<bool, Error> {
        if signatures.is_empty() {
            return Ok(false);
        }
        for &signature in signatures {
            let data = self.signatures.get(signature)?;
            let has_rest = data.flags & crate::signature_flags::HAS_REST_PARAMETER != 0;
            let minimum = data.min_argument_count;
            let parameters = data.parameters.as_ref().map_or(0, |list| list.len());
            if minimum != 0
                || has_rest
                || parameters >= self.decorator_argument_count(decorator, signature)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The head message of errors when resolving a decorator as a call.
    // port: tsc/internal/checker/checker.go:Checker.getDiagnosticHeadMessageForDecoratorResolution
    pub(crate) fn decorator_resolution_head_message(
        &self,
        node: NodeId,
    ) -> Result<&'static tsr_diagnostics::Message, Error> {
        let parent = self.decorated_node(node)?;
        Ok(match self.node(parent)?.kind().known() {
            Some(K::ClassDeclaration | K::ClassExpression) => {
                d::Unable_to_resolve_signature_of_class_decorator_when_called_as_an_expression
            }
            Some(K::Parameter) => {
                d::Unable_to_resolve_signature_of_parameter_decorator_when_called_as_an_expression
            }
            Some(K::PropertyDeclaration) => {
                d::Unable_to_resolve_signature_of_property_decorator_when_called_as_an_expression
            }
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => {
                d::Unable_to_resolve_signature_of_method_decorator_when_called_as_an_expression
            }
            _ => {
                return Err(Error::MissingLink(
                    "getDiagnosticHeadMessageForDecoratorResolution: unhandled decorated node",
                ))
            }
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getDecoratorArgumentCount
    pub(crate) fn decorator_argument_count(
        &mut self,
        node: NodeId,
        signature: SignatureId,
    ) -> Result<usize, Error> {
        if self.legacy_decorators()? {
            return self.legacy_decorator_argument_count(node, signature);
        }
        Ok(self.parameter_count(signature)?.clamp(1, 2))
    }

    /// The argument count of a decorator that works like a function invocation.
    // port: tsc/internal/checker/checker.go:Checker.getLegacyDecoratorArgumentCount
    fn legacy_decorator_argument_count(
        &mut self,
        node: NodeId,
        signature: SignatureId,
    ) -> Result<usize, Error> {
        let parent = self.decorated_node(node)?;
        Ok(match self.node(parent)?.kind().known() {
            Some(K::ClassDeclaration | K::ClassExpression) => 1,
            Some(K::PropertyDeclaration) => {
                if tsr_ast::utilities::has_accessor_modifier(self.ast(parent)?, parent)? {
                    3
                } else {
                    2
                }
            }
            // For decorators with only two parameters we supply only two arguments.
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => {
                if self.parameter_count(signature)? <= 2 {
                    2
                } else {
                    3
                }
            }
            Some(K::Parameter) => 3,
            _ => {
                return Err(Error::MissingLink(
                    "getLegacyDecoratorArgumentCount: unhandled decorated node",
                ))
            }
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectiveDecoratorArguments
    pub(crate) fn effective_decorator_arguments(
        &mut self,
        node: NodeId,
    ) -> Result<Vec<NodeId>, Error> {
        let expression = self.decorator_expression(node)?;
        let signature = self
            .decorator_call_signature(node)?
            .ok_or(Error::MissingLink("decorator signature not found"))?;
        let parameters = self
            .signatures
            .get(signature)?
            .parameters
            .clone()
            .unwrap_or_else(|| [].into());
        let mut args = Vec::with_capacity(parameters.len());
        for &parameter in parameters.iter() {
            let ty = self.get_type_of_symbol(parameter)?;
            args.push(self.synthetic_call_argument(expression, ty, false, None)?);
        }
        Ok(args)
    }

    // port: tsc/internal/checker/checker.go:Checker.getDecoratorCallSignature
    pub(crate) fn decorator_call_signature(
        &mut self,
        decorator: NodeId,
    ) -> Result<Option<SignatureId>, Error> {
        if self.legacy_decorators()? {
            self.legacy_decorator_call_signature(decorator)
        } else {
            self.es_decorator_call_signature(decorator)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getLegacyDecoratorCallSignature
    fn legacy_decorator_call_signature(
        &mut self,
        decorator: NodeId,
    ) -> Result<Option<SignatureId>, Error> {
        let node = self.decorated_node(decorator)?;
        if let Some(&cached) = self.calls.decorator_signatures.get(&node) {
            return Ok((cached != self.builtins.any_signature).then_some(cached));
        }
        let any = self.builtins.any_signature;
        self.calls.decorator_signatures.insert(node, any);
        let read = self.node(node)?;
        let parent = read.parent();
        let kind = read.kind();
        let signature = match kind.known() {
            Some(K::ClassDeclaration | K::ClassExpression) => {
                // The target is the static side of the class.
                let symbol = self
                    .get_symbol_of_declaration(node)?
                    .ok_or(Error::MissingLink("decorated class symbol"))?;
                let target = self.get_type_of_symbol(symbol)?;
                let target_parameter = self.new_parameter(b"target", target)?;
                let void = self.builtins.void_type;
                let returned = self.get_union_type(&[target, void])?;
                Some(self.new_call_signature(
                    None,
                    None,
                    Some([target_parameter].into()),
                    returned,
                )?)
            }
            Some(K::Parameter) => {
                let function = parent.ok_or(Error::MissingLink("decorated parameter parent"))?;
                let function_read = self.node(function)?;
                let function_kind = function_read.kind();
                let class_member = matches!(
                    function_kind.known(),
                    Some(K::MethodDeclaration | K::SetAccessor)
                ) && match function_read.parent() {
                    Some(class) => tsr_ast::utilities::is_class_like(&self.node(class)?),
                    None => false,
                };
                let this_parameter =
                    tsr_ast::utilities_class::get_this_parameter(self.ast(function)?, function)?;
                if function_kind != K::Constructor && !class_member || this_parameter == Some(node)
                {
                    None
                } else {
                    let parameters = self.source_list(function, function_read.parameter_list())?;
                    let position = parameters
                        .iter()
                        .position(|&parameter| parameter == node)
                        .ok_or(Error::MissingLink("decorated parameter index"))?;
                    let index = position - usize::from(this_parameter.is_some());
                    // A parameter decorator has three arguments (ParameterDecorator in core.d.ts).
                    let (target, key) = if function_kind == K::Constructor {
                        let class = function_read
                            .parent()
                            .ok_or(Error::MissingLink("decorated constructor class"))?;
                        let symbol = self
                            .get_symbol_of_declaration(class)?
                            .ok_or(Error::MissingLink("decorated constructor class symbol"))?;
                        (
                            self.get_type_of_symbol(symbol)?,
                            self.builtins.undefined_type,
                        )
                    } else {
                        (
                            self.parent_type_of_class_element(function)?,
                            self.class_element_property_key_type(function)?,
                        )
                    };
                    #[allow(clippy::cast_precision_loss, reason = "a parameter index is small")]
                    let index_type =
                        self.get_number_literal_type(tsr_jsnum::Number::new(index as f64))?;
                    let target_parameter = self.new_parameter(b"target", target)?;
                    let key_parameter = self.new_parameter(b"propertyKey", key)?;
                    let index_parameter = self.new_parameter(b"parameterIndex", index_type)?;
                    let void = self.builtins.void_type;
                    Some(self.new_call_signature(
                        None,
                        None,
                        Some([target_parameter, key_parameter, index_parameter].into()),
                        void,
                    )?)
                }
            }
            Some(
                K::MethodDeclaration | K::GetAccessor | K::SetAccessor | K::PropertyDeclaration,
            ) => {
                let class_like = match parent {
                    Some(class) => tsr_ast::utilities::is_class_like(&self.node(class)?),
                    None => false,
                };
                if class_like {
                    // A method or accessor decorator has two or three arguments
                    // (PropertyDecorator and MethodDecorator in core.d.ts).
                    let target = self.parent_type_of_class_element(node)?;
                    let target_parameter = self.new_parameter(b"target", target)?;
                    let key = self.class_element_property_key_type(node)?;
                    let key_parameter = self.new_parameter(b"propertyKey", key)?;
                    let property = kind == K::PropertyDeclaration;
                    let returned = if property {
                        self.builtins.void_type
                    } else {
                        let ty = self.get_type_at_location(node)?;
                        self.typed_property_descriptor_type(ty)?
                    };
                    let has_descriptor = !property
                        || tsr_ast::utilities::has_accessor_modifier(self.ast(node)?, node)?;
                    let void = self.builtins.void_type;
                    if has_descriptor {
                        let ty = self.get_type_at_location(node)?;
                        let descriptor = self.typed_property_descriptor_type(ty)?;
                        let descriptor_parameter = self.new_parameter(b"descriptor", descriptor)?;
                        let returned = self.get_union_type(&[returned, void])?;
                        Some(self.new_call_signature(
                            None,
                            None,
                            Some([target_parameter, key_parameter, descriptor_parameter].into()),
                            returned,
                        )?)
                    } else {
                        let returned = self.get_union_type(&[returned, void])?;
                        Some(self.new_call_signature(
                            None,
                            None,
                            Some([target_parameter, key_parameter].into()),
                            returned,
                        )?)
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(signature) = signature {
            self.calls.decorator_signatures.insert(node, signature);
        }
        Ok(signature)
    }

    /// The ES decorator call signature: `(target, context) => output | void`,
    /// where the target and output describe the decorated value and the
    /// context type describes the decorated element.
    // port: tsc/internal/checker/checker.go:Checker.getESDecoratorCallSignature
    fn es_decorator_call_signature(
        &mut self,
        decorator: NodeId,
    ) -> Result<Option<SignatureId>, Error> {
        let node = self.decorated_node(decorator)?;
        if let Some(&cached) = self.calls.decorator_signatures.get(&node) {
            return Ok((cached != self.builtins.any_signature).then_some(cached));
        }
        let any = self.builtins.any_signature;
        self.calls.decorator_signatures.insert(node, any);
        let read = self.node(node)?;
        let parent = read.parent();
        let kind = read.kind();
        let class_like = match parent {
            Some(class) => tsr_ast::utilities::is_class_like(&self.node(class)?),
            None => false,
        };
        let signature = match kind.known() {
            Some(K::ClassDeclaration | K::ClassExpression) => {
                let symbol = self
                    .get_symbol_of_declaration(node)?
                    .ok_or(Error::MissingLink("decorated class symbol"))?;
                let target = self.get_type_of_symbol(symbol)?;
                let context = self.class_decorator_context_type(target)?;
                Some(self.new_es_decorator_call_signature(target, context, target)?)
            }
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) if class_like => {
                let value = if kind == K::MethodDeclaration {
                    let signature = self.signature_from_declaration(node)?;
                    self.isolated_signature_type(signature)?
                } else {
                    self.get_type_at_location(node)?
                };
                let this = self.decorator_this_type(node)?;
                let target = match kind.known() {
                    Some(K::GetAccessor) => self.getter_function_type(value)?,
                    Some(K::SetAccessor) => self.setter_function_type(value)?,
                    _ => value,
                };
                let context =
                    self.class_member_decorator_context_type_for_node(node, this, value)?;
                Some(self.new_es_decorator_call_signature(target, context, target)?)
            }
            Some(K::PropertyDeclaration) if class_like => {
                let value = self.get_type_at_location(node)?;
                let this = self.decorator_this_type(node)?;
                let accessor = tsr_ast::utilities::has_accessor_modifier(self.ast(node)?, node)?;
                let target = if accessor {
                    self.class_accessor_decorator_target_type(this, value)?
                } else {
                    self.builtins.undefined_type
                };
                let returned = if accessor {
                    self.class_accessor_decorator_result_type(this, value)?
                } else {
                    self.class_field_decorator_initializer_mutator_type(this, value)?
                };
                let context =
                    self.class_member_decorator_context_type_for_node(node, this, value)?;
                Some(self.new_es_decorator_call_signature(target, context, returned)?)
            }
            _ => None,
        };
        if let Some(signature) = signature {
            self.calls.decorator_signatures.insert(node, signature);
        }
        Ok(signature)
    }

    /// The `this` type of a decorated class member: the static side for a
    /// static member, the declared instance type otherwise.
    fn decorator_this_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let class = self
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("decorated member class"))?;
        let symbol = self
            .get_symbol_of_declaration(class)?
            .ok_or(Error::MissingLink("decorated member class symbol"))?;
        if tsr_ast::utilities::has_static_modifier(self.ast(node)?, node)? {
            self.get_type_of_symbol(symbol)
        } else {
            self.declared_interface_type(symbol)
        }
    }

    /// A lib generic instantiated with the context arguments, or the unknown
    /// type when the lib lacks the generic (`tryCreateTypeReference`).
    fn decorator_lib_reference(
        &mut self,
        name: &'static str,
        arity: usize,
        arguments: &[TypeId],
    ) -> Result<TypeId, Error> {
        let target = self.cached_global_generic_type(name, arity)?;
        // port: tsc/internal/checker/checker.go:Checker.tryCreateTypeReference
        if !arguments.is_empty() && target == self.builtins.empty_generic_type {
            return Ok(self.builtins.unknown_type);
        }
        self.create_type_reference(target, arguments)
    }

    /// `getGlobalTypeResolver(name, arity, true)`: resolved once and cached.
    fn cached_global_generic_type(
        &mut self,
        name: &'static str,
        arity: usize,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.query.global_types.get(name) {
            return Ok(ty);
        }
        let ty = self.get_global_type(name, arity, true)?;
        self.query.global_types.insert(name, ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassDecoratorContextType
    fn class_decorator_context_type(&mut self, class: TypeId) -> Result<TypeId, Error> {
        self.decorator_lib_reference("ClassDecoratorContext", 1, &[class])
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassMethodDecoratorContextType
    // port: tsc/internal/checker/checker.go:Checker.newClassGetterDecoratorContextType
    // port: tsc/internal/checker/checker.go:Checker.newClassSetterDecoratorContextType
    // port: tsc/internal/checker/checker.go:Checker.newClassAccessorDecoratorContextType
    // port: tsc/internal/checker/checker.go:Checker.newClassFieldDecoratorContextType
    fn class_member_decorator_context_type(
        &mut self,
        name: &'static str,
        this: TypeId,
        value: TypeId,
    ) -> Result<TypeId, Error> {
        self.decorator_lib_reference(name, 2, &[this, value])
    }

    /// The `{ name, private, static }` object a member context intersects,
    /// cached per name type and flags.
    // port: tsc/internal/checker/checker.go:Checker.getClassMemberDecoratorContextOverrideType
    fn class_member_decorator_context_override_type(
        &mut self,
        name_type: TypeId,
        private: bool,
        is_static: bool,
    ) -> Result<TypeId, Error> {
        let kind =
            if private { CONTEXT_PRIVATE } else { 0 } | if is_static { CONTEXT_STATIC } else { 0 };
        if let Some(&ty) = self
            .query
            .decorator_context_overrides
            .get(&(kind, name_type))
        {
            return Ok(ty);
        }
        let mut members = tsr_ast::SymbolTable::default();
        let name = self.new_property(b"name", name_type)?;
        members.insert(JsString::from_bytes(b"name".as_slice()), Some(name));
        let private_type = if private {
            self.builtins.true_type
        } else {
            self.builtins.false_type
        };
        let private_symbol = self.new_property(b"private", private_type)?;
        members.insert(
            JsString::from_bytes(b"private".as_slice()),
            Some(private_symbol),
        );
        let static_type = if is_static {
            self.builtins.true_type
        } else {
            self.builtins.false_type
        };
        let static_symbol = self.new_property(b"static", static_type)?;
        members.insert(
            JsString::from_bytes(b"static".as_slice()),
            Some(static_symbol),
        );
        let members = self.alloc_symbol_table(members);
        let ty = self.new_anonymous_type(None, Some(members), &[], &[], &[])?;
        self.query
            .decorator_context_overrides
            .insert((kind, name_type), ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassMemberDecoratorContextTypeForNode
    fn class_member_decorator_context_type_for_node(
        &mut self,
        node: NodeId,
        this: TypeId,
        value: TypeId,
    ) -> Result<TypeId, Error> {
        let view = self.ast(node)?;
        let is_static = tsr_ast::utilities::has_static_modifier(view, node)?;
        let name = self
            .node(node)?
            .name()
            .ok_or(Error::MissingLink("decorated member name"))?;
        let private = self.node(name)?.kind() == K::PrivateIdentifier;
        let name_type = if private {
            let text = self.node_text(name)?.into_js_string();
            self.get_string_literal_type(text)?
        } else {
            self.literal_type_from_property_name(name)?
        };
        let context = match self.node(node)?.kind().known() {
            Some(K::MethodDeclaration) => self.class_member_decorator_context_type(
                "ClassMethodDecoratorContext",
                this,
                value,
            )?,
            Some(K::GetAccessor) => self.class_member_decorator_context_type(
                "ClassGetterDecoratorContext",
                this,
                value,
            )?,
            Some(K::SetAccessor) => self.class_member_decorator_context_type(
                "ClassSetterDecoratorContext",
                this,
                value,
            )?,
            Some(K::PropertyDeclaration)
                if tsr_ast::utilities::is_auto_accessor_property_declaration(
                    self.ast(node)?,
                    node,
                )? =>
            {
                self.class_member_decorator_context_type(
                    "ClassAccessorDecoratorContext",
                    this,
                    value,
                )?
            }
            Some(K::PropertyDeclaration) => {
                self.class_member_decorator_context_type("ClassFieldDecoratorContext", this, value)?
            }
            _ => {
                return Err(Error::MissingLink(
                    "createClassMemberDecoratorContextTypeForNode: unhandled member",
                ))
            }
        };
        let override_type =
            self.class_member_decorator_context_override_type(name_type, private, is_static)?;
        self.get_intersection_type(&[context, override_type])
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassAccessorDecoratorTargetType
    fn class_accessor_decorator_target_type(
        &mut self,
        this: TypeId,
        value: TypeId,
    ) -> Result<TypeId, Error> {
        self.decorator_lib_reference("ClassAccessorDecoratorTarget", 2, &[this, value])
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassAccessorDecoratorResultType
    fn class_accessor_decorator_result_type(
        &mut self,
        this: TypeId,
        value: TypeId,
    ) -> Result<TypeId, Error> {
        self.decorator_lib_reference("ClassAccessorDecoratorResult", 2, &[this, value])
    }

    // port: tsc/internal/checker/checker.go:Checker.newClassFieldDecoratorInitializerMutatorType
    fn class_field_decorator_initializer_mutator_type(
        &mut self,
        this: TypeId,
        value: TypeId,
    ) -> Result<TypeId, Error> {
        let this_parameter = self.new_parameter(b"this", this)?;
        let value_parameter = self.new_parameter(b"value", value)?;
        self.function_type_of(Some(this_parameter), &[value_parameter], value)
    }

    // port: tsc/internal/checker/checker.go:Checker.newESDecoratorCallSignature
    fn new_es_decorator_call_signature(
        &mut self,
        target: TypeId,
        context: TypeId,
        non_optional_return: TypeId,
    ) -> Result<SignatureId, Error> {
        let target_parameter = self.new_parameter(b"target", target)?;
        let context_parameter = self.new_parameter(b"context", context)?;
        let void = self.builtins.void_type;
        let returned = self.get_union_type(&[non_optional_return, void])?;
        self.new_call_signature(
            None,
            None,
            Some([target_parameter, context_parameter].into()),
            returned,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.newFunctionType
    fn function_type_of(
        &mut self,
        this_parameter: Option<SymbolId>,
        parameters: &[SymbolId],
        returned: TypeId,
    ) -> Result<TypeId, Error> {
        let parameters = (!parameters.is_empty()).then(|| parameters.into());
        let signature = self.new_call_signature(None, this_parameter, parameters, returned)?;
        self.isolated_signature_type(signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.newGetterFunctionType
    fn getter_function_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.function_type_of(None, &[], ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.newSetterFunctionType
    fn setter_function_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let value = self.new_parameter(b"value", ty)?;
        let void = self.builtins.void_type;
        self.function_type_of(None, &[value], void)
    }

    // port: tsc/internal/checker/checker.go:Checker.newTypedPropertyDescriptorType
    fn typed_property_descriptor_type(&mut self, property: TypeId) -> Result<TypeId, Error> {
        let global = self.cached_global_generic_type("TypedPropertyDescriptor", 1)?;
        self.type_from_generic_global(global, property)
    }

    // port: tsc/internal/checker/checker.go:Checker.newParameter
    pub(crate) fn new_parameter(&mut self, name: &[u8], ty: TypeId) -> Result<SymbolId, Error> {
        let symbol = self.new_symbol(
            sf::FUNCTION_SCOPED_VARIABLE,
            JsString::from_bytes(name.to_vec()),
        )?;
        let key = self.value_symbol_key(symbol)?;
        self.value_symbol_links.get_or_default(key).resolved_type = Some(ty);
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.newProperty
    pub(crate) fn new_property(&mut self, name: &[u8], ty: TypeId) -> Result<SymbolId, Error> {
        let symbol = self.new_symbol(sf::PROPERTY, JsString::from_bytes(name.to_vec()))?;
        let key = self.value_symbol_key(symbol)?;
        self.value_symbol_links.get_or_default(key).resolved_type = Some(ty);
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.getParentTypeOfClassElement
    fn parent_type_of_class_element(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let class = self
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("class element parent"))?;
        let symbol = self
            .get_symbol_of_declaration(class)?
            .ok_or(Error::MissingLink("class element class symbol"))?;
        if tsr_ast::utilities::has_static_modifier(self.ast(node)?, node)? {
            self.get_type_of_symbol(symbol)
        } else {
            self.get_declared_type_of_symbol(symbol)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getClassElementPropertyKeyType
    fn class_element_property_key_type(&mut self, element: NodeId) -> Result<TypeId, Error> {
        let name = self
            .node(element)?
            .name()
            .ok_or(Error::MissingLink("class element name"))?;
        match self.node(name)?.kind().known() {
            Some(K::Identifier | K::NumericLiteral | K::StringLiteral) => {
                let text = self.node_text(name)?.into_js_string();
                self.get_string_literal_type(text)
            }
            Some(K::ComputedPropertyName) => {
                let name_type = self.check_computed_property_name(name)?;
                if self.type_assignable_to_kind(name_type, tf::ES_SYMBOL_LIKE)? {
                    Ok(name_type)
                } else {
                    Ok(self.builtins.string_type)
                }
            }
            _ => Ok(self.builtins.error_type),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForDecorator
    pub(crate) fn contextual_type_for_decorator(
        &mut self,
        decorator: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        match self.decorator_call_signature(decorator)? {
            Some(signature) => self.isolated_signature_type(signature).map(Some),
            None => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getFirstTransformableStaticClassElement
    fn first_transformable_static_class_element(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let options = self.program()?.host.options();
        let target = options.emit_script_target();
        let emit_standard_class_fields = options.emit_standard_class_fields();
        let legacy = self.legacy_decorators()?;
        let decorated = tsr_ast::utilities_class::class_or_constructor_parameter_is_decorated(
            self.ast(node)?,
            false,
            node,
        )?;
        let transform_static_elements_of_decorated_class =
            !legacy && target < tsr_core::ScriptTarget::ESNEXT && decorated;
        let transform_private_elements_or_static_blocks =
            target < tsr_core::ScriptTarget::ES2022 || target < tsr_core::ScriptTarget::ESNEXT;
        let transform_initializers = !emit_standard_class_fields;
        if !transform_static_elements_of_decorated_class
            && !transform_private_elements_or_static_blocks
        {
            return Ok(None);
        }
        for member in self.source_list(node, self.node(node)?.member_list())? {
            if transform_static_elements_of_decorated_class
                && tsr_ast::utilities_class::class_element_or_class_element_parameter_is_decorated(
                    self.ast(member)?,
                    false,
                    member,
                    Some(node),
                )?
            {
                return Ok(Some(self.first_decorator(node)?.unwrap_or(node)));
            } else if transform_private_elements_or_static_blocks {
                let read = self.node(member)?;
                if read.kind() == K::ClassStaticBlockDeclaration {
                    return Ok(Some(member));
                }
                if tsr_ast::utilities::has_static_modifier(self.ast(member)?, member)?
                    && (tsr_ast::utilities::is_private_identifier_class_element_declaration(
                        self.ast(member)?,
                        member,
                    )? || transform_initializers
                        && tsr_ast::utilities_middle::is_initialized_property(&read))
                {
                    return Ok(Some(member));
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarDecorator
    fn check_grammar_decorator(&mut self, decorator: NodeId) -> Result<bool, Error> {
        let view = self.ast(decorator)?;
        let source = tsr_ast::utilities::get_source_file_of_node(view, Some(decorator))?
            .ok_or(Error::MissingLink("decorator source file"))?;
        if !view.source_file(source)?.diagnostics().is_empty() {
            return Ok(false);
        }
        let mut node = self.decorator_expression(decorator)?;
        // DecoratorParenthesizedExpression : `(` Expression `)`
        if self.node(node)?.kind() == K::ParenthesizedExpression {
            return Ok(false);
        }
        let mut can_have_call_expression = true;
        let mut error_node = None;
        loop {
            let read = self.node(node)?;
            // Allow TS syntax such as non-null assertions and instantiation expressions.
            if matches!(
                read.kind().known(),
                Some(K::ExpressionWithTypeArguments | K::NonNullExpression)
            ) {
                node = read
                    .expression()
                    .ok_or(Error::MissingLink("decorator operand"))?;
                continue;
            }
            // DecoratorCallExpression : DecoratorMemberExpression Arguments
            if read.kind() == K::CallExpression {
                if !can_have_call_expression {
                    error_node = Some(node);
                }
                if let Some(question) = read.question_dot_token() {
                    // Error at the `?.` token since it appears earlier.
                    error_node = Some(question);
                }
                node = read
                    .expression()
                    .ok_or(Error::MissingLink("decorator call target"))?;
                can_have_call_expression = false;
                continue;
            }
            // DecoratorMemberExpression : IdentifierReference, `.` IdentifierName, `.` PrivateIdentifier
            if read.kind() == K::PropertyAccessExpression {
                if let Some(question) = read.question_dot_token() {
                    error_node = Some(question);
                }
                node = read
                    .expression()
                    .ok_or(Error::MissingLink("decorator member target"))?;
                can_have_call_expression = false;
                continue;
            }
            if read.kind() != K::Identifier {
                // Error at this node since it appears earlier.
                error_node = Some(node);
            }
            break;
        }
        if let Some(error_node) = error_node {
            let expression = self.decorator_expression(decorator)?;
            let mut diagnostic = self.diagnostic_for_node(
                Some(expression),
                d::Expression_must_be_enclosed_in_parentheses_to_be_used_as_a_decorator,
                vec![],
            )?;
            diagnostic
                .related_information
                .push(Arc::new(self.diagnostic_for_node(
                    Some(error_node),
                    d::Invalid_syntax_in_decorator,
                    vec![],
                )?));
            self.add_diagnostic(diagnostic)?;
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.markDecoratorAliasReferenced
    pub(crate) fn mark_decorator_alias_referenced(&mut self, node: NodeId) -> Result<(), Error> {
        if !self
            .program()?
            .host
            .options()
            .emit_decorator_metadata
            .is_true()
        {
            return Ok(());
        }
        let Some(first) = self.first_decorator(node)? else {
            return Ok(());
        };
        self.check_external_emit_helpers(first, eh::METADATA)?;
        // Only needed when emitting serialized type metadata for a decorator's target.
        let kind = self.node(node)?.kind();
        match kind.known() {
            Some(K::ClassDeclaration) => {
                if let Some(constructor) =
                    tsr_ast::utilities_class::get_first_constructor_with_body(
                        self.ast(node)?,
                        node,
                    )?
                {
                    for parameter in
                        self.source_list(constructor, self.node(constructor)?.parameter_list())?
                    {
                        let type_node = self.parameter_type_node_for_decorator_check(parameter)?;
                        self.mark_decorator_metadata_type_node_as_referenced(type_node)?;
                    }
                }
            }
            Some(K::GetAccessor | K::SetAccessor) => {
                let other = if kind == K::SetAccessor {
                    K::GetAccessor
                } else {
                    K::SetAccessor
                };
                let symbol = self
                    .get_symbol_of_declaration(node)?
                    .ok_or(Error::MissingLink("decorated accessor symbol"))?;
                let other_accessor = self.declaration_of_kind(symbol, other)?;
                let mut annotation = self.annotated_accessor_type_node(Some(node))?;
                if annotation.is_none() && other_accessor.is_some() {
                    annotation = self.annotated_accessor_type_node(other_accessor)?;
                }
                self.mark_decorator_metadata_type_node_as_referenced(annotation)?;
            }
            Some(K::MethodDeclaration) => {
                for parameter in self.source_list(node, self.node(node)?.parameter_list())? {
                    let type_node = self.parameter_type_node_for_decorator_check(parameter)?;
                    self.mark_decorator_metadata_type_node_as_referenced(type_node)?;
                }
                let returned = self.node(node)?.type_node();
                self.mark_decorator_metadata_type_node_as_referenced(returned)?;
            }
            Some(K::PropertyDeclaration) => {
                let annotation = self.node(node)?.type_node();
                self.mark_decorator_metadata_type_node_as_referenced(annotation)?;
            }
            Some(K::Parameter) => {
                let type_node = self.parameter_type_node_for_decorator_check(node)?;
                self.mark_decorator_metadata_type_node_as_referenced(type_node)?;
                let signature = self
                    .node(node)?
                    .parent()
                    .ok_or(Error::MissingLink("decorated parameter signature"))?;
                for parameter in
                    self.source_list(signature, self.node(signature)?.parameter_list())?
                {
                    let type_node = self.parameter_type_node_for_decorator_check(parameter)?;
                    self.mark_decorator_metadata_type_node_as_referenced(type_node)?;
                }
                let returned = self.node(signature)?.type_node();
                self.mark_decorator_metadata_type_node_as_referenced(returned)?;
            }
            _ => {}
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getParameterTypeNodeForDecoratorCheck
    fn parameter_type_node_for_decorator_check(
        &self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let read = self.node(node)?;
        let type_node = read.type_node();
        let rest = read
            .data_source()
            .as_parameter_declaration()
            .is_some_and(|data| data.dot_dot_dot_token().is_some());
        if rest {
            return Ok(tsr_ast::utilities_tail::get_rest_parameter_element_type(
                self.ast(node)?,
                type_node,
            )?);
        }
        Ok(type_node)
    }

    // port: tsc/internal/checker/checker.go:Checker.markDecoratorMedataDataTypeNodeAsReferenced
    fn mark_decorator_metadata_type_node_as_referenced(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<(), Error> {
        let Some(entity_name) = self.entity_name_for_decorator_metadata(node)? else {
            return Ok(());
        };
        if matches!(
            self.node(entity_name)?.kind().known(),
            Some(K::Identifier | K::QualifiedName)
        ) {
            self.mark_entity_name_or_entity_expression_as_reference(Some(entity_name), true)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getEntityNameForDecoratorMetadata
    fn entity_name_for_decorator_metadata(
        &self,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let Some(node) = node else {
            return Ok(None);
        };
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::UnionType) => {
                let list = read
                    .data_source()
                    .as_union_type_node()
                    .ok_or(tsr_arena::Error::InvalidGraph)?
                    .types();
                let types = self.source_list(node, list)?;
                self.entity_name_for_decorator_metadata_from_type_list(&types)
            }
            Some(K::IntersectionType) => {
                let list = read
                    .data_source()
                    .as_intersection_type_node()
                    .ok_or(tsr_arena::Error::InvalidGraph)?
                    .types();
                let types = self.source_list(node, list)?;
                self.entity_name_for_decorator_metadata_from_type_list(&types)
            }
            Some(K::ConditionalType) => {
                let data = read
                    .data_source()
                    .as_conditional_type_node()
                    .ok_or(Error::MissingLink("conditional type node"))?;
                let types: Vec<NodeId> = [data.true_type(), data.false_type()]
                    .into_iter()
                    .flatten()
                    .collect();
                self.entity_name_for_decorator_metadata_from_type_list(&types)
            }
            Some(K::ParenthesizedType | K::NamedTupleMember) => {
                self.entity_name_for_decorator_metadata(read.type_node())
            }
            Some(K::TypeReference) => Ok(read
                .data_source()
                .as_type_reference_node()
                .and_then(|data| data.type_name())),
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getEntityNameForDecoratorMetadataFromTypeList
    fn entity_name_for_decorator_metadata_from_type_list(
        &self,
        type_nodes: &[NodeId],
    ) -> Result<Option<NodeId>, Error> {
        let strict_null_checks = self.options.strict_null_checks;
        let mut common: Option<NodeId> = None;
        for &type_node in type_nodes {
            let read = self.node(type_node)?;
            if read.kind() == K::NeverKeyword {
                continue; // Always elide `never` from the union/intersection if possible.
            }
            if !strict_null_checks {
                let null_literal = read.kind() == K::LiteralType
                    && read
                        .data_source()
                        .as_literal_type_node()
                        .and_then(|data| data.literal())
                        .map(|literal| self.node(literal).map(|node| node.kind() == K::NullKeyword))
                        .transpose()?
                        .unwrap_or(false);
                if null_literal || read.kind() == K::UndefinedKeyword {
                    // Elide null and undefined from unions for metadata.
                    continue;
                }
            }
            let Some(individual) = self.entity_name_for_decorator_metadata(Some(type_node))? else {
                // Something like string or number: serialized as itself or Object.
                return Ok(None);
            };
            match common {
                None => common = Some(individual),
                Some(existing) => {
                    // In sync with the transformation of a type node: the
                    // entity names must be the same identifier text.
                    let same = self.node(existing)?.kind() == K::Identifier
                        && self.node(individual)?.kind() == K::Identifier
                        && self.node_text(existing)?.as_bytes()
                            == self.node_text(individual)?.as_bytes();
                    if !same {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(common)
    }

    // port: tsc/internal/checker/checker.go:Checker.markEntityNameOrEntityExpressionAsReference
    pub(crate) fn mark_entity_name_or_entity_expression_as_reference(
        &mut self,
        type_name: Option<NodeId>,
        for_decorator_metadata: bool,
    ) -> Result<(), Error> {
        let Some(type_name) = type_name else {
            return Ok(());
        };
        let root =
            tsr_ast::utilities_middle::get_first_identifier(self.ast(type_name)?, type_name)?;
        let meaning = if self.node(type_name)?.kind() == K::Identifier {
            sf::TYPE
        } else {
            sf::NAMESPACE
        } | sf::ALIAS;
        let text = self.node_text(root)?.into_js_string();
        let Some(root_symbol) =
            self.resolve_name_ex(Some(root), text.as_bytes(), meaning, None, true, false)?
        else {
            return Ok(());
        };
        if self.symbol(root_symbol)?.flags() & sf::ALIAS == 0 {
            return Ok(());
        }
        let options = self.program()?.host.options();
        let can_collect = !options.verbatim_module_syntax.is_true();
        let isolated_modules = options.isolated_modules();
        let module_kind = options.emit_module_kind();
        if can_collect && self.symbol_is_value(root_symbol)? {
            let target = self.resolve_alias(root_symbol)?;
            let flags = self.symbol(target)?.flags();
            // isConstEnumOrConstEnumOnlyModule
            let const_enum = flags & sf::CONST_ENUM != 0 || flags & sf::CONST_ENUM_ONLY_MODULE != 0;
            if !const_enum
                && self
                    .direct_type_only_alias_declaration(root_symbol)?
                    .is_none()
            {
                return self.mark_module_alias_referenced(root_symbol);
            }
        }
        if for_decorator_metadata
            && isolated_modules
            && module_kind >= tsr_core::ModuleKind::ES2015
            && !self.symbol_is_value(root_symbol)?
        {
            let declarations: Vec<NodeId> = self
                .symbol_declarations(root_symbol)?
                .iter()
                .flatten()
                .collect();
            let mut type_only = false;
            for &declaration in &declarations {
                if tsr_ast::utilities_modules::is_type_only_import_or_export_declaration(
                    self.ast(declaration)?,
                    declaration,
                )? {
                    type_only = true;
                    break;
                }
            }
            if !type_only {
                let mut diagnostic = self.diagnostic_for_node(
                    Some(type_name),
                    d::A_type_referenced_in_a_decorated_signature_must_be_imported_with_import_type_or_a_namespace_import_when_isolatedModules_and_emitDecoratorMetadata_are_enabled,
                    vec![],
                )?;
                for &declaration in &declarations {
                    if tsr_ast::is_alias_symbol_declaration(self.ast(declaration)?, declaration)? {
                        diagnostic.related_information = vec![Arc::new(self.diagnostic_for_node(
                            Some(declaration),
                            d::X_0_was_imported_here,
                            vec![text.clone()],
                        )?)];
                        break;
                    }
                }
                self.add_diagnostic(diagnostic)?;
            }
        }
        Ok(())
    }
}
