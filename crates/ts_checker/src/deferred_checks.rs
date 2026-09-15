//! Deferred diagnostics carry non-owning identities instead of closures over
//! the mutable checker. Failed callbacks remain pending and fail again on retry.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;

#[derive(Clone, Copy)]
pub(crate) enum DeferredCheck {
    MissingProperty {
        name: NodeId,
        containing: TypeId,
        unchecked_js: bool,
    },
    Iteration {
        index: usize,
    },
    WeakMapSetCollision {
        node: NodeId,
    },
    ReflectCollision {
        node: NodeId,
    },
}

#[derive(Default)]
pub(crate) struct DeferredChecks {
    pub(crate) pending: Vec<DeferredCheck>,
    pub(crate) cursor: usize,
    pub(crate) reported_properties: crate::types::Set<NodeId>,
}

impl CheckerState {
    pub(crate) fn defer_iteration_diagnostic(&mut self, index: usize) {
        self.deferred_checks
            .pending
            .push(DeferredCheck::Iteration { index });
    }
    pub(crate) fn defer_missing_property_ex(
        &mut self,
        name: NodeId,
        containing: TypeId,
        unchecked_js: bool,
    ) {
        self.deferred_checks
            .pending
            .push(DeferredCheck::MissingProperty {
                name,
                containing,
                unchecked_js,
            });
    }

