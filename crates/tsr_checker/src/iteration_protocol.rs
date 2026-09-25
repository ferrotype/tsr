//! Synchronous and asynchronous iterator protocol extraction. Native global identities retain
//! their fast path; structural iterators combine every applicable signature.
use crate::{
    iteration::IterationTypes, type_flags as tf, CheckerState, Error, RelationKind, TypeId,
};
use tsr_arena::NodeId;
use tsr_ast::{symbol_flags as sf, Diagnostic, JsString};
use tsr_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIterableFast
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIteratorFast
    pub(crate) fn iteration_types_fast(
        &mut self,
        ty: TypeId,
        iterator: bool,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        for name in [
            if asynchronous {
                if iterator {
                    "AsyncIterator"
                } else {
                    "AsyncIterable"
                }
            } else {
                if iterator {
                    "Iterator"
                } else {
                    "Iterable"
                }
            },
            if asynchronous {
                "AsyncIteratorObject"
            } else {
                "IteratorObject"
            },
            if asynchronous {
                "AsyncIterableIterator"
            } else {
                "IterableIterator"
            },
            if asynchronous {
                "AsyncGenerator"
            } else {
                "Generator"
            },
        ] {
            let target = self.iteration_global(name, 3)?;
            if self.iteration_is_reference(ty, target)? {
                // The pin indexes the first three arguments; a reference resolved
                // through a this argument carries one more.
                let arguments = self.get_type_arguments(ty)?;
                if arguments.len() < 3 {
                    return Err(tsr_arena::Error::InvalidGraph.into());
                }
                return self.resolved_iteration_types(
                    arguments[0],
                    arguments[1],
                    arguments[2],
                    asynchronous,
                );
            }
        }
        let builtin_names: &[&'static str] = if asynchronous {
            &["ReadableStreamAsyncIterator"]
        } else {
            &[
                "ArrayIterator",
                "MapIterator",
                "SetIterator",
                "StringIterator",
            ]
        };
        let mut builtin_targets = Vec::with_capacity(builtin_names.len());
        for &name in builtin_names {
            builtin_targets.push(self.iteration_global(name, 1)?);
        }
        for target in builtin_targets {
            if self.iteration_is_reference(ty, target)? {
                let arguments = self.get_type_arguments(ty)?;
                let yield_type = *arguments.first().ok_or(tsr_arena::Error::InvalidGraph)?;
                let options = self.program()?.host.options();
                let return_type =
                    if options.strict_option_value(options.strict_builtin_iterator_return) {
                        self.builtins.undefined_type
                    } else {
                        self.builtins.any_type
                    };
                return self.resolved_iteration_types(
                    yield_type,
                    return_type,
                    self.builtins.unknown_type,
                    asynchronous,
                );
            }
        }
        Ok(IterationTypes::default())
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIterableSlow
    pub(crate) fn iteration_types_of_iterable_slow(
        &mut self,
        ty: TypeId,
        error_node: Option<NodeId>,
        diagnostics: &mut Vec<Diagnostic>,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        let name = self.property_name_for_known_symbol(if asynchronous {
            "asyncIterator"
        } else {
            "iterator"
        })?;
        if let Some(method) = self.constituent_property(ty, name.as_bytes(), false)? {
            if self.symbol(method)?.flags() & sf::OPTIONAL == 0 {
                let method_type = self.get_type_of_symbol(method)?;
                if self.types.flags(method_type)? & tf::ANY != 0 {
                    return Ok(IterationTypes::any(self.builtins.any_type));
                }
                let signatures = self.signatures_of_type(method_type, false)?;
                let mut returns = Vec::new();
                for &signature in &signatures {
                    if self.min_argument_count(signature)? == 0 {
                        returns.push(self.return_type_of_signature(signature)?);
                    }
                }
                if !returns.is_empty() {
                    let iterator = self.get_intersection_type(&returns)?;
                    return self.iteration_types_of_iterator(
                        iterator,
                        error_node,
                        diagnostics,
                        asynchronous,
                    );
                }
                if error_node.is_some() && !signatures.is_empty() {
                    let target = self.iteration_global_checked(
                        if asynchronous {
                            "AsyncIterable"
                        } else {
                            "Iterable"
                        },
                        3,
                    )?;
                    let (_, diagnostic) = self.check_type_related_ex(
                        ty,
                        target,
                        RelationKind::Assignable,
                        error_node,
                        None,
                    )?;
                    if let Some(diagnostic) = diagnostic {
                        diagnostics.push(diagnostic);
                    }
                }
            }
        }
        Ok(IterationTypes::default())
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIteratorWorker
    pub(crate) fn iteration_types_of_iterator(
        &mut self,
        ty: TypeId,
        error_node: Option<NodeId>,
        diagnostics: &mut Vec<Diagnostic>,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(IterationTypes::any(self.builtins.any_type));
        }
        let fast = self.iteration_types_fast(ty, true, asynchronous)?;
        if fast.has_types() {
            return Ok(fast);
        }
        let next =
            self.iteration_types_of_method(ty, b"next", error_node, diagnostics, asynchronous)?;
        let return_ =
            self.iteration_types_of_method(ty, b"return", error_node, diagnostics, asynchronous)?;
        let throw =
            self.iteration_types_of_method(ty, b"throw", error_node, diagnostics, asynchronous)?;
        self.combine_iteration_types(&[next, return_, throw])
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfMethod
    fn iteration_types_of_method(
        &mut self,
        ty: TypeId,
        name: &[u8],
        error_node: Option<NodeId>,
        diagnostics: &mut Vec<Diagnostic>,
        asynchronous: bool,
    ) -> Result<IterationTypes, Error> {
        let method = self.constituent_property(ty, name, false)?;
        if method.is_none() && name != b"next" {
            return Ok(IterationTypes::default());
        }
        let mut method_type = None;
        if let Some(method) = method {
            if name != b"next" || self.symbol(method)?.flags() & sf::OPTIONAL == 0 {
                let ty = self.get_type_of_symbol(method)?;
                method_type = Some(if name == b"next" {
                    ty
                } else {
                    self.type_with_facts(ty, crate::type_facts::NE_UNDEFINED_OR_NULL)?
                });
            }
        }
        let signatures = if let Some(method_type) = method_type {
            if self.types.flags(method_type)? & tf::ANY != 0 {
                return Ok(IterationTypes::any(self.builtins.any_type));
            }
            self.signatures_of_type(method_type, false)?
        } else {
            Vec::new()
        };
        if signatures.is_empty() {
            if let Some(node) = error_node {
                let message = if name == b"next" {
                    if asynchronous {
                        d::An_async_iterator_must_have_a_next_method
                    } else {
                        d::An_iterator_must_have_a_next_method
                    }
                } else {
                    if asynchronous {
                        d::The_0_property_of_an_async_iterator_must_be_a_method
                    } else {
                        d::The_0_property_of_an_iterator_must_be_a_method
                    }
                };
                diagnostics.push(self.diagnostic_for_node(
                    Some(node),
                    message,
                    vec![JsString::from_bytes(name)],
                )?);
            }
            return Ok(IterationTypes::default());
        }
        let method_type = method_type.ok_or(tsr_arena::Error::InvalidGraph)?;
        if signatures.len() == 1 {
            if let Some(method_symbol) = self.types.get(method_type)?.symbol {
                for global_name in if asynchronous {
                    ["AsyncGenerator", "AsyncIterator"]
                } else {
                    ["Generator", "Iterator"]
                } {
                    let global = self.iteration_global(global_name, 3)?;
                    if let Some(global_symbol) = self.types.get(global)?.symbol {
                        let member = self
                            .symbol(global_symbol)?
                            .members()
                            .map(|members| {
                                self.table(members).map(|table| table.get(name).flatten())
                            })
                            .transpose()?
                            .flatten();
                        if member == Some(method_symbol) {
                            let parameters =
                                self.types.interface(global)?.type_parameters().to_vec();
                            let mapper = self.types.object(method_type)?.mapper;
                            let yield_type = self.instantiate_type(parameters[0], mapper)?;
                            let return_type = self.instantiate_type(parameters[1], mapper)?;
                            let next_type = if name == b"next" {
                                Some(self.instantiate_type(parameters[2], mapper)?)
                            } else {
                                None
                            };
                            return Ok(IterationTypes {
                                yield_type: Some(yield_type),
                                return_type: Some(return_type),
                                next_type,
                            });
                        }
                    }
                }
            }
        }
        let mut parameter_types = Vec::new();
        let mut method_return_types = Vec::new();
        for &signature in &signatures {
            if name != b"throw"
                && self
                    .signatures
                    .get(signature)?
                    .parameters
                    .as_ref()
                    .is_some_and(|parameters| !parameters.is_empty())
            {
                parameter_types.push(
                    self.parameter_type_at(signature, 0)?
                        .unwrap_or(self.builtins.any_type),
                );
            }
            method_return_types.push(self.return_type_of_signature(signature)?);
        }
        let mut return_types = Vec::new();
        let mut next_type = None;
        if name != b"throw" {
            let parameter_type = if parameter_types.is_empty() {
                self.builtins.unknown_type
            } else {
                self.get_union_type(&parameter_types)?
            };
            if name == b"next" {
                next_type = Some(parameter_type);
            } else {
                return_types.push(
                    self.resolve_iteration_type(parameter_type, asynchronous, error_node)?
                        .unwrap_or(self.builtins.any_type),
                );
            }
        }
        let method_return_type = if method_return_types.is_empty() {
            self.builtins.never_type
        } else {
            self.get_intersection_type(&method_return_types)?
        };
        let method_return_type = self
            .resolve_iteration_type(method_return_type, asynchronous, error_node)?
            .unwrap_or(self.builtins.any_type);
        let result = self.iteration_types_of_iterator_result(method_return_type)?;
        let yield_type = if result.has_types() {
            return_types.extend(result.return_type);
            result.yield_type
        } else {
            if let Some(node) = error_node {
                diagnostics.push(self.diagnostic_for_node(
                    Some(node),
                    if asynchronous {d::The_type_returned_by_the_0_method_of_an_async_iterator_must_be_a_promise_for_a_type_with_a_value_property} else {d::The_type_returned_by_the_0_method_of_an_iterator_must_have_a_value_property},
                    vec![JsString::from_bytes(name)],
                )?);
            }
            return_types.push(self.builtins.any_type);
            Some(self.builtins.any_type)
        };
        Ok(IterationTypes {
            yield_type,
            return_type: Some(self.get_union_type(&return_types)?),
            next_type,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.getIterationTypesOfIteratorResult
    fn iteration_types_of_iterator_result(&mut self, ty: TypeId) -> Result<IterationTypes, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(IterationTypes::any(self.builtins.any_type));
        }
        for (name, yield_) in [
            ("IteratorYieldResult", true),
            ("IteratorReturnResult", false),
        ] {
            let target = self.iteration_global(name, 1)?;
            if self.iteration_is_reference(ty, target)? {
                let arguments = self.get_type_arguments(ty)?;
                let argument = *arguments.first().ok_or(tsr_arena::Error::InvalidGraph)?;
                return Ok(IterationTypes {
                    yield_type: yield_.then_some(argument),
                    return_type: (!yield_).then_some(argument),
                    next_type: None,
                });
            }
        }
        let yield_result = self.filter_type(ty, &mut |checker, part| {
            checker.is_iteration_result(part, true)
        })?;
        let yield_type = if yield_result == self.builtins.never_type {
            None
        } else {
            self.property_type(yield_result, b"value")?
        };
        let return_result = self.filter_type(ty, &mut |checker, part| {
            checker.is_iteration_result(part, false)
        })?;
        let return_type = if return_result == self.builtins.never_type {
            None
        } else {
            self.property_type(return_result, b"value")?
        };
        if yield_type.is_none() && return_type.is_none() {
            return Ok(IterationTypes::default());
        }
        Ok(IterationTypes {
            yield_type,
            return_type: Some(return_type.unwrap_or(self.builtins.void_type)),
            next_type: None,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.isIteratorResult
    fn is_iteration_result(&mut self, ty: TypeId, yield_: bool) -> Result<bool, Error> {
        let done = self
            .property_type(ty, b"done")?
            .unwrap_or(self.builtins.false_type);
        self.is_type_related_to(
            if yield_ {
                self.builtins.false_type
            } else {
                self.builtins.true_type
            },
            done,
            RelationKind::Assignable,
        )
    }
}
