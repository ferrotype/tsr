//! Element diagnostics retain contextual expression checks and declaration
//! provenance rather than replacing a failed aggregate relation with a summary.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use std::sync::Arc;
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, Diagnostic, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.elaborateObjectLiteral
    pub(crate) fn elaborate_object_error(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        if self.types.flags(target)? & (tf::PRIMITIVE | tf::NEVER) != 0 {
            return Ok(false);
        }
        let mut reported = false;
        for property in self.source_list(node, self.ast(node)?.node(node)?.property_list())? {
            if self.ast(property)?.node(property)?.kind() == K::SpreadAssignment {
                continue;
            }
            let symbol = self
                .get_symbol_of_declaration(property)?
                .ok_or(Error::MissingLink("elaborated property symbol"))?;
            let name_type =
                self.literal_type_from_property(symbol, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
            if self.types.flags(name_type)? & tf::NEVER != 0 {
                continue;
            }
            let read = self.ast(property)?.node(property)?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("elaborated property name"))?;
            let (next, message) = if read.kind() == K::PropertyAssignment {
                let next = read.initializer();
                let name_read = self.ast(name)?.node(name)?;
                let computed = if name_read.kind() == K::ComputedPropertyName {
                    let expression = name_read
                        .expression()
                        .ok_or(Error::MissingLink("computed property expression"))?;
                    !matches!(
                        self.ast(expression)?.node(expression)?.kind().known(),
                        Some(
                            K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral
                        )
                    )
                } else {
                    false
                };
                (
                    next,
                    computed.then_some(
                        d::Type_of_computed_property_s_value_is_0_which_is_not_assignable_to_type_1,
                    ),
                )
            } else {
                (None, None)
            };
            reported = self.elaborate_element_error(
                source, target, relation, name, next, name_type, message, output,
            )? || reported;
        }
        Ok(reported)
    }

    // port: tsc/internal/checker/relater.go:Checker.elaborateArrayLiteral
    pub(crate) fn elaborate_array_error(
        &mut self,
        node: NodeId,
        mut source: TypeId,
        target: TypeId,
        relation: RelationKind,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        if self.types.flags(target)? & (tf::PRIMITIVE | tf::NEVER) != 0 {
            return Ok(false);
        }
        if !self.tuple_like_type(source)? {
            self.calls.contexts.push(crate::calls::ArgumentContext {
                node,
                ty: target,
                inference: None,
            });
            let result = self.check_expression_ex(node, 1 | 128);
            self.calls.contexts.pop();
            source = result?;
            if !self.tuple_like_type(source)? {
                return Ok(false);
            }
        }
        let mut reported = false;
        for (index, element) in self
            .source_list(node, self.ast(node)?.node(node)?.element_list())?
            .into_iter()
            .enumerate()
        {
            if self.ast(element)?.node(element)?.kind() == K::OmittedExpression {
                continue;
            }
            if self.tuple_like_type(target)?
                && self
                    .constituent_property(target, index.to_string().as_bytes(), false)?
                    .is_none()
            {
                continue;
            }
            let name = self.get_number_literal_type(ts_jsnum::Number::new(index as f64))?;
            let check = self.effective_expression_check_node(element)?;
            reported = self.elaborate_element_error(
                source,
                target,
                relation,
                check,
                Some(check),
                name,
                None,
                output,
            )? || reported;
        }
        Ok(reported)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectiveCheckNode
    pub(crate) fn effective_expression_check_node(
        &self,
        mut node: NodeId,
    ) -> Result<NodeId, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            if !matches!(
                read.kind().known(),
                Some(K::ParenthesizedExpression | K::SatisfiesExpression)
            ) {
                return Ok(node);
            }
            if read.kind() == K::ParenthesizedExpression
                && read.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE != 0
                && read.type_node().is_some()
            {
                return Ok(node);
            }
            node = read
                .expression()
                .ok_or(Error::MissingLink("effective check expression"))?;
        }
    }

    // port: tsc/internal/checker/relater.go:Checker.getBestMatchIndexedAccessTypeOrUndefined
    fn best_match_indexed_access(
        &mut self,
        source: TypeId,
        target: TypeId,
        name: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if let Some(indexed) = self.indexed_access_or_undefined(target, name, 0, None, None)? {
            return Ok(Some(indexed));
        }
        if self.types.flags(target)? & tf::UNION != 0 {
            if let Some(best) = self.best_assignable_error_target(source, target)? {
                return self.indexed_access_or_undefined(best, name, 0, None, None);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/relater.go:Checker.checkExpressionForMutableLocationWithContextualType
    fn elaborate_mutable_expression(
        &mut self,
        node: NodeId,
        context: TypeId,
    ) -> Result<TypeId, Error> {
        self.calls.contexts.push(crate::calls::ArgumentContext {
            node,
            ty: context,
            inference: None,
        });
        let previous = std::mem::replace(&mut self.expression_mode, 1);
        let result = self.check_expression_for_mutable_location(node);
        self.expression_mode = previous;
        self.calls.contexts.pop();
        result
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Pinned elaborateElement keeps the relation, property, nested expression and diagnostic head independent"
    )]
    // port: tsc/internal/checker/relater.go:Checker.elaborateElement
    fn elaborate_element_error(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        property: NodeId,
        next: Option<NodeId>,
        name: TypeId,
        message: Option<&'static d::Message>,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        let Some(mut target_type) = self.best_match_indexed_access(source, target, name)? else {
            return Ok(false);
        };
        if self.types.flags(target_type)? & tf::INDEXED_ACCESS != 0 {
            return Ok(false);
        }
        let Some(mut source_type) =
            self.indexed_access_or_undefined(source, name, 0, None, None)?
        else {
            return Ok(false);
        };
        if self.is_type_related_to(source_type, target_type, relation)? {
            return Ok(false);
        }
        if let Some(next) = next {
            if self.elaborate_call_error(next, source_type, target_type, relation, None, output)? {
                return Ok(true);
            }
        }
        let specific = match next {
            Some(next) => self.elaborate_mutable_expression(next, source_type)?,
            None => source_type,
        };
        let property_name = self.index_property_name(name)?;
        let target_property = match &property_name {
            Some(name) => self.constituent_property(target, name.as_bytes(), false)?,
            None => None,
        };
        let mut diagnostic = if self.options.exact_optional_property_types
            && self.maybe_type_of_kind(specific, tf::UNDEFINED)?
            && self.type_contains_missing(target_type)?
        {
            let specific = self.type_to_string(specific, crate::type_display::DEFAULT_FLAGS)?;
            let target = self.type_to_string(target_type, crate::type_display::DEFAULT_FLAGS)?;
            Some(self.diagnostic_for_node(Some(property), d::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_type_of_the_target, vec![specific, target])?)
        } else {
            let source_property = match &property_name {
                Some(name) => self.constituent_property(source, name.as_bytes(), false)?,
                None => None,
            };
            let optional_target = target_property
                .map(|symbol| {
                    self.symbol(symbol)
                        .map(|symbol| symbol.flags() & sf::OPTIONAL != 0)
                })
                .transpose()?
                .unwrap_or(false);
            let optional_source = source_property
                .map(|symbol| {
                    self.symbol(symbol)
                        .map(|symbol| symbol.flags() & sf::OPTIONAL != 0)
                })
                .transpose()?
                .unwrap_or(false);
            target_type = self.remove_missing_type(target_type, optional_target)?;
            source_type =
                self.remove_missing_type(source_type, optional_target && optional_source)?;
            let (related, diagnostic) = self.check_type_related_ex(
                specific,
                target_type,
                relation,
                Some(property),
                message,
            )?;
            if related && specific != source_type {
                self.check_type_related_ex(
                    source_type,
                    target_type,
                    relation,
                    Some(property),
                    message,
                )?
                .1
            } else {
                diagnostic
            }
        };
        let Some(mut diagnostic) = diagnostic.take() else {
            return Ok(false);
        };
        let mut index_elaboration = false;
        if target_property.is_none() {
            if let Some(index) = self.applicable_index_info(target, name)? {
                if let Some(declaration) = self.signatures.index_info(index)?.declaration {
                    if !self.elaboration_in_default_library(declaration)? {
                        diagnostic
                            .related_information
                            .push(Arc::new(self.diagnostic_for_node(
                                Some(declaration),
                                d::The_expected_type_comes_from_this_index_signature,
                                vec![],
                            )?));
                        index_elaboration = true;
                    }
                }
            }
        }
        if !index_elaboration {
            let target_symbol = self.types.get(target)?.symbol;
            let declaration = match target_property {
                Some(symbol) => self.symbol_declarations(symbol)?.first().flatten(),
                None => None,
            }
            .or(match target_symbol {
                Some(symbol) => self.symbol_declarations(symbol)?.first().flatten(),
                None => None,
            });
            if let Some(declaration) = declaration {
                if !self.elaboration_in_default_library(declaration)? {
                    let property_name = if property_name.as_ref().is_none_or(JsString::is_empty)
                        || self.types.flags(name)? & tf::UNIQUE_ES_SYMBOL != 0
                    {
                        self.type_to_string(name, crate::type_display::DEFAULT_FLAGS)?
                    } else {
                        property_name.expect("nonempty property name")
                    };
                    let target_name =
                        self.type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
                    diagnostic
                        .related_information
                        .push(Arc::new(self.diagnostic_for_node(
                        Some(declaration),
                        d::The_expected_type_comes_from_property_0_which_is_declared_here_on_type_1,
                        vec![property_name, target_name],
                    )?));
                }
            }
        }
        output.push(diagnostic);
        Ok(true)
    }

    fn elaboration_in_default_library(&self, node: NodeId) -> Result<bool, Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("elaboration declaration source"))?;
        let source = self.ast(source)?.source_file(source)?;
        Ok(self
            .program()?
            .host
            .is_source_file_default_library(source.parse_options().path.as_bytes()))
    }
}
