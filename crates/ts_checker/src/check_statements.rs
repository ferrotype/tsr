//! Statement checks retain the native order of grammar, expressions and bodies.
use crate::{type_flags as tf, CheckerState, Error, RelationKind};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, symbol_flags as sf, Diagnostic, SyntaxKind as K};
use ts_core::{TextRange, Tristate};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkDoStatement
    // port: tsc/internal/checker/checker.go:Checker.checkWhileStatement
    pub(crate) fn check_loop_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_statement_ambient_context(node)?;
        let read = self.ast(node)?.node(node)?;
        let expression = required(read.expression(), "loop condition")?;
        let statement = required(read.statement(), "loop statement")?;
        if read.kind() == K::DoStatement {
            self.check_source_element(statement)?;
            self.check_truthiness_expression(expression)?;
        } else {
            self.check_truthiness_expression(expression)?;
            self.check_source_element(statement)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkForStatement
    pub(crate) fn check_for_statement(&mut self, node: NodeId) -> Result<(), Error> {
        let ambient = self.check_statement_ambient_context(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_for_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let initializer = data.initializer();
        let condition = data.condition();
        let incrementor = data.incrementor();
        let statement = required(data.statement(), "for body")?;
        if let Some(initializer) = initializer {
            if self.ast(initializer)?.node(initializer)?.kind() == K::VariableDeclarationList {
                if !ambient {
                    self.check_grammar_variable_list(initializer)?;
                }
                self.check_source_element(initializer)?;
            } else {
                self.check_expression(initializer)?;
            }
        }
        if let Some(condition) = condition {
            self.check_truthiness_expression(condition)?;
        }
        if let Some(incrementor) = incrementor {
            self.check_expression(incrementor)?;
        }
        self.check_source_element(statement)?;
        self.register_locals_for_unused_check(node)
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForInOrForOfStatement
    fn check_grammar_for_in_or_of(&mut self, node: NodeId) -> Result<bool, Error> {
        if self.check_statement_ambient_context(node)? {
            return Ok(true);
        }
        let read = self.ast(node)?.node(node)?;
        let for_in = read.kind() == K::ForInStatement;
        let data = read
            .data_source()
            .as_for_in_or_of_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let initializer = required(data.initializer(), "iteration initializer")?;
        let await_modifier = data.await_modifier();
        let await_context = read.flags() & nf::AWAIT_CONTEXT != 0;
        if let Some(modifier) = await_modifier {
            if !await_context && self.check_grammar_for_await_context(node, modifier)? {
                return Ok(true);
            }
        }
        if !for_in
            && !await_context
            && self.ast(initializer)?.node(initializer)?.kind() == K::Identifier
            && self.ast(initializer)?.node_text(initializer)?.as_bytes() == b"async"
        {
            self.grammar_error_node(
                initializer,
                d::The_left_hand_side_of_a_for_of_statement_may_not_be_async,
                vec![],
            )?;
            return Ok(false);
        }
        if self.ast(initializer)?.node(initializer)?.kind() == K::VariableDeclarationList
            && !self.check_grammar_variable_list(initializer)?
        {
            let declarations = self.source_list(
                initializer,
                self.ast(initializer)?
                    .node(initializer)?
                    .data_source()
                    .as_variable_declaration_list()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .declarations(),
            )?;
            if declarations.len() > 1 {
                return self.grammar_error_first_token(
                    declarations[1],
                    if for_in {
                        d::Only_a_single_variable_declaration_is_allowed_in_a_for_in_statement
                    } else {
                        d::Only_a_single_variable_declaration_is_allowed_in_a_for_of_statement
                    },
                    vec![],
                );
            }
            if let Some(&declaration) = declarations.first() {
                let read = self.ast(declaration)?.node(declaration)?;
                if read.initializer().is_some() {
                    return self.grammar_error_node(required(read.name(),"iteration variable name")?,if for_in{d::The_variable_declaration_of_a_for_in_statement_cannot_have_an_initializer}else{d::The_variable_declaration_of_a_for_of_statement_cannot_have_an_initializer},vec![]);
                }
                if read.type_node().is_some() {
                    return self.grammar_error_node(
                        declaration,
                        if for_in {
                            d::The_left_hand_side_of_a_for_in_statement_cannot_use_a_type_annotation
                        } else {
                            d::The_left_hand_side_of_a_for_of_statement_cannot_use_a_type_annotation
                        },
                        vec![],
                    );
                }
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkForInStatement
    pub(crate) fn check_for_in_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_grammar_for_in_or_of(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_for_in_or_of_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let initializer = required(data.initializer(), "for-in initializer")?;
        let expression = required(data.expression(), "for-in expression")?;
        let statement = required(data.statement(), "for-in body")?;
        let right = self.check_expression(expression)?;
        let right = self.non_nullable_type_if_needed(right)?;
        if self.ast(initializer)?.node(initializer)?.kind() == K::VariableDeclarationList {
            let declarations = self.source_list(
                initializer,
                self.ast(initializer)?
                    .node(initializer)?
                    .data_source()
                    .as_variable_declaration_list()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .declarations(),
            )?;
            if let Some(&declaration) = declarations.first() {
                let name = required(
                    self.ast(declaration)?.node(declaration)?.name(),
                    "for-in binding",
                )?;
                if matches!(
                    self.ast(name)?.node(name)?.kind().known(),
                    Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                ) {
                    self.error_at(Some(name),d::The_left_hand_side_of_a_for_in_statement_cannot_be_a_destructuring_pattern,vec![])?;
                }
            }
            self.check_source_element(initializer)?;
        } else {
            let left = self.check_expression(initializer)?;
            if matches!(
                self.ast(initializer)?.node(initializer)?.kind().known(),
                Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression)
            ) {
                self.error_at(
                    Some(initializer),
                    d::The_left_hand_side_of_a_for_in_statement_cannot_be_a_destructuring_pattern,
                    vec![],
                )?;
            } else {
                let index = self.for_in_index_type(right)?;
                if self.is_type_related_to(index, left, RelationKind::Assignable)? {
                    self.check_reference_expression(initializer,d::The_left_hand_side_of_a_for_in_statement_must_be_a_variable_or_a_property_access,d::The_left_hand_side_of_a_for_in_statement_may_not_be_an_optional_property_access)?;
                } else {
                    self.error_at(
                        Some(initializer),
                        d::The_left_hand_side_of_a_for_in_statement_must_be_of_type_string_or_any,
                        vec![],
                    )?;
                }
            }
        }
        if right == self.builtins.never_type
            || !self.type_assignable_to_kind(
                right,
                tf::NON_PRIMITIVE | tf::INSTANTIABLE_NON_PRIMITIVE,
            )?
        {
            let text = self.type_to_string(right, crate::type_display::DEFAULT_FLAGS)?;
            self.error_at(Some(expression),d::The_right_hand_side_of_a_for_in_statement_must_be_of_type_any_an_object_type_or_a_type_parameter_but_here_has_type_0,vec![text])?;
        }
        self.check_source_element(statement)?;
        self.register_locals_for_unused_check(node)
    }
    // port: tsc/internal/checker/checker.go:Checker.getIndexTypeOrString
    pub(crate) fn for_in_index_type(&mut self, ty: crate::TypeId) -> Result<crate::TypeId, Error> {
        let key = self.get_index_type(ty, 0)?;
        let extracted = self.extract_string_type(key)?;
        Ok(if self.types.flags(extracted)? & tf::NEVER != 0 {
            self.builtins.string_type
        } else {
            extracted
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getExtractStringType
    pub(crate) fn extract_string_type(
        &mut self,
        key: crate::TypeId,
    ) -> Result<crate::TypeId, Error> {
        if let Some(symbol) =
            self.lookup_symbol(self.builtins.globals, b"Extract", sf::TYPE_ALIAS)?
        {
            let declared = self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.clone())
                .unwrap_or_default();
            if parameters.len() == 2 {
                return self.type_alias_instantiation(
                    symbol,
                    declared,
                    &parameters,
                    &[key, self.builtins.string_type],
                    None,
                );
            }
        }
        Ok(self.builtins.string_type)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkForOfStatement
    pub(crate) fn check_for_of_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_grammar_for_in_or_of(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_for_in_or_of_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let await_modifier = data.await_modifier();
        let initializer = required(data.initializer(), "for-of initializer")?;
        let statement = required(data.statement(), "for-of body")?;
        if let Some(modifier) = await_modifier {
            self.check_for_await_container(node, modifier)?;
        }
        if self.ast(initializer)?.node(initializer)?.kind() == K::VariableDeclarationList {
            self.check_source_element(initializer)?;
        } else {
            let iterated = self.check_right_hand_side_of_for_of(node)?;
            if matches!(
                self.ast(initializer)?.node(initializer)?.kind().known(),
                Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression)
            ) {
                self.check_destructuring_assignment(initializer, iterated, 0, false)?;
            } else {
                let left = self.check_expression(initializer)?;
                self.check_reference_expression(
                    initializer,
                    d::The_left_hand_side_of_a_for_of_statement_must_be_a_variable_or_a_property_access,
                    d::The_left_hand_side_of_a_for_of_statement_may_not_be_an_optional_property_access,
                )?;
                self.check_assignable_at(iterated, left, initializer)?;
            }
        }
        self.check_source_element(statement)?;
        self.register_locals_for_unused_check(node)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkReferenceExpression
    pub(crate) fn check_reference_expression(
        &mut self,
        expression: NodeId,
        invalid: &'static d::Message,
        optional: &'static d::Message,
    ) -> Result<bool, Error> {
        let mut node = expression;
        loop {
            let read = self.ast(node)?.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::ParenthesizedExpression
                        | K::TypeAssertionExpression
                        | K::AsExpression
                        | K::NonNullExpression
                        | K::SatisfiesExpression
                )
            ) {
                node = required(read.expression(), "reference assertion operand")?;
                continue;
            }
            if !matches!(
                read.kind().known(),
                Some(K::Identifier | K::PropertyAccessExpression | K::ElementAccessExpression)
            ) {
                self.error_at(Some(expression), invalid, vec![])?;
                return Ok(false);
            }
            if read.flags() & nf::OPTIONAL_CHAIN != 0 {
                self.error_at(Some(expression), optional, vec![])?;
                return Ok(false);
            }
            return Ok(true);
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkBreakOrContinueStatement
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarBreakOrContinueStatement
    pub(crate) fn check_jump_statement(&mut self, node: NodeId) -> Result<(), Error> {
        if self.check_statement_ambient_context(node)? {
            return Ok(());
        }
        let read = self.ast(node)?.node(node)?;
        let is_continue = read.kind() == K::ContinueStatement;
        let label = read.label();
        let label = label
            .map(|label| {
                self.ast(label)?
                    .node_text(label)
                    .map(ts_ast::NodeText::into_js_string)
                    .map_err(Error::from)
            })
            .transpose()?;
        let mut current = Some(node);
        while let Some(id) = current {
            let read = self.ast(id)?.node(id)?;
            if ts_ast::utilities::is_function_like(Some(&read))
                || read.kind() == K::ClassStaticBlockDeclaration
            {
                self.grammar_error_node(
                    node,
                    d::Jump_target_cannot_cross_function_boundary,
                    vec![],
                )?;
                return Ok(());
            }
            if read.kind() == K::LabeledStatement {
                let current_label = required(read.label(), "jump target label")?;
                let current_text = self.ast(current_label)?.node_text(current_label)?;
                if label
                    .as_ref()
                    .is_some_and(|label| current_text.as_bytes() == label.as_bytes())
                {
                    let statement = required(read.statement(), "jump target statement")?;
                    if is_continue && !self.iteration_statement(statement, true)? {
                        self.grammar_error_node(node,d::A_continue_statement_can_only_jump_to_a_label_of_an_enclosing_iteration_statement,vec![])?;
                    }
                    return Ok(());
                }
            } else if read.kind() == K::SwitchStatement && !is_continue && label.is_none()
                || self.iteration_statement(id, false)? && label.is_none()
            {
                return Ok(());
            }
            current = self.ast(id)?.node(id)?.parent();
        }
        let message = match (is_continue, label.is_some()) {
            (false,true) => d::A_break_statement_can_only_jump_to_a_label_of_an_enclosing_statement,
            (true,true) => d::A_continue_statement_can_only_jump_to_a_label_of_an_enclosing_iteration_statement,
            (false,false) => d::A_break_statement_can_only_be_used_within_an_enclosing_iteration_or_switch_statement,
            (true,false) => d::A_continue_statement_can_only_be_used_within_an_enclosing_iteration_statement,
        };
        self.grammar_error_node(node, message, vec![])?;
        Ok(())
    }
    fn iteration_statement(&self, mut node: NodeId, labels: bool) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            if labels && read.kind() == K::LabeledStatement {
                node = required(read.statement(), "labeled iteration")?;
                continue;
            }
            return Ok(matches!(
                read.kind().known(),
                Some(
                    K::DoStatement
                        | K::WhileStatement
                        | K::ForStatement
                        | K::ForInStatement
                        | K::ForOfStatement
                )
            ));
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.checkSwitchStatement
    pub(crate) fn check_switch_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_statement_ambient_context(node)?;
        let expression = required(
            self.ast(node)?.node(node)?.expression(),
            "switch expression",
        )?;
        let expression_type = self.check_expression(expression)?;
        let block = required(
            self.ast(node)?
                .node(node)?
                .data_source()
                .as_switch_statement()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .case_block(),
            "switch case block",
        )?;
        let clauses = self.source_list(
            block,
            self.ast(block)?
                .node(block)?
                .data_source()
                .as_case_block()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .clauses(),
        )?;
        let mut default_seen = false;
        let mut duplicate_reported = false;
        for clause in clauses {
            let read = self.ast(clause)?.node(clause)?;
            if read.kind() == K::DefaultClause && !duplicate_reported {
                if default_seen {
                    self.grammar_error_node(
                        clause,
                        d::A_default_clause_cannot_appear_more_than_once_in_a_switch_statement,
                        vec![],
                    )?;
                    duplicate_reported = true;
                } else {
                    default_seen = true;
                }
            }
            if self.ast(clause)?.node(clause)?.kind() == K::CaseClause {
                let expression = required(
                    self.ast(clause)?.node(clause)?.expression(),
                    "case expression",
                )?;
                let case_type = self.check_expression(expression)?;
                if self.types.flags(case_type)? & tf::NULLABLE == 0
                    && !self.is_type_related_to(
                        expression_type,
                        case_type,
                        RelationKind::Comparable,
                    )?
                {
                    let (_, diagnostic) = self.check_type_related_ex(
                        case_type,
                        expression_type,
                        RelationKind::Comparable,
                        Some(expression),
                        None,
                    )?;
                    if let Some(diagnostic) = diagnostic {
                        self.add_diagnostic(diagnostic)?;
                    }
                }
            }
            let view = self.ast(clause)?;
            let statements: Vec<_> = view
                .node_slice(view.node(clause)?.statements(view)?)?
                .iter()
                .collect();
            for statement in statements {
                self.check_source_element(required(statement, "case statement")?)?;
            }
            if self
                .program()?
                .host
                .options()
                .no_fallthrough_cases_in_switch
                == Tristate::TRUE
            {
                if let Some(flow) = self
                    .program()?
                    .bound(clause)?
                    .node_binding(clause)?
                    .and_then(|binding| binding.fallthrough_flow_node)
                {
                    if self.reachable_flow(clause, flow)? {
                        self.error_at(Some(clause), d::Fallthrough_case_in_switch, vec![])?;
                    }
                }
            }
        }
        self.register_locals_for_unused_check(block)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkLabeledStatement
    pub(crate) fn check_labeled_statement(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let label = required(read.label(), "statement label")?;
        let statement = required(read.statement(), "labeled statement")?;
        let text = self.ast(label)?.node_text(label)?.into_js_string();
        if !self.check_statement_ambient_context(node)? {
            let mut current = self.ast(node)?.node(node)?.parent();
            while let Some(id) = current {
                let read = self.ast(id)?.node(id)?;
                if ts_ast::utilities::is_function_like(Some(&read)) {
                    break;
                }
                if read.kind() == K::LabeledStatement {
                    let name = required(read.label(), "enclosing label")?;
                    if self.ast(name)?.node_text(name)?.as_bytes() == text.as_bytes() {
                        self.grammar_error_node(label, d::Duplicate_label_0, vec![text.clone()])?;
                        break;
                    }
                }
                current = self.ast(id)?.node(id)?.parent();
            }
        }
        let setting = self.program()?.host.options().allow_unused_labels;
        if self.ast(label)?.node(label)?.flags() & nf::UNREACHABLE != 0 && setting != Tristate::TRUE
        {
            let diagnostic = self.diagnostic_for_node(Some(label), d::Unused_label, vec![])?;
            if setting == Tristate::FALSE {
                self.add_diagnostic(diagnostic)?;
            } else {
                self.add_suggestion_diagnostic(diagnostic)?;
            }
        }
        self.check_source_element(statement)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkWithStatement
    pub(crate) fn check_with_statement(&mut self, node: NodeId) -> Result<(), Error> {
        if !self.check_statement_ambient_context(node)?
            && self.ast(node)?.node(node)?.flags() & nf::AWAIT_CONTEXT != 0
        {
            self.grammar_error_first_token(
                node,
                d::X_with_statements_are_not_allowed_in_an_async_function_block,
                vec![],
            )?;
        }
        let read = self.ast(node)?.node(node)?;
        let expression = required(read.expression(), "with expression")?;
        let statement = required(read.statement(), "with statement")?;
        self.check_expression(expression)?;
        let source = required(
            ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?,
            "with source",
        )?;
        let view = self.ast(source)?;
        let file = view.source_file(source)?;
        if file.diagnostics().is_empty() {
            let start =
                ts_scanner::skip_trivia(file.text().as_bytes(), i64::from(view.node(node)?.pos()));
            let end = i64::from(view.node(statement)?.pos());
            self.add_diagnostic(Diagnostic::new(Some(source),TextRange::new(start,end),d::The_with_statement_is_not_supported_All_symbols_in_a_with_block_will_have_type_any,vec![]))?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkTryStatement
    pub(crate) fn check_try_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_statement_ambient_context(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_try_statement()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let block = required(data.try_block(), "try block")?;
        let catch = data.catch_clause();
        let finally = data.finally_block();
        self.check_block_statement(block)?;
        if let Some(catch) = catch {
            self.check_catch_clause(catch)?;
        }
        if let Some(finally) = finally {
            self.check_block_statement(finally)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkCatchClause
    pub(crate) fn check_catch_clause(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_catch_clause()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let declaration = data.variable_declaration();
        let block = required(data.block(), "catch block")?;
        if let Some(declaration) = declaration {
            // Catch bindings have their own grammar below. In particular,
            // destructuring here does not require a variable initializer.
            self.check_variable_like(declaration)?;
            let read = self.ast(declaration)?.node(declaration)?;
            if let Some(annotation) = read.type_node() {
                let ty = self.get_type_from_type_node(annotation)?;
                if self.types.flags(ty)? & tf::ANY_OR_UNKNOWN == 0 {
                    self.grammar_error_first_token(annotation,d::Catch_clause_variable_type_annotation_must_be_any_or_unknown_if_specified,vec![])?;
                }
            } else if let Some(initializer) = read.initializer() {
                self.grammar_error_first_token(
                    initializer,
                    d::Catch_clause_variable_cannot_have_an_initializer,
                    vec![],
                )?;
            } else {
                let block_locals = self
                    .program()?
                    .bound(block)?
                    .node_binding(block)?
                    .and_then(|binding| binding.locals);
                let locals = self
                    .program()?
                    .bound(node)?
                    .node_binding(node)?
                    .and_then(|binding| binding.locals);
                if let (Some(block_locals), Some(locals)) = (block_locals, locals) {
                    let names: Vec<_> = self
                        .table(locals)?
                        .iter()
                        .map(|(name, _)| ts_ast::JsString::from_bytes(name))
                        .collect();
                    for name in names {
                        if let Some(Some(symbol)) = self.table(block_locals)?.get(name.as_bytes()) {
                            let symbol = self.symbol(symbol)?;
                            if symbol.flags() & sf::BLOCK_SCOPED_VARIABLE != 0 {
                                if let Some(declaration) = symbol.value_declaration() {
                                    self.grammar_error_node(
                                        declaration,
                                        d::Cannot_redeclare_identifier_0_in_catch_clause,
                                        vec![name],
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }
        self.check_block_statement(block)
    }
}
