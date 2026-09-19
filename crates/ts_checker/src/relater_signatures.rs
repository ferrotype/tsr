//! Signature relations retain the caller's mode, recursion assumptions and
//! intersection state while comparing parameters, returns and predicates.
use crate::{
    object_flags as of,
    relater::{Relater, RelationKind, BOTH},
    signature_flags as sg, ternary as tr, Error, SignatureId, Ternary, TypeId, TypePredicateKind,
};

const BIVARIANT_CALLBACK: u32 = 1;
const STRICT_CALLBACK: u32 = 2;
const CALLBACK: u32 = BIVARIANT_CALLBACK | STRICT_CALLBACK;
pub(crate) const IGNORE_RETURN_TYPES: u32 = 4;
const STRICT_ARITY: u32 = 8;
const STRICT_TOP: u32 = 16;

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.signaturesRelatedTo
    pub(crate) fn signatures_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        construct: bool,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        if self.kind != RelationKind::Identity {
            if source == self.checker.builtins.any_function_type {
                return Ok(tr::TRUE);
            }
            if target == self.checker.builtins.any_function_type {
                return Ok(tr::FALSE);
            }
        }
        let sources = self.checker.signatures_of_type(source, construct)?;
        let targets = self.checker.signatures_of_type(target, construct)?;
        if self.kind == RelationKind::Identity {
            if sources.len() != targets.len() {
                return Ok(tr::FALSE);
            }
            let mut result = tr::TRUE;
            for (&source, &target) in sources.iter().zip(&targets) {
                result &= self.signatures_identical(source, target)?;
                if result == tr::FALSE {
                    break;
                }
            }
            return Ok(result);
        }
        if construct && !sources.is_empty() && !targets.is_empty() {
            if self.checker.signatures.get(sources[0])?.flags & sg::ABSTRACT != 0
                && self.checker.signatures.get(targets[0])?.flags & sg::ABSTRACT == 0
            {
                // An abstract constructor type is not assignable to a non-abstract
                // constructor type, as it would otherwise be possible to new an
                // abstract class.
                if self.report_errors {
                    self.report_error(
                        ts_diagnostics::Cannot_assign_an_abstract_constructor_type_to_a_non_abstract_constructor_type,
                        vec![],
                    );
                }
                return Ok(tr::FALSE);
            }
            if !self.constructor_visibilities_are_compatible(sources[0], targets[0])? {
                return Ok(tr::FALSE);
            }
        }
        let sr = *self.checker.types.get(source)?;
        let tt = *self.checker.types.get(target)?;
        let paired = sr.object_flags & tt.object_flags & of::INSTANTIATED != 0
            && sr.symbol == tt.symbol
            || sr.object_flags & tt.object_flags & of::REFERENCE != 0
                && self.checker.types.target(source)? == self.checker.types.target(target)?;
        if paired {
            if sources.len() != targets.len() {
                return Err(Error::MissingLink("instantiated signature arity"));
            }
            let mut result = tr::TRUE;
            for (&source, &target) in sources.iter().zip(&targets) {
                result &= self.signature_related(source, target, true, intersection)?;
                if result == tr::FALSE {
                    break;
                }
            }
            return Ok(result);
        }
        if sources.len() == 1 && targets.len() == 1 {
            return self.signature_related(
                sources[0],
                targets[0],
                self.kind == RelationKind::Comparable,
                intersection,
            );
        }
        let mut result = tr::TRUE;
        for target in targets {
            let mut related = tr::FALSE;
            let saved = self.errors.clone();
            for (index, &source) in sources.iter().enumerate() {
                related = self.with_reporting(self.report_errors && index == 0, |this| {
                    this.signature_related(source, target, true, intersection)
                })?;
                if related != tr::FALSE {
                    self.errors = saved;
                    break;
                }
            }
            if related == tr::FALSE && sources.is_empty() && self.report_errors {
                let source = self
                    .checker
                    .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                let target = self.checker.signature_to_string(target)?;
                self.report_error(
                    ts_diagnostics::Type_0_provides_no_match_for_the_signature_1,
                    vec![source, target],
                );
            }
            result &= related;
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.signatureRelatedTo
    fn signature_related(
        &mut self,
        mut source: SignatureId,
        mut target: SignatureId,
        erase: bool,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let mode = match self.kind {
            RelationKind::Subtype => STRICT_TOP,
            RelationKind::StrictSubtype => STRICT_TOP | STRICT_ARITY,
            _ => 0,
        };
        if erase {
            source = self.checker.erased_signature(source)?;
            target = self.checker.erased_signature(target)?;
        }
        self.compare_signatures(source, target, mode, intersection)
    }

    // port: tsc/internal/checker/relater.go:Checker.compareSignaturesRelated
    pub(crate) fn compare_signatures(
        &mut self,
        mut source: SignatureId,
        target: SignatureId,
        mode: u32,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        if source == target {
            return Ok(tr::TRUE);
        }
        let source_top = if mode & STRICT_TOP != 0 {
            self.checker.top_signature(source)?
        } else {
            false
        };
        let target_top = self.checker.top_signature(target)?;
        if !source_top && target_top {
            return Ok(tr::TRUE);
        }
        if source_top && !target_top {
            return Ok(tr::FALSE);
        }
        let target_count = self.checker.parameter_count(target)?;
        if !self.checker.effective_rest_parameter(target)? {
            let more = if mode & STRICT_ARITY != 0 {
                self.checker.effective_rest_parameter(source)?
                    || self.checker.parameter_count(source)? > target_count
            } else {
                self.checker.min_argument_count(source)? > target_count
            };
            if more {
                if self.report_errors && mode & STRICT_ARITY == 0 {
                    let count = self.checker.min_argument_count(source)?;
                    self.report_error(ts_diagnostics::Target_signature_provides_too_few_arguments_Expected_0_or_more_but_got_1, vec![ts_ast::JsString::from_bytes(count.to_string().into_bytes()), ts_ast::JsString::from_bytes(target_count.to_string().into_bytes())]);
                }
                return Ok(tr::FALSE);
            }
        }
        let source_parameters = self.checker.signatures.get(source)?.type_parameters.clone();
        if source_parameters
            .as_ref()
            .is_some_and(|types| !types.is_empty())
            && source_parameters != self.checker.signatures.get(target)?.type_parameters
        {
            source = self.checker.contextual_signature_instantiation(
                source,
                target,
                None,
                Some(self.frame),
            )?;
        }
        let source_count = self.checker.parameter_count(source)?;
        let source_rest = self.checker.non_array_rest_type(source)?;
        let target_rest = self.checker.non_array_rest_type(target)?;
        if let Some(rest) = source_rest.or(target_rest) {
            self.checker.report_unreliable_markers(rest, false)?;
        }
        let strict = self.checker.strict_function_types();
        let kind = self
            .checker
            .signatures
            .get(target)?
            .declaration
            .map(|node| {
                self.checker
                    .ast(node)?
                    .node(node)
                    .map(|node| node.kind())
                    .map_err(Error::from)
            })
            .transpose()?;
        let method = kind.is_some_and(|kind| {
            matches!(
                kind.known(),
                Some(
                    ts_ast::SyntaxKind::MethodDeclaration
                        | ts_ast::SyntaxKind::MethodSignature
                        | ts_ast::SyntaxKind::Constructor
                )
            )
        });
        let strict_variance = mode & CALLBACK == 0 && strict && !method;
        let mut result = tr::TRUE;
        let source_this = self
            .checker
            .signatures
            .get(source)?
            .this_parameter
            .map(|symbol| self.checker.get_type_of_symbol(symbol))
            .transpose()?;
        if let Some(source_this) = source_this.filter(|&ty| ty != self.checker.builtins.void_type) {
            if let Some(target_this) = self
                .checker
                .signatures
                .get(target)?
                .this_parameter
                .map(|symbol| self.checker.get_type_of_symbol(symbol))
                .transpose()?
            {
                let mut related = if strict_variance {
                    tr::FALSE
                } else {
                    self.related_with_errors(source_this, target_this, BOTH, intersection, false)?
                };
                if related == tr::FALSE {
                    related = self.related(target_this, source_this, BOTH, intersection)?;
                }
                if related == tr::FALSE {
                    if self.report_errors {
                        self.report_error(
                            ts_diagnostics::The_this_types_of_each_signature_are_incompatible,
                            vec![],
                        );
                    }
                    return Ok(tr::FALSE);
                }
                result &= related;
            }
        }
        let has_rest = source_rest.is_some() || target_rest.is_some();
        let count = if has_rest {
            source_count.min(target_count)
        } else {
            source_count.max(target_count)
        };
        for i in 0..count {
            let source_type = if has_rest && i + 1 == count {
                Some(self.checker.rest_or_any_at(source, i)?)
            } else {
                self.checker.parameter_type_at(source, i)?
            };
            let target_type = if has_rest && i + 1 == count {
                Some(self.checker.rest_or_any_at(target, i)?)
            } else {
                self.checker.parameter_type_at(target, i)?
            };
            let (Some(source_type), Some(target_type)) = (source_type, target_type) else {
                continue;
            };
            if source_type == target_type && mode & STRICT_ARITY == 0 {
                continue;
            }
            let source_callback = if mode & CALLBACK == 0
                && !self.checker.instantiated_generic_parameter(source, i)?
            {
                self.checker.non_nullable_single_signature(source_type)?
            } else {
                None
            };
            let target_callback = if mode & CALLBACK == 0
                && !self.checker.instantiated_generic_parameter(target, i)?
            {
                self.checker.non_nullable_single_signature(target_type)?
            } else {
                None
            };
            let callbacks = match (source_callback, target_callback) {
                (Some(s), Some(t))
                    if self.checker.type_predicate_of_signature(s)?.is_none()
                        && self.checker.type_predicate_of_signature(t)?.is_none() =>
                {
                    if self.checker.nullable_parameter_facts(source_type)?
                        == self.checker.nullable_parameter_facts(target_type)?
                    {
                        Some((s, t))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let mut related = if let Some((s, t)) = callbacks {
                self.compare_signatures(
                    t,
                    s,
                    mode & STRICT_ARITY
                        | if strict_variance {
                            STRICT_CALLBACK
                        } else {
                            BIVARIANT_CALLBACK
                        },
                    intersection,
                )?
            } else {
                let first = if mode & CALLBACK == 0 && !strict_variance {
                    self.related_with_errors(source_type, target_type, BOTH, intersection, false)?
                } else {
                    tr::FALSE
                };
                if first == tr::FALSE {
                    self.related(target_type, source_type, BOTH, intersection)?
                } else {
                    first
                }
            };
            if related != tr::FALSE
                && mode & STRICT_ARITY != 0
                && i >= self.checker.min_argument_count(source)?
                && i < self.checker.min_argument_count(target)?
                && self.related_with_errors(source_type, target_type, BOTH, intersection, false)?
                    != tr::FALSE
            {
                related = tr::FALSE;
            }
            result &= related;
            if result == tr::FALSE {
                if self.report_errors {
                    let source = self.checker.parameter_name_at(source, i)?;
                    let target = self.checker.parameter_name_at(target, i)?;
                    self.report_error(
                        ts_diagnostics::Types_of_parameters_0_and_1_are_incompatible,
                        vec![source, target],
                    );
                }
                return Ok(result);
            }
        }
        if mode & IGNORE_RETURN_TYPES != 0 {
            return Ok(result);
        }
        let target_return = self.checker.non_circular_return_type(target)?;
        if target_return == self.checker.builtins.void_type
            || target_return == self.checker.builtins.any_type
        {
            return Ok(result);
        }
        let source_return = self.checker.non_circular_return_type(source)?;
        if let Some(target_predicate) = self.checker.type_predicate_of_signature(target)? {
            if let Some(source_predicate) = self.checker.type_predicate_of_signature(source)? {
                result &=
                    self.predicate_related(source_predicate, target_predicate, intersection)?;
            } else if matches!(
                self.checker.signatures.predicate(target_predicate)?.kind,
                TypePredicateKind::This | TypePredicateKind::Identifier
            ) {
                if self.report_errors {
                    let signature = self.checker.signature_to_string(source)?;
                    self.report_error(
                        ts_diagnostics::Signature_0_must_be_a_type_predicate,
                        vec![signature],
                    );
                }
                return Ok(tr::FALSE);
            }
        } else {
            let reverse = if mode & BIVARIANT_CALLBACK != 0 {
                self.related_with_errors(target_return, source_return, BOTH, intersection, false)?
            } else {
                tr::FALSE
            };
            result &= if reverse == tr::FALSE {
                self.related(source_return, target_return, BOTH, intersection)?
            } else {
                reverse
            };
            if result == tr::FALSE && self.report_errors {
                let construct = self.checker.signatures.get(source)?.flags & sg::CONSTRUCT != 0;
                let no_arguments = self
                    .checker
                    .signatures
                    .get(source)?
                    .parameters
                    .as_ref()
                    .is_none_or(|p| p.is_empty())
                    && self
                        .checker
                        .signatures
                        .get(target)?
                        .parameters
                        .as_ref()
                        .is_none_or(|p| p.is_empty());
                let message = match (construct, no_arguments) {
                    (false, true) => ts_diagnostics::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1,
                    (true, true) => ts_diagnostics::Construct_signatures_with_no_arguments_have_incompatible_return_types_0_and_1,
                    (false, false) => ts_diagnostics::Call_signature_return_types_0_and_1_are_incompatible,
                    (true, false) => ts_diagnostics::Construct_signature_return_types_0_and_1_are_incompatible,
                };
                let source = self
                    .checker
                    .type_to_string(source_return, crate::type_display::DEFAULT_FLAGS)?;
                let target = self
                    .checker
                    .type_to_string(target_return, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(message, vec![source, target]);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Checker.compareTypePredicateRelatedTo
    fn predicate_related(
        &mut self,
        source: crate::TypePredicateId,
        target: crate::TypePredicateId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let s = self.checker.signatures.predicate(source)?.clone();
        let t = self.checker.signatures.predicate(target)?.clone();
        let related = if s.kind != t.kind {
            if self.report_errors {
                self.report_error(ts_diagnostics::A_this_based_type_guard_is_not_compatible_with_a_parameter_based_type_guard, vec![]);
            }
            tr::FALSE
        } else if matches!(
            s.kind,
            TypePredicateKind::Identifier | TypePredicateKind::AssertsIdentifier
        ) && s.parameter_index != t.parameter_index
        {
            if self.report_errors {
                self.report_error(
                    ts_diagnostics::Parameter_0_is_not_in_the_same_position_as_parameter_1,
                    vec![s.parameter_name, t.parameter_name],
                );
            }
            tr::FALSE
        } else if s.t == t.t {
            tr::TRUE
        } else {
            match (s.t, t.t) {
                (Some(s), Some(t)) => self.related(s, t, BOTH, intersection)?,
                _ => tr::FALSE,
            }
        };
        if related == tr::FALSE && self.report_errors {
            let source = self.checker.type_predicate_to_string(source)?;
            let target = self.checker.type_predicate_to_string(target)?;
            self.report_error(
                ts_diagnostics::Type_predicate_0_is_not_assignable_to_1,
                vec![source, target],
            );
        }
        Ok(related)
    }

    pub(crate) fn signatures_identical(
        &mut self,
        source: SignatureId,
        target: SignatureId,
    ) -> Result<Ternary, Error> {
        let frame = self.frame;
        self.checker.compare_signatures_identical(
            source,
            target,
            false,
            false,
            false,
            &mut |checker, source, target| checker.compare_in_relation_frame(frame, source, target),
        )
    }

    // port: tsc/internal/checker/relater.go:Relater.constructorVisibilitiesAreCompatible
    fn constructor_visibilities_are_compatible(
        &mut self,
        source: crate::SignatureId,
        target: crate::SignatureId,
    ) -> Result<bool, Error> {
        use ts_ast::modifier_flags as mf;
        let (Some(source_declaration), Some(target_declaration)) = (
            self.checker.signatures.get(source)?.declaration,
            self.checker.signatures.get(target)?.declaration,
        ) else {
            return Ok(true);
        };
        let accessibility = |checker: &crate::CheckerState, node: ts_arena::NodeId| {
            Ok::<_, Error>(
                checker
                    .ast(node)?
                    .node(node)?
                    .modifier_flags(checker.ast(node)?)?
                    & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER,
            )
        };
        let source_accessibility = accessibility(self.checker, source_declaration)?;
        let target_accessibility = accessibility(self.checker, target_declaration)?;
        // A public, protected and private signature is assignable to a private signature.
        if target_accessibility == mf::PRIVATE {
            return Ok(true);
        }
        // A public and protected signature is assignable to a protected signature.
        if target_accessibility == mf::PROTECTED && source_accessibility != mf::PRIVATE {
            return Ok(true);
        }
        // Only a public signature is assignable to public signature.
        if target_accessibility != mf::PROTECTED && source_accessibility == 0 {
            return Ok(true);
        }
        if self.report_errors {
            let visibility = |flags: u32| -> ts_ast::JsString {
                ts_ast::JsString::from_bytes(if flags == mf::PRIVATE {
                    b"private".as_slice()
                } else if flags == mf::PROTECTED {
                    b"protected".as_slice()
                } else {
                    b"public".as_slice()
                })
            };
            self.report_error(
                ts_diagnostics::Cannot_assign_a_0_constructor_type_to_a_1_constructor_type,
                vec![
                    visibility(source_accessibility),
                    visibility(target_accessibility),
                ],
            );
        }
        Ok(false)
    }
}