    // port: tsc/internal/checker/checker.go:Checker.produceDeferredDiagnostics
    pub(crate) fn check_deferred_diagnostics(&mut self) -> Result<(), Error> {
        while let Some(&check) = self
            .deferred_checks
            .pending
            .get(self.deferred_checks.cursor)
        {
            match check {
                DeferredCheck::MissingProperty {
                    name,
                    containing,
                    unchecked_js,
                } => self.report_missing_property(name, containing, unchecked_js)?,
                DeferredCheck::Iteration { index } => self.report_iteration_diagnostic(index)?,
                DeferredCheck::WeakMapSetCollision { node } => {
                    self.check_weak_map_set_collision(node)?;
                }
                DeferredCheck::ReflectCollision { node } => self.check_reflect_collision(node)?,
            }
            self.deferred_checks.cursor += 1;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.reportNonexistentProperty
    fn report_missing_property(
        &mut self,
        name: NodeId,
        containing: TypeId,
        unchecked_js: bool,
    ) -> Result<(), Error> {
        if self.deferred_checks.reported_properties.contains(&name) {
            return Ok(());
        }
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let spelling = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
        let mut child = None;
        if self.ast(name)?.node(name)?.kind() != ts_ast::SyntaxKind::PrivateIdentifier
            && self.types.flags(containing)? & (tf::UNION | tf::PRIMITIVE) == tf::UNION
        {
            for &part in self.types.compound_types(containing)?.clone().iter() {
                if self
                    .constituent_property(part, text.as_bytes(), false)?
                    .is_none()
                {
                    let key = self.get_string_literal_type(text.clone())?;
                    if self.applicable_index_info(part, key)?.is_none() {
                        let display =
                            self.type_to_string(part, crate::type_display::DEFAULT_FLAGS)?;
                        child = Some(std::sync::Arc::new(self.diagnostic_for_node(
                            Some(name),
                            ts_diagnostics::Property_0_does_not_exist_on_type_1,
                            vec![spelling.clone(), display],
                        )?));
                        break;
                    }
                }
            }
        }
        let apparent = self.reduced_apparent_type(containing)?;
        if self.index_type_has_static_property(text.as_bytes(), containing)? {
            let display = self.type_to_string(containing, crate::type_display::DEFAULT_FLAGS)?;
            let mut qualified = display.as_bytes().to_vec();
            qualified.push(b'.');
            qualified.extend_from_slice(spelling.as_bytes());
            let message = ts_diagnostics::Property_0_does_not_exist_on_type_1_Did_you_mean_to_access_the_static_member_2_instead;
            let args = vec![spelling, display, ts_ast::JsString::from_bytes(qualified)];
            let diagnostic = if child.is_some() {
                ts_ast::Diagnostic::chain(child, message, args)
            } else {
                self.diagnostic_for_node(Some(name), message, args)?
            };
            self.add_diagnostic(diagnostic)?;
            self.deferred_checks.reported_properties.insert(name);
            return Ok(());
        }
        if self
            .constituent_property(apparent, b"then", false)?
            .is_some()
        {
            return Err(Error::Unsupported(
                "reportNonexistentProperty: promised type",
            ));
        }
        // port: tsc/internal/checker/checker.go:Checker.getSuggestedLibForNonExistentProperty
        let unreduced_apparent = self.apparent_type(containing)?;
        if let Some(symbol) = self.types.get(unreduced_apparent)?.symbol {
            let container_name = self.symbol(symbol)?.name_to_owned();
            if let Some(lib) = crate::lib_features::suggested_lib_for_property(
                container_name.as_bytes(),
                spelling.as_bytes(),
            ) {
                let display =
                    self.type_to_string(containing, crate::type_display::DEFAULT_FLAGS)?;
                let message = ts_diagnostics::Property_0_does_not_exist_on_type_1_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_2_or_later;
                let args = vec![
                    spelling,
                    display,
                    ts_ast::JsString::from_bytes(lib.as_bytes()),
                ];
                let diagnostic = if child.is_some() {
                    ts_ast::Diagnostic::chain(child, message, args)
                } else {
                    self.diagnostic_for_node(Some(name), message, args)?
                };
                self.add_diagnostic(diagnostic)?;
                self.deferred_checks.reported_properties.insert(name);
                return Ok(());
            }
        }
        let properties = self.get_properties_of_type(containing)?;
        let mut names = Vec::new();
        for property in properties {
            if let Some(parent) = self.ast(name)?.node(name)?.parent() {
                if self.ast(parent)?.node(parent)?.kind()
                    == ts_ast::SyntaxKind::PropertyAccessExpression
                {
                    let receiver = self
                        .ast(parent)?
                        .node(parent)?
                        .expression()
                        .ok_or(Error::MissingLink("property completion receiver"))?;
                    let is_super = self.ast(receiver)?.node(receiver)?.kind()
                        == ts_ast::SyntaxKind::SuperKeyword;
                    if !self.is_access_property_accessible(
                        parent, is_super, false, containing, property,
                    )? {
                        continue;
                    }
                }
            }
            names.push((self.symbol(property)?.name_to_owned(), property));
        }
        let suggestion = ts_scanner::get_spelling_suggestion_for_strings(
            text.as_bytes(),
            names.iter().map(|(name, _)| name.as_bytes()),
        );
        let display = self.type_to_string(containing, crate::type_display::DEFAULT_FLAGS)?;
        let (message, args, suggested) = if let Some(suggestion) = suggestion {
            let (_, symbol) = names
                .iter()
                .find(|(name, _)| name.as_bytes() == suggestion)
                .ok_or(Error::MissingLink("property suggestion"))?;
            (
                if unchecked_js {
                    ts_diagnostics::Property_0_may_not_exist_on_type_1_Did_you_mean_2
                } else {
                    ts_diagnostics::Property_0_does_not_exist_on_type_1_Did_you_mean_2
                },
                vec![spelling, display, self.symbol(*symbol)?.name_to_owned()],
                Some(*symbol),
            )
        } else {
            child = self.elaborate_never_intersection(child, name, containing)?;
            let message = if self.container_seems_to_be_empty_dom_element(containing)? {
                ts_diagnostics::Property_0_does_not_exist_on_type_1_Try_changing_the_lib_compiler_option_to_include_dom
            } else {
                ts_diagnostics::Property_0_does_not_exist_on_type_1
            };
            (message, vec![spelling, display], None)
        };
        let mut diagnostic = if child.is_some() {
            ts_ast::Diagnostic::chain(child, message, args)
        } else {
            self.diagnostic_for_node(Some(name), message, args)?
        };
        if let Some(symbol) = suggested {
            if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                diagnostic.related_information.push(std::sync::Arc::new(
                    self.diagnostic_for_node(
                        Some(declaration),
                        ts_diagnostics::X_0_is_declared_here,
                        vec![self.symbol(symbol)?.name_to_owned()],
                    )?,
                ));
            }
        }
        // port: tsc/internal/checker/checker.go:Checker.addErrorOrSuggestion
        if !unchecked_js
            || diagnostic.code
                != ts_diagnostics::Property_0_may_not_exist_on_type_1_Did_you_mean_2.code
        {
            self.add_diagnostic(diagnostic)?;
        } else {
            diagnostic.category = ts_diagnostics::Category::Suggestion as i32;
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        self.deferred_checks.reported_properties.insert(name);
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.elaborateNeverIntersection
    pub(crate) fn elaborate_never_intersection(
        &mut self,
        chain: Option<std::sync::Arc<ts_ast::Diagnostic>>,
        node: NodeId,
        ty: TypeId,
    ) -> Result<Option<std::sync::Arc<ts_ast::Diagnostic>>, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::INTERSECTION == 0
            || record.object_flags & crate::object_flags::IS_NEVER_INTERSECTION == 0
        {
            return Ok(chain);
        }
        let Some((message, property)) = self.never_intersection_cause(ty)? else {
            return Ok(chain);
        };
        let display = self.type_to_string(ty, crate::type_format_flags::NO_TYPE_REDUCTION)?;
        let name = self.symbol_to_string(property)?;
        let diagnostic = if chain.is_some() {
            ts_ast::Diagnostic::chain(chain, message, vec![display, name])
        } else {
            self.diagnostic_for_node(Some(node), message, vec![display, name])?
        };
        Ok(Some(std::sync::Arc::new(diagnostic)))
    }

    // Both diagnostic paths prefer a discriminant over a private-name conflict.
    pub(crate) fn never_intersection_cause(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<(&'static ts_diagnostics::Message, ts_arena::SymbolId)>, Error> {
        let properties = self.get_properties_of_union_or_intersection_type(ty)?;
        for &property in &properties {
            if self.is_discriminant_with_never_type(property)? {
                return Ok(Some((ts_diagnostics::The_intersection_0_was_reduced_to_never_because_property_1_has_conflicting_types_in_some_constituents, property)));
            }
        }
        for property in properties {
            if self.is_conflicting_private_property(property)? {
                return Ok(Some((ts_diagnostics::The_intersection_0_was_reduced_to_never_because_property_1_exists_in_multiple_constituents_and_is_private_in_some, property)));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.containerSeemsToBeEmptyDomElement
    fn container_seems_to_be_empty_dom_element(
        &mut self,
        containing: TypeId,
    ) -> Result<bool, Error> {
        let options = self.program()?.host.options();
        if options
            .lib
            .as_ref()
            .is_some_and(|libs| libs.iter().any(|lib| lib.as_bytes() == b"lib.dom.d.ts"))
        {
            return Ok(false);
        }
        let parts: Vec<TypeId> = if self.types.flags(containing)? & tf::UNION_OR_INTERSECTION != 0 {
            self.types.types_of(containing)?.to_vec()
        } else {
            vec![containing]
        };
        for part in parts {
            let Some(symbol) = self.types.get(part)?.symbol else {
                return Ok(false);
            };
            let name = self.symbol(symbol)?.name_to_owned();
            let name = name.as_bytes();
            let common = name == b"EventTarget"
                || name == b"Node"
                || name == b"Element"
                || name.starts_with(b"HTML") && name.ends_with(b"Element");
            if !common {
                return Ok(false);
            }
        }
        self.empty_object_type(containing)
    }
}
