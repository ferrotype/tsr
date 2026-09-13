//! Shared grammar lists retain native ranges and first-error ordering.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, JsString, NodeListId, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.grammarErrorAtPos
    pub(crate) fn grammar_error_range(
        &mut self,
        node: NodeId,
        start: i64,
        end: i64,
        message: &'static d::Message,
    ) -> Result<bool, Error> {
        self.grammar_error_range_with_args(node, start, end, message, vec![])
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForDisallowedTrailingComma
    pub(crate) fn check_grammar_trailing_comma(
        &mut self,
        owner: NodeId,
        list: NodeListId,
        message: &'static d::Message,
    ) -> Result<bool, Error> {
        let view = self.ast(owner)?;
        if !view.list_has_trailing_comma(list)? {
            return Ok(false);
        }
        let read = view.list(list)?;
        let first = view
            .node_slice(read.nodes())?
            .iter()
            .next()
            .flatten()
            .ok_or(Error::MissingLink("trailing comma first element"))?;
        self.grammar_error_range(first, read.loc().end() - 1, read.loc().end(), message)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarTypeParameterList
    pub(crate) fn check_grammar_type_parameter_list(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let Some(list) = view.node(node)?.type_parameter_list() else {
            return Ok(false);
        };
        let list = view.list(list)?;
        if !view.node_slice(list.nodes())?.is_empty() {
            return Ok(false);
        }
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::MissingLink("type parameter source"))?;
        let end = ts_scanner::skip_trivia(
            view.source_file(source)?.text().as_bytes(),
            list.loc().end(),
        ) + 1;
        self.grammar_error_range(
            source,
            list.loc().pos() - 1,
            end,
            d::Type_parameter_list_cannot_be_empty,
        )
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarFunctionLikeDeclaration
    pub(crate) fn check_grammar_function_like(&mut self, node: NodeId) -> Result<bool, Error> {
        if self.check_grammar_modifiers(node)? || self.check_grammar_type_parameter_list(node)? {
            return Ok(true);
        }
        let list = self.ast(node)?.node(node)?.parameter_list();
        let parameters = self.source_list(node, list)?;
        if self.check_parameter_list_grammar(&parameters)? {
            return Ok(true);
        }
        if self.ast(node)?.node(node)?.kind() == K::ArrowFunction {
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("arrow grammar source"))?;
            if let Some(list) = self.ast(node)?.node(node)?.type_parameter_list() {
                let parameters = self.source_list(node, Some(list))?;
                let constraint = match parameters.first() {
                    Some(&first) => self
                        .ast(first)?
                        .node(first)?
                        .data_source()
                        .as_type_parameter_declaration()
                        .ok_or(Error::MissingLink("arrow parameter"))?
                        .constraint()
                        .is_some(),
                    None => false,
                };
                let file = self.ast(source)?.source_file(source)?;
                if parameters.len() <= 1
                    && !self.ast(node)?.list_has_trailing_comma(list)?
                    && !constraint
                    && (file.parse_options().file_name.as_bytes().ends_with(b".mts")
                        || file.parse_options().file_name.as_bytes().ends_with(b".cts"))
                {
                    self.grammar_error_node(*parameters.first().ok_or(Error::MissingLink("arrow type parameter"))?,d::This_syntax_is_reserved_in_files_with_the_mts_or_cts_extension_Add_a_trailing_comma_or_explicit_constraint,vec![])?;
                }
            }
            let token = self
                .ast(node)?
                .node(node)?
                .data_source()
                .as_arrow_function()
                .ok_or(Error::MissingLink("arrow grammar"))?
                .equals_greater_than_token()
                .ok_or(Error::MissingLink("arrow token"))?;
            let read = self.ast(token)?.node(token)?;
            let file = self.ast(source)?.source_file(source)?;
            let bytes = file.text().as_bytes();
            let start =
                usize::try_from(read.pos()).map_err(|_| Error::MissingLink("arrow start"))?;
            let end = usize::try_from(read.end()).map_err(|_| Error::MissingLink("arrow end"))?;
            let text = bytes
                .get(start..end)
                .ok_or(Error::MissingLink("arrow source range"))?;
            if text.contains(&b'\n')
                || text.contains(&b'\r')
                || text
                    .windows(3)
                    .any(|bytes| matches!(bytes, b"\xe2\x80\xa8" | b"\xe2\x80\xa9"))
            {
                return self.grammar_error_node(
                    token,
                    d::Line_terminator_not_permitted_before_arrow,
                    vec![],
                );
            }
        }
        self.check_body_use_strict_parameters(node)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarVariableDeclarationList
    pub(crate) fn check_grammar_variable_list(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let list = read
            .data_source()
            .as_variable_declaration_list()
            .ok_or(Error::MissingLink("variable list"))?
            .declarations()
            .ok_or(Error::MissingLink("variable declarations"))?;
        if self.check_grammar_trailing_comma(node, list, d::Trailing_comma_not_allowed)? {
            return Ok(true);
        }
        let read = self.ast(node)?.list(list)?;
        if self.ast(node)?.node_slice(read.nodes())?.is_empty() {
            return self.grammar_error_range(
                node,
                read.loc().pos(),
                read.loc().end(),
                d::Variable_declaration_list_cannot_be_empty,
            );
        }
        let read = self.ast(node)?.node(node)?;
        let flags = read.flags();
        let block_scope = flags & nf::BLOCK_SCOPED;
        if block_scope == nf::USING || block_scope == nf::AWAIT_USING {
            let using = block_scope == nf::USING;
            let parent = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("variable list parent"))?;
            let parent_read = self.ast(parent)?.node(parent)?;
            if parent_read.kind() == K::ForInStatement {
                return self.grammar_error_node(
                    node,
                    if using {
                        d::The_left_hand_side_of_a_for_in_statement_cannot_be_a_using_declaration
                    } else {
                        d::The_left_hand_side_of_a_for_in_statement_cannot_be_an_await_using_declaration
                    },
                    vec![],
                );
            }
            if flags & nf::AMBIENT != 0 {
                return self.grammar_error_node(
                    node,
                    if using {
                        d::X_using_declarations_are_not_allowed_in_ambient_contexts
                    } else {
                        d::X_await_using_declarations_are_not_allowed_in_ambient_contexts
                    },
                    vec![],
                );
            }
            let in_clause = parent_read.kind() == K::VariableStatement
                && parent_read
                    .parent()
                    .map(|grandparent| {
                        Ok::<_, Error>(matches!(
                            self.ast(grandparent)?.node(grandparent)?.kind().known(),
                            Some(K::CaseClause | K::DefaultClause)
                        ))
                    })
                    .transpose()?
                    .unwrap_or(false);
            if in_clause {
                return self.grammar_error_node(
                    node,
                    if using {
                        d::X_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block
                    } else {
                        d::X_await_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block
                    },
                    vec![],
                );
            }
        }
        if block_scope == nf::AWAIT_USING {
            return self.check_grammar_await_or_await_using(node);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.containerAllowsBlockScopedVariable
    pub(crate) fn container_allows_block_scoped_variable(
        &self,
        mut node: NodeId,
    ) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(
                    K::IfStatement
                    | K::DoStatement
                    | K::WhileStatement
                    | K::WithStatement
                    | K::ForStatement
                    | K::ForInStatement
                    | K::ForOfStatement,
                ) => return Ok(false),
                Some(K::LabeledStatement) => {
                    node = read.parent().ok_or(Error::MissingLink("label parent"))?;
                }
                _ => return Ok(true),
            }
        }
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForDisallowedBlockScopedVariableStatement
    pub(crate) fn check_grammar_block_variable(
        &mut self,
        node: NodeId,
        list: NodeId,
    ) -> Result<(), Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("variable statement parent"))?;
        if !self.container_allows_block_scoped_variable(parent)? {
            let flags = self.ast(list)?.node(list)?.flags() & nf::BLOCK_SCOPED;
            let keyword = match flags {
                nf::LET => Some(b"let".as_slice()),
                nf::CONST => Some(b"const".as_slice()),
                nf::USING => Some(b"using".as_slice()),
                nf::AWAIT_USING => Some(b"await using".as_slice()),
                _ => None,
            };
            if let Some(keyword) = keyword {
                self.error_at(
                    Some(node),
                    d::X_0_declarations_can_only_be_declared_inside_a_block,
                    vec![JsString::from_bytes(keyword)],
                )?;
            }
        }
        Ok(())
    }
}
