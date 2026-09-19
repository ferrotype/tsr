use crate::tokens::token_is_identifier_or_keyword;
use crate::{parse_flags, Parser, ParserFactory, ParsingContext};
use ts_ast::{
    node_flags, FactoryMethods, JsString, NodeDataRead, NodeId, NodeListId, SyntaxKind as K,
};
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// Read actual modifiers for source operations that use core.Some, rather
    /// than the separately cached ModifierFlags field.
    pub(crate) fn has_modifier_kind(&self, list: Option<NodeListId>, kind: K) -> bool {
        list.is_some_and(|list| {
            self.factory
                .read_nodes(self.factory.read_list(list).nodes())
                .iter()
                .any(|node| self.factory.node(node.expect("parser modifier")).kind() == kind)
        })
    }
    pub(crate) fn mark_modifiers_ambient(&mut self, list: NodeListId) {
        let nodes = self.factory.read_list(list).nodes();
        for index in 0..nodes.len() {
            let node = self
                .factory
                .read_nodes(nodes)
                .at(index)
                .expect("parser modifier");
            let flags = self.factory.node(node).flags() | node_flags::AMBIENT;
            self.factory.set_node_flags(node, flags);
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassDeclaration
    pub(crate) fn parse_class_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_class_declaration_or_expression(pos, jsdoc, modifiers, K::ClassDeclaration)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassExpression
    pub(crate) fn parse_class_expression(&mut self) -> NodeId {
        self.parse_class_declaration_or_expression(
            self.node_pos(),
            self.jsdoc_scanner_info(),
            None,
            K::ClassExpression,
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassDeclarationOrExpression
    pub(crate) fn parse_class_declaration_or_expression(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        kind: K,
    ) -> NodeId {
        let saved_context = self.context_flags;
        let saved_await = self.statement_has_await_identifier;
        self.parse_expected(K::ClassKeyword);
        let name = self.parse_name_of_class_declaration_or_expression();
        let type_parameters = self.parse_type_parameters();
        if self.parsing_contexts & (1 << ParsingContext::SourceElements as u8) != 0
            && self.parsing_contexts
                & ((1 << ParsingContext::BlockStatements as u8)
                    | (1 << ParsingContext::SwitchClauseStatements as u8))
                == 0
            && self.has_modifier_kind(modifiers, K::ExportKeyword)
        {
            self.set_context_flags(node_flags::AWAIT_CONTEXT, true);
        }
        let heritage = self.parse_heritage_clauses(false);
        let members = if self.parse_expected(K::OpenBraceToken) {
            let members = self.parse_list(ParsingContext::ClassMembers, Self::parse_class_element);
            self.parse_expected(K::CloseBraceToken);
            members
        } else {
            self.create_missing_list()
        };
        self.context_flags = saved_context;
        // The source recomputes flags here rather than reading the list cache.
        if self.has_modifier_kind(modifiers, K::DeclareKeyword) {
            self.statement_has_await_identifier = saved_await;
        }
        let node = if kind == K::ClassDeclaration {
            self.factory.new_class_declaration(
                modifiers,
                name,
                type_parameters,
                heritage,
                Some(members),
            )
        } else {
            self.factory.new_class_expression(
                modifiers,
                name,
                type_parameters,
                heritage,
                Some(members),
            )
        };
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        if self.factory.node(node).flags() & node_flags::JAVA_SCRIPT_FILE != 0 {
            self.check_js_syntax(node);
            if let Some(heritage) = heritage {
                let clauses = self.factory.read_list(heritage).nodes();
                for index in 0..clauses.len() {
                    let clause = self
                        .factory
                        .read_nodes(clauses)
                        .at(index)
                        .expect("heritage clause");
                    let (token, types) = {
                        let clause = self.factory.node(clause);
                        let NodeDataRead::HeritageClause(data) = clause.data() else {
                            unreachable!()
                        };
                        (data.token(), data.types())
                    };
                    if token == K::ExtendsKeyword {
                        if let Some(types) = types {
                            let types = self.factory.read_list(types).nodes();
                            for index in 0..types.len() {
                                let ty = self
                                    .factory
                                    .read_nodes(types)
                                    .at(index)
                                    .expect("heritage expression");
                                self.check_js_syntax(ty);
                            }
                        }
                    }
                }
            }
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseNameOfClassDeclarationOrExpression
    pub(crate) fn parse_name_of_class_declaration_or_expression(&mut self) -> Option<NodeId> {
        if self.is_binding_identifier() && !self.is_implements_clause() {
            let saved = self.statement_has_await_identifier;
            let node = self.create_identifier(self.is_binding_identifier());
            self.statement_has_await_identifier = saved;
            Some(node)
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isImplementsClause
    pub(crate) fn is_implements_clause(&mut self) -> bool {
        self.token == K::ImplementsKeyword
            && self.look_ahead(Self::next_token_is_identifier_or_keyword)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseHeritageClauses
    pub(crate) fn parse_heritage_clauses(&mut self, is_interface: bool) -> Option<NodeListId> {
        self.is_heritage_clause().then(|| {
            self.parse_list(ParsingContext::HeritageClauses, |p| {
                p.parse_heritage_clause(is_interface)
            })
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseHeritageClause
    pub(crate) fn parse_heritage_clause(&mut self, is_interface: bool) -> NodeId {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        let types = self.parse_delimited_list(ParsingContext::HeritageClauseElement, |p| {
            if is_interface && kind == K::ExtendsKeyword
                || !is_interface && kind == K::ImplementsKeyword
            {
                p.parse_type_heritage_clause_element()
            } else {
                p.parse_expression_with_type_arguments()
            }
        });
        let node = self.factory.new_heritage_clause(kind.into(), types);
        self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeHeritageClauseElement
    pub(crate) fn parse_type_heritage_clause_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let node = self.parse_expression_with_type_arguments();
        let (expression, arguments) = {
            let node = self.factory.node(node);
            let NodeDataRead::ExpressionWithTypeArguments(data) = node.data() else {
                unreachable!()
            };
            (
                data.expression().expect("heritage expression"),
                data.type_arguments(),
            )
        };
        if !self.is_valid_heritage_type_reference_expression(expression) {
            return node;
        }
        let name = self.convert_entity_name_expression_to_entity_name(expression);
        let result = self.factory.new_type_reference_node(Some(name), arguments);
        self.finish_node(result, pos)
    }
    /// port: tsc/internal/parser/parser.go:isValidHeritageTypeReferenceExpression
    pub(crate) fn is_valid_heritage_type_reference_expression(&self, mut node: NodeId) -> bool {
        loop {
            let data = self.factory.node(node);
            if data.kind() == K::Identifier {
                return self.node_is_present(Some(node));
            }
            if data.kind() != K::PropertyAccessExpression
                || data.flags() & node_flags::OPTIONAL_CHAIN != 0
            {
                return false;
            }
            let NodeDataRead::PropertyAccessExpression(data) = data.data() else {
                unreachable!()
            };
            if self.node_is_missing(data.name()) {
                return false;
            }
            node = data.expression().expect("property expression");
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.convertEntityNameExpressionToEntityName
    pub(crate) fn convert_entity_name_expression_to_entity_name(
        &mut self,
        mut node: NodeId,
    ) -> NodeId {
        let mut path = Vec::new();
        while self.factory.node(node).kind() != K::Identifier {
            let record = self.factory.node(node);
            let NodeDataRead::PropertyAccessExpression(data) = record.data() else {
                unreachable!()
            };
            path.push((data.name(), record.range()));
            node = data.expression().expect("property expression");
        }
        for (name, range) in path.into_iter().rev() {
            node = self.factory.new_qualified_name(Some(node), name);
            self.finish_node_with_end(node, range.pos(), range.end());
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpressionWithTypeArguments
    pub(crate) fn parse_expression_with_type_arguments(&mut self) -> NodeId {
        let pos = self.node_pos();
        let expression = self.parse_left_hand_side_expression_or_higher();
        if self.factory.node(expression).kind() == K::ExpressionWithTypeArguments {
            return expression;
        }
        let arguments = self.parse_type_arguments();
        let node = self
            .factory
            .new_expression_with_type_arguments(Some(expression), arguments);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassElement
    pub(crate) fn parse_class_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.token == K::SemicolonToken {
            self.next_token();
            let node = self.factory.new_semicolon_class_element();
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            return node;
        }
        let modifiers = self.parse_modifiers_ex(true, true, true);
        if self.token == K::StaticKeyword && self.look_ahead(Self::next_token_is_open_brace) {
            return self.parse_class_static_block_declaration(pos, jsdoc, modifiers);
        }
        if self.parse_contextual_modifier(K::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                K::GetAccessor,
                parse_flags::NONE,
            );
        }
        if self.parse_contextual_modifier(K::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                K::SetAccessor,
                parse_flags::NONE,
            );
        }
        if matches!(self.token, K::ConstructorKeyword | K::StringLiteral) {
            if let Some(node) = self.try_parse_constructor_declaration(pos, jsdoc, modifiers) {
                return node;
            }
        }
        if self.is_index_signature() {
            let node = self.parse_index_signature_declaration(pos, jsdoc, modifiers);
            return self.check_js_syntax(node);
        }
        if token_is_identifier_or_keyword(self.token)
            || matches!(
                self.token,
                K::StringLiteral
                    | K::NumericLiteral
                    | K::BigIntLiteral
                    | K::AsteriskToken
                    | K::OpenBracketToken
            )
        {
            let saved = self.context_flags;
            if self.has_modifier_kind(modifiers, K::DeclareKeyword) {
                self.mark_modifiers_ambient(modifiers.expect("declare modifier"));
                self.set_context_flags(node_flags::AMBIENT, true);
            }
            let node = self.parse_property_or_method_declaration(pos, jsdoc, modifiers);
            self.context_flags = saved;
            return node;
        }
        if modifiers.is_some() {
            self.parse_error_at(
                self.node_pos(),
                self.node_pos(),
                diag::Declaration_expected,
                Vec::new(),
            );
            let name = self.create_missing_identifier();
            return self.parse_property_declaration(pos, jsdoc, modifiers, name, None);
        }
        panic!("Should not have attempted to parse class member declaration.")
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassStaticBlockDeclaration
    pub(crate) fn parse_class_static_block_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        self.parse_expected_token(K::StaticKeyword);
        let body = self.parse_class_static_block_body();
        let node = self
            .factory
            .new_class_static_block_declaration(modifiers, Some(body));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseClassStaticBlockBody
    pub(crate) fn parse_class_static_block_body(&mut self) -> NodeId {
        let saved = self.context_flags;
        self.set_context_flags(node_flags::YIELD_CONTEXT, false);
        self.set_context_flags(node_flags::AWAIT_CONTEXT, true);
        let body = self.parse_block(false, None);
        self.context_flags = saved;
        body
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseConstructorDeclaration
    pub(crate) fn try_parse_constructor_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> Option<NodeId> {
        let state = self.mark();
        if self.token == K::ConstructorKeyword
            || self.token == K::StringLiteral
                && self.scanner.token_value() == b"constructor"
                && self.look_ahead(Self::next_token_is_open_paren)
        {
            self.next_token();
            let type_parameters = self.parse_type_parameters();
            let parameters = self.parse_parameters(parse_flags::NONE);
            let ty = self.parse_return_type(K::ColonToken, false);
            let body = self
                .parse_function_block_or_semicolon(parse_flags::NONE, Some(diag::X_or_expected));
            let node = self.factory.new_constructor_declaration(
                modifiers,
                type_parameters,
                parameters,
                ty,
                None,
                body,
            );
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            self.check_js_syntax(node);
            self.commit(state);
            Some(node)
        } else {
            self.rewind(state);
            None
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsOpenParen
    pub(crate) fn next_token_is_open_paren(&mut self) -> bool {
        self.next_token() == K::OpenParenToken
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyOrMethodDeclaration
    pub(crate) fn parse_property_or_method_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let asterisk = self.parse_optional_token(K::AsteriskToken);
        let name = self.parse_property_name();
        let question = self.parse_optional_token(K::QuestionToken);
        if asterisk.is_some() || matches!(self.token, K::OpenParenToken | K::LessThanToken) {
            self.parse_method_declaration(
                pos,
                jsdoc,
                modifiers,
                asterisk,
                name,
                question,
                Some(diag::X_or_expected),
            )
        } else {
            self.parse_property_declaration(pos, jsdoc, modifiers, name, question)
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseMethodDeclaration
    #[allow(clippy::too_many_arguments)] // The pinned grammar production's inputs.
    pub(crate) fn parse_method_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        asterisk: Option<NodeId>,
        name: NodeId,
        question: Option<NodeId>,
        message: Option<&'static diag::Message>,
    ) -> NodeId {
        let flags = if asterisk.is_some() {
            parse_flags::YIELD
        } else {
            0
        } | if self.modifier_list_has_async(modifiers) {
            parse_flags::AWAIT
        } else {
            0
        };
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(flags);
        let ty = self.parse_return_type(K::ColonToken, false);
        let body = self.parse_function_block_or_semicolon(flags, message);
        let node = self.factory.new_method_declaration(
            modifiers,
            asterisk,
            Some(name),
            question,
            type_parameters,
            parameters,
            ty,
            None,
            body,
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePropertyDeclaration
    pub(crate) fn parse_property_declaration(
        &mut self,
        pos: i64,
        jsdoc: u8,
        modifiers: Option<NodeListId>,
        name: NodeId,
        question: Option<NodeId>,
    ) -> NodeId {
        let mut postfix = question;
        if postfix.is_none() && !self.has_preceding_line_break() {
            postfix = self.parse_optional_token(K::ExclamationToken);
        }
        let ty = self.parse_type_annotation();
        let initializer = self.do_in_context(
            node_flags::YIELD_CONTEXT | node_flags::AWAIT_CONTEXT | node_flags::DISALLOW_IN_CONTEXT,
            false,
            Self::parse_initializer,
        );
        self.parse_semicolon_after_property_name(name, ty, initializer);
        let node =
            self.factory
                .new_property_declaration(modifiers, Some(name), postfix, ty, initializer);
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSemicolonAfterPropertyName
    pub(crate) fn parse_semicolon_after_property_name(
        &mut self,
        name: NodeId,
        ty: Option<NodeId>,
        initializer: Option<NodeId>,
    ) {
        if self.token == K::AtToken && !self.has_preceding_line_break() {
            self.parse_error_at_current_token(
                diag::Decorators_must_precede_the_name_and_all_keywords_of_property_declarations,
                Vec::new(),
            );
            return;
        }
        if self.token == K::OpenParenToken {
            self.parse_error_at_current_token(
                diag::Cannot_start_a_function_call_in_a_type_annotation,
                Vec::new(),
            );
            self.next_token();
            return;
        }
        if ty.is_some() && !self.can_parse_semicolon() {
            if initializer.is_some() {
                self.parse_error_at_current_token(
                    diag::X_0_expected,
                    vec![JsString::from_bytes(&b";"[..])],
                );
            } else {
                self.parse_error_at_current_token(
                    diag::Expected_for_property_initializer,
                    Vec::new(),
                );
            }
            return;
        }
        if self.try_parse_semicolon() {
            return;
        }
        if initializer.is_some() {
            self.parse_error_at_current_token(
                diag::X_0_expected,
                vec![JsString::from_bytes(&b";"[..])],
            );
            return;
        }
        self.parse_error_for_missing_semicolon_after(name);
    }
}
