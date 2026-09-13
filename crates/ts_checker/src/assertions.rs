//! Assertion overlap is deferred on the same source queue as function bodies.
use crate::{CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkAssertion
    pub(crate) fn check_assertion_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("assertion expression"))?;
        let annotation = read
            .type_node()
            .ok_or(Error::MissingLink("assertion type"))?;
        if read.kind() == K::TypeAssertionExpression {
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("assertion source"))?;
            let file = self.ast(source)?.source_file(source)?;
            if file.file_name().ends_with(b".mts") || file.file_name().ends_with(b".cts") {
                self.grammar_error_node(node,d::This_syntax_is_reserved_in_files_with_the_mts_or_cts_extension_Use_an_as_expression_instead,vec![])?;
            }
            if self
                .program()?
                .host
                .options()
                .erasable_syntax_only
                .is_true()
                && self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE == 0
            {
                let read = self.ast(node)?.node(node)?;
                let start = ts_scanner::skip_trivia(
                    self.ast(source)?.source_file(source)?.text().as_bytes(),
                    i64::from(read.pos()),
                );
                let end = i64::from(self.ast(expression)?.node(expression)?.pos());
                self.add_diagnostic(ts_ast::Diagnostic::new(
                    Some(source),
                    ts_core::TextRange::new(start, end),
                    d::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                    vec![],
                ))?;
            }
        }
        let ty = self.check_expression_ex(expression, self.expression_mode)?;
        self.check_source_element(annotation)?;
        if ts_ast::utilities_middle::is_const_type_reference(
            self.ast(annotation)?,
            &self.ast(annotation)?.node(annotation)?,
        )? {
            if !self.valid_const_assertion_argument(expression)? {
                self.error_at(Some(expression),d::A_const_assertion_can_only_be_applied_to_references_to_enum_members_or_string_number_boolean_array_or_object_literals,vec![])?;
            }
            return self.get_regular_type_of_literal_type(ty);
        }
        self.query.assertion_types.insert(node, ty);
        self.defer_checker_node(node)?;
        self.get_type_from_type_node(annotation)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAssertionDeferred
    pub(crate) fn check_assertion_deferred(&mut self, node: NodeId) -> Result<(), Error> {
        let annotation = self
            .ast(node)?
            .node(node)?
            .type_node()
            .ok_or(Error::MissingLink("deferred assertion annotation"))?;
        let source = *self
            .query
            .assertion_types
            .get(&node)
            .ok_or(Error::MissingLink("deferred assertion expression"))?;
        let source = self.base_literal_type(source)?;
        let source = self.regular_object_literal_type(source)?;
        let target = self.get_type_from_type_node(annotation)?;
        if !self.is_error_type(target)? {
            let widened = self.widened_type(source)?;
            if !self.is_type_related_to(target, widened, RelationKind::Comparable)? {
                let node = if self.ast(annotation)?.node(annotation)?.flags() & nf::REPARSED != 0 {
                    annotation
                } else {
                    node
                };
                let (_,diagnostic)=self.check_type_related_ex(source,target,RelationKind::Comparable,Some(node),Some(d::Conversion_of_type_0_to_type_1_may_be_a_mistake_because_neither_type_sufficiently_overlaps_with_the_other_If_this_was_intentional_convert_the_expression_to_unknown_first))?;
                if let Some(diagnostic) = diagnostic {
                    self.add_diagnostic(diagnostic)?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSatisfiesExpression
    pub(crate) fn check_satisfies_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let annotation = read
            .type_node()
            .ok_or(Error::MissingLink("satisfies annotation"))?;
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("satisfies expression"))?;
        self.check_source_element(annotation)?;
        let source = self.check_expression(expression)?;
        let target = self.get_type_from_type_node(annotation)?;
        if self.is_error_type(target)? {
            return Ok(target);
        }
        self.check_expression_related_with_elaboration(
            source,
            target,
            RelationKind::Assignable,
            Some(node),
            Some(expression),
            Some(d::Type_0_does_not_satisfy_the_expected_type_1),
        )?;
        Ok(source)
    }
}
