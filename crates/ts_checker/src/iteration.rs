//! Iteration uses one cache per checker and records deferred diagnostic inputs
//! by identity. Reporting a cached failure repeats the native diagnostic walk.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{Diagnostic, JsString, SyntaxKind as K};
use ts_core::Tristate;
use ts_diagnostics as d;

pub(crate) const ALLOW_SYNC: u32 = 1;
pub(crate) const ALLOW_ASYNC: u32 = 1 << 1;
pub(crate) const ALLOW_STRING: u32 = 1 << 2;
pub(crate) const FOR_OF_FLAG: u32 = 1 << 3;
pub(crate) const YIELD_STAR_FLAG: u32 = 1 << 4;
pub(crate) const SPREAD_FLAG: u32 = 1 << 5;
pub(crate) const DESTRUCTURING_FLAG: u32 = 1 << 6;
pub(crate) const POSSIBLY_OUT_OF_BOUNDS: u32 = 1 << 7;
pub(crate) const FOR_OF: u32 = ALLOW_SYNC | ALLOW_STRING | FOR_OF_FLAG;
pub(crate) const SPREAD: u32 = ALLOW_SYNC | SPREAD_FLAG;
const CACHE_FLAGS: u32 = ALLOW_SYNC | ALLOW_ASYNC | FOR_OF_FLAG;

#[derive(Clone, Copy)]
pub(crate) enum IterationKind {
    Yield,
    Return,
    Next,
}

