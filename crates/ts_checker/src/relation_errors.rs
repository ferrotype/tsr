//! Error elaboration is part of the relation operation: failed cached pairs are
//! revisited while reporting, and speculative alternatives preserve the chain.
use crate::{
    relater::{Relater, RelationKind},
    type_flags as tf, type_format_flags as fmt, CheckerState, Error, Ternary, TypeId,
};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{Diagnostic, JsString};
use ts_diagnostics::{self as messages, Message};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ErrorEntry {
    pub message: &'static Message,
    pub args: Vec<JsString>,
}
#[derive(Clone, Default)]
pub(crate) struct RelationErrors {
    pub chain: Vec<ErrorEntry>,
    pub related: Vec<Arc<Diagnostic>>,
    /// `Relater.errorNode`; excess-property reporting narrows it to the
    /// offending property name.
    pub error_node: Option<NodeId>,
}

impl Relater<'_> {
    pub(crate) fn with_reporting<T>(
        &mut self,
        report: bool,
        run: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let saved = self.report_errors;
        self.report_errors = report;
        let result = run(self);
        self.report_errors = saved;
        result
    }

    pub(crate) fn related_with_errors(
        &mut self,
        source: TypeId,
        target: TypeId,
        recursion: u32,
        intersection: u32,
        report: bool,
    ) -> Result<Ternary, Error> {
        self.with_reporting(report, |this| {
            this.related(source, target, recursion, intersection)
        })
    }

    pub(crate) fn error_diagnostic(
        &self,
        node: Option<NodeId>,
    ) -> Result<Option<Diagnostic>, Error> {
        let mut result = None;
        for entry in &self.errors.chain {
            if entry.message.elided_in_compatibility_pyramid {
                continue;
            }
            result = Some(if let Some(child) = result {
                Diagnostic::chain(Some(Arc::new(child)), entry.message, entry.args.clone())
            } else {
                let mut diagnostic =
                    self.checker
                        .diagnostic_for_node(node, entry.message, entry.args.clone())?;
                diagnostic
                    .related_information
                    .clone_from(&self.errors.related);
                diagnostic
            });
        }
        Ok(result)
    }

    pub(crate) fn chain_message(&self, offset: usize) -> Option<&'static Message> {
        self.errors
            .chain
            .len()
            .checked_sub(offset + 1)
            .map(|i| self.errors.chain[i].message)
    }

    // port: tsc/internal/checker/relater.go:Relater.reportError
    pub(crate) fn report_error(&mut self, mut message: &'static Message, mut args: Vec<JsString>) {
        if message.key == messages::Types_of_property_0_are_incompatible.key {
            if matches!(self.chain_message(0).map(|m| m.code), Some(2353 | 2561)) {
                return;
            }
            let suffix = match self.chain_message(1) {
                Some(m) if m.key == messages::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1.key => Some((b"".as_slice(), b"()".as_slice())),
                Some(m) if m.key == messages::Construct_signatures_with_no_arguments_have_incompatible_return_types_0_and_1.key => Some((b"new ".as_slice(), b"()".as_slice())),
                Some(m) if m.key == messages::Call_signature_return_types_0_and_1_are_incompatible.key => Some((b"".as_slice(), b"(...)".as_slice())),
                Some(m) if m.key == messages::Construct_signature_return_types_0_and_1_are_incompatible.key => Some((b"new ".as_slice(), b"(...)".as_slice())),
                _ => None,
            };
            if let Some((prefix, suffix)) = suffix {
                let mut name = prefix.to_vec();
                name.extend(property_name(args[0].as_bytes()));
                name.extend(suffix);
                args[0] = JsString::from_bytes(name);
                message = messages::The_types_returned_by_0_are_incompatible_between_these_types;
                self.errors.chain.truncate(self.errors.chain.len() - 2);
            }
            if self.chain_message(1).is_some_and(|m| {
                m.key == messages::Types_of_property_0_are_incompatible.key
                    || m.key == messages::The_types_of_0_are_incompatible_between_these_types.key
                    || m.key
                        == messages::The_types_returned_by_0_are_incompatible_between_these_types
                            .key
            }) {
                let head = property_name(args[0].as_bytes());
                let tail = property_name(
                    self.errors.chain[self.errors.chain.len() - 2].args[0].as_bytes(),
                );
                args[0] = dotted_name(&head, &tail);
                self.errors.chain.truncate(self.errors.chain.len() - 2);
                if message.key == messages::Types_of_property_0_are_incompatible.key {
                    message = messages::The_types_of_0_are_incompatible_between_these_types;
                }
                self.report_error(message, args);
                return;
            }
        }
        self.errors.chain.push(ErrorEntry { message, args });
    }

    // port: tsc/internal/checker/relater.go:Relater.reportErrorResults
    pub(crate) fn report_error_results(
        &mut self,
        original_source: TypeId,
        original_target: TypeId,
        mut source: TypeId,
        mut target: TypeId,
        head: Option<&'static Message>,
    ) -> Result<(), Error> {
        let source_has_base = self
            .checker
            .single_equivalent_base(original_source)?
            .is_some();
        let target_has_base = self
            .checker
            .single_equivalent_base(original_target)?
            .is_some();
        if self.checker.types.get(original_source)?.alias.is_some() || source_has_base {
            source = original_source;
        }
        if self.checker.types.get(original_target)?.alias.is_some() || target_has_base {
            target = original_target;
        }
        if self.checker.types.flags(source)? & self.checker.types.flags(target)? & tf::OBJECT != 0 {
            let readonly = if self.checker.is_tuple_type(source)? {
                self.checker
                    .types
                    .tuple(self.checker.types.target(source)?)?
                    .readonly
            } else {
                self.checker.is_array_type(source)?
                    && self.checker.query.global_types.get("ReadonlyArray")
                        == Some(&self.checker.types.target(source)?)
            };
            let mutable = if self.checker.is_tuple_type(target)? {
                !self
                    .checker
                    .types
                    .tuple(self.checker.types.target(target)?)?
                    .readonly
            } else {
                self.checker.is_array_type(target)?
                    && self.checker.query.global_types.get("Array")
                        == Some(&self.checker.types.target(target)?)
            };
            if readonly && mutable {
                let source = self
                    .checker
                    .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                let target = self
                    .checker
                    .type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    messages::The_type_0_is_readonly_and_cannot_be_assigned_to_the_mutable_type_1,
                    vec![source, target],
                );
            }
        }
        let source_flags = self.checker.types.flags(source)?;
        let target_flags = self.checker.types.flags(target)?;
        if source_flags & tf::OBJECT != 0 && target_flags & tf::PRIMITIVE != 0 {
            self.try_elaborate_errors_for_primitives_and_objects(source, target)?;
        } else if self.checker.types.get(source)?.symbol.is_some()
            && source_flags & tf::OBJECT != 0
            && self.checker.get_global_type("Object", 0, false)? == source
        {
            self.report_error(
                messages::The_Object_type_is_assignable_to_very_few_other_types_Did_you_mean_to_use_the_any_type_instead,
                vec![],
            );
        }
        if self.checker.types.flags(original_target)? & tf::INTERSECTION != 0
            && self.checker.types.object_flags(original_target)?
                & crate::object_flags::IS_NEVER_INTERSECTION
                != 0
        {
            if let Some((message, property)) =
                self.checker.never_intersection_cause(original_target)?
            {
                let target = self
                    .checker
                    .type_to_string(original_target, fmt::NO_TYPE_REDUCTION)?;
                let name = self.checker.symbol_to_string(property)?;
                self.report_error(message, vec![target, name]);
            }
        }
        self.report_relation_error(source, target, head)
    }

    // port: tsc/internal/checker/relater.go:Relater.reportRelationError
    fn report_relation_error(
        &mut self,
        source: TypeId,
        target: TypeId,
        head: Option<&'static Message>,
    ) -> Result<(), Error> {
        let (source_name, target_name) =
            self.checker.type_names_for_error_display(source, target)?;
        let mut generalized = source;
        let mut generalized_name = source_name.clone();
        if self.checker.types.flags(target)? & tf::NEVER == 0
            && self.checker.is_literal_type(source)?
            && !self.checker.type_could_have_top_level_singletons(target)?
        {
            generalized = self.checker.base_literal_type(source)?;
            generalized_name = self.checker.type_name_for_error_display(generalized)?;
        }
        let target_flags = if self.checker.types.flags(target)? & tf::INDEXED_ACCESS != 0
            && self.checker.types.flags(source)? & tf::INDEXED_ACCESS == 0
        {
            self.checker
                .types
                .flags(self.checker.types.indexed_access(target)?.object_type)?
        } else {
            self.checker.types.flags(target)?
        };
        if target_flags & tf::TYPE_PARAMETER != 0
            && !self.checker.variance.markers.contains(&target)
        {
            let constraint = self.checker.base_constraint_of_type(target)?;
            if let Some(constraint) = constraint {
                let name = if self.checker.is_type_related_to(
                    generalized,
                    constraint,
                    RelationKind::Assignable,
                )? {
                    Some(generalized_name.clone())
                } else if self.checker.is_type_related_to(
                    source,
                    constraint,
                    RelationKind::Assignable,
                )? {
                    Some(source_name.clone())
                } else {
                    None
                };
                if let Some(name) = name {
                    let constraint = self
                        .checker
                        .type_to_string(constraint, crate::type_display::DEFAULT_FLAGS)?;
                    self.report_error(messages::X_0_is_assignable_to_the_constraint_of_type_1_but_1_could_be_instantiated_with_a_different_subtype_of_constraint_2, vec![name, target_name.clone(), constraint]);
                } else {
                    self.errors.chain.clear();
                    self.report_error(messages::X_0_could_be_instantiated_with_an_arbitrary_type_which_could_be_unrelated_to_1, vec![target_name.clone(), generalized_name.clone()]);
                }
            } else {
                self.errors.chain.clear();
                self.report_error(messages::X_0_could_be_instantiated_with_an_arbitrary_type_which_could_be_unrelated_to_1, vec![target_name.clone(), generalized_name.clone()]);
            }
        }
        let exact_optional_mismatch = |this: &mut Self| -> Result<bool, Error> {
            Ok(this.checker.options.exact_optional_property_types
                && !this
                    .checker
                    .exact_optional_unassignable_properties(source, target)?
                    .is_empty())
        };
        let message = if let Some(head) = head {
            if head.code
                == messages::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1.code
                && exact_optional_mismatch(self)?
            {
                messages::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_types_of_the_target_s_properties
            } else {
                head
            }
        } else if self.kind == RelationKind::Comparable {
            messages::Type_0_is_not_comparable_to_type_1
        } else if source_name == target_name {
            messages::Type_0_is_not_assignable_to_type_1_Two_different_types_with_this_name_exist_but_they_are_unrelated
        } else if exact_optional_mismatch(self)? {
            messages::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_types_of_the_target_s_properties
        } else {
            messages::Type_0_is_not_assignable_to_type_1
        };
        if head.is_none()
            && self.kind != RelationKind::Comparable
            && source_name != target_name
            && self.checker.types.flags(source)? & tf::STRING_LITERAL != 0
            && self.checker.types.flags(target)? & tf::UNION != 0
        {
            if let Some(suggested) = self.checker.suggested_literal_type(source, target)? {
                let suggested = self
                    .checker
                    .type_to_string(suggested, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    messages::Type_0_is_not_assignable_to_type_1_Did_you_mean_2,
                    vec![generalized_name, target_name, suggested],
                );
                return Ok(());
            }
        }
        if let Some(head) = self.errors.chain.last() {
            match head.message.code {
                2353 | 2561 => return Ok(()),
                2859 | 4104 if head.args == [generalized_name.clone(), target_name.clone()] => {
                    return Ok(())
                }
                2741 if !is_conversion_or_interface_implementation_message(message)
                    && head.args.get(1) == Some(&generalized_name)
                    && head.args.get(2) == Some(&target_name) =>
                {
                    return Ok(())
                }
                2739 | 2740
                    if !is_conversion_or_interface_implementation_message(message)
                        && head.args.first() == Some(&generalized_name)
                        && head.args.get(1) == Some(&target_name) =>
                {
                    return Ok(())
                }
                _ => {}
            }
        }
        self.report_error(message, vec![generalized_name, target_name]);
        Ok(())
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.tryElaborateErrorsForPrimitivesAndObjects
    fn try_elaborate_errors_for_primitives_and_objects(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        let wrapper =
            |name: &'static str, primitive: TypeId| -> (&'static str, TypeId) { (name, primitive) };
        let candidates = [
            wrapper("String", self.checker.builtins.string_type),
            wrapper("Number", self.checker.builtins.number_type),
            wrapper("Boolean", self.checker.builtins.boolean_type),
            wrapper("Symbol", self.checker.builtins.es_symbol_type),
        ];
        for (name, primitive) in candidates {
            if target != primitive {
                continue;
            }
            if self.checker.get_global_type(name, 0, false)? == source {
                let target = self
                    .checker
                    .type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
                let source = self
                    .checker
                    .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                self.report_error(
                    messages::X_0_is_a_primitive_but_1_is_a_wrapper_object_Prefer_using_0_when_possible,
                    vec![target, source],
                );
            }
            return Ok(());
        }
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:isStaticPrivateIdentifierProperty
    pub(crate) fn is_static_private_identifier_property(
        &self,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        let view = self.ast(declaration)?;
        Ok(
            ts_ast::utilities::is_private_identifier_class_element_declaration(view, declaration)?
                && ts_ast::utilities::is_static(view, declaration)?,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getExactOptionalUnassignableProperties
    pub(crate) fn exact_optional_unassignable_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Vec<SymbolId>, Error> {
        if self.is_tuple_type(source)? && self.is_tuple_type(target)? {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for property in self.get_properties_of_type(target)? {
            let name = self.symbol(property)?.name_to_owned();
            let Some(source_type) = self.property_type(source, name.as_bytes())? else {
                continue;
            };
            let target_type = self.get_type_of_symbol(property)?;
            if self.is_exact_optional_property_mismatch(source_type, target_type)? {
                result.push(property);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isExactOptionalPropertyMismatch
    fn is_exact_optional_property_mismatch(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        Ok(
            self.maybe_type_of_kind(source, tf::UNDEFINED)?
                && self.contains_missing_type(target)?,
        )
    }

    /// Mirrors containsMissingType in checker.go.
    fn contains_missing_type(&self, ty: TypeId) -> Result<bool, Error> {
        if ty == self.builtins.missing_type {
            return Ok(true);
        }
        Ok(self.types.flags(ty)? & tf::UNION != 0
            && self
                .types
                .types_of(ty)?
                .contains(&self.builtins.missing_type))
    }

    // port: tsc/internal/checker/relater.go:Checker.getTypeNamesForErrorDisplay
    pub(crate) fn type_names_for_error_display(
        &mut self,
        left: TypeId,
        right: TypeId,
    ) -> Result<(JsString, JsString), Error> {
        let mut left_name = self.type_name_for_error_display_ex(left)?;
        let mut right_name = self.type_name_for_error_display_ex(right)?;
        if left_name == right_name {
            left_name = self.type_name_for_error_display(left)?;
            right_name = self.type_name_for_error_display(right)?;
        }
        Ok((left_name, right_name))
    }

    // port: tsc/internal/checker/relater.go:Checker.getTypeNameForErrorDisplay
    pub(crate) fn type_name_for_error_display(&mut self, ty: TypeId) -> Result<JsString, Error> {
        self.type_to_string(ty, fmt::USE_FULLY_QUALIFIED_TYPE)
    }

    fn type_name_for_error_display_ex(&mut self, ty: TypeId) -> Result<JsString, Error> {
        let symbol = self.types.get(ty)?.symbol;
        if let Some(declaration) = self.symbol_value_declaration_is_context_sensitive(symbol)? {
            return self.type_to_string_at(ty, Some(declaration), fmt::NONE);
        }
        self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)
    }

    // port: tsc/internal/checker/relater.go:Checker.symbolValueDeclarationIsContextSensitive
    fn symbol_value_declaration_is_context_sensitive(
        &mut self,
        symbol: Option<SymbolId>,
    ) -> Result<Option<NodeId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(None);
        };
        if crate::query::is_expression_node(self.ast(declaration)?, declaration)?
            && !self.expression_is_context_sensitive(declaration)?
        {
            return Ok(Some(declaration));
        }
        Ok(None)
    }
}

// port: tsc/internal/checker/relater.go:isConversionOrInterfaceImplementationMessage
fn is_conversion_or_interface_implementation_message(message: &'static Message) -> bool {
    [
        messages::Class_0_incorrectly_implements_interface_1,
        messages::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass,
        messages::Conversion_of_type_0_to_type_1_may_be_a_mistake_because_neither_type_sufficiently_overlaps_with_the_other_If_this_was_intentional_convert_the_expression_to_unknown_first,
        messages::Its_instance_type_0_is_not_a_valid_JSX_element,
        messages::Its_return_type_0_is_not_a_valid_JSX_element,
        messages::Its_element_type_0_is_not_a_valid_JSX_element,
    ]
    .iter()
    .any(|candidate| candidate.code == message.code)
}

fn property_name(name: &[u8]) -> Vec<u8> {
    if matches!(name.first(), Some(b'"' | b'\'' | b'`')) {
        let mut result = vec![b'['];
        result.extend_from_slice(name);
        result.push(b']');
        result
    } else {
        name.to_vec()
    }
}
// port: tsc/internal/checker/relater.go:addToDottedName
fn dotted_name(head: &[u8], tail: &[u8]) -> JsString {
    let mut pos = 0;
    loop {
        if tail[pos..].starts_with(b"(") {
            pos += 1;
        } else if tail[pos..].starts_with(b"new ") {
            pos += 4;
        } else {
            break;
        }
    }
    let mut result = tail[..pos].to_vec();
    if head.starts_with(b"new ") {
        result.push(b'(');
        result.extend_from_slice(head);
        result.push(b')');
    } else {
        result.extend_from_slice(head);
    }
    if !tail[pos..].starts_with(b"[") {
        result.push(b'.');
    }
    result.extend_from_slice(&tail[pos..]);
    JsString::from_bytes(result)
}

impl Relater<'_> {
    pub(crate) fn report_no_common(
        &mut self,
        source: TypeId,
        target: TypeId,
        original_source: TypeId,
        original_target: TypeId,
    ) -> Result<(), Error> {
        let source_display = if self.checker.types.get(original_source)?.alias.is_some() {
            original_source
        } else {
            source
        };
        let target_display = if self.checker.types.get(original_target)?.alias.is_some() {
            original_target
        } else {
            target
        };
        let source_name = self
            .checker
            .type_to_string(source_display, crate::type_display::DEFAULT_FLAGS)?;
        let target_name = self
            .checker
            .type_to_string(target_display, crate::type_display::DEFAULT_FLAGS)?;
        let calls = self.checker.signatures_of_type(source, false)?;
        let constructs = self.checker.signatures_of_type(source, true)?;
        let mut invokable = false;
        for signature in [calls.first(), constructs.first()].into_iter().flatten() {
            let returned = self.checker.return_type_of_signature(*signature)?;
            if self.related_with_errors(returned, target, crate::relater::SOURCE, 0, false)?
                != crate::ternary::FALSE
            {
                invokable = true;
                break;
            }
        }
        let message = if invokable {
            messages::Value_of_type_0_has_no_properties_in_common_with_type_1_Did_you_mean_to_call_it
        } else {
            messages::Type_0_has_no_properties_in_common_with_type_1
        };
        self.report_error(message, vec![source_name, target_name]);
        Ok(())
    }

    // port: tsc/internal/checker/relater.go:Relater.reportUnmatchedProperty
    pub(crate) fn report_unmatched_property(
        &mut self,
        source: TypeId,
        target: TypeId,
        first: ts_arena::SymbolId,
        require_optional: bool,
    ) -> Result<(), Error> {
        // Give a specific error when private names have the same description.
        if let Some(declaration) = self.checker.symbol(first)?.value_declaration() {
            let view = self.checker.ast(declaration)?;
            if let Some(name) = view.node(declaration)?.name() {
                if view.node(name)?.kind() == ts_ast::SyntaxKind::PrivateIdentifier {
                    if let Some(source_symbol) = self.checker.types.get(source)?.symbol {
                        if self.checker.symbol(source_symbol)?.flags() & ts_ast::symbol_flags::CLASS
                            != 0
                        {
                            let description = view.node_text(name)?.into_js_string();
                            let key = ts_binder::get_symbol_name_for_private_identifier(
                                &self.checker.symbol(source_symbol)?,
                                description.as_bytes(),
                            );
                            if self
                                .checker
                                .constituent_property(source, key.as_bytes(), false)?
                                .is_some()
                            {
                                let source_name = self.checker.symbol_to_string(source_symbol)?;
                                let target_name = match self.checker.types.get(target)?.symbol {
                                    Some(symbol) => self.checker.symbol_to_string(symbol)?,
                                    None => self.checker.type_to_string(
                                        target,
                                        crate::type_display::DEFAULT_FLAGS,
                                    )?,
                                };
                                self.report_error(
                                    messages::Property_0_in_type_1_refers_to_a_different_member_that_cannot_be_accessed_from_within_type_2,
                                    vec![description, source_name, target_name],
                                );
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        let calls = self.checker.signatures_of_type(source, false)?;
        let constructs = self.checker.signatures_of_type(source, true)?;
        if (!calls.is_empty() || !constructs.is_empty())
            && self.checker.get_properties_of_type(source)?.is_empty()
            && (calls.is_empty() || self.checker.signatures_of_type(target, false)?.is_empty())
            && (constructs.is_empty() || self.checker.signatures_of_type(target, true)?.is_empty())
        {
            return Ok(());
        }
        let mut missing = Vec::new();
        for property in self.checker.get_properties_of_type(target)? {
            let read = self.checker.symbol(property)?;
            if self
                .checker
                .is_static_private_identifier_property(property)?
            {
                continue;
            }
            if require_optional
                || read.flags() & ts_ast::symbol_flags::OPTIONAL == 0
                    && read.check_flags() & ts_ast::check_flags::PARTIAL == 0
            {
                let name = read.name_to_owned();
                if self
                    .checker
                    .constituent_property(source, name.as_bytes(), false)?
                    .is_none()
                {
                    missing.push(property);
                }
            }
        }
        if missing.len() == 1 {
            let (source, target) = self.checker.type_names_for_error_display(source, target)?;
            let name = self.checker.symbol_to_string(first)?;
            self.report_error(
                messages::Property_0_is_missing_in_type_1_but_required_in_type_2,
                vec![name.clone(), source, target],
            );
            if let Some(node) = self.checker.symbol_declarations(first)?.first().flatten() {
                let diagnostic = self.checker.diagnostic_for_node(
                    Some(node),
                    messages::X_0_is_declared_here,
                    vec![name],
                )?;
                self.errors.related.push(Arc::new(diagnostic));
            }
        } else {
            let array_like = if self.checker.is_tuple_type(source)? {
                !(self.checker.readonly_array_or_tuple(source)?
                    && (self.checker.is_tuple_type(target)?
                        || self.checker.is_array_type(target)?)
                    && !self.checker.readonly_array_or_tuple(target)?)
                    && (self.checker.is_tuple_type(target)?
                        || self.checker.is_array_type(target)?)
            } else if self.checker.readonly_array_or_tuple(source)?
                && (self.checker.is_tuple_type(target)? || self.checker.is_array_type(target)?)
                && !self.checker.readonly_array_or_tuple(target)?
            {
                false
            } else if self.checker.is_tuple_type(target)? {
                self.checker.is_array_type(source)?
            } else {
                true
            };
            if array_like {
                let (source, target) = self.checker.type_names_for_error_display(source, target)?;
                let mut names = Vec::new();
                for &property in
                    missing
                        .iter()
                        .take(if missing.len() > 5 { 4 } else { missing.len() })
                {
                    if !names.is_empty() {
                        names.extend_from_slice(b", ");
                    }
                    names.extend_from_slice(self.checker.symbol_to_string(property)?.as_bytes());
                }
                let mut args = vec![source, target, JsString::from_bytes(names)];
                let message = if missing.len() > 5 {
                    args.push(JsString::from_bytes(
                        (missing.len() - 4).to_string().into_bytes(),
                    ));
                    messages::Type_0_is_missing_the_following_properties_from_type_1_Colon_2_and_3_more
                } else {
                    messages::Type_0_is_missing_the_following_properties_from_type_1_Colon_2
                };
                self.report_error(message, args);
            }
        }
        Ok(())
    }
}

impl crate::CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSuggestedTypeForNonexistentStringLiteralType
    fn suggested_literal_type(
        &self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let crate::LiteralValue::String(name) = &self.types.literal(source)?.value else {
            return Err(Error::MissingLink("string literal payload"));
        };
        let mut candidates = Vec::new();
        for &ty in self.types.types_of(target)? {
            if self.types.flags(ty)? & tf::STRING_LITERAL != 0 {
                let crate::LiteralValue::String(value) = &self.types.literal(ty)?.value else {
                    return Err(Error::MissingLink("string literal candidate payload"));
                };
                candidates.push((ty, value));
            }
        }
        let failure = std::cell::Cell::new(None);
        let suggestion = ts_scanner::get_spelling_suggestion(
            name.as_bytes(),
            candidates.iter(),
            |entry| entry.1.as_bytes(),
            |a, b| match self.compare_types(a.0, b.0) {
                Ok(order) => order,
                Err(error) => {
                    if failure.get().is_none() {
                        failure.set(Some(error));
                    }
                    std::cmp::Ordering::Equal
                }
            },
            1000,
        );
        if let Some(error) = failure.get() {
            return Err(error);
        }
        Ok(suggestion.map(|entry| entry.0))
    }
}
