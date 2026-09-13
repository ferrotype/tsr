//! Class heritage grammar preserves the first invalid clause and exact list ranges.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.grammarErrorAtPos
    pub(crate) fn grammar_error_range_with_args(
        &mut self,
        node: NodeId,
        start: i64,
        end: i64,
        message: &'static d::Message,
        args: Vec<JsString>,
    ) -> Result<bool, Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("grammar range source"))?;
        if !self
            .ast(source)?
            .source_file(source)?
            .diagnostics()
            .is_empty()
        {
            return Ok(false);
        }
        self.add_diagnostic(ts_ast::Diagnostic::new(
            Some(source),
            ts_core::TextRange::new(start, end),
            message,
            args,
        ))?;
        Ok(true)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarTypeArguments
    pub(crate) fn check_grammar_type_arguments(&mut self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let Some(list) = view.node(node)?.type_argument_list() else {
            return Ok(false);
        };
        if self.check_grammar_trailing_comma(node, list, d::Trailing_comma_not_allowed)? {
            return Ok(true);
        }
        let view = self.ast(node)?;
        let read = view.list(list)?;
        if !view.node_slice(read.nodes())?.is_empty() {
            return Ok(false);
        }
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::MissingLink("type argument source"))?;
        let end = ts_scanner::skip_trivia(
            view.source_file(source)?.text().as_bytes(),
            read.loc().end(),
        ) + 1;
        self.grammar_error_range(
            source,
            read.loc().pos() - 1,
            end,
            d::Type_argument_list_cannot_be_empty,
        )
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarHeritageClause
    fn check_heritage_clause_grammar(&mut self, clause: NodeId) -> Result<bool, Error> {
        let read = self.ast(clause)?.node(clause)?;
        let data = read
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::MissingLink("heritage clause"))?;
        let token = data.token();
        let list = data.types();
        if let Some(list) = list {
            if self.check_grammar_trailing_comma(clause, list, d::Trailing_comma_not_allowed)? {
                return Ok(true);
            }
            let view = self.ast(clause)?;
            let read = view.list(list)?;
            if view.node_slice(read.nodes())?.is_empty() {
                let text = ts_scanner::token_to_string(
                    token.known().ok_or(ts_arena::Error::InvalidGraph)?,
                );
                return self.grammar_error_range_with_args(
                    clause,
                    read.loc().pos(),
                    read.loc().pos(),
                    d::X_0_list_cannot_be_empty,
                    vec![JsString::from_bytes(text.as_bytes())],
                );
            }
        }
        for node in self.source_list(clause, list)? {
            let read = self.ast(node)?.node(node)?;
            if read.kind() == K::ExpressionWithTypeArguments && read.type_argument_list().is_some()
            {
                let expression = read
                    .expression()
                    .ok_or(Error::MissingLink("heritage expression"))?;
                if self.ast(expression)?.node(expression)?.kind() == K::ImportKeyword {
                    return self.grammar_error_node(node,d::This_use_of_import_is_invalid_import_calls_can_be_written_but_they_must_have_parentheses_and_cannot_have_type_arguments,vec![]);
                }
            }
            if self.check_grammar_type_arguments(node)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarClassDeclarationHeritageClauses
    pub(crate) fn check_class_heritage_grammar(&mut self, node: NodeId) -> Result<bool, Error> {
        let mut extends = false;
        let mut implements = false;
        if !self.check_grammar_modifiers(node)? {
            for clause in self.class_heritage_clauses(node)? {
                let read = self.ast(clause)?.node(clause)?;
                let data = read
                    .data_source()
                    .as_heritage_clause()
                    .ok_or(Error::MissingLink("class heritage clause"))?;
                if data.token() == K::ExtendsKeyword {
                    if extends {
                        return self.grammar_error_first_token(
                            clause,
                            d::X_extends_clause_already_seen,
                            vec![],
                        );
                    }
                    if implements {
                        return self.grammar_error_first_token(
                            clause,
                            d::X_extends_clause_must_precede_implements_clause,
                            vec![],
                        );
                    }
                    let nodes = self.source_list(clause, data.types())?;
                    if nodes.len() > 1 {
                        return self.grammar_error_first_token(
                            nodes[1],
                            d::Classes_can_only_extend_a_single_class,
                            vec![],
                        );
                    }
                    extends = true;
                } else if data.token() == K::ImplementsKeyword {
                    if implements {
                        return self.grammar_error_first_token(
                            clause,
                            d::X_implements_clause_already_seen,
                            vec![],
                        );
                    }
                    implements = true;
                } else {
                    return Err(ts_arena::Error::InvalidGraph.into());
                }
                self.check_heritage_clause_grammar(clause)?;
            }
        }
        Ok(false)
    }
}
