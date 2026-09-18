//! Tagged templates resolve ordinary call signatures after inserting the
//! TemplateStringsArray argument. Substitutions keep their own source nodes.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, SignatureId, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkTaggedTemplateExpression
    pub(crate) fn check_tagged_template_expression(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_tagged_template_expression()
            .ok_or(Error::MissingLink("tagged template"))?;
        let template = data
            .template()
            .ok_or(Error::MissingLink("tagged template literal"))?;
        let invalid_chain =
            data.question_dot_token().is_some() || read.flags() & nf::OPTIONAL_CHAIN != 0;
        // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarTaggedTemplateChain
        if !invalid_chain
            || !self.grammar_error_node(
                template,
                d::Tagged_template_expressions_are_not_permitted_in_an_optional_chain,
                vec![],
            )?
        {
            self.check_grammar_type_arguments(node)?;
        }
        let saved = self.expression_mode;
        self.expression_mode = 0;
        let signature = self.resolved_call_signature(node);
        self.expression_mode = saved;
        let signature = signature?;
        self.check_deprecated_signature(signature, node)?;
        self.return_type_of_signature(signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveTaggedTemplateExpression
    pub(crate) fn resolve_tagged_template_expression(
        &mut self,
        node: NodeId,
    ) -> Result<SignatureId, Error> {
        let tag = ts_ast::utilities_middle::get_invoked_expression(self.ast(node)?, node)?
            .ok_or(Error::MissingLink("template tag"))?;
        let ty = self.check_expression(tag)?;
        let apparent = self.apparent_type(ty)?;
        if self.is_error_type(apparent)? {
            self.resolve_untyped_call(node)?;
            return Ok(self.builtins.unknown_signature);
        }
        let calls = self.signatures_of_type(apparent, false)?;
        let construct_count = self.signatures_of_type(apparent, true)?.len();
        if self.is_untyped_function_call(ty, apparent, calls.len(), construct_count)? {
            return self.resolve_untyped_call(node);
        }
        if calls.is_empty() {
            let parent = self.node(node)?.parent();
            let array_parent = match parent {
                Some(parent) => self.node(parent)?.kind() == K::ArrayLiteralExpression,
                None => false,
            };
            if array_parent {
                self.error_at(Some(tag), d::It_is_likely_that_you_are_missing_a_comma_to_separate_these_two_template_expressions_They_form_a_tagged_template_expression_which_cannot_be_invoked, vec![])?;
            } else {
                self.call_invocation_error(node, apparent, false)?;
            }
            self.resolve_untyped_call(node)?;
            return Ok(self.builtins.unknown_signature);
        }
        self.resolve_typed_call(node, &calls)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUntypedFunctionCall
    pub(crate) fn is_untyped_function_call(
        &mut self,
        ty: TypeId,
        apparent: TypeId,
        calls: usize,
        constructs: usize,
    ) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::ANY != 0
            || self.types.flags(apparent)? & tf::ANY != 0
                && self.types.flags(ty)? & tf::TYPE_PARAMETER != 0
        {
            return Ok(true);
        }
        if calls == 0 && constructs == 0 && self.types.flags(apparent)? & tf::UNION == 0 {
            let reduced = self.get_reduced_type(apparent)?;
            if self.types.flags(reduced)? & tf::NEVER == 0 {
                let function = *self
                    .query
                    .global_types
                    .get("Function")
                    .ok_or(Error::MissingLink("global Function type"))?;
                return self.is_type_related_to(ty, function, RelationKind::Assignable);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getEffectiveCallArguments
    pub(crate) fn tagged_template_arguments(&mut self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.node(node)?;
        let template = read
            .data_source()
            .as_tagged_template_expression()
            .ok_or(Error::MissingLink("tagged template"))?
            .template()
            .ok_or(Error::MissingLink("tagged template literal"))?;
        let strings = if let Some(&ty) = self.query.global_types.get("TemplateStringsArray") {
            ty
        } else {
            let ty = self.get_global_type("TemplateStringsArray", 0, true)?;
            self.query.global_types.insert("TemplateStringsArray", ty);
            ty
        };
        let mut result = vec![self.synthetic_call_argument(template, strings, false, None)?];
        if self.node(template)?.kind() == K::TemplateExpression {
            let spans = self
                .ast(template)?
                .node(template)?
                .data_source()
                .as_template_expression()
                .ok_or(Error::MissingLink("template expression"))?
                .template_spans();
            for span in self.source_list(template, spans)? {
                result.push(
                    self.node(span)?
                        .expression()
                        .ok_or(Error::MissingLink("template span expression"))?,
                );
            }
        }
        Ok(result)
    }

    pub(crate) fn tagged_template_is_incomplete(&self, node: NodeId) -> Result<bool, Error> {
        let template = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_tagged_template_expression()
            .ok_or(Error::MissingLink("tagged template"))?
            .template()
            .ok_or(Error::MissingLink("tagged template literal"))?;
        let read = self.node(template)?;
        if read.kind() != K::TemplateExpression {
            return Ok(ts_ast::utilities_middle::is_unterminated_literal(&read));
        }
        let spans = read
            .data_source()
            .as_template_expression()
            .ok_or(Error::MissingLink("template expression"))?
            .template_spans();
        let spans = self.source_list(template, spans)?;
        let span = *spans
            .last()
            .ok_or(Error::MissingLink("template final span"))?;
        let literal = self
            .ast(span)?
            .node(span)?
            .data_source()
            .as_template_span()
            .ok_or(Error::MissingLink("template final span"))?
            .literal()
            .ok_or(Error::MissingLink("template final literal"))?;
        let read = self.node(literal)?;
        Ok(ts_ast::node_is_missing(Some(&read))
            || ts_ast::utilities_middle::is_unterminated_literal(&read))
    }
}
