//! Source checks for tuple, array and signature declarations. Grammar checks
//! keep their short-circuit order while source-element checks still run.

use crate::{element_flags as ef, type_flags as tf, CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarTypeOperatorNode
    fn check_unique_type_operator(&mut self, node: NodeId) -> Result<(), Error> {
        use ts_ast::{modifier_flags as mf, node_flags as nf};
        use ts_diagnostics as d;
        let read = self.ast(node)?.node(node)?;
        let annotation = read
            .type_node()
            .ok_or(Error::MissingLink("unique type annotation"))?;
        let mut parent = read
            .parent()
            .ok_or(Error::MissingLink("unique type parent"))?;
        while self.ast(parent)?.node(parent)?.kind() == K::ParenthesizedType {
            parent = self
                .ast(parent)?
                .node(parent)?
                .parent()
                .ok_or(Error::MissingLink("parenthesized unique type parent"))?;
        }
        if self.ast(annotation)?.node(annotation)?.kind() == K::SymbolKeyword {
            let read = self.ast(parent)?.node(parent)?;
            let name = read.name().unwrap_or(parent);
            let diagnostic = match read.kind().known() {
                Some(K::VariableDeclaration) => {
                    let list = read.parent().ok_or(Error::MissingLink("unique variable list"))?;
                    let list = self.ast(list)?.node(list)?;
                    if self.ast(name)?.node(name)?.kind() != K::Identifier {
                        Some((node, d::X_unique_symbol_types_may_not_be_used_on_a_variable_declaration_with_a_binding_name))
                    } else if list.kind() != K::VariableDeclarationList || list.parent().map(|parent| self.ast(parent)?.node(parent).map(|read| read.kind() != K::VariableStatement).map_err(Error::from)).transpose()?.unwrap_or(true) {
                        Some((node, d::X_unique_symbol_types_are_only_allowed_on_variables_in_a_variable_statement))
                    } else if list.flags() & nf::CONSTANT == 0 {
                        Some((name, d::A_variable_whose_type_is_a_unique_symbol_type_must_be_const))
                    } else { None }
                }
                Some(K::PropertyDeclaration) if read.modifier_flags(self.ast(parent)?)? & (mf::STATIC | mf::READONLY) != mf::STATIC | mf::READONLY => Some((name, d::A_property_of_a_class_whose_type_is_a_unique_symbol_type_must_be_both_static_and_readonly)),
                Some(K::PropertySignature) if read.modifier_flags(self.ast(parent)?)? & mf::READONLY == 0 => Some((name, d::A_property_of_an_interface_or_type_literal_whose_type_is_a_unique_symbol_type_must_be_readonly)),
                Some(K::PropertyDeclaration | K::PropertySignature) => None,
                _ => Some((node, d::X_unique_symbol_types_are_not_allowed_here)),
            };
            if let Some((node, message)) = diagnostic {
                self.error_at(Some(node), message, Vec::new())?;
            }
        } else {
            self.error_at(
                Some(annotation),
                d::X_0_expected,
                vec![ts_ast::JsString::from_bytes(b"symbol".as_slice())],
            )?;
        }
        self.check_source_element(annotation)?;
        self.get_type_from_type_node(node)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypePredicate
    pub(crate) fn check_type_predicate(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let annotation = read.type_node();
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("predicate parent"))?;
        if let Some(annotation) = annotation {
            self.check_source_element(annotation)?;
        }
        let read = self.ast(parent)?.node(parent)?;
        if !matches!(
            read.kind().known(),
            Some(
                K::ArrowFunction
                    | K::CallSignature
                    | K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::FunctionType
                    | K::MethodDeclaration
                    | K::MethodSignature
            )
        ) || read.type_node() != Some(node)
        {
            self.error_at(Some(node), ts_diagnostics::A_type_predicate_is_only_allowed_in_return_type_position_for_functions_and_methods, vec![])?;
            return Ok(());
        }
        let signature = self.signature_from_declaration(parent)?;
        let Some(predicate) = self.type_predicate_of_signature(signature)? else {
            return Ok(());
        };
        let data = self.signatures.predicate(predicate)?.clone();
        if matches!(
            data.kind,
            crate::TypePredicateKind::This | crate::TypePredicateKind::AssertsThis
        ) {
            return Ok(());
        }
        let name = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_type_predicate_node()
            .ok_or(Error::MissingLink("predicate"))?
            .parameter_name();
        if data.parameter_index < 0 {
            for parameter in
                self.source_list(parent, self.ast(parent)?.node(parent)?.parameter_list())?
            {
                if let Some(name) = self.ast(parameter)?.node(parameter)?.name() {
                    if self.ast(name)?.node(name)?.kind() != K::Identifier {
                        return Err(Error::Unsupported(
                            "checkIfTypePredicateVariableIsDeclaredInBindingPattern",
                        ));
                    }
                }
            }
            self.error_at(
                name,
                ts_diagnostics::Cannot_find_parameter_0,
                vec![data.parameter_name],
            )?;
        } else {
            let sig = self.signatures.get(signature)?;
            let parameters = sig.parameters.as_deref().unwrap_or_default();
            let index = data.parameter_index as usize;
            if sig.flags & crate::signature_flags::HAS_REST_PARAMETER != 0
                && index + 1 == parameters.len()
            {
                self.error_at(
                    name,
                    ts_diagnostics::A_type_predicate_cannot_reference_a_rest_parameter,
                    vec![],
                )?;
            } else if let Some(source) = data.t {
                let target = self.get_type_of_symbol(parameters[index])?;
                let (_, diagnostic) = self.check_type_related_ex(
                    source,
                    target,
                    crate::RelationKind::Assignable,
                    annotation,
                    None,
                )?;
                if let Some(diagnostic) = diagnostic {
                    self.add_diagnostic(ts_ast::Diagnostic::chain(Some(std::sync::Arc::new(diagnostic)), ts_diagnostics::A_type_predicate_s_type_must_be_assignable_to_its_parameter_s_type, vec![]))?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkArrayType
    // port: tsc/internal/checker/checker.go:Checker.checkTupleType
    pub(crate) fn check_array_tuple_syntax(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::ArrayType) => {
                let element = self
                    .array_element_type_node(node)?
                    .ok_or(Error::MissingLink("array type element"))?;
                self.check_source_element(element)
            }
            Some(K::TypeOperator) => {
                let data = read
                    .data_source()
                    .as_type_operator_node()
                    .ok_or(Error::MissingLink("type operator"))?;
                if data.operator() == K::KeyOfKeyword {
                    let operand = read
                        .type_node()
                        .ok_or(Error::MissingLink("keyof operand"))?;
                    self.check_source_element(operand)?;
                    self.get_type_from_type_node(node)?;
                    return Ok(());
                }
                if data.operator() == K::UniqueKeyword {
                    return self.check_unique_type_operator(node);
                }
                if data.operator() != K::ReadonlyKeyword {
                    return Err(Error::Unsupported("checkTypeOperator: unknown operator"));
                }
                let annotation = read
                    .type_node()
                    .ok_or(Error::MissingLink("readonly type"))?;
                if !matches!(
                    self.ast(annotation)?.node(annotation)?.kind().known(),
                    Some(K::ArrayType | K::TupleType)
                ) {
                    self.grammar_error_first_token(
                        node,
                        ts_diagnostics::X_readonly_type_modifier_is_only_permitted_on_array_and_tuple_literal_types,
                        vec![ts_ast::JsString::from_bytes(b"symbol".as_slice())],
                    )?;
                }
                self.check_source_element(annotation)
            }
            Some(K::TupleType) => {
                let elements = self.source_list(node, read.element_list())?;
                let mut seen_optional = false;
                let mut seen_rest = false;
                for &element in &elements {
                    let mut flags = self.tuple_element_info(element)?.flags;
                    if flags & ef::VARIADIC != 0 {
                        let annotation = self
                            .ast(element)?
                            .node(element)?
                            .type_node()
                            .ok_or(Error::MissingLink("variadic annotation"))?;
                        let ty = self.get_type_from_type_node(annotation)?;
                        if !self.is_array_like_type(ty)? {
                            self.error_at(
                                Some(element),
                                ts_diagnostics::A_rest_element_type_must_be_an_array_type,
                                vec![],
                            )?;
                            break;
                        }
                        if self.is_array_type(ty)?
                            || self.is_tuple_type(ty)?
                                && self.types.tuple(self.types.target(ty)?)?.combined_flags
                                    & ef::REST
                                    != 0
                        {
                            flags |= ef::REST;
                        }
                    }
                    let diagnostic = if flags & ef::REST != 0 {
                        let repeated = seen_rest;
                        seen_rest = true;
                        repeated.then_some(
                            ts_diagnostics::A_rest_element_cannot_follow_another_rest_element,
                        )
                    } else if flags & ef::OPTIONAL != 0 {
                        seen_optional = true;
                        seen_rest.then_some(
                            ts_diagnostics::An_optional_element_cannot_follow_a_rest_element,
                        )
                    } else {
                        (flags & ef::REQUIRED != 0 && seen_optional).then_some(
                            ts_diagnostics::A_required_element_cannot_follow_an_optional_element,
                        )
                    };
                    if let Some(diagnostic) = diagnostic {
                        self.error_at(Some(element), diagnostic, vec![])?;
                        break;
                    }
                }
                for element in elements {
                    self.check_source_element(element)?;
                }
                self.get_type_from_type_node(node)?;
                Ok(())
            }
            Some(K::OptionalType | K::RestType) => {
                let annotation = read
                    .type_node()
                    .ok_or(Error::MissingLink("tuple element annotation"))?;
                self.check_source_element(annotation)
            }
            // port: tsc/internal/checker/checker.go:Checker.checkNamedTupleMember
            Some(K::NamedTupleMember) => {
                let data = read
                    .data_source()
                    .as_named_tuple_member()
                    .ok_or(Error::MissingLink("named tuple member"))?;
                let annotation = read
                    .type_node()
                    .ok_or(Error::MissingLink("tuple element annotation"))?;
                if data.dot_dot_dot_token().is_some() && data.question_token().is_some() {
                    self.grammar_error_node(
                        node,
                        ts_diagnostics::A_tuple_member_cannot_be_both_optional_and_rest,
                        vec![],
                    )?;
                }
                match self.ast(annotation)?.node(annotation)?.kind().known() {
                    Some(K::OptionalType) => {
                        self.grammar_error_node(annotation, ts_diagnostics::A_labeled_tuple_element_is_declared_as_optional_with_a_question_mark_after_the_name_and_before_the_colon_rather_than_after_the_type, vec![])?;
                    }
                    Some(K::RestType) => {
                        self.grammar_error_node(annotation, ts_diagnostics::A_labeled_tuple_element_is_declared_as_rest_with_a_before_the_name_rather_than_before_the_type, vec![])?;
                    }
                    _ => {}
                }
                self.check_source_element(annotation)?;
                self.get_type_from_type_node(node)?;
                Ok(())
            }
            _ => Err(Error::MissingLink("array/tuple syntax kind")),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSignatureDeclaration
    pub(crate) fn check_signature_syntax(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let index = read.kind() == K::IndexSignature;
        let parameters = self.source_list(node, read.parameter_list())?;
        let annotation = read.type_node();
        if index {
            if !self.check_grammar_modifiers(node)? {
                self.check_index_signature_grammar(node, &parameters)?;
            }
        } else if matches!(
            read.kind().known(),
            Some(
                K::FunctionType
                    | K::FunctionDeclaration
                    | K::ConstructorType
                    | K::CallSignature
                    | K::Constructor
                    | K::ConstructSignature
            )
        ) {
            self.check_grammar_function_like(node)?;
        }
        if self.ast(node)?.node(node)?.body().is_some() {
            let (asynchronous, generator) = self.body_function_flags(node)?;
            let target = self.program()?.host.options().emit_script_target();
            if asynchronous && generator && target < ts_core::ScriptTarget::ES2018 {
                // Async generators prior to ES2018 require the __await and __asyncGenerator helpers
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::ASYNC_GENERATOR_INCLUDES,
                )?;
            }
            if asynchronous && !generator && target < ts_core::ScriptTarget::ES2017 {
                self.check_external_emit_helpers(node, crate::external_emit_helpers::AWAITER)?;
            }
        }
        self.check_type_parameters(node)?;
        self.check_unmatched_jsdoc_parameters(node)?;
        for parameter in parameters {
            self.check_parameter(parameter)?;
        }
        if let Some(annotation) = annotation {
            self.check_source_element(annotation)?;
        } else {
            let options = self.program()?.host.options();
            if options.strict_option_value(options.no_implicit_any) {
                let kind = self.ast(node)?.node(node)?.kind();
                let diagnostic = if kind == K::CallSignature {
                    Some(ts_diagnostics::Call_signature_which_lacks_return_type_annotation_implicitly_has_an_any_return_type)
                } else if kind == K::ConstructSignature {
                    Some(ts_diagnostics::Construct_signature_which_lacks_return_type_annotation_implicitly_has_an_any_return_type)
                } else {
                    None
                };
                if let Some(diagnostic) = diagnostic {
                    self.error_at(Some(node), diagnostic, vec![])?;
                }
            }
        }
        if let Some(annotation) = annotation {
            let flags = self.body_function_flags(node)?;
            if flags.1 && self.ast(node)?.node(node)?.body().is_some() {
                self.check_generator_return_annotation(node, annotation)?;
            } else if flags.0 && !flags.1 {
                self.check_async_return_annotation(node, annotation)?;
            }
        }
        if !index {
            self.register_for_unused_identifiers_check(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarParameterList
    pub(crate) fn check_parameter_list_grammar(
        &mut self,
        parameters: &[NodeId],
    ) -> Result<bool, Error> {
        let mut optional = false;
        for (index, &parameter) in parameters.iter().enumerate() {
            let read = self.ast(parameter)?.node(parameter)?;
            let data = read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("parameter"))?;
            let name = read.name();
            let rest = data.dot_dot_dot_token();
            let question = data.question_token();
            let initializer = data.initializer();
            if rest.is_some()
                && index + 1 == parameters.len()
                && read.flags() & ts_ast::node_flags::AMBIENT == 0
            {
                let owner = read
                    .parent()
                    .ok_or(Error::MissingLink("parameter parent"))?;
                let list = self
                    .ast(owner)?
                    .node(owner)?
                    .parameter_list()
                    .ok_or(Error::MissingLink("parameter list"))?;
                // Native reports the comma and still checks optionality and
                // initializers on the same rest parameter.
                self.check_grammar_trailing_comma(owner, list,
                    ts_diagnostics::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma)?;
            }
            let diagnostic = if let Some(rest) = rest {
                if index + 1 != parameters.len() {
                    Some((
                        Some(rest),
                        ts_diagnostics::A_rest_parameter_must_be_last_in_a_parameter_list,
                    ))
                } else if question.is_some() {
                    Some((
                        question,
                        ts_diagnostics::A_rest_parameter_cannot_be_optional,
                    ))
                } else if initializer.is_some() {
                    Some((
                        name,
                        ts_diagnostics::A_rest_parameter_cannot_have_an_initializer,
                    ))
                } else {
                    None
                }
            } else if let Some(question) = question {
                optional = true;
                // A reparsed '?' token indicates a bracketed name in a @param tag.
                let reparsed =
                    self.ast(question)?.node(question)?.flags() & ts_ast::node_flags::REPARSED != 0;
                initializer.filter(|_| !reparsed).map(|_| {
                    (
                        name,
                        ts_diagnostics::Parameter_cannot_have_question_mark_and_initializer,
                    )
                })
            } else if optional && initializer.is_none() {
                Some((
                    name,
                    ts_diagnostics::A_required_parameter_cannot_follow_an_optional_parameter,
                ))
            } else {
                None
            };
            if let Some((node, diagnostic)) = diagnostic {
                let node = node.ok_or(Error::MissingLink("parameter grammar node"))?;
                return self.grammar_error_node(node, diagnostic, vec![]);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarIndexSignatureParameters
    fn check_index_signature_grammar(
        &mut self,
        node: NodeId,
        parameters: &[NodeId],
    ) -> Result<(), Error> {
        if parameters.len() != 1 {
            let at = match parameters.first() {
                Some(&parameter) => self.ast(parameter)?.node(parameter)?.name(),
                None => Some(node),
            };
            let at = at.ok_or(Error::MissingLink("index parameter name"))?;
            self.grammar_error_node(
                at,
                ts_diagnostics::An_index_signature_must_have_exactly_one_parameter,
                vec![],
            )?;
            return Ok(());
        }
        if let Some(list) = self.ast(node)?.node(node)?.parameter_list() {
            self.check_grammar_trailing_comma(
                node,
                list,
                ts_diagnostics::An_index_signature_cannot_have_a_trailing_comma,
            )?;
        }
        let read = self.ast(parameters[0])?.node(parameters[0])?;
        let data = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingLink("index parameter"))?;
        let name = read.name();
        let annotation = read.type_node();
        let diagnostic = if data.dot_dot_dot_token().is_some() {
            Some((
                data.dot_dot_dot_token(),
                ts_diagnostics::An_index_signature_cannot_have_a_rest_parameter,
            ))
        } else if read.modifiers().is_some() {
            Some((
                name,
                ts_diagnostics::An_index_signature_parameter_cannot_have_an_accessibility_modifier,
            ))
        } else if data.question_token().is_some() {
            Some((
                data.question_token(),
                ts_diagnostics::An_index_signature_parameter_cannot_have_a_question_mark,
            ))
        } else if data.initializer().is_some() {
            Some((
                name,
                ts_diagnostics::An_index_signature_parameter_cannot_have_an_initializer,
            ))
        } else if annotation.is_none() {
            Some((
                name,
                ts_diagnostics::An_index_signature_parameter_must_have_a_type_annotation,
            ))
        } else {
            None
        };
        if let Some((node, diagnostic)) = diagnostic {
            let node = node.ok_or(Error::MissingLink("index signature grammar node"))?;
            self.grammar_error_node(node, diagnostic, vec![])?;
            return Ok(());
        }
        let ty = self.get_type_from_type_node(
            annotation.ok_or(Error::MissingLink("index type annotation"))?,
        )?;
        let types = if self.types.flags(ty)? & tf::UNION != 0 {
            self.types.union(ty)?.types.clone()
        } else {
            vec![ty].into()
        };
        for &ty in types.iter() {
            if self.types.flags(ty)? & (tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE | tf::TYPE_PARAMETER)
                != 0
            {
                let name = name.ok_or(Error::MissingLink("index parameter name"))?;
                self.grammar_error_node(name, ts_diagnostics::An_index_signature_parameter_type_cannot_be_a_literal_type_or_generic_type_Consider_using_a_mapped_object_type_instead, vec![])?;
                return Ok(());
            }
        }
        for &ty in types.iter() {
            if self.types.flags(ty)? & (tf::STRING | tf::NUMBER | tf::ES_SYMBOL) == 0
                && !self.is_pattern_literal_type(ty)?
            {
                let name = name.ok_or(Error::MissingLink("index parameter name"))?;
                self.grammar_error_node(name, ts_diagnostics::An_index_signature_parameter_type_must_be_string_number_symbol_or_a_template_literal_type, vec![])?;
                return Ok(());
            }
        }
        if self.ast(node)?.node(node)?.type_node().is_none() {
            self.grammar_error_node(
                node,
                ts_diagnostics::An_index_signature_must_have_a_type_annotation,
                vec![],
            )?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkInferType
    pub(crate) fn check_infer_type(&mut self, node: ts_arena::NodeId) -> Result<(), Error> {
        let mut current = node;
        let mut valid = false;
        while let Some(parent) = self.ast(current)?.node(current)?.parent() {
            let read = self.ast(parent)?.node(parent)?;
            if read
                .data_source()
                .as_conditional_type_node()
                .is_some_and(|d| d.extends_type() == Some(current))
            {
                valid = true;
                break;
            }
            current = parent;
        }
        if !valid {
            self.error_at(Some(node), ts_diagnostics::X_infer_declarations_are_only_permitted_in_the_extends_clause_of_a_conditional_type, Vec::new())?;
        }
        let parameter = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_infer_type_node()
            .and_then(|d| d.type_parameter())
            .ok_or(Error::MissingLink("infer parameter"))?;
        self.check_source_element(parameter)?;
        let symbol = self
            .get_symbol_of_declaration(parameter)?
            .ok_or(Error::MissingLink("infer symbol"))?;
        if self.symbol_declarations(symbol)?.len() > 1
            && self.query.type_parameters_checked.insert(symbol)
        {
            let parameter = self.get_declared_type_of_type_parameter(symbol)?;
            let mut declarations = Vec::new();
            for node in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                if self.ast(node)?.node(node)?.kind() == K::TypeParameter {
                    declarations.push(node);
                }
            }
            let result = self.type_parameter_declarations_identical(&declarations, parameter);
            if result.is_err() {
                self.query.type_parameters_checked.remove(&symbol);
            }
            if !result? {
                let name = self.symbol_to_string(symbol)?;
                for node in declarations {
                    self.error_at(
                        self.ast(node)?.node(node)?.name(),
                        ts_diagnostics::All_declarations_of_0_must_have_identical_constraints,
                        vec![name.clone()],
                    )?;
                }
            }
        }
        self.register_for_unused_identifiers_check(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.areTypeParametersIdentical
    pub(crate) fn type_parameter_declarations_identical(
        &mut self,
        declarations: &[ts_arena::NodeId],
        parameter: crate::TypeId,
    ) -> Result<bool, Error> {
        let symbol = self
            .types
            .get(parameter)?
            .symbol
            .ok_or(Error::MissingLink("infer parameter symbol"))?;
        let name = self.symbol(symbol)?.name_to_owned();
        for &node in declarations {
            let read = self.ast(node)?.node(node)?;
            let node_name = read
                .name()
                .ok_or(Error::MissingLink("type parameter name"))?;
            if self.ast(node_name)?.node_text(node_name)?.as_bytes() != name.as_bytes() {
                return Ok(false);
            }
            let data = read
                .data_source()
                .as_type_parameter_declaration()
                .ok_or(Error::MissingLink("type parameter declaration"))?;
            let constraint_node = data.constraint();
            let default_node = data.default_type();
            let constraint = self.constraint_of_type_parameter(parameter)?;
            if let (Some(node), Some(constraint)) = (constraint_node, constraint) {
                let ty = self.get_type_from_type_node(node)?;
                if !self.is_type_related_to(ty, constraint, crate::RelationKind::Identity)? {
                    return Ok(false);
                }
            }
            let default = self.resolved_type_parameter_default(parameter)?;
            if let Some(node) = default_node {
                if default != self.builtins.no_constraint_type
                    && default != self.builtins.circular_constraint_type
                {
                    let ty = self.get_type_from_type_node(node)?;
                    if !self.is_type_related_to(ty, default, crate::RelationKind::Identity)? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
}

impl CheckerState {
    // port: tsc/internal/ast/utilities.go:IsPartOfTypeNode
    pub(crate) fn is_part_of_type_node(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let kind = read.kind();
        if kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16 {
            return Ok(true);
        }
        Ok(match kind.known() {
            Some(
                K::AnyKeyword
                | K::UnknownKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::StringKeyword
                | K::BooleanKeyword
                | K::SymbolKeyword
                | K::ObjectKeyword
                | K::UndefinedKeyword
                | K::NullKeyword
                | K::NeverKeyword,
            ) => true,
            Some(K::VoidKeyword) => read
                .parent()
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|p| p.kind() != K::VoidExpression)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(true),
            Some(K::ExpressionWithTypeArguments) => self.is_type_heritage_expression(node)?,
            Some(K::TypeParameter) => read
                .parent()
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|p| matches!(p.kind().known(), Some(K::MappedType | K::InferType)))
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false),
            Some(K::Identifier) => {
                let mut target = node;
                if let Some(parent) = read.parent() {
                    let read = self.ast(parent)?.node(parent)?;
                    if read.kind() == K::QualifiedName
                        && read
                            .data_source()
                            .as_qualified_name()
                            .ok_or(Error::MissingLink("qualified name"))?
                            .right()
                            == Some(node)
                        || read.kind() == K::PropertyAccessExpression && read.name() == Some(node)
                    {
                        target = parent;
                    }
                }
                self.is_part_of_type_in_parent(target)?
            }
            Some(K::QualifiedName | K::PropertyAccessExpression | K::ThisKeyword) => {
                self.is_part_of_type_in_parent(node)?
            }
            _ => false,
        })
    }

    // port: tsc/internal/ast/utilities.go:isPartOfTypeNodeInParent
    fn is_part_of_type_in_parent(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        let kind = read.kind();
        if kind == K::TypeQuery {
            return Ok(false);
        }
        if kind == K::ImportType {
            return Ok(!read
                .data_source()
                .as_import_type_node()
                .ok_or(Error::MissingLink("import type"))?
                .is_type_of());
        }
        if kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16 {
            return Ok(true);
        }
        Ok(match kind.known() {
            Some(K::ExpressionWithTypeArguments) => self.is_type_heritage_expression(parent)?,
            Some(K::TypeParameter) => {
                read.data_source()
                    .as_type_parameter_declaration()
                    .ok_or(Error::MissingLink("type parameter"))?
                    .constraint()
                    == Some(node)
            }
            Some(
                K::VariableDeclaration
                | K::Parameter
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::Constructor
                | K::MethodDeclaration
                | K::MethodSignature
                | K::GetAccessor
                | K::SetAccessor
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::TypeAssertionExpression,
            ) => read.type_node() == Some(node),
            Some(K::CallExpression | K::NewExpression | K::TaggedTemplateExpression) => self
                .source_list(parent, read.type_argument_list())?
                .contains(&node),
            _ => false,
        })
    }

    // port: tsc/internal/ast/utilities.go:isPartOfTypeExpressionWithTypeArguments
    pub(crate) fn is_type_heritage_expression(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        if matches!(
            read.kind().known(),
            Some(K::JSDocImplementsTag | K::JSDocAugmentsTag)
        ) {
            return Ok(true);
        }
        if read.kind() != K::HeritageClause {
            return Ok(false);
        }
        if read
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::MissingLink("heritage"))?
            .token()
            == K::ImplementsKeyword
        {
            return Ok(true);
        }
        Ok(!read
            .parent()
            .map(|parent| {
                self.ast(parent)?
                    .node(parent)
                    .map(|p| {
                        matches!(
                            p.kind().known(),
                            Some(K::ClassDeclaration | K::ClassExpression)
                        )
                    })
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false))
    }
}
