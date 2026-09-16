//! Call signature resolution. A provisional resolving signature breaks
//! contextual argument cycles; completed signatures are not cached while a
//! flow loop is using temporary types. Candidate order is source-observable.

use crate::{
    signature_flags as sg, type_flags as tf, types::Map, CheckerState, Error, InferenceId,
    RelationKind, SignatureId, TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as messages;

#[derive(Clone, Copy)]
enum Resolution {
    Signature(SignatureId),
    Failed(Error),
}

#[derive(Clone, Copy)]
pub(crate) struct ArgumentContext {
    pub node: NodeId,
    pub ty: TypeId,
    pub inference: Option<InferenceId>,
}

#[derive(Clone)]
pub(crate) struct TypedCall {
    pub node: NodeId,
    pub type_arguments: Vec<NodeId>,
    pub args: Vec<NodeId>,
    pub candidates: Vec<SignatureId>,
    pub argument_mode: u32,
    pub single_non_generic: bool,
    pub argument_errors: Vec<SignatureId>,
    pub arity_error: Option<SignatureId>,
    pub constraint_error: Option<SignatureId>,
}

#[derive(Default)]
pub(crate) struct CallState {
    resolved: Map<NodeId, Resolution>,
    optional_signatures: Map<(SignatureId, u32), SignatureId>,
    #[allow(
        clippy::option_option,
        reason = "Distinguish an unqueried symbol from a cached absent symbol"
    )]
    symbol_constructor: Option<Option<SymbolId>>,
    pub contexts: Vec<ArgumentContext>,
    pub instantiation_expressions: Map<(NodeId, TypeId), TypeId>,
    /// Native inference scopes are separate from cached contextual types: a
    /// cache must not hide an enclosing call's inference context.
    pub inference_contexts: Vec<(NodeId, Option<InferenceId>)>,
}

