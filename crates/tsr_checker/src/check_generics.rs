//! Checking source generic parameters and argument lists. Type-node resolution
//! is separate from checking their source children, even when resolution hits a
//! cache or eliminates a constituent.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeParametersForTypeAndSymbol
    pub(crate) fn type_parameters_for_type_and_symbol(
        &mut self,
        ty: TypeId,
        symbol: tsr_arena::SymbolId,
    ) -> Result<crate::TypeList, Error> {
        if self.is_error_type(ty)? {
            return Ok(crate::TypeList::default());
        }
        if self.symbol(symbol)?.flags() & tsr_ast::symbol_flags::TYPE_ALIAS != 0 {
            if let Some(parameters) = &self.query.type_aliases.get_or_default(symbol).parameters {
                if !parameters.is_empty() {
                    return Ok(parameters.clone());
                }
            }
        }
        if self.types.object_flags(ty)? & crate::object_flags::REFERENCE != 0 {
            let interface = self.types.interface(self.types.target(ty)?)?;
            return Ok(interface.type_parameters()
                [interface.outer_type_parameter_count as usize..]
                .to_vec()
                .into());
        }
        Ok(crate::TypeList::default())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeParameters
    pub(crate) fn check_type_parameters(&mut self, node: NodeId) -> Result<(), Error> {
        let nodes = self.source_list(node, self.node(node)?.type_parameter_list())?;
        let mut seen_default = false;
        for (index, &parameter) in nodes.iter().enumerate() {
            self.check_type_parameter(parameter)?;
            let read = self.node(parameter)?;
            let default = read
                .data_source()
                .as_type_parameter_declaration()
                .ok_or(Error::MissingLink("type parameter declaration"))?
                .default_type();
            let name = read.name();
            if let Some(default) = default {
                seen_default = true;
                self.check_default_references(default, &nodes[index..])?;
            } else if seen_default {
                self.error_at(Some(parameter), tsr_diagnostics::Required_type_parameters_may_not_follow_optional_type_parameters, vec![])?;
            }
            let symbol = self.get_symbol_of_declaration(parameter)?;
            for &previous in &nodes[..index] {
                if self.get_symbol_of_declaration(previous)? == symbol {
                    let text = tsr_scanner::declaration_name_to_string(self.ast(parameter)?, name)?;
                    self.error_at(name, tsr_diagnostics::Duplicate_identifier_0, vec![text])?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeParametersNotReferenced
    fn check_default_references(&mut self, node: NodeId, later: &[NodeId]) -> Result<(), Error> {
        if self.node(node)?.kind() == K::TypeReference {
            let ty = self.source_type_reference(node)?;
            if self.types.flags(ty)? & tf::TYPE_PARAMETER != 0 {
                for &parameter in later {
                    let parameter_symbol = self.get_symbol_of_declaration(parameter)?;
                    if self.types.get(ty)?.symbol == parameter_symbol {
                        self.error_at(Some(node), tsr_diagnostics::Type_parameter_defaults_can_only_reference_previously_declared_type_parameters, vec![])?;
                    }
                }
            }
        }
        for child in self.source_children(node)? {
            self.check_default_references(child, later)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeParameter
    pub(crate) fn check_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_grammar_modifiers(node)?;
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingLink("type parameter declaration"))?;
        let constraint = data.constraint();
        let default = data.default_type();
        if let Some(expression) = data.expression() {
            self.grammar_error_first_token(expression, tsr_diagnostics::Type_expected, vec![])?;
        }
        if let Some(constraint) = constraint {
            self.check_source_element(constraint)?;
        }
        if let Some(default) = default {
            self.check_source_element(default)?;
        }
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("type parameter symbol"))?;
        let ty = self.get_declared_type_of_type_parameter(symbol)?;
        self.base_constraint_of_type(ty)?;
        let default_type = self.resolved_type_parameter_default(ty)?;
        if default_type == self.builtins.circular_constraint_type {
            let name = self.type_to_string(ty, crate::type_format_flags::NONE)?;
            self.error_at(
                default,
                tsr_diagnostics::Type_parameter_0_has_a_circular_default,
                vec![name],
            )?;
        }
        if let Some(constraint) = self.constraint_of_type_parameter(ty)? {
            if default_type != self.builtins.circular_constraint_type
                && default_type != self.builtins.no_constraint_type
            {
                let mapper = self.new_type_mapper(&[ty], &[default_type])?;
                let constraint = self.instantiate_type(constraint, Some(mapper))?;
                let constraint =
                    self.get_type_with_this_argument(constraint, default_type, false)?;
                self.check_constraint_assignable(default_type, constraint, default)?;
            }
        }
        let name = self
            .ast(node)?
            .node(node)?
            .name()
            .ok_or(Error::MissingLink("type parameter name"))?;
        let text = self.node_text(name)?.into_js_string();
        if matches!(
            text.as_bytes(),
            b"any"
                | b"unknown"
                | b"never"
                | b"number"
                | b"bigint"
                | b"boolean"
                | b"string"
                | b"symbol"
                | b"void"
                | b"object"
                | b"undefined"
        ) {
            self.error_at(
                Some(name),
                tsr_diagnostics::Type_parameter_name_cannot_be_0,
                vec![text],
            )?;
        }
        self.defer_checker_node(node)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeParameterDeferred
    pub(crate) fn check_type_parameter_deferred(&mut self, node: NodeId) -> Result<(), Error> {
        use tsr_ast::modifier_flags as mf;
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("type parameter parent"))?;
        let kind = self.node(parent)?.kind();
        if !matches!(
            kind.known(),
            Some(
                K::InterfaceDeclaration
                    | K::ClassDeclaration
                    | K::ClassExpression
                    | K::TypeAliasDeclaration
            )
        ) {
            return Ok(());
        }
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("type parameter symbol"))?;
        let parameter = self.get_declared_type_of_type_parameter(symbol)?;
        let modifiers = self.type_parameter_modifiers(parameter)? & (mf::IN | mf::OUT);
        if modifiers == 0 {
            return Ok(());
        }
        let parent_symbol = self
            .get_symbol_of_declaration(parent)?
            .ok_or(Error::MissingLink("variance container symbol"))?;
        if kind == K::TypeAliasDeclaration {
            let declared = self.get_declared_type_of_symbol(parent_symbol)?;
            if self.types.object_flags(declared)?
                & (crate::object_flags::ANONYMOUS | crate::object_flags::MAPPED)
                == 0
            {
                self.error_at(Some(node), tsr_diagnostics::Variance_annotations_are_only_supported_in_type_aliases_for_object_function_constructor_and_mapped_types, vec![])?;
                return Ok(());
            }
        }
        if modifiers == mf::IN || modifiers == mf::OUT {
            let (sub, super_) = (
                self.builtins.marker_sub_type_for_check,
                self.builtins.marker_super_type_for_check,
            );
            let (source, target) = if modifiers == mf::OUT {
                (sub, super_)
            } else {
                (super_, sub)
            };
            let source = self.create_marker_type(parent_symbol, parameter, source)?;
            let target = self.create_marker_type(parent_symbol, parameter, target)?;
            self.variance.checked_parameter = Some(parameter);
            let result = self.check_type_related_ex(source, target, crate::RelationKind::Assignable, Some(node), Some(tsr_diagnostics::Type_0_is_not_assignable_to_type_1_as_implied_by_variance_annotation));
            // The pin retains this parameter after the check (its saved value is
            // the current parameter), which also controls later marker display.
            self.variance.checked_parameter = Some(parameter);
            if let (_, Some(diagnostic)) = result? {
                self.add_diagnostic(diagnostic)?;
            }
        }
        Ok(())
    }

    fn check_constraint_assignable(
        &mut self,
        source: TypeId,
        target: TypeId,
        node: Option<NodeId>,
    ) -> Result<bool, Error> {
        let (related, diagnostic) = self.check_type_related_ex(
            source,
            target,
            crate::RelationKind::Assignable,
            node,
            Some(tsr_diagnostics::Type_0_does_not_satisfy_the_constraint_1),
        )?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(related)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeReferenceNode
    // port: tsc/internal/checker/checker.go:Checker.checkTypeArgumentConstraints
    pub(crate) fn check_type_reference_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_grammar_type_arguments(node)?;
        let read = self.node(node)?;
        if read.kind() == tsr_ast::SyntaxKind::TypeReference
            && read.flags() & tsr_ast::node_flags::JS_DOC == 0
        {
            if let (Some(name), Some(arguments)) = (read.name(), read.type_argument_list()) {
                let end = self.node(name)?.end();
                let list = self.ast(node)?.list(arguments)?;
                if i64::from(end) != list.loc().pos() {
                    let view = self.ast(node)?;
                    let source_id = tsr_ast::utilities::get_source_file_of_node(view, Some(node))?
                        .ok_or(Error::MissingLink("type reference source"))?;
                    let source = view.source_file(source_id)?;
                    if tsr_scanner::scan_token_at_position(view, source_id, i64::from(end))?
                        == tsr_ast::SyntaxKind::DotToken
                    {
                        let start =
                            tsr_scanner::skip_trivia(source.text().as_bytes(), i64::from(end));
                        self.grammar_error_range(node, start, start + 1, tsr_diagnostics::JSDoc_types_can_only_be_used_inside_documentation_comments)?;
                    }
                }
            }
        }
        let nodes = self.source_list(node, self.node(node)?.type_argument_list())?;
        for &node in &nodes {
            self.check_source_element(node)?;
        }
        self.check_type_reference_or_import(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeReferenceOrImport
    pub(crate) fn check_type_reference_or_import(&mut self, node: NodeId) -> Result<(), Error> {
        let ty = self.get_type_from_type_node(node)?;
        if self.is_error_type(ty)? {
            return Ok(());
        }
        let Some(symbol) = self.query.resolved_symbols.try_get(node).copied().flatten() else {
            return Ok(());
        };
        if !self
            .source_list(node, self.node(node)?.type_argument_list())?
            .is_empty()
        {
            let parameters = self.type_parameters_for_type_and_symbol(ty, symbol)?;
            if !parameters.is_empty() {
                self.check_type_argument_constraints(node, &parameters)?;
            }
        }
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        for declaration in declarations.iter().flatten() {
            if self.is_type_declaration(*declaration)?
                && self.is_deprecated_declaration(*declaration)?
            {
                let location = self.deprecated_suggestion_node(node)?;
                let name = self.symbol(symbol)?.name_to_owned();
                self.add_deprecated_suggestion(location, &declarations, name)?;
                break;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeArgumentConstraints
    pub(crate) fn check_type_argument_constraints(
        &mut self,
        node: NodeId,
        parameters: &[TypeId],
    ) -> Result<bool, Error> {
        let nodes = self.source_list(node, self.node(node)?.type_argument_list())?;
        let mut arguments = None;
        let mut mapper = None;
        let mut valid = true;
        for (index, &parameter) in parameters.iter().enumerate() {
            if let Some(constraint) = self.constraint_of_type_parameter(parameter)? {
                if arguments.is_none() {
                    let effective = self.effective_type_arguments(node, parameters)?;
                    mapper = Some(self.new_type_mapper(parameters, &effective)?);
                    arguments = Some(effective);
                }
                if valid {
                    let source = arguments
                        .as_ref()
                        .ok_or(Error::MissingLink("effective arguments"))?[index];
                    let constraint = self.instantiate_type(constraint, mapper)?;
                    valid = self.check_constraint_assignable(
                        source,
                        constraint,
                        nodes.get(index).copied(),
                    )?;
                }
            }
        }
        Ok(valid)
    }
}