#[derive(Clone, Copy, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "Names distinguish the native yield, return and next type slots from values"
)]
pub(crate) struct IterationTypes {
    pub(crate) yield_type: Option<TypeId>,
    pub(crate) return_type: Option<TypeId>,
    pub(crate) next_type: Option<TypeId>,
}
impl IterationTypes {
    pub(crate) fn get(self, kind: IterationKind) -> Option<TypeId> {
        match kind {
            IterationKind::Yield => self.yield_type,
            IterationKind::Return => self.return_type,
            IterationKind::Next => self.next_type,
        }
    }
    pub(crate) fn has_types(self) -> bool {
        self.yield_type.is_some() || self.return_type.is_some() || self.next_type.is_some()
    }
    pub(crate) fn any(ty: TypeId) -> Self {
        Self {
            yield_type: Some(ty),
            return_type: Some(ty),
            next_type: Some(ty),
        }
    }
}
struct IterationDiagnostic {
    node: NodeId,
    input: TypeId,
    allow_async: bool,
    related: Vec<Diagnostic>,
}
#[derive(Default)]
pub(crate) struct IterationState {
    globals: crate::types::Map<(&'static str, bool), TypeId>,
    cache: crate::types::Map<(TypeId, u32), IterationTypes>,
    pending: Vec<IterationDiagnostic>,
}
impl IterationState {
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.map("iteration_cache", &self.globals);
        census.map("iteration_cache", &self.cache);
        census.vec_capacity("iteration_cache", &self.pending, self.pending.capacity());
        for pending in &self.pending {
            census.vec_capacity(
                "iteration_cache",
                &pending.related,
                pending.related.capacity(),
            );
            for diagnostic in &pending.related {
                census.diagnostic(diagnostic);
            }
        }
    }
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn type_roots(&self) -> Vec<TypeId> {
        let mut roots: Vec<_> = self.globals.values().copied().collect();
        for (&(input, _), value) in &self.cache {
            roots.push(input);
            roots.extend(
                [value.yield_type, value.return_type, value.next_type]
                    .into_iter()
                    .flatten(),
            );
        }
        roots.extend(self.pending.iter().map(|pending| pending.input));
        roots
    }
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.createIterableType
    pub(crate) fn create_iterable_type(&mut self, element: TypeId) -> Result<TypeId, Error> {
        let target = self.iteration_global_checked("Iterable", 3)?;
        if target == self.builtins.empty_generic_type {
            return Ok(self.builtins.empty_object_type);
        }
        self.create_type_reference(
            target,
            &[
                element,
                self.builtins.void_type,
                self.builtins.undefined_type,
            ],
        )
    }
    pub(crate) fn iteration_global(
        &mut self,
        name: &'static str,
        arity: usize,
    ) -> Result<TypeId, Error> {
        self.iteration_global_with_report(name, arity, false)
    }
    pub(crate) fn iteration_global_checked(
        &mut self,
        name: &'static str,
        arity: usize,
    ) -> Result<TypeId, Error> {
        self.iteration_global_with_report(name, arity, true)
    }
    fn iteration_global_with_report(
        &mut self,
        name: &'static str,
        arity: usize,
        report: bool,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.iteration.globals.get(&(name, report)) {
            return Ok(ty);
        }
        let ty = self.get_global_type(name, arity, report)?;
        self.iteration.globals.insert((name, report), ty);
        Ok(ty)
    }
    pub(crate) fn iteration_is_reference(&self, ty: TypeId, target: TypeId) -> Result<bool, Error> {
        Ok(self.types.get(ty)?.object_flags & of::REFERENCE != 0
            && self.types.target(ty)? == target)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkRightHandSideOfForOf
    pub(crate) fn check_right_hand_side_of_for_of(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_for_in_or_of_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let use_ = FOR_OF
            | if data.await_modifier().is_some() {
                ALLOW_ASYNC
            } else {
                0
            };
        let expression = data
            .expression()
            .ok_or(Error::MissingLink("for-of expression"))?;
        let input = self.check_expression(expression)?;
        let input = self.check_non_null_type(input, expression)?;
        self.check_iterated_type_or_element_type(
            use_,
            input,
            self.builtins.undefined_type,
            Some(expression),
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.checkIteratedTypeOrElementType
    pub(crate) fn check_iterated_type_or_element_type(
        &mut self,
        use_: u32,
        input: TypeId,
        sent: TypeId,
        error_node: Option<NodeId>,
    ) -> Result<TypeId, Error> {
        if self.types.flags(input)? & tf::ANY != 0 {
            return Ok(input);
        }
        Ok(self
            .iterated_type_or_element_type(use_, input, sent, error_node, true)?
            .unwrap_or(self.builtins.any_type))
    }
    // port: tsc/internal/checker/checker.go:Checker.getIteratedTypeOrElementType
    pub(crate) fn iterated_type_or_element_type(
        &mut self,
        use_: u32,
        input: TypeId,
        sent: TypeId,
        error_node: Option<NodeId>,
        check_assignability: bool,
    ) -> Result<Option<TypeId>, Error> {
        let async_ = use_ & ALLOW_ASYNC != 0;
        if input == self.builtins.never_type {
            if let Some(node) = error_node {
                self.report_type_not_iterable(node, input, async_)?;
            }
            return Ok(None);
        }
        let iterable_exists =
            self.iteration_global("Iterable", 3)? != self.builtins.empty_generic_type;
        let unchecked =
            self.program()?.host.options().no_unchecked_indexed_access == Tristate::TRUE;
        let out_of_bounds = unchecked && use_ & POSSIBLY_OUT_OF_BOUNDS != 0;
        if iterable_exists || async_ {
            let types = self.iteration_types_of_iterable(
                input,
                use_,
                error_node.filter(|_| iterable_exists),
            )?;
            if check_assignability {
                if let Some(next) = types.next_type {
                    let head = if use_ & FOR_OF_FLAG != 0 {
                        Some(d::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_for_of_will_always_send_0)
                    } else if use_ & SPREAD_FLAG != 0 {
                        Some(d::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_array_spread_will_always_send_0)
                    } else if use_ & DESTRUCTURING_FLAG != 0 {
                        Some(d::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_array_destructuring_will_always_send_0)
                    } else if use_ & YIELD_STAR_FLAG != 0 {
                        Some(d::Cannot_delegate_iteration_to_value_because_the_next_method_of_its_iterator_expects_type_1_but_the_containing_generator_will_always_send_0)
                    } else {
                        None
                    };
                    if head.is_some() {
                        let (_, diagnostic) = self.check_type_related_ex(
                            sent,
                            next,
                            RelationKind::Assignable,
                            error_node,
                            head,
                        )?;
                        if let Some(diagnostic) = diagnostic {
                            self.add_diagnostic(diagnostic)?;
                        }
                    }
                }
            }
            if types.yield_type.is_some() || iterable_exists {
                return types
                    .yield_type
                    .map(|ty| self.iteration_include_undefined(ty, out_of_bounds))
                    .transpose();
            }
        }
        let mut array = input;
        if use_ & ALLOW_STRING != 0 {
            if self.types.flags(array)? & tf::UNION != 0 {
                let types = self.types.compound_types(array)?.clone();
                let mut filtered = Vec::new();
                for &ty in types.iter() {
                    if self.types.flags(ty)? & tf::STRING_LIKE == 0 {
                        filtered.push(ty);
                    }
                }
                if filtered.len() != types.len() {
                    array = self.get_union_type_ex(
                        &filtered,
                        crate::UnionReduction::Subtype,
                        None,
                        None,
                    )?;
                }
            } else if self.types.flags(array)? & tf::STRING_LIKE != 0 {
                array = self.builtins.never_type;
            }
        }
        let has_string = array != input;
        if has_string && self.types.flags(array)? & tf::NEVER != 0 {
            return Ok(Some(self.iteration_include_undefined(
                self.builtins.string_type,
                out_of_bounds,
            )?));
        }
        if !self.is_array_like_type(array)? {
            if let Some(node) = error_node {
                let (message, maybe_await) = self.iteration_diagnostic_details(
                    use_,
                    input,
                    use_ & ALLOW_STRING != 0 && !has_string,
                )?;
                let suggest_await = maybe_await && self.awaited_type_of_promise(array)?.is_some();
                self.iteration_error(node, array, suggest_await, message)?;
            }
            return if has_string {
                Ok(Some(self.iteration_include_undefined(
                    self.builtins.string_type,
                    out_of_bounds,
                )?))
            } else {
                Ok(None)
            };
        }
        let element = self
            .applicable_index_info(array, self.builtins.number_type)?
            .map(|index| {
                self.signatures
                    .index_info(index)
                    .map(|data| data.value_type)
            })
            .transpose()?;
        if let Some(element) = element {
            if has_string {
                if self.types.flags(element)? & tf::STRING_LIKE != 0 && !unchecked {
                    return Ok(Some(self.builtins.string_type));
                }
                let mut types = vec![element, self.builtins.string_type];
                if out_of_bounds {
                    types.push(self.builtins.undefined_type);
                }
                return Ok(Some(self.get_union_type_ex(
                    &types,
                    crate::UnionReduction::Subtype,
                    None,
                    None,
                )?));
            }
            return Ok(Some(
                self.iteration_include_undefined(element, out_of_bounds)?,
            ));
        }
        Ok(None)
    }
    fn iteration_include_undefined(&mut self, ty: TypeId, include: bool) -> Result<TypeId, Error> {
        if include {
            self.get_union_type(&[ty, self.builtins.undefined_type])
        } else {
            Ok(ty)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIterable
    pub(crate) fn iteration_types_of_iterable(
        &mut self,
        ty: TypeId,
        use_: u32,
        error_node: Option<NodeId>,
    ) -> Result<IterationTypes, Error> {
        let ty = self.get_reduced_type(ty)?;
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(IterationTypes::any(self.builtins.any_type));
        }
        let key = (ty, use_ & CACHE_FLAGS);
        let mut no_cache = false;
        if let Some(&cached) = self.iteration.cache.get(&key) {
            if error_node.is_none() || cached.has_types() {
                return Ok(cached);
            }
            no_cache = true;
        }
        let result = self.iteration_types_of_iterable_worker(ty, use_, error_node)?;
        if !no_cache {
            self.iteration.cache.insert(key, result);
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIterableWorker
    fn iteration_types_of_iterable_worker(
        &mut self,
        ty: TypeId,
        use_: u32,
        error_node: Option<NodeId>,
    ) -> Result<IterationTypes, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut values = Vec::new();
            for &part in parts.iter() {
                let value = self.iteration_types_of_iterable_worker(part, use_, None)?;
                if !value.has_types() {
                    if let Some(node) = error_node {
                        self.queue_iteration_diagnostic(
                            node,
                            ty,
                            use_ & ALLOW_ASYNC != 0,
                            Vec::new(),
                        );
                    }
                    return Ok(IterationTypes::default());
                }
                values.push(value);
            }
            return self.combine_iteration_types(&values);
        }
        let mut diagnostics = Vec::new();
        if use_ & ALLOW_ASYNC != 0 {
            let fast = self.iteration_types_fast(ty, false, true)?;
            if fast.has_types() {
                return if use_ & FOR_OF_FLAG != 0 {
                    self.async_from_sync_iteration_types(fast, error_node)
                } else {
                    Ok(fast)
                };
            }
            let slow =
                self.iteration_types_of_iterable_slow(ty, error_node, &mut diagnostics, true)?;
            if slow.has_types() {
                for diagnostic in diagnostics {
                    self.add_diagnostic(diagnostic)?;
                }
                return Ok(slow);
            }
        }
        if use_ & ALLOW_SYNC != 0 {
            let fast = self.iteration_types_fast(ty, false, false)?;
            if fast.has_types() {
                return if use_ & ALLOW_ASYNC != 0 {
                    self.async_from_sync_iteration_types(fast, error_node)
                } else {
                    Ok(fast)
                };
            }
            let slow =
                self.iteration_types_of_iterable_slow(ty, error_node, &mut diagnostics, false)?;
            if slow.has_types() {
                for diagnostic in diagnostics {
                    self.add_diagnostic(diagnostic)?;
                }
                return if use_ & ALLOW_ASYNC != 0 {
                    self.async_from_sync_iteration_types(slow, error_node)
                } else {
                    Ok(slow)
                };
            }
        }
        if let Some(node) = error_node {
            self.queue_iteration_diagnostic(node, ty, use_ & ALLOW_ASYNC != 0, diagnostics);
        }
        Ok(IterationTypes::default())
    }
    // port: tsc/internal/checker/checker.go:Checker.getAsyncFromSyncIterationTypes
    fn async_from_sync_iteration_types(
        &mut self,
        types: IterationTypes,
        error: Option<NodeId>,
    ) -> Result<IterationTypes, Error> {
        if !types.has_types()
            || types.yield_type == Some(self.builtins.any_type)
                && types.return_type == Some(self.builtins.any_type)
                && types.next_type == Some(self.builtins.any_type)
        {
            return Ok(types);
        }
        if error.is_some() {
            self.global_awaited_symbol(true)?;
        }
        let yield_type = match types.yield_type {
            Some(ty) => self.awaited_type_ex(ty, error, None, &[])?,
            None => None,
        };
        let return_type = match types.return_type {
            Some(ty) => self.awaited_type_ex(ty, error, None, &[])?,
            None => None,
        };
        Ok(IterationTypes {
            yield_type: Some(yield_type.unwrap_or(self.builtins.any_type)),
            return_type: Some(return_type.unwrap_or(self.builtins.any_type)),
            next_type: types.next_type,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.initializeIterationResolvers
    pub(crate) fn resolve_iteration_type(
        &mut self,
        ty: TypeId,
        asynchronous: bool,
        error: Option<NodeId>,
    ) -> Result<Option<TypeId>, Error> {
        if asynchronous {
            self.awaited_type_ex(ty,error,Some(d::Type_of_await_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member),&[])
        } else {
            Ok(Some(ty))
        }
    }
    // port: tsc/internal/checker/checker.go:IterationTypesResolver.getResolvedIterationTypes
    pub(crate) fn resolved_iteration_types(
        &mut self,
        yield_type: TypeId,
        return_type: TypeId,
        next_type: TypeId,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        let yielded = self
            .resolve_iteration_type(yield_type, asynchronous, None)?
            .unwrap_or(yield_type);
        let returned = self
            .resolve_iteration_type(return_type, asynchronous, None)?
            .unwrap_or(return_type);
        Ok(IterationTypes {
            yield_type: Some(yielded),
            return_type: Some(returned),
            next_type: Some(next_type),
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfGeneratorFunctionReturnType
    pub(crate) fn generator_return_iteration_types(
        &mut self,
        ty: TypeId,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(IterationTypes::any(self.builtins.any_type));
        }
        let result = self.iteration_types_of_iterable(
            ty,
            if asynchronous {
                ALLOW_ASYNC
            } else {
                ALLOW_SYNC
            },
            None,
        )?;
        if result.has_types() {
            return Ok(result);
        }
        self.iteration_types_of_iterator(ty, None, &mut Vec::new(), asynchronous)
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypeOfGeneratorFunctionReturnType
    pub(crate) fn generator_return_iteration_type(
        &mut self,
        kind: IterationKind,
        ty: TypeId,
        asynchronous: bool,
    ) -> Result<Option<TypeId>, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(None);
        }
        Ok(self
            .generator_return_iteration_types(ty, asynchronous)?
            .get(kind))
    }
    pub(crate) fn combine_iteration_types(
        &mut self,
        values: &[IterationTypes],
    ) -> Result<IterationTypes, Error> {
        let union = |checker: &mut Self, types: Vec<TypeId>| {
            if types.is_empty() {
                Ok(None)
            } else {
                checker.get_union_type(&types).map(Some)
            }
        };
        Ok(IterationTypes {
            yield_type: union(self, values.iter().filter_map(|v| v.yield_type).collect())?,
            return_type: union(self, values.iter().filter_map(|v| v.return_type).collect())?,
            next_type: union(self, values.iter().filter_map(|v| v.next_type).collect())?,
        })
    }
    fn queue_iteration_diagnostic(
        &mut self,
        node: NodeId,
        input: TypeId,
        allow_async: bool,
        related: Vec<Diagnostic>,
    ) {
        let index = self.iteration.pending.len();
        self.iteration.pending.push(IterationDiagnostic {
            node,
            input,
            allow_async,
            related,
        });
        self.defer_iteration_diagnostic(index);
    }
    pub(crate) fn report_iteration_diagnostic(&mut self, index: usize) -> Result<(), Error> {
        let pending = self
            .iteration
            .pending
            .get(index)
            .ok_or(ts_arena::Error::InvalidSlot)?;
        let (node, input, allow_async, related) = (
            pending.node,
            pending.input,
            pending.allow_async,
            pending.related.clone(),
        );
        let primary = self.report_type_not_iterable(node, input, allow_async)?;
        if let Some(primary) = primary {
            for diagnostic in related {
                self.add_related_diagnostic(primary, diagnostic)?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.reportTypeNotIterableError
    fn report_type_not_iterable(
        &mut self,
        node: NodeId,
        input: TypeId,
        allow_async: bool,
    ) -> Result<Option<usize>, Error> {
        let message = if allow_async {
            d::Type_0_must_have_a_Symbol_asyncIterator_method_that_returns_an_async_iterator
        } else {
            d::Type_0_must_have_a_Symbol_iterator_method_that_returns_an_iterator
        };
        let mut suggest = self.awaited_type_of_promise(input)?.is_some();
        if !suggest && !allow_async {
            if let Some(parent) = self.node(node)?.parent() {
                if self.node(parent)?.kind() == K::ForOfStatement
                    && self.node(parent)?.expression() == Some(node)
                {
                    let global = self.iteration_global("AsyncIterable", 3)?;
                    if global != self.builtins.empty_generic_type {
                        let target =
                            self.create_type_reference(global, &[self.builtins.any_type; 3])?;
                        suggest =
                            self.is_type_related_to(input, target, RelationKind::Assignable)?;
                    }
                }
            }
        }
        self.iteration_error(node, input, suggest, message)
    }
    fn iteration_error(
        &mut self,
        node: NodeId,
        input: TypeId,
        suggest: bool,
        message: &'static d::Message,
    ) -> Result<Option<usize>, Error> {
        let text = self.type_to_string(input, crate::type_display::DEFAULT_FLAGS)?;
        let primary = self.error_at(Some(node), message, vec![text])?;
        if let Some(primary) = primary.filter(|_| suggest) {
            let related =
                self.diagnostic_for_node(Some(node), d::Did_you_forget_to_use_await, vec![])?;
            self.add_related_diagnostic(primary, related)?;
        }
        Ok(primary)
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationDiagnosticDetails
    fn iteration_diagnostic_details(
        &mut self,
        use_: u32,
        input: TypeId,
        allow_string: bool,
    ) -> Result<(&'static d::Message, bool), Error> {
        if self.types.flags(input)? & tf::ANY == 0
            && self
                .iteration_types_of_iterable(input, use_, None)?
                .yield_type
                .is_some()
        {
            return Ok((d::Type_0_can_only_be_iterated_through_when_using_the_downlevelIteration_flag_or_with_a_target_of_es2015_or_higher,false));
        }
        if let Some(symbol) = self.types.get(input)?.symbol {
            if matches!(
                self.symbol(symbol)?.name_bytes(),
                b"Float32Array"
                    | b"Float64Array"
                    | b"Int16Array"
                    | b"Int32Array"
                    | b"Int8Array"
                    | b"NodeList"
                    | b"Uint16Array"
                    | b"Uint32Array"
                    | b"Uint8Array"
                    | b"Uint8ClampedArray"
            ) {
                return Ok((d::Type_0_can_only_be_iterated_through_when_using_the_downlevelIteration_flag_or_with_a_target_of_es2015_or_higher,true));
            }
        }
        Ok((
            if allow_string {
                d::Type_0_is_not_an_array_type_or_a_string_type
            } else {
                d::Type_0_is_not_an_array_type
            },
            true,
        ))
    }
    // port: tsc/internal/checker/flow.go:Checker.getPropertyNameForKnownSymbolName
    pub(crate) fn property_name_for_known_symbol(&mut self, name: &str) -> Result<JsString, Error> {
        if let Some(symbol) = self.lookup_symbol(
            self.builtins.globals,
            b"Symbol",
            ts_ast::symbol_flags::VALUE,
        )? {
            let constructor = self.get_type_of_symbol(symbol)?;
            if let Some(ty) = self.property_type(constructor, name.as_bytes())? {
                if let Some(key) = self.index_property_name(ty)? {
                    return Ok(key);
                }
            }
        }
        let mut key = ts_ast::INTERNAL_SYMBOL_NAME_PREFIX.to_vec();
        key.push(b'@');
        key.extend_from_slice(name.as_bytes());
        Ok(JsString::from_bytes(key))
    }
}

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForInOrForOfStatement
    pub(crate) fn check_grammar_for_await_context(
        &mut self,
        node: NodeId,
        modifier: NodeId,
    ) -> Result<bool, Error> {
        use ts_core::{ModuleKind as M, ScriptTarget};
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("for-await source"))?;
        if !self
            .ast(source)?
            .source_file(source)?
            .diagnostics()
            .is_empty()
        {
            return Ok(false);
        }
        if ts_ast::is_in_top_level_context(self.ast(node)?, node)? {
            let options = self.program()?.host.options();
            let module = options.emit_module_kind();
            let target = options.emit_script_target();
            let file = self.source_file_read(source)?;
            let external = file.external_module_indicator.is_some()
                || (module == M::COMMON_JS || M::NODE16 <= module && module <= M::NODE_NEXT)
                    && file.common_js_module_indicator().is_some();
            if !external {
                self.error_at(Some(modifier),d::X_for_await_loops_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module,vec![])?;
            }
            let node_module = matches!(module, M::NODE16 | M::NODE18 | M::NODE20 | M::NODE_NEXT);
            let common_js = if node_module {
                let file = self.source_file_read(source)?;
                self.program()?
                    .host
                    .get_source_file_meta_data(file.parse_options().file_name.as_bytes())?
                    .implied_node_format
                    == M::COMMON_JS
            } else {
                false
            };
            if common_js {
                self.error_at(
                    Some(modifier),
                    d::The_current_file_is_a_CommonJS_module_and_cannot_use_await_at_the_top_level,
                    vec![],
                )?;
            } else if !(node_module
                || matches!(module, M::ES2022 | M::ESNEXT | M::PRESERVE | M::SYSTEM))
                || target < ScriptTarget::ES2017
            {
                self.error_at(Some(modifier),d::Top_level_for_await_loops_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher,vec![])?;
            }
            Ok(false)
        } else {
            let mut diagnostic=self.diagnostic_for_node(Some(modifier),d::X_for_await_loops_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules,vec![])?;
            let mut current = self.node(node)?.parent();
            while let Some(function) = current {
                let read = self.node(function)?;
                if ts_ast::utilities::is_function_like(Some(&read)) {
                    if read.kind() != K::Constructor {
                        diagnostic.related_information.push(std::sync::Arc::new(
                            self.diagnostic_for_node(
                                Some(function),
                                d::Did_you_mean_to_mark_this_function_as_async,
                                vec![],
                            )?,
                        ));
                    }
                    break;
                }
                current = read.parent();
            }
            self.add_diagnostic(diagnostic)?;
            Ok(true)
        }
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkForOfStatement
    pub(crate) fn check_for_await_container(
        &mut self,
        node: NodeId,
        modifier: NodeId,
    ) -> Result<(), Error> {
        if let Some(container) = self.containing_function_or_static_block(node)? {
            let read = self.node(container)?;
            if read.kind() == K::ClassStaticBlockDeclaration {
                self.grammar_error_node(
                    modifier,
                    d::X_for_await_loops_cannot_be_used_inside_a_class_static_block,
                    vec![],
                )?;
            } else if read.body().is_some()
                && self.body_function_flags(container)?.0
                && self.program()?.host.options().emit_script_target()
                    < ts_core::ScriptTarget::ES2018
            {
                // for..await..of in an async function or async generator function prior to ESNext requires the __asyncValues helper
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::FOR_AWAIT_OF_INCLUDES,
                )?;
            }
        }
        Ok(())
    }
}
