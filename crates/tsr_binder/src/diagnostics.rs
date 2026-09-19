use crate::{
    ast as a, checked, need,
    target::{target_payload, BindingNode},
    Binder,
};
use ts_ast::{node_flags as nf, AstView, Diagnostic, JsString, NodeId, SyntaxKind as K};
use ts_diagnostics::{self as d, Message};

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.errorOnNode
    pub fn error_on_node(&mut self, node: NodeId, message: &'static Message, args: Vec<JsString>) {
        let diagnostic = self.create_diagnostic_for_node(node, message, args);
        self.add_diagnostic(diagnostic);
    }
    // port: tsc/internal/binder/binder.go:Binder.errorOnFirstToken
    pub fn error_on_first_token(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: Vec<JsString>,
    ) {
        let range = checked(ts_scanner::get_range_of_token_at_position(
            self.view(),
            self.file,
            i64::from(self.n(node).pos()),
        ));
        self.add_diagnostic(Diagnostic::new(Some(self.file), range, message, args));
    }
    // port: tsc/internal/binder/binder.go:Binder.createDiagnosticForNode
    pub fn create_diagnostic_for_node(
        &self,
        node: NodeId,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Diagnostic {
        Diagnostic::new(
            Some(self.file),
            checked(ts_scanner::get_error_range_for_node(
                self.view(),
                self.file,
                node,
            )),
            message,
            args,
        )
    }
    // port: tsc/internal/binder/binder.go:Binder.addDiagnostic
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.builder.diagnostics_mut().push(diagnostic);
    }
    // port: tsc/internal/binder/binder.go:Binder.checkContextualIdentifier
    pub fn check_contextual_identifier(&mut self, node: NodeId) {
        if self.source_has_parse_errors {
            return;
        }
        let flags = if let crate::backend::Backend::Local(local) = &self.builder {
            if let Ok(id) = local.import_node(node) {
                if local.node(id).as_identifier().is_some() {
                    self.check_local_contextual_identifier(id);
                    return;
                }
            }
            self.n(node).flags()
        } else {
            self.n(node).flags()
        };
        if flags & (nf::AMBIENT | nf::JS_DOC) == 0
            && !checked(a::is_identifier_name(self.view(), node))
        {
            let text = self.view().node_text(node).expect("text payload required");
            let keyword = ts_scanner::get_identifier_token(text.as_bytes());
            self.check_contextual_keyword(node, keyword, flags);
        }
    }
    pub(crate) fn check_local_contextual_identifier(
        &mut self,
        node: ts_ast::local_bind::BindNode<'scope>,
    ) {
        if self.source_has_parse_errors {
            return;
        }
        let crate::backend::Backend::Local(local) = &self.builder else {
            unreachable!("local binder scope");
        };
        let read = local.node(node);
        let flags = read.flags();
        if flags & (nf::AMBIENT | nf::JS_DOC) != 0 || local.is_identifier_name(node) {
            return;
        }
        // Preserve parent/name classification before reading identifier text.
        let keyword = ts_scanner::get_identifier_token(
            read.as_identifier().expect("Identifier payload").text(),
        );
        if keyword == K::Identifier {
            return;
        }
        let id = local.node_id(node);
        self.check_contextual_keyword(id, keyword, flags);
    }
    fn check_contextual_keyword(&mut self, node: NodeId, keyword: K, flags: u32) {
        if keyword == K::Identifier {
            return;
        }
        let message =
            if keyword >= K::FirstFutureReservedWord && keyword <= K::LastFutureReservedWord {
                Some(self.get_strict_mode_identifier_message(node))
            } else if keyword == K::AwaitKeyword {
                if self.source_is_external_module
                    && checked(a::is_in_top_level_context(self.view(), node))
                {
                    Some(d::Identifier_expected_0_is_a_reserved_word_at_the_top_level_of_a_module)
                } else if flags & nf::AWAIT_CONTEXT != 0 {
                    Some(d::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here)
                } else {
                    None
                }
            } else if keyword == K::YieldKeyword && flags & nf::YIELD_CONTEXT != 0 {
                Some(d::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here)
            } else {
                None
            };
        if let Some(message) = message {
            self.error_on_node(
                node,
                message,
                vec![checked(ts_scanner::declaration_name_to_string(
                    self.view(),
                    Some(node),
                ))],
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkPrivateIdentifier
    pub fn check_private_identifier(&mut self, node: NodeId) {
        if self.text(node).as_bytes() == b"#constructor" && !self.source_has_parse_errors {
            self.error_on_node(
                node,
                d::X_constructor_is_a_reserved_word,
                vec![checked(ts_scanner::declaration_name_to_string(
                    self.view(),
                    Some(node),
                ))],
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.getStrictModeIdentifierMessage
    pub fn get_strict_mode_identifier_message(&self, node: NodeId) -> &'static Message {
        if checked(a::get_containing_class(self.view(), node)).is_some() {
            d::Identifier_expected_0_is_a_reserved_word_in_strict_mode_Class_definitions_are_automatically_in_strict_mode
        } else if self.source_is_external_module {
            d::Identifier_expected_0_is_a_reserved_word_in_strict_mode_Modules_are_automatically_in_strict_mode
        } else {
            d::Identifier_expected_0_is_a_reserved_word_in_strict_mode
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeFunctionName
    pub fn check_strict_mode_function_name(&mut self, node: BindingNode<'scope>) {
        if self.node_flags(node) & nf::AMBIENT == 0 {
            self.check_strict_mode_eval_or_arguments(node, self.node_name(node));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeBinaryExpression
    pub fn check_strict_mode_binary_expression(&mut self, node: BindingNode<'scope>) {
        let (left, operator) = target_payload!(self, node, as_binary_expression, "binary payload"; node: left, node: operator_token);
        let left_hand_side = match need(left) {
            BindingNode::Local(left) => {
                self.target_is_left_hand_side_expression(BindingNode::Local(left))
            }
            BindingNode::Checked(left) => {
                checked(a::is_left_hand_side_expression(self.view(), left))
            }
        };
        if left_hand_side && a::is_assignment_operator(self.node_kind(need(operator))) {
            self.check_strict_mode_eval_or_arguments(node, left);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeCatchClause
    pub fn check_strict_mode_catch_clause(&mut self, node: BindingNode<'scope>) {
        let (declaration,) = target_payload!(self, node, as_catch_clause, "catch payload"; node: variable_declaration);
        if let Some(declaration) = declaration {
            self.check_strict_mode_eval_or_arguments(node, self.node_name(declaration));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeDeleteExpression
    pub fn check_strict_mode_delete_expression(&mut self, node: BindingNode<'scope>) {
        let expression = need(self.node_expression(node));
        if self.node_kind(expression) == K::Identifier {
            self.error_on_node(
                self.node_id(expression),
                d::X_delete_cannot_be_called_on_an_identifier_in_strict_mode,
                Vec::new(),
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModePostfixUnaryExpression
    pub fn check_strict_mode_postfix_unary_expression(&mut self, node: BindingNode<'scope>) {
        let (operand,) = target_payload!(self, node, as_postfix_unary_expression, "postfix payload"; node: operand);
        self.check_strict_mode_eval_or_arguments(node, operand);
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModePrefixUnaryExpression
    pub fn check_strict_mode_prefix_unary_expression(&mut self, node: BindingNode<'scope>) {
        let (operator, operand) = target_payload!(self, node, as_prefix_unary_expression, "prefix payload"; scalar: operator, node: operand);
        if matches!(
            operator.known(),
            Some(K::PlusPlusToken | K::MinusMinusToken)
        ) {
            self.check_strict_mode_eval_or_arguments(node, operand);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeWithStatement
    pub fn check_strict_mode_with_statement(&mut self, node: BindingNode<'scope>) {
        self.error_on_first_token(
            self.node_id(node),
            d::X_with_statements_are_not_allowed_in_strict_mode,
            Vec::new(),
        );
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeLabeledStatement
    pub fn check_strict_mode_labeled_statement(&mut self, node: BindingNode<'scope>) {
        let (label, statement) = target_payload!(self, node, as_labeled_statement, "label payload"; node: label, node: statement);
        let kind = self.node_kind(need(statement));
        if a::is_declaration_statement_kind(kind) || kind == K::VariableStatement {
            self.error_on_first_token(
                self.node_id(need(label)),
                d::A_label_is_not_allowed_here,
                Vec::new(),
            );
        }
    }
    fn target_is_eval_or_arguments_identifier(&self, node: BindingNode<'scope>) -> bool {
        if self.node_kind(node) != K::Identifier {
            return false;
        }
        if let BindingNode::Local(node) = node {
            let crate::backend::Backend::Local(local) = &self.builder else {
                unreachable!("local binder scope")
            };
            let read = local.node(node);
            if let Some(identifier) = read.as_identifier() {
                return is_eval_or_arguments_text(identifier.text());
            }
        }
        is_eval_or_arguments_identifier(self.view(), self.node_id(node))
    }
    // port: tsc/internal/binder/binder.go:Binder.checkStrictModeEvalOrArguments
    pub fn check_strict_mode_eval_or_arguments(
        &mut self,
        context: BindingNode<'scope>,
        name: Option<BindingNode<'scope>>,
    ) {
        if let Some(name) = name {
            if self.target_is_eval_or_arguments_identifier(name) {
                self.error_on_node(
                    self.node_id(name),
                    self.get_strict_mode_eval_or_arguments_message(self.node_id(context)),
                    vec![self.target_text(name)],
                );
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.getStrictModeEvalOrArgumentsMessage
    pub fn get_strict_mode_eval_or_arguments_message(&self, node: NodeId) -> &'static Message {
        if checked(a::get_containing_class(self.view(), node)).is_some() {
            d::Code_contained_in_a_class_is_evaluated_in_JavaScript_s_strict_mode_which_does_not_allow_this_use_of_0_For_more_information_see_https_Colon_Slash_Slashdeveloper_mozilla_org_Slashen_US_Slashdocs_SlashWeb_SlashJavaScript_SlashReference_SlashStrict_mode
        } else if self.source_is_external_module {
            d::Invalid_use_of_0_Modules_are_automatically_in_strict_mode
        } else {
            d::Invalid_use_of_0_in_strict_mode
        }
    }
}
// port: tsc/internal/binder/binder.go:isEvalOrArgumentsIdentifier
pub fn is_eval_or_arguments_identifier(view: AstView<'_>, node: NodeId) -> bool {
    checked(view.node(node)).kind() == K::Identifier
        && is_eval_or_arguments_text(checked(view.node_text(node)).as_bytes())
}
fn is_eval_or_arguments_text(bytes: &[u8]) -> bool {
    matches!(bytes, b"eval" | b"arguments")
}
// port: tsc/internal/binder/binder.go:isUseStrictPrologueDirective
pub fn is_use_strict_prologue_directive(view: AstView<'_>, source: NodeId, node: NodeId) -> bool {
    matches!(
        checked(ts_scanner::get_source_text_of_node_from_source_file(
            view,
            source,
            checked(view.node(node)).expression(),
            false
        ))
        .as_bytes(),
        b"\"use strict\"" | b"'use strict'"
    )
}
// port: tsc/internal/binder/binder.go:FindUseStrictPrologue
pub fn find_use_strict_prologue(
    view: AstView<'_>,
    source: NodeId,
    statements: &[Option<NodeId>],
) -> Option<NodeId> {
    for &statement in statements {
        let statement = need(statement);
        if !checked(a::is_prologue_directive(view, statement)) {
            return None;
        }
        if is_use_strict_prologue_directive(view, source, statement) {
            return Some(statement);
        }
    }
    None
}