#[cfg(any(test, feature = "storage-pilot"))]
impl CallState {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.map("call_resolution", &self.resolved);
        census.map("optional_call_signatures", &self.optional_signatures);
        census.map("instantiation_expression", &self.instantiation_expressions);
        census.vec_capacity("call_resolution", &self.contexts, self.contexts.capacity());
        census.vec_capacity(
            "call_resolution",
            &self.inference_contexts,
            self.inference_contexts.capacity(),
        );
    }
    pub(crate) fn census_signatures(&self) -> impl Iterator<Item = SignatureId> + '_ {
        self.resolved
            .values()
            .filter_map(|resolution| match resolution {
                Resolution::Signature(signature) => Some(*signature),
                Resolution::Failed(_) => None,
            })
            .chain(
                self.optional_signatures
                    .iter()
                    .flat_map(|(&(source, _), &result)| [source, result]),
            )
    }
    pub(crate) fn census_types(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.contexts.iter().map(|context| context.ty).chain(
            self.instantiation_expressions
                .iter()
                .flat_map(|(&(_, source), &result)| [source, result]),
        )
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getContextuallyTypedParameterType
    pub(crate) fn iife_parameter_argument_type(
        &mut self,
        call: NodeId,
        index: usize,
        has_initializer: bool,
    ) -> Result<Option<TypeId>, Error> {
        let args = self.effective_call_arguments(call)?;
        let saved = self
            .calls
            .resolved
            .insert(call, Resolution::Signature(self.builtins.any_signature));
        let result = if let Some(&argument) = args.get(index) {
            self.check_expression(argument)
                .and_then(|ty| self.widen_literal_type(ty))
                .map(Some)
        } else if has_initializer {
            Ok(None)
        } else {
            Ok(Some(self.builtins.undefined_widening_type))
        };
        if let Some(saved) = saved {
            self.calls.resolved.insert(call, saved);
        } else {
            self.calls.resolved.remove(&call);
        }
        result
    }

    pub(crate) fn cached_call_signature(&self, node: NodeId) -> Option<SignatureId> {
        match self.calls.resolved.get(&node) {
            Some(Resolution::Signature(signature)) => Some(*signature),
            _ => None,
        }
    }

    pub(crate) fn contextual_call_argument(&self, node: NodeId) -> Option<ArgumentContext> {
        self.calls
            .contexts
            .iter()
            .rev()
            .find(|context| context.node == node)
            .copied()
    }

    // port: tsc/internal/checker/checker.go:Checker.getInferenceContext
    pub(crate) fn call_inference_at_node(
        &self,
        node: NodeId,
    ) -> Result<Option<InferenceId>, Error> {
        for &(scope, inference) in self.calls.inference_contexts.iter().rev() {
            if ts_ast::utilities::is_node_descendant_of(self.ast(node)?, Some(node), Some(scope))? {
                return Ok(inference);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getResolvedSignature
    pub(crate) fn resolved_call_signature(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        let cached = self.calls.resolved.get(&node).copied();
        match cached {
            Some(Resolution::Failed(error)) => return Err(error),
            Some(Resolution::Signature(signature))
                if signature != self.builtins.resolving_signature =>
            {
                return Ok(signature)
            }
            _ => {}
        }
        let saved_start = if cached.is_none() {
            Some(
                self.resolution
                    .set_resolution_start(self.resolution.depth()),
            )
        } else {
            None
        };
        self.calls.resolved.insert(
            node,
            Resolution::Signature(self.builtins.resolving_signature),
        );
        let result = self.resolve_call_expression(node);
        if let Some(saved_start) = saved_start {
            self.resolution.set_resolution_start(saved_start);
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.calls.resolved.insert(node, Resolution::Failed(error));
                return Err(error);
            }
        };
        if result == self.builtins.resolving_signature {
            return Ok(result);
        }
        let result = match self.calls.resolved.get(&node) {
            Some(Resolution::Signature(nested)) if *nested != self.builtins.resolving_signature => {
                *nested
            }
            _ => result,
        };
        if self.flow.loop_stack.is_empty() {
            self.calls
                .resolved
                .insert(node, Resolution::Signature(result));
        } else if let Some(cached) = cached {
            self.calls.resolved.insert(node, cached);
        } else {
            self.calls.resolved.remove(&node);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkCallExpression
    pub(crate) fn check_call_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_grammar_type_arguments(node)?;
        let signature = self.resolved_call_signature(node)?;
        if signature == self.builtins.resolving_signature {
            return Ok(self.builtins.silent_never_type);
        }
        self.check_deprecated_signature(signature, node)?;
        if let Some(expression) = self.ast(node)?.node(node)?.expression() {
            if self.ast(expression)?.node(expression)?.kind() == K::SuperKeyword {
                return Ok(self.builtins.void_type);
            }
        }
        if self.ast(node)?.node(node)?.kind() == K::NewExpression {
            if let Some(declaration) = self.signatures.get(signature)?.declaration {
                if !matches!(
                    self.ast(declaration)?.node(declaration)?.kind().known(),
                    Some(K::Constructor | K::ConstructSignature | K::ConstructorType)
                ) {
                    let options = self.program()?.host.options();
                    if options.strict_option_value(options.no_implicit_any) {
                        self.error_at(Some(node), messages::X_new_expression_whose_target_lacks_a_construct_signature_implicitly_has_an_any_type, vec![])?;
                    }
                    return Ok(self.builtins.any_type);
                }
            }
        }
        if self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0
            && self.is_common_js_require(node)?
        {
            let arguments = self.source_list(node, self.ast(node)?.node(node)?.argument_list())?;
            return self.external_module_type_by_literal(arguments[0]);
        }
        let ty = self.return_type_of_signature(signature)?;
        if self.types.flags(ty)? & tf::ES_SYMBOL_LIKE != 0
            && self.is_symbol_or_symbol_for_call(node)?
        {
            let mut parent = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("Symbol call parent"))?;
            while self.ast(parent)?.node(parent)?.kind() == K::ParenthesizedExpression {
                parent = self
                    .ast(parent)?
                    .node(parent)?
                    .parent()
                    .ok_or(Error::MissingLink("Symbol parenthesized parent"))?;
            }
            return self.es_symbol_like_type_for_node(parent);
        }
        if self.ast(node)?.node(node)?.kind() == K::CallExpression
            && self.ast(node)?.node(node)?.question_dot_token().is_none()
            && self.types.flags(ty)? & tf::VOID != 0
        {
            let parent = self.ast(node)?.node(node)?.parent();
            let expression_statement = match parent {
                Some(parent) => self.ast(parent)?.node(parent)?.kind() == K::ExpressionStatement,
                None => false,
            };
            if expression_statement && self.type_predicate_of_signature(signature)?.is_some() {
                self.check_assertion_call_target(node, signature)?;
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.isSymbolOrSymbolForCall
    pub(crate) fn is_symbol_or_symbol_for_call(&mut self, node: NodeId) -> Result<bool, Error> {
        if self.ast(node)?.node(node)?.kind() != K::CallExpression {
            return Ok(false);
        }
        let mut left = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("Symbol call expression"))?;
        if self.ast(left)?.node(left)?.kind() == K::PropertyAccessExpression {
            let name = self
                .ast(left)?
                .node(left)?
                .name()
                .ok_or(Error::MissingLink("Symbol property name"))?;
            if self.ast(name)?.node_text(name)?.as_bytes() == b"for" {
                left = self
                    .ast(left)?
                    .node(left)?
                    .expression()
                    .ok_or(Error::MissingLink("Symbol for receiver"))?;
            }
        }
        if self.ast(left)?.node(left)?.kind() != K::Identifier
            || self.ast(left)?.node_text(left)?.as_bytes() != b"Symbol"
        {
            return Ok(false);
        }
        let global = if let Some(symbol) = self.calls.symbol_constructor {
            symbol
        } else {
            let symbol = self.resolve_name(None, b"Symbol", sf::VALUE, None, false)?;
            self.calls.symbol_constructor = Some(symbol);
            symbol
        };
        let Some(global) = global else {
            return Ok(false);
        };
        Ok(self.resolve_name(Some(left), b"Symbol", sf::VALUE, None, false)? == Some(global))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModuleTypeByLiteral
    fn external_module_type_by_literal(&mut self, name: NodeId) -> Result<TypeId, Error> {
        if let Some(symbol) = self.resolve_external_module_name(name, name, false)? {
            if let Some(symbol) = self.resolve_external_module_symbol(Some(symbol), false)? {
                return self.get_type_of_symbol(symbol);
            }
        }
        Ok(self.builtins.any_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveCallExpression
    fn resolve_call_expression(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::NewExpression {
            return self.resolve_new_expression(node);
        }
        if read.kind() == K::BinaryExpression {
            return self.resolve_instanceof_expression(node);
        }
        if read.kind() == K::TaggedTemplateExpression {
            return self.resolve_tagged_template_expression(node);
        }
        if read.kind() != K::CallExpression {
            return Err(Error::Unsupported("resolveSignature: decorator/JSX"));
        }
        let expression = read.expression().ok_or(Error::MissingLink("call target"))?;
        if self.ast(expression)?.node(expression)?.kind() == K::SuperKeyword {
            let ty = self.check_super_expression(expression)?;
            if self.types.flags(ty)? & tf::ANY != 0 {
                for argument in
                    self.source_list(node, self.ast(node)?.node(node)?.argument_list())?
                {
                    self.check_expression(argument)?;
                }
                return Ok(self.builtins.any_signature);
            }
            if !self.is_error_type(ty)? {
                let class = ts_ast::utilities::get_containing_class(self.ast(node)?, node)?
                    .ok_or(Error::MissingLink("super call class"))?;
                if let Some(base) = self
                    .class_heritage_nodes(class, K::ExtendsKeyword)?
                    .first()
                    .copied()
                {
                    let constructors = self.instantiated_constructors_for_arguments(ty, base)?;
                    return self.resolve_typed_call(node, &constructors);
                }
            }
            return self.resolve_untyped_call(node);
        }
        if self.ast(expression)?.node(expression)?.kind() == K::ImportKeyword {
            return self.resolve_untyped_call(node);
        }
        let mut ty = self.check_expression(expression)?;
        let mut call_chain_flags = 0;
        if self.ast(node)?.node(node)?.flags() & nf::OPTIONAL_CHAIN != 0 {
            let non_optional = self.optional_expression_type(ty, expression)?;
            if non_optional != ty {
                call_chain_flags =
                    if ts_ast::utilities::is_outermost_optional_chain(self.ast(node)?, node)? {
                        sg::IS_OUTER_CALL_CHAIN
                    } else {
                        sg::IS_INNER_CALL_CHAIN
                    };
            }
            ty = non_optional;
        }
        let ty = self.check_non_null_type_with_reporter(ty, expression, true)?;
        if ty == self.builtins.silent_never_type {
            return Ok(self.builtins.silent_never_signature);
        }
        let apparent = self.apparent_type(ty)?;
        if self.is_error_type(apparent)? {
            self.resolve_untyped_call(node)?;
            return Ok(self.builtins.unknown_signature);
        }
        let signatures = self.signatures_of_type(apparent, false)?;
        let constructors = self.signatures_of_type(apparent, true)?;
        let untyped =
            self.is_untyped_function_call(ty, apparent, signatures.len(), constructors.len())?;
        if untyped {
            if self.ast(node)?.node(node)?.type_argument_list().is_some() {
                self.error_at(
                    Some(node),
                    messages::Untyped_function_calls_may_not_accept_type_arguments,
                    vec![],
                )?;
            }
            return self.resolve_untyped_call(node);
        }
        if signatures.is_empty() {
            if !constructors.is_empty() {
                let text = self.type_to_string(ty, crate::type_format_flags::NONE)?;
                self.error_at(
                    Some(node),
                    messages::Value_of_type_0_is_not_callable_Did_you_mean_to_include_new,
                    vec![text],
                )?;
                self.resolve_untyped_call(node)?;
                return Ok(self.builtins.unknown_signature);
            }
            self.call_invocation_error(node, apparent, false)?;
            self.resolve_untyped_call(node)?;
            return Ok(self.builtins.unknown_signature);
        }
        if self.expression_mode & 8 != 0
            && self.ast(node)?.node(node)?.type_argument_list().is_none()
        {
            for &signature in &signatures {
                if self
                    .signatures
                    .get(signature)?
                    .type_parameters
                    .as_ref()
                    .is_some_and(|types| !types.is_empty())
                {
                    let returned = self.return_type_of_signature(signature)?;
                    if self.types.flags(returned)? & tf::OBJECT != 0
                        && !self.signatures_of_type(returned, false)?.is_empty()
                    {
                        if self.expression_mode & 2 != 0 {
                            let context = self
                                .call_inference_at_node(node)?
                                .ok_or(Error::MissingLink("skipped generic function inference"))?;
                            self.inference_context_mut(context)?.flags |= 4; // InferenceFlagsSkippedGenericFunction
                        }
                        return Ok(self.builtins.resolving_signature);
                    }
                }
            }
        }
        self.resolve_typed_call_chain(node, &signatures, call_chain_flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveNewExpression
    fn resolve_new_expression(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        let expression = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("new target"))?;
        let ty = self.check_expression(expression)?;
        let ty = self.check_non_null_type_with_reporter(ty, expression, false)?;
        if ty == self.builtins.silent_never_type {
            return Ok(self.builtins.silent_never_signature);
        }
        let ty = self.apparent_type(ty)?;
        if ty == self.builtins.error_type {
            self.resolve_untyped_call(node)?;
            return Ok(self.builtins.unknown_signature);
        }
        if self.types.flags(ty)? & tf::ANY != 0 {
            if self.ast(node)?.node(node)?.type_argument_list().is_some() {
                self.error_at(
                    Some(node),
                    messages::Untyped_function_calls_may_not_accept_type_arguments,
                    vec![],
                )?;
            }
            return self.resolve_untyped_call(node);
        }
        let signatures = self.signatures_of_type(ty, true)?;
        if !signatures.is_empty() {
            if let Some((modifier, class_type)) = self.constructor_accessibility_error(
                node,
                &signatures,
                ts_ast::modifier_flags::NON_PUBLIC_ACCESSIBILITY_MODIFIER,
            )? {
                let message = if modifier == ts_ast::modifier_flags::PRIVATE {
                    messages::Constructor_of_class_0_is_private_and_only_accessible_within_the_class_declaration
                } else {
                    messages::Constructor_of_class_0_is_protected_and_only_accessible_within_the_class_declaration
                };
                let text = self.type_to_string(
                    class_type,
                    crate::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                        | crate::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                )?;
                self.error_at(Some(node), message, vec![text])?;
                self.resolve_untyped_call(node)?;
                return Ok(self.builtins.unknown_signature);
            }
            let mut abstract_class = false;
            for &signature in &signatures {
                // someSignature examines the members of a union composite;
                // the composite's own flags come from its first signature.
                let signature = self.signatures.get(signature)?;
                let found = if let Some(composite) = &signature.composite {
                    let mut found = false;
                    if composite.is_union {
                        for &part in composite.signatures.iter() {
                            found |= self.signatures.get(part)?.flags & sg::ABSTRACT != 0;
                        }
                    }
                    found
                } else {
                    signature.flags & sg::ABSTRACT != 0
                };
                if found {
                    abstract_class = true;
                    break;
                }
            }
            if let Some(symbol) = self.types.get(ty)?.symbol {
                for declaration in self
                    .symbol_declarations(symbol)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    let read = self.ast(declaration)?.node(declaration)?;
                    if matches!(
                        read.kind().known(),
                        Some(K::ClassDeclaration | K::ClassExpression)
                    ) && read.modifier_flags(self.ast(declaration)?)?
                        & ts_ast::modifier_flags::ABSTRACT
                        != 0
                    {
                        abstract_class = true;
                        break;
                    }
                }
            }
            if abstract_class {
                self.error_at(
                    Some(node),
                    messages::Cannot_create_an_instance_of_an_abstract_class,
                    vec![],
                )?;
                self.resolve_untyped_call(node)?;
                return Ok(self.builtins.unknown_signature);
            }
            return self.resolve_typed_call(node, &signatures);
        }
        let signatures = self.signatures_of_type(ty, false)?;
        if !signatures.is_empty() {
            let signature = self.resolve_typed_call(node, &signatures)?;
            let options = self.program()?.host.options();
            if !options.strict_option_value(options.no_implicit_any) {
                if self.signatures.get(signature)?.declaration.is_some()
                    && self.return_type_of_signature(signature)? != self.builtins.void_type
                {
                    self.error_at(
                        Some(node),
                        messages::Only_a_void_function_can_be_called_with_the_new_keyword,
                        vec![],
                    )?;
                }
                if let Some(this) = self.signatures.get(signature)?.this_parameter {
                    if self.get_type_of_symbol(this)? == self.builtins.void_type {
                        self.error_at(Some(node), messages::A_function_that_is_called_with_the_new_keyword_cannot_have_a_this_type_that_is_void, vec![])?;
                    }
                }
            }
            return Ok(signature);
        }
        self.call_invocation_error(node, ty, true)?;
        self.resolve_untyped_call(node)?;
        Ok(self.builtins.unknown_signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveUntypedCall
    pub(crate) fn resolve_untyped_call(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        if self.ast(node)?.node(node)?.kind() != K::BinaryExpression {
            for argument in
                self.source_list(node, self.ast(node)?.node(node)?.type_argument_list())?
            {
                self.check_source_element(argument)?;
            }
        }
        for argument in self.effective_call_arguments(node)? {
            self.check_expression(argument)?;
        }
        Ok(self.builtins.any_signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.reorderCandidates
    fn reorder_call_candidates(
        &mut self,
        signatures: &[SignatureId],
        call_chain_flags: u32,
    ) -> Result<Vec<SignatureId>, Error> {
        let mut last_parent = None;
        let mut last_symbol = None;
        let mut index = 0;
        let mut cutoff = 0;
        let mut specialized = 0;
        let mut result = Vec::with_capacity(signatures.len());
        for &signature in signatures {
            let declaration = self.signatures.get(signature)?.declaration;
            let (symbol, parent) = match declaration {
                Some(declaration) => (
                    self.get_symbol_of_declaration(declaration)?,
                    self.ast(declaration)?.node(declaration)?.parent(),
                ),
                None => (None, None),
            };
            if last_symbol.is_none() || symbol == last_symbol {
                if last_parent.is_some() && parent == last_parent {
                    index += 1;
                } else {
                    last_parent = parent;
                    index = cutoff;
                }
            } else {
                index = result.len();
                cutoff = result.len();
                last_parent = parent;
            }
            last_symbol = symbol;
            let position = if self.signatures.get(signature)?.flags & sg::HAS_LITERAL_TYPES != 0 {
                let position = specialized;
                specialized += 1;
                cutoff += 1;
                position
            } else {
                index
            };
            let signature = if call_chain_flags != 0 {
                self.optional_call_signature(signature, call_chain_flags)?
            } else {
                signature
            };
            result.insert(position, signature);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getOptionalCallSignature
    fn optional_call_signature(
        &mut self,
        signature: SignatureId,
        flags: u32,
    ) -> Result<SignatureId, Error> {
        if self.signatures.get(signature)?.flags & sg::CALL_CHAIN_FLAGS == flags {
            return Ok(signature);
        }
        if let Some(&result) = self.calls.optional_signatures.get(&(signature, flags)) {
            return Ok(result);
        }
        let result = self.clone_signature(signature)?;
        self.signatures.get_mut(result)?.flags |= flags;
        self.calls
            .optional_signatures
            .insert((signature, flags), result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveCall
    pub(crate) fn resolve_typed_call(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
    ) -> Result<SignatureId, Error> {
        self.resolve_typed_call_chain(node, signatures, 0)
    }

    fn resolve_typed_call_chain(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        call_chain_flags: u32,
    ) -> Result<SignatureId, Error> {
        let type_arguments = if self.ast(node)?.node(node)?.kind() == K::BinaryExpression {
            Vec::new()
        } else {
            self.source_list(node, self.ast(node)?.node(node)?.type_argument_list())?
        };
        for &argument in &type_arguments {
            self.check_source_element(argument)?;
        }
        let candidates = self.reorder_call_candidates(signatures, call_chain_flags)?;
        let args = self.effective_call_arguments(node)?;
        let single_non_generic = candidates.len() == 1
            && self
                .signatures
                .get(candidates[0])?
                .type_parameters
                .as_ref()
                .is_none_or(|types| types.is_empty());
        let mut argument_mode = 0;
        if !single_non_generic {
            for &argument in &args {
                if self.expression_is_context_sensitive(argument)? {
                    argument_mode = 4;
                    break;
                }
            }
        }
        let mut state = TypedCall {
            node,
            type_arguments,
            args,
            candidates,
            argument_mode,
            single_non_generic,
            argument_errors: Vec::new(),
            arity_error: None,
            constraint_error: None,
        };
        if state.candidates.len() > 1 {
            if let Some(result) = self.choose_typed_call(&mut state, RelationKind::Subtype)? {
                return Ok(result);
            }
        }
        if let Some(result) = self.choose_typed_call(&mut state, RelationKind::Assignable)? {
            return Ok(result);
        }
        let candidate = self.overload_failure_candidate(&mut state)?;
        self.calls
            .resolved
            .insert(node, Resolution::Signature(candidate));
        self.report_typed_call_failure(&state, signatures)?;
        Ok(candidate)
    }

    // port: tsc/internal/checker/checker.go:Checker.chooseOverload
    pub(crate) fn choose_typed_call(
        &mut self,
        state: &mut TypedCall,
        relation: RelationKind,
    ) -> Result<Option<SignatureId>, Error> {
        let node = state.node;
        let args = &state.args;
        let type_arguments = &state.type_arguments;
        state.argument_errors.clear();
        state.arity_error = None;
        state.constraint_error = None;
        if state.single_non_generic {
            let candidate = state.candidates[0];
            if !type_arguments.is_empty() || !self.call_has_correct_arity(node, args, candidate)? {
                return Ok(None);
            }
            if !self.call_signature_applicable_ex(node, args, candidate, relation, false, 0)? {
                state.argument_errors.push(candidate);
                return Ok(None);
            }
            return Ok(Some(candidate));
        }
        for candidate in &mut state.candidates {
            let parameters = self
                .signatures
                .get(*candidate)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            if !type_arguments.is_empty()
                && (type_arguments.len() < self.min_type_argument_count(&parameters)?
                    || type_arguments.len() > parameters.len())
            {
                continue;
            }
            if !self.call_has_correct_arity(node, args, *candidate)? {
                continue;
            }
            let mut inference = None;
            let mut checked = if parameters.is_empty() {
                *candidate
            } else {
                let type_arguments = if type_arguments.is_empty() {
                    let flags =
                        if ts_ast::utilities::is_in_js_file(Some(&self.ast(node)?.node(node)?)) {
                            crate::inference::ANY_DEFAULT
                        } else {
                            0
                        };
                    let context =
                        self.new_inference_context(&parameters, Some(*candidate), flags)?;
                    inference = Some(context);
                    let arguments = self.infer_call_type_arguments_ex(
                        node,
                        *candidate,
                        args,
                        state.argument_mode | 8,
                        context,
                    )?;
                    if self.inference_context(context)?.flags & 4 != 0 {
                        state.argument_mode |= 8;
                    }
                    arguments
                } else {
                    let Some(arguments) =
                        self.call_type_arguments(*candidate, type_arguments, false)?
                    else {
                        state.constraint_error = Some(*candidate);
                        continue;
                    };
                    arguments
                };
                self.call_signature_instantiation(*candidate, &type_arguments, false, inference)?
            };
            if self.non_array_rest_type(*candidate)?.is_some()
                && !self.call_has_correct_arity(node, args, checked)?
            {
                state.arity_error = Some(checked);
                continue;
            }
            let mode = if state.single_non_generic {
                0
            } else {
                state.argument_mode
            };
            if !self.call_signature_applicable_ex(node, args, checked, relation, false, mode)? {
                state.argument_errors.push(checked);
                continue;
            }
            if state.argument_mode != 0 {
                state.argument_mode = 0;
                if let Some(context) = inference {
                    let arguments =
                        self.infer_call_type_arguments_ex(node, *candidate, args, 0, context)?;
                    checked = self.call_signature_instantiation(
                        *candidate,
                        &arguments,
                        false,
                        Some(context),
                    )?;
                    if self.non_array_rest_type(*candidate)?.is_some()
                        && !self.call_has_correct_arity(node, args, checked)?
                    {
                        state.arity_error = Some(checked);
                        continue;
                    }
                }
                if !self.call_signature_applicable_ex(node, args, checked, relation, false, 0)? {
                    state.argument_errors.push(checked);
                    continue;
                }
            }
            *candidate = checked;
            return Ok(Some(checked));
        }
        Ok(None)
    }
}
