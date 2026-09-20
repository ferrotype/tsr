//! Statements, declarations, class members, JSX and the expressions type
//! display never produces. The same conventions as the parent module apply:
//! every emit function is the upstream function of the same name and keeps its
//! order of writes. Name generation only matters for auto-generated
//! identifiers, which this port refuses where it meets them, so the name
//! generation scopes upstream opens around bodies and members are not kept.

use super::{greatest_end, Session, Span, WriteKind};
use crate::{list_format as lf, Error, ListFormat, TypePrecedence};
use tsr_ast::{operator_precedence as op, NodeId, NodeListId, SyntaxKind as K};

impl Session<'_, '_> {
    fn end_of(&self, node: Option<NodeId>) -> Result<Option<i64>, Error> {
        match node {
            Some(node) => Ok(Some(i64::from(self.node(node)?.end()))),
            None => Ok(None),
        }
    }

    fn list_end(&self, list: Option<NodeListId>, what: &'static str) -> Result<i64, Error> {
        let list = list.ok_or(Error::MissingNode(what))?;
        Ok(self.view.list(list)?.loc().end())
    }

    //
    // Names
    //

    // port: tsc/internal/printer/printer.go:Printer.emitLabelIdentifier
    fn emit_label_identifier(&mut self, node: NodeId) -> Result<(), Error> {
        // Upstream converts the label to an identifier without a check. A
        // decoded tree can carry another node there.
        let kind = self.node(node)?.kind();
        if kind != K::Identifier {
            return Err(Error::InterfaceConversion {
                found: kind,
                expected: "Identifier",
            });
        }
        self.enter_node(node);
        self.emit_identifier_text(node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitModuleName
    fn emit_module_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_binding_identifier(node),
            K::StringLiteral => self.emit_string_literal(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected ModuleName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitModuleExportName
    // port: tsc/internal/printer/printer.go:Printer.emitNestedModuleName
    fn emit_module_export_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::StringLiteral => self.emit_string_literal(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected ModuleExportName",
                kind: kind.into(),
            }),
        }
    }

    //
    // Signature elements
    //

    // port: tsc/internal/printer/printer.go:Printer.emitDecorator
    pub(super) fn emit_decorator(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let expression = self
            .node(node)?
            .expression()
            .ok_or(Error::MissingNode("decorator expression"))?;
        self.write_punctuation(b"@");
        self.emit_expression(expression, op::LEFT_HAND_SIDE)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitModifierLike
    pub(super) fn emit_modifier_like(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        if read.kind() == K::Decorator {
            self.emit_decorator(node)
        } else if tsr_ast::utilities::is_modifier(&read) {
            self.emit_keyword_node(Some(node))
        } else {
            Err(Error::UnexpectedKind {
                context: "unhandled ModifierLike",
                kind: read.kind(),
            })
        }
    }

    /// Only an arrow function with a single plain identifier parameter, and
    /// nothing parsed between its start and that parameter, has a simple head.
    // port: tsc/internal/printer/printer.go:canEmitSimpleArrowHead
    fn can_emit_simple_arrow_head(
        &self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<bool, Error> {
        let parent_read = self.node(parent)?;
        let Some(list) = parameters else {
            return Ok(false);
        };
        let nodes = self.list_nodes(list)?;
        if parent_read.kind() != K::ArrowFunction || nodes.len() != 1 {
            return Ok(false);
        }
        let parameter_read = self.node(nodes[0])?;
        let parameter = parameter_read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingNode("parameter payload"))?;
        let name_is_identifier = match parameter.name() {
            Some(name) => self.node(name)?.kind() == K::Identifier,
            None => false,
        };
        Ok(parameter_read.pos() == parent_read.pos()
            && parent_read.type_parameter_list().is_none()
            && parent_read.type_node().is_none()
            && self.list_len(parent_read.modifiers())? == 0
            && !self.view.list_has_trailing_comma(list)?
            && parameter.modifiers().is_none()
            && parameter.dot_dot_dot_token().is_none()
            && parameter.question_token().is_none()
            && parameter.r#type().is_none()
            && parameter.initializer().is_none()
            && name_is_identifier)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParametersForArrow
    fn emit_parameters_for_arrow(
        &mut self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<(), Error> {
        if self.can_emit_simple_arrow_head(parent, parameters)? {
            self.emit_list(
                Self::emit_parameter_declaration_node,
                parent,
                parameters,
                lf::SINGLE_ARROW_PARAMETER,
            )
        } else {
            self.emit_parameters(parent, parameters)
        }
    }

    /// A body is on one line when it is flagged so. It is on several when it
    /// is marked multi-line, when a parsed body spans lines, or when any of its
    /// statements starts on a new line.
    // port: tsc/internal/printer/printer.go:Printer.shouldEmitBlockFunctionBodyOnSingleLine
    fn should_emit_block_function_body_on_single_line(&self, body: NodeId) -> Result<bool, Error> {
        if self.should_emit_on_single_line(body) {
            return Ok(true);
        }
        let read = self.node(body)?;
        let block = read
            .data_source()
            .as_block()
            .ok_or(Error::MissingNode("block payload"))?;
        if block.multi_line() {
            return Ok(false);
        }
        if !tsr_ast::utilities::node_is_synthesized(&read)
            && self.current_source.is_some()
            && !self.range_is_on_single_line(Span::of(&read))
        {
            return Ok(false);
        }
        let statements = match block.statements() {
            Some(list) => self.list_nodes(list)?,
            None => Vec::new(),
        };
        if self.get_leading_line_terminator_count(
            Some(body),
            statements.first().copied(),
            lf::PRESERVE_LINES,
        )? > 0
            || self.get_closing_line_terminator_count(
                Some(body),
                statements.last().copied(),
                lf::PRESERVE_LINES,
            )? > 0
        {
            return Ok(false);
        }
        let mut previous = None;
        for &statement in &statements {
            if self.get_separating_line_terminator_count(
                previous,
                Some(statement),
                lf::PRESERVE_LINES,
            )? > 0
            {
                return Ok(false);
            }
            previous = Some(statement);
        }
        Ok(true)
    }

    /// Returns the index of the first statement that is not a directive.
    // port: tsc/internal/printer/printer.go:Printer.emitPrologueDirectives
    fn emit_prologue_directives(&mut self, statements: Option<NodeListId>) -> Result<i64, Error> {
        let nodes = match statements {
            Some(list) => self.list_nodes(list)?,
            None => Vec::new(),
        };
        for (index, &statement) in nodes.iter().enumerate() {
            if !tsr_ast::utilities::is_prologue_directive(self.view, statement)? {
                return Ok(index as i64);
            }
            self.write_line();
            self.emit_statement(statement)?;
        }
        Ok(nodes.len() as i64)
    }

    /// The body block gets the emit notifications only, not the comment
    /// pipeline. Detached comments and emit helpers are not emitted.
    // port: tsc/internal/printer/printer.go:Printer.emitFunctionBody
    fn emit_function_body(&mut self, body: NodeId) -> Result<(), Error> {
        self.writer.on_before_emit_node(body);
        let read = self.node(body)?;
        let Some(block) = read.data_source().as_block() else {
            // Upstream converts the body to a block without a check. A decoded
            // tree can carry another node there.
            return Err(Error::InterfaceConversion {
                found: read.kind(),
                expected: "Block",
            });
        };
        let statements = block.statements();
        self.write_punctuation(b"{");
        self.increase_indent();
        let statement_offset = self.emit_prologue_directives(statements)?;
        let pos = self.writer.get_text_pos();
        if self.should_emit_block_function_body_on_single_line(body)?
            && statement_offset == 0
            && pos == self.writer.get_text_pos()
        {
            self.decrease_indent();
            self.emit_list_range(
                Self::emit_statement,
                body,
                statements,
                lf::SINGLE_LINE_FUNCTION_BODY_STATEMENTS,
                statement_offset,
                -1,
            )?;
            self.increase_indent();
        } else {
            self.emit_list_range(
                Self::emit_statement,
                body,
                statements,
                lf::MULTI_LINE_FUNCTION_BODY_STATEMENTS,
                statement_offset,
                -1,
            )?;
        }
        self.decrease_indent();
        let end = self.list_end(statements, "function body statements")?;
        self.emit_token(K::CloseBraceToken, end, WriteKind::Punctuation, body);
        self.writer.on_after_emit_node(body);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitFunctionBodyNode
    pub(super) fn emit_function_body_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            self.write_trailing_semicolon();
            return Ok(());
        };
        self.write_space();
        self.emit_function_body(node)
    }

    /// The part every function-like declaration shares after its name: the
    /// optional indentation, the signature and the body.
    fn emit_signature_and_body(&mut self, node: NodeId) -> Result<(), Error> {
        let body = self.node(node)?.body();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_signature(node)?;
        self.emit_function_body_node(body)?;
        self.decrease_indent_if(indented);
        Ok(())
    }

    //
    // Class members
    //

    // port: tsc/internal/printer/printer.go:Printer.emitPropertyDeclaration
    fn emit_property_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (modifiers, name, postfix, type_node, initializer) = (
            read.modifiers(),
            read.name(),
            read.postfix_token(),
            read.type_node(),
            read.initializer(),
        );
        self.emit_modifier_list(node, modifiers, true)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        self.emit_type_annotation(type_node)?;
        let name_end = self
            .end_of(name)?
            .ok_or(Error::MissingNode("property name"))?;
        let equals_pos = greatest_end(name_end, &[self.end_of(type_node)?, self.end_of(postfix)?]);
        self.emit_initializer(initializer, equals_pos, node)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMethodDeclaration
    fn emit_method_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let asterisk = read
            .data_source()
            .as_method_declaration()
            .ok_or(Error::MissingNode("method payload"))?
            .asterisk_token();
        let (modifiers, name, postfix) = (read.modifiers(), read.name(), read.postfix_token());
        self.emit_modifier_list(node, modifiers, true)?;
        self.emit_token_node(asterisk)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        self.emit_signature_and_body(node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitClassStaticBlockDeclaration
    fn emit_class_static_block_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let body = self.node(node)?.body();
        self.write_keyword(b"static");
        self.emit_function_body_node(body)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConstructor
    fn emit_constructor(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let modifiers = self.node(node)?.modifiers();
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"constructor");
        self.emit_signature_and_body(node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSemicolonClassElement
    fn emit_semicolon_class_element(&mut self, node: NodeId) {
        self.enter_node(node);
        self.write_trailing_semicolon();
        self.exit_node(node);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNotEmittedStatement
    // port: tsc/internal/printer/printer.go:Printer.emitNotEmittedTypeElement
    // port: tsc/internal/printer/printer.go:Printer.emitOmittedExpression
    pub(super) fn emit_nothing(&mut self, node: NodeId) {
        self.enter_node(node);
        self.exit_node(node);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitClassElement
    pub(super) fn emit_class_element(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::PropertyDeclaration => self.emit_property_declaration(node),
            K::MethodDeclaration => self.emit_method_declaration(node),
            K::ClassStaticBlockDeclaration => self.emit_class_static_block_declaration(node),
            K::Constructor => self.emit_constructor(node),
            K::GetAccessor | K::SetAccessor => self.emit_accessor_declaration(node),
            K::IndexSignature => self.emit_index_signature(node),
            K::SemicolonClassElement => {
                self.emit_semicolon_class_element(node);
                Ok(())
            }
            K::NotEmittedStatement => {
                self.emit_nothing(node);
                Ok(())
            }
            K::JSTypeAliasDeclaration => self.emit_type_alias_declaration(node),
            kind => Err(Error::UnexpectedKind {
                context: "unexpected ClassElement",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitObjectLiteralElement
    fn emit_object_literal_element(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::PropertyAssignment => self.emit_property_assignment(node),
            K::ShorthandPropertyAssignment => self.emit_shorthand_property_assignment(node),
            K::SpreadAssignment => self.emit_spread_assignment(node),
            K::MethodDeclaration => self.emit_method_declaration(node),
            K::GetAccessor | K::SetAccessor => self.emit_accessor_declaration(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled ObjectLiteralElement",
                kind: kind.into(),
            }),
        }
    }

    //
    // Expressions
    //

    // port: tsc/internal/printer/printer.go:Printer.shouldAllowTrailingComma
    fn should_allow_trailing_comma(
        &self,
        node: NodeId,
        list: Option<NodeListId>,
    ) -> Result<bool, Error> {
        let Some((_, source)) = &self.current_source else {
            return Ok(false);
        };
        if source.script_kind == tsr_core::ScriptKind::JSON {
            return Ok(false);
        }
        let read = self.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::ObjectLiteralExpression
                | K::ArrayLiteralExpression
                | K::ArrowFunction
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::FunctionType
                | K::ConstructorType
                | K::CallSignature
                | K::ConstructSignature
                | K::TaggedTemplateExpression
                | K::ObjectBindingPattern
                | K::ArrayBindingPattern
                | K::NamedImports
                | K::NamedExports
                | K::ImportAttributes
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::MethodDeclaration
                | K::CallExpression
                | K::NewExpression,
            ) => true,
            Some(K::ClassExpression | K::ClassDeclaration | K::InterfaceDeclaration) => {
                list == read.type_parameter_list()
            }
            _ => false,
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArrayLiteralExpressionElement
    fn emit_array_literal_expression_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_expression(node, op::SPREAD)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArrayLiteralExpression
    pub(super) fn emit_array_literal_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let multi_line = read
            .data_source()
            .as_array_literal_expression()
            .ok_or(Error::MissingNode("array literal payload"))?
            .multi_line();
        let prefer = if multi_line {
            lf::PREFER_NEW_LINE
        } else {
            lf::NONE
        };
        self.emit_list(
            Self::emit_array_literal_expression_element,
            node,
            read.element_list(),
            lf::ARRAY_LITERAL_EXPRESSION_ELEMENTS | prefer,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitObjectLiteralExpression
    pub(super) fn emit_object_literal_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let multi_line = read
            .data_source()
            .as_object_literal_expression()
            .ok_or(Error::MissingNode("object literal payload"))?
            .multi_line();
        let properties = read.property_list();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        let mut format = lf::OBJECT_LITERAL_EXPRESSION_PROPERTIES;
        if multi_line {
            format |= lf::PREFER_NEW_LINE;
        }
        if self.should_allow_trailing_comma(node, properties)? {
            format |= lf::ALLOW_TRAILING_COMMA;
        }
        self.emit_list(Self::emit_object_literal_element, node, properties, format)?;
        self.decrease_indent_if(indented);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNewExpression
    pub(super) fn emit_new_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("new expression callee"))?;
        self.emit_token(
            K::NewKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        let skipped = tsr_ast::skip_partially_emitted_expressions(self.view, expression)?;
        // `C()` under `new` is parenthesized, so it reads `new (C())` and not
        // `new C()`.
        let precedence = if self.node(skipped)?.kind() == K::CallExpression {
            op::PARENTHESES
        } else {
            op::MEMBER
        };
        self.emit_expression(expression, precedence)?;
        self.emit_type_arguments(node, read.type_argument_list())?;
        self.emit_list(
            Self::emit_argument,
            node,
            read.argument_list(),
            lf::NEW_EXPRESSION_ARGUMENTS,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateLiteral
    fn emit_template_literal(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::NoSubstitutionTemplateLiteral => self.emit_no_substitution_template_literal(node),
            K::TemplateExpression => self.emit_template_expression(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled TemplateLiteral",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTaggedTemplateExpression
    pub(super) fn emit_tagged_template_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let tagged = read
            .data_source()
            .as_tagged_template_expression()
            .ok_or(Error::MissingNode("tagged template payload"))?;
        let (tag, type_arguments, template) = (
            tagged.tag().ok_or(Error::MissingNode("template tag"))?,
            tagged.type_arguments(),
            tagged.template().ok_or(Error::MissingNode("template"))?,
        );
        self.emit_callee(tag, node)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.write_space();
        self.emit_template_literal(template)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeAssertionExpression
    pub(super) fn emit_type_assertion_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (type_node, expression) = (
            read.type_node()
                .ok_or(Error::MissingNode("assertion type"))?,
            read.expression()
                .ok_or(Error::MissingNode("assertion expression"))?,
        );
        self.write_punctuation(b"<");
        self.emit_type_node_outside_extends(type_node)?;
        self.write_punctuation(b">");
        self.emit_expression(expression, op::UPDATE)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitFunctionExpression
    pub(super) fn emit_function_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let asterisk = read
            .data_source()
            .as_function_expression()
            .ok_or(Error::MissingNode("function expression payload"))?
            .asterisk_token();
        let (modifiers, name) = (read.modifiers(), read.name());
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"function");
        self.emit_token_node(asterisk)?;
        self.write_space();
        // port: tsc/internal/printer/printer.go:Printer.emitIdentifierNameNode
        if let Some(name) = name {
            self.emit_identifier_name(name)?;
        }
        self.emit_signature_and_body(node)?;
        self.exit_node(node);
        Ok(())
    }

    /// Upstream wraps an object-literal body in a parenthesized expression it
    /// creates on the spot, with the body's range. That node is never part of
    /// the tree, so only what printing it writes is reproduced.
    // port: tsc/internal/printer/printer.go:Printer.emitConciseBody
    fn emit_concise_body(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        if read.kind() == K::Block {
            return self.emit_function_body(node);
        }
        let leftmost = tsr_ast::get_leftmost_expression(self.view, node, false)?;
        if self.node(leftmost)?.kind() == K::ObjectLiteralExpression {
            self.write_punctuation(b"(");
            let leading = if self.printer.options.preserve_source_newlines {
                self.get_leading_line_terminator_count(None, Some(node), lf::NONE)?
            } else {
                0
            };
            self.write_lines_and_indent(leading, false);
            self.emit_expression(node, op::COMMA)?;
            if self.printer.options.preserve_source_newlines {
                let trailing =
                    self.get_closing_line_terminator_count(None, Some(node), lf::NONE)?;
                self.write_line_repeat(trailing);
            }
            self.decrease_indent_if(leading > 0);
            self.write_punctuation(b")");
            return Ok(());
        }
        if tsr_ast::utilities::is_expression_kind(read.kind()) {
            return self.emit_expression(node, op::YIELD);
        }
        Err(Error::UnexpectedKind {
            context: "unexpected ConciseBody",
            kind: read.kind(),
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArrowFunction
    pub(super) fn emit_arrow_function(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let arrow = read
            .data_source()
            .as_arrow_function()
            .ok_or(Error::MissingNode("arrow function payload"))?
            .equals_greater_than_token();
        let (modifiers, type_parameters, parameters, type_node, body) = (
            read.modifiers(),
            read.type_parameter_list(),
            read.parameter_list(),
            read.type_node(),
            read.body()
                .ok_or(Error::MissingNode("arrow function body"))?,
        );
        self.emit_modifier_list(node, modifiers, false)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters_for_arrow(node, parameters)?;
        self.emit_type_annotation(type_node)?;
        self.write_space();
        self.emit_token_node(arrow)?;
        self.write_space();
        self.emit_concise_body(body)?;
        self.decrease_indent_if(indented);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDeleteExpression
    // port: tsc/internal/printer/printer.go:Printer.emitTypeOfExpression
    // port: tsc/internal/printer/printer.go:Printer.emitVoidExpression
    // port: tsc/internal/printer/printer.go:Printer.emitAwaitExpression
    pub(super) fn emit_keyword_unary_expression(
        &mut self,
        node: NodeId,
        keyword: K,
    ) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("unary operand"))?;
        self.emit_token(keyword, i64::from(read.pos()), WriteKind::Keyword, node);
        self.write_space();
        self.emit_expression(expression, op::UNARY)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPostfixUnaryExpression
    pub(super) fn emit_postfix_unary_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let postfix = read
            .data_source()
            .as_postfix_unary_expression()
            .ok_or(Error::MissingNode("postfix payload"))?;
        let operand = postfix
            .operand()
            .ok_or(Error::MissingNode("postfix operand"))?;
        let operator = postfix.operator().known().ok_or(Error::UnexpectedKind {
            context: "postfix operator",
            kind: postfix.operator(),
        })?;
        self.emit_expression(operand, op::LEFT_HAND_SIDE)?;
        let end = i64::from(self.node(operand)?.end());
        self.emit_token(operator, end, WriteKind::Operator, node);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitShortCircuitExpression
    fn emit_short_circuit_expression(&mut self, node: NodeId) -> Result<(), Error> {
        // port: tsc/internal/printer/utilities.go:isBinaryOperation
        let skipped = tsr_ast::skip_partially_emitted_expressions(self.view, node)?;
        let is_coalesce = if self.node(skipped)?.kind() == K::BinaryExpression {
            let (_, operator, _) = self.binary_parts(skipped)?;
            self.node(operator)?.kind() == K::QuestionQuestionToken
        } else {
            false
        };
        self.emit_expression(
            node,
            if is_coalesce {
                op::COALESCE
            } else {
                op::LOGICAL_OR
            },
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConditionalExpression
    pub(super) fn emit_conditional_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let conditional = read
            .data_source()
            .as_conditional_expression()
            .ok_or(Error::MissingNode("conditional payload"))?;
        let (condition, question, when_true, colon, when_false) = (
            conditional
                .condition()
                .ok_or(Error::MissingNode("condition"))?,
            conditional
                .question_token()
                .ok_or(Error::MissingNode("question token"))?,
            conditional
                .when_true()
                .ok_or(Error::MissingNode("whenTrue"))?,
            conditional
                .colon_token()
                .ok_or(Error::MissingNode("colon token"))?,
            conditional
                .when_false()
                .ok_or(Error::MissingNode("whenFalse"))?,
        );
        let span = |session: &Self, id: NodeId| -> Result<Span, Error> {
            Ok(Span::of(&session.node(id)?))
        };
        let before_question =
            self.get_lines_between_nodes(node, span(self, condition)?, span(self, question)?)?;
        let after_question =
            self.get_lines_between_nodes(node, span(self, question)?, span(self, when_true)?)?;
        let before_colon =
            self.get_lines_between_nodes(node, span(self, when_true)?, span(self, colon)?)?;
        let after_colon =
            self.get_lines_between_nodes(node, span(self, colon)?, span(self, when_false)?)?;
        self.emit_short_circuit_expression(condition)?;
        self.write_lines_and_indent(before_question, true);
        self.emit_punctuation_node(Some(question))?;
        self.write_lines_and_indent(after_question, true);
        self.emit_expression(when_true, op::YIELD)?;
        self.decrease_indent_if(after_question > 0);
        self.decrease_indent_if(before_question > 0);
        self.write_lines_and_indent(before_colon, true);
        self.emit_punctuation_node(Some(colon))?;
        self.write_lines_and_indent(after_colon, true);
        self.emit_expression(when_false, op::YIELD)?;
        self.decrease_indent_if(after_colon > 0);
        self.decrease_indent_if(before_colon > 0);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateSpan
    pub(super) fn emit_template_span(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let span = read
            .data_source()
            .as_template_span()
            .ok_or(Error::MissingNode("template span payload"))?;
        let (expression, literal) = (
            span.expression()
                .ok_or(Error::MissingNode("span expression"))?,
            span.literal().ok_or(Error::MissingNode("span literal"))?,
        );
        self.emit_expression(expression, op::COMMA)?;
        self.emit_template_middle_tail(literal)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateExpression
    pub(super) fn emit_template_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let template = read
            .data_source()
            .as_template_expression()
            .ok_or(Error::MissingNode("template payload"))?;
        let (head, spans) = (
            template.head().ok_or(Error::MissingNode("template head"))?,
            template.template_spans(),
        );
        self.emit_template_head(head)?;
        self.emit_list(
            Self::emit_template_span,
            node,
            spans,
            lf::TEMPLATE_EXPRESSION_SPANS,
        )?;
        self.exit_node(node);
        Ok(())
    }

    /// Upstream parenthesizes an operand whose leading comment would put a
    /// line break after `return`, `throw` or `yield`. No comment is emitted
    /// here, so the operand is printed as it is.
    // port: tsc/internal/printer/printer.go:Printer.emitExpressionNoASI
    fn emit_expression_no_asi(&mut self, node: NodeId, precedence: i32) -> Result<(), Error> {
        self.emit_expression(node, precedence)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitYieldExpression
    pub(super) fn emit_yield_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let asterisk = read
            .data_source()
            .as_yield_expression()
            .ok_or(Error::MissingNode("yield payload"))?
            .asterisk_token();
        let expression = read.expression();
        self.emit_token(
            K::YieldKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.emit_punctuation_node(asterisk)?;
        if let Some(expression) = expression {
            self.write_space();
            self.emit_expression_no_asi(expression, op::DISALLOW_COMMA)?;
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSpreadElement
    pub(super) fn emit_spread_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("spread expression"))?;
        self.emit_token(
            K::DotDotDotToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        self.emit_expression(expression, op::DISALLOW_COMMA)?;
        self.exit_node(node);
        Ok(())
    }

    /// What a class expression and a class declaration share.
    // port: tsc/internal/printer/printer.go:Printer.emitClassExpression
    // port: tsc/internal/printer/printer.go:Printer.emitClassDeclaration
    pub(super) fn emit_class(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let heritage = match read.kind().known() {
            Some(K::ClassExpression) => read
                .data_source()
                .as_class_expression()
                .and_then(|class| class.heritage_clauses()),
            _ => read
                .data_source()
                .as_class_declaration()
                .and_then(|class| class.heritage_clauses()),
        };
        let (modifiers, name, type_parameters, members) = (
            read.modifiers(),
            read.name(),
            read.type_parameter_list(),
            read.member_list(),
        );
        let pos = self.emit_modifier_list(node, modifiers, true)?;
        self.emit_token(K::ClassKeyword, pos, WriteKind::Keyword, node);
        if let Some(name) = name {
            self.write_space();
            self.emit_identifier_name(name)?;
        }
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_list(
            Self::emit_heritage_clause,
            node,
            heritage,
            lf::CLASS_HERITAGE_CLAUSES,
        )?;
        self.write_space();
        self.write_punctuation(b"{");
        self.emit_list(Self::emit_class_element, node, members, lf::CLASS_MEMBERS)?;
        self.write_punctuation(b"}");
        self.decrease_indent_if(indented);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitAsExpression
    // port: tsc/internal/printer/printer.go:Printer.emitSatisfiesExpression
    pub(super) fn emit_as_or_satisfies_expression(
        &mut self,
        node: NodeId,
        keyword: &'static [u8],
    ) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (expression, type_node) = (
            read.expression().ok_or(Error::MissingNode("as operand"))?,
            read.type_node().ok_or(Error::MissingNode("as type"))?,
        );
        self.emit_expression(expression, op::RELATIONAL)?;
        self.write_space();
        self.write_keyword(keyword);
        self.write_space();
        self.emit_type_node_outside_extends(type_node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNonNullExpression
    pub(super) fn emit_non_null_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let expression = self
            .node(node)?
            .expression()
            .ok_or(Error::MissingNode("non-null operand"))?;
        self.emit_expression(expression, op::MEMBER)?;
        self.write_operator(b"!");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMetaProperty
    pub(super) fn emit_meta_property(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let keyword = read
            .data_source()
            .as_meta_property()
            .ok_or(Error::MissingNode("meta property payload"))?
            .keyword_token();
        let keyword = keyword.known().ok_or(Error::UnexpectedKind {
            context: "meta property keyword",
            kind: keyword,
        })?;
        let name = read
            .name()
            .ok_or(Error::MissingNode("meta property name"))?;
        self.emit_token(keyword, i64::from(read.pos()), WriteKind::Punctuation, node);
        self.write_punctuation(b".");
        self.emit_identifier_name(name)?;
        self.exit_node(node);
        Ok(())
    }

    /// Nested partially emitted expressions are entered outermost first and
    /// left innermost first. Upstream leaves them through a variable that still
    /// names the innermost one on the first pass, which is reproduced. The
    /// comments it would emit between are not.
    // port: tsc/internal/printer/printer.go:Printer.emitPartiallyEmittedExpression
    pub(super) fn emit_partially_emitted_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let mut stack = Vec::new();
        let mut current = node;
        let inner = loop {
            self.enter_node(current);
            stack.push(current);
            let expression = self
                .node(current)?
                .expression()
                .ok_or(Error::MissingNode("partially emitted expression"))?;
            if self.node(expression)?.kind() != K::PartiallyEmittedExpression {
                break expression;
            }
            current = expression;
        };
        self.emit_expression(inner, op::LOWEST)?;
        while let Some(entry) = stack.pop() {
            self.exit_node(current);
            current = entry;
        }
        Ok(())
    }

    //
    // Statements
    //

    // port: tsc/internal/printer/printer.go:Printer.isEmptyBlock
    fn is_empty_block(&self, block: NodeId, statements: Option<NodeListId>) -> Result<bool, Error> {
        if self.list_len(statements)? != 0 {
            return Ok(false);
        }
        if self.current_source.is_none() {
            return Ok(true);
        }
        let span = Span::of(&self.node(block)?);
        Ok(self.range_end_is_on_same_line_as_range_start(span, span))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBlock
    // port: tsc/internal/printer/printer.go:Printer.emitModuleBlock
    pub(super) fn emit_block(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        // A module block has no multi-line mark of its own.
        let multi_line = read
            .data_source()
            .as_block()
            .is_some_and(|block| block.multi_line());
        let statements = read.statement_list();
        self.emit_token(
            K::OpenBraceToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        let format = if !multi_line && self.is_empty_block(node, statements)?
            || self.should_emit_on_single_line(node)
        {
            lf::SINGLE_LINE_BLOCK_STATEMENTS
        } else {
            lf::MULTI_LINE_BLOCK_STATEMENTS
        };
        self.emit_list(Self::emit_statement, node, statements, format)?;
        let end = self.list_end(statements, "block statements")?;
        self.emit_token(K::CloseBraceToken, end, WriteKind::Punctuation, node);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitVariableStatement
    fn emit_variable_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let declaration_list = read
            .data_source()
            .as_variable_statement()
            .ok_or(Error::MissingNode("variable statement payload"))?
            .declaration_list()
            .ok_or(Error::MissingNode("declaration list"))?;
        self.emit_modifier_list(node, read.modifiers(), false)?;
        self.emit_variable_declaration_list(declaration_list)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    /// Most trailing semicolons may be dropped by a writer that omits them. An
    /// embedded empty statement is significant and may not.
    // port: tsc/internal/printer/printer.go:Printer.emitEmptyStatement
    fn emit_empty_statement(&mut self, node: NodeId, is_embedded_statement: bool) {
        self.enter_node(node);
        if is_embedded_statement {
            self.write_punctuation(b";");
        } else {
            self.write_trailing_semicolon();
        }
        self.exit_node(node);
    }

    fn is_json_source(&self) -> bool {
        self.current_source
            .as_ref()
            .is_some_and(|(_, source)| source.script_kind == tsr_core::ScriptKind::JSON)
    }

    // port: tsc/internal/printer/utilities.go:isImmediatelyInvokedFunctionExpressionOrArrowFunction
    fn is_immediately_invoked_function(&self, node: NodeId) -> Result<bool, Error> {
        let call = tsr_ast::skip_partially_emitted_expressions(self.view, node)?;
        let read = self.node(call)?;
        if read.kind() != K::CallExpression {
            return Ok(false);
        }
        let Some(callee) = read.expression() else {
            return Ok(false);
        };
        let callee = tsr_ast::skip_partially_emitted_expressions(self.view, callee)?;
        let kind = self.node(callee)?.kind();
        Ok(kind == K::FunctionExpression || kind == K::ArrowFunction)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExpressionStatement
    fn emit_expression_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let expression = self
            .node(node)?
            .expression()
            .ok_or(Error::MissingNode("statement expression"))?;
        if self.is_json_source() {
            self.emit_expression(expression, op::COMMA)?;
        } else if self.is_immediately_invoked_function(expression)? {
            // Only the callee is parenthesized: `(function () { })()`.
            self.emit_iife_with_parenthesized_callee(expression)?;
        } else {
            let leftmost = tsr_ast::get_leftmost_expression(self.view, expression, false)?;
            let kind = self.node(leftmost)?.kind();
            let precedence = if kind == K::FunctionExpression || kind == K::ObjectLiteralExpression
            {
                op::PARENTHESES
            } else {
                op::COMMA
            };
            self.emit_expression(expression, precedence)?;
        }
        // A JSON file takes a semicolon only after a synthesized expression.
        if !self.is_json_source()
            || tsr_ast::utilities::node_is_synthesized(&self.node(expression)?)
        {
            self.write_trailing_semicolon();
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIIFEWithParenthesizedCallee
    fn emit_iife_with_parenthesized_callee(&mut self, node: NodeId) -> Result<(), Error> {
        let call = tsr_ast::skip_partially_emitted_expressions(self.view, node)?;
        self.enter_node(call);
        let read = self.node(call)?;
        let data = read
            .data_source()
            .as_call_expression()
            .ok_or(Error::MissingNode("call payload"))?;
        let (callee, question_dot, type_arguments, arguments) = (
            data.expression().ok_or(Error::MissingNode("callee"))?,
            data.question_dot_token(),
            data.type_arguments(),
            data.arguments(),
        );
        self.write_punctuation(b"(");
        self.emit_expression(callee, op::LOWEST)?;
        self.write_punctuation(b")");
        self.emit_token_node(question_dot)?;
        self.emit_type_arguments(call, type_arguments)?;
        self.emit_list(
            Self::emit_argument,
            call,
            arguments,
            lf::CALL_EXPRESSION_ARGUMENTS,
        )?;
        self.exit_node(call);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLineOrSpace
    fn write_line_or_space(
        &mut self,
        parent: NodeId,
        previous: NodeId,
        next: NodeId,
    ) -> Result<(), Error> {
        if self.should_emit_on_single_line(parent) {
            self.write_space();
        } else if self.printer.options.preserve_source_newlines {
            let lines = self.get_lines_between_nodes(
                parent,
                Span::of(&self.node(previous)?),
                Span::of(&self.node(next)?),
            )?;
            if lines > 0 {
                self.write_line_repeat(lines);
            } else {
                self.write_space();
            }
        } else {
            self.write_line();
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitEmbeddedStatement
    fn emit_embedded_statement(&mut self, parent: NodeId, node: NodeId) -> Result<(), Error> {
        let kind = self.node(node)?.kind();
        if kind == K::Block
            || self.should_emit_on_single_line(parent)
            || self.printer.options.preserve_source_newlines
                && self.get_leading_line_terminator_count(Some(parent), Some(node), lf::NONE)? == 0
        {
            self.write_space();
            return self.emit_statement(node);
        }
        self.write_line();
        self.increase_indent();
        if kind == K::EmptyStatement {
            self.emit_empty_statement(node, true);
        } else {
            self.emit_statement(node)?;
        }
        self.decrease_indent();
        Ok(())
    }

    /// `keyword (expression)`, returning nothing: every caller continues from
    /// the end of the expression.
    fn emit_keyword_and_parenthesized_expression(
        &mut self,
        node: NodeId,
        keyword: K,
        keyword_pos: i64,
        expression: NodeId,
    ) -> Result<(), Error> {
        let pos = self.emit_token(keyword, keyword_pos, WriteKind::Keyword, node);
        self.write_space();
        self.emit_token(K::OpenParenToken, pos, WriteKind::Punctuation, node);
        self.emit_expression(expression, op::LOWEST)?;
        let end = i64::from(self.node(expression)?.end());
        self.emit_token(K::CloseParenToken, end, WriteKind::Punctuation, node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIfStatement
    fn emit_if_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let statement = read
            .data_source()
            .as_if_statement()
            .ok_or(Error::MissingNode("if payload"))?;
        let (expression, then_statement, else_statement) = (
            statement
                .expression()
                .ok_or(Error::MissingNode("if condition"))?,
            statement
                .then_statement()
                .ok_or(Error::MissingNode("then statement"))?,
            statement.else_statement(),
        );
        self.emit_keyword_and_parenthesized_expression(
            node,
            K::IfKeyword,
            i64::from(read.pos()),
            expression,
        )?;
        self.emit_embedded_statement(node, then_statement)?;
        if let Some(else_statement) = else_statement {
            self.write_line_or_space(node, then_statement, else_statement)?;
            let then_end = i64::from(self.node(then_statement)?.end());
            self.emit_token(K::ElseKeyword, then_end, WriteKind::Keyword, node);
            if self.node(else_statement)?.kind() == K::IfStatement {
                self.write_space();
                self.emit_if_statement(else_statement)?;
            } else {
                self.emit_embedded_statement(node, else_statement)?;
            }
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDoStatement
    fn emit_do_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (statement, expression) = (
            read.statement().ok_or(Error::MissingNode("do body"))?,
            read.expression()
                .ok_or(Error::MissingNode("do condition"))?,
        );
        self.emit_token(
            K::DoKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.emit_embedded_statement(node, statement)?;
        if self.node(statement)?.kind() == K::Block
            && !self.printer.options.preserve_source_newlines
        {
            self.write_space();
        } else {
            self.write_line_or_space(node, statement, expression)?;
        }
        // port: tsc/internal/printer/printer.go:Printer.emitWhileClause
        let statement_end = i64::from(self.node(statement)?.end());
        self.emit_keyword_and_parenthesized_expression(
            node,
            K::WhileKeyword,
            statement_end,
            expression,
        )?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitWhileStatement
    // port: tsc/internal/printer/printer.go:Printer.emitWithStatement
    fn emit_while_or_with_statement(&mut self, node: NodeId, keyword: K) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (statement, expression) = (
            read.statement()
                .ok_or(Error::MissingNode("statement body"))?,
            read.expression()
                .ok_or(Error::MissingNode("statement expression"))?,
        );
        self.emit_keyword_and_parenthesized_expression(
            node,
            keyword,
            i64::from(read.pos()),
            expression,
        )?;
        self.emit_embedded_statement(node, statement)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitForInitializer
    fn emit_for_initializer(&mut self, node: NodeId) -> Result<(), Error> {
        if self.node(node)?.kind() == K::VariableDeclarationList {
            self.emit_variable_declaration_list(node)
        } else {
            self.emit_expression(node, op::LOWEST)
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitForStatement
    fn emit_for_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_for_statement()
            .ok_or(Error::MissingNode("for payload"))?;
        let (initializer, condition, incrementor, statement) = (
            data.initializer(),
            data.condition(),
            data.incrementor(),
            data.statement().ok_or(Error::MissingNode("for body"))?,
        );
        let mut pos = self.emit_token(
            K::ForKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        pos = self.emit_token(K::OpenParenToken, pos, WriteKind::Punctuation, node);
        if let Some(initializer) = initializer {
            self.emit_for_initializer(initializer)?;
            pos = i64::from(self.node(initializer)?.end());
        }
        pos = self.emit_token(K::SemicolonToken, pos, WriteKind::Punctuation, node);
        if let Some(condition) = condition {
            self.write_space();
            self.emit_expression(condition, op::LOWEST)?;
            pos = i64::from(self.node(condition)?.end());
        }
        pos = self.emit_token(K::SemicolonToken, pos, WriteKind::Punctuation, node);
        if let Some(incrementor) = incrementor {
            self.write_space();
            self.emit_expression(incrementor, op::LOWEST)?;
            pos = i64::from(self.node(incrementor)?.end());
        }
        self.emit_token(K::CloseParenToken, pos, WriteKind::Punctuation, node);
        self.emit_embedded_statement(node, statement)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitForInStatement
    // port: tsc/internal/printer/printer.go:Printer.emitForOfStatement
    fn emit_for_in_or_of_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let is_of = read.kind() == K::ForOfStatement;
        let data = read
            .data_source()
            .as_for_in_or_of_statement()
            .ok_or(Error::MissingNode("for-in payload"))?;
        let (await_modifier, initializer, expression, statement) = (
            data.await_modifier(),
            data.initializer()
                .ok_or(Error::MissingNode("for initializer"))?,
            data.expression()
                .ok_or(Error::MissingNode("for expression"))?,
            data.statement().ok_or(Error::MissingNode("for body"))?,
        );
        let pos = self.emit_token(
            K::ForKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        if is_of {
            if let Some(await_modifier) = await_modifier {
                self.emit_keyword_node(Some(await_modifier))?;
                self.write_space();
            }
        }
        self.emit_token(K::OpenParenToken, pos, WriteKind::Punctuation, node);
        self.emit_for_initializer(initializer)?;
        self.write_space();
        let initializer_end = i64::from(self.node(initializer)?.end());
        self.emit_token(
            if is_of { K::OfKeyword } else { K::InKeyword },
            initializer_end,
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        self.emit_expression(expression, op::LOWEST)?;
        let expression_end = i64::from(self.node(expression)?.end());
        self.emit_token(
            K::CloseParenToken,
            expression_end,
            WriteKind::Punctuation,
            node,
        );
        self.emit_embedded_statement(node, statement)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitContinueStatement
    // port: tsc/internal/printer/printer.go:Printer.emitBreakStatement
    fn emit_break_or_continue_statement(&mut self, node: NodeId, keyword: K) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let label = read.label();
        self.emit_token(keyword, i64::from(read.pos()), WriteKind::Keyword, node);
        if let Some(label) = label {
            self.write_space();
            self.emit_label_identifier(label)?;
        }
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitReturnStatement
    fn emit_return_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read.expression();
        self.emit_token(
            K::ReturnKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        if let Some(expression) = expression {
            self.write_space();
            self.emit_expression_no_asi(expression, op::LOWEST)?;
        }
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSwitchStatement
    fn emit_switch_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let case_block = read
            .data_source()
            .as_switch_statement()
            .ok_or(Error::MissingNode("switch payload"))?
            .case_block()
            .ok_or(Error::MissingNode("case block"))?;
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("switch expression"))?;
        self.emit_keyword_and_parenthesized_expression(
            node,
            K::SwitchKeyword,
            i64::from(read.pos()),
            expression,
        )?;
        self.write_space();
        self.emit_case_block(case_block)?;
        self.exit_node(node);
        Ok(())
    }

    /// Upstream writes a space and the statement rather than an embedded
    /// statement, to keep the output close to Strada's.
    // port: tsc/internal/printer/printer.go:Printer.emitLabeledStatement
    fn emit_labeled_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (label, statement) = (
            read.label().ok_or(Error::MissingNode("label"))?,
            read.statement()
                .ok_or(Error::MissingNode("labeled statement"))?,
        );
        self.emit_label_identifier(label)?;
        let label_end = i64::from(self.node(label)?.end());
        self.emit_token(K::ColonToken, label_end, WriteKind::Punctuation, node);
        self.write_space();
        self.emit_statement(statement)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitThrowStatement
    fn emit_throw_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("throw expression"))?;
        self.emit_token(
            K::ThrowKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        self.emit_expression_no_asi(expression, op::LOWEST)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTryStatement
    fn emit_try_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_try_statement()
            .ok_or(Error::MissingNode("try payload"))?;
        let (try_block, catch_clause, finally_block) = (
            data.try_block().ok_or(Error::MissingNode("try block"))?,
            data.catch_clause(),
            data.finally_block(),
        );
        self.emit_token(
            K::TryKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        self.emit_block(try_block)?;
        if let Some(catch_clause) = catch_clause {
            self.write_line_or_space(node, try_block, catch_clause)?;
            self.emit_catch_clause(catch_clause)?;
        }
        if let Some(finally_block) = finally_block {
            let previous = catch_clause.unwrap_or(try_block);
            self.write_line_or_space(node, previous, finally_block)?;
            let previous_end = i64::from(self.node(previous)?.end());
            self.emit_token(K::FinallyKeyword, previous_end, WriteKind::Keyword, node);
            self.write_space();
            self.emit_block(finally_block)?;
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDebuggerStatement
    fn emit_debugger_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let pos = i64::from(self.node(node)?.pos());
        self.emit_token(K::DebuggerKeyword, pos, WriteKind::Keyword, node);
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    //
    // Declarations
    //

    /// The type node an emit context may attach to the name is not kept here.
    // port: tsc/internal/printer/printer.go:Printer.emitVariableDeclaration
    pub(super) fn emit_variable_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let exclamation = read
            .data_source()
            .as_variable_declaration()
            .ok_or(Error::MissingNode("variable declaration payload"))?
            .exclamation_token();
        let (name, type_node, initializer) = (read.name(), read.type_node(), read.initializer());
        self.emit_binding_name(name)?;
        self.emit_punctuation_node(exclamation)?;
        self.emit_type_annotation(type_node)?;
        let name_end = self
            .end_of(name)?
            .ok_or(Error::MissingNode("variable name"))?;
        let equals_pos = greatest_end(name_end, &[self.end_of(type_node)?]);
        self.emit_initializer(initializer, equals_pos, node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitVariableDeclarationList
    pub(super) fn emit_variable_declaration_list(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let declarations = self
            .node(node)?
            .data_source()
            .as_variable_declaration_list()
            .ok_or(Error::MissingNode("declaration list payload"))?
            .declarations();
        if tsr_ast::utilities::is_var_let(self.view, node)? {
            self.write_keyword(b"let");
        } else if tsr_ast::utilities::is_var_const(self.view, node)? {
            self.write_keyword(b"const");
        } else if tsr_ast::utilities::is_var_using(self.view, node)? {
            self.write_keyword(b"using");
        } else if tsr_ast::utilities::is_var_await_using(self.view, node)? {
            self.write_keyword(b"await");
            self.write_space();
            self.write_keyword(b"using");
        } else {
            self.write_keyword(b"var");
        }
        self.write_space();
        self.emit_list(
            Self::emit_variable_declaration,
            node,
            declarations,
            lf::VARIABLE_DECLARATION_LIST,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitFunctionDeclaration
    fn emit_function_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let asterisk = read
            .data_source()
            .as_function_declaration()
            .ok_or(Error::MissingNode("function payload"))?
            .asterisk_token();
        let (modifiers, name) = (read.modifiers(), read.name());
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"function");
        self.emit_token_node(asterisk)?;
        self.write_space();
        if let Some(name) = name {
            self.emit_identifier_name(name)?;
        }
        self.emit_signature_and_body(node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInterfaceDeclaration
    fn emit_interface_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let heritage = read
            .data_source()
            .as_interface_declaration()
            .ok_or(Error::MissingNode("interface payload"))?
            .heritage_clauses();
        let (modifiers, name, type_parameters, members) = (
            read.modifiers(),
            read.name().ok_or(Error::MissingNode("interface name"))?,
            read.type_parameter_list(),
            read.member_list(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"interface");
        self.write_space();
        self.emit_binding_identifier(name)?;
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_list(
            Self::emit_heritage_clause,
            node,
            heritage,
            lf::HERITAGE_CLAUSES,
        )?;
        self.write_space();
        self.write_punctuation(b"{");
        self.emit_list(
            Self::emit_type_element,
            node,
            members,
            lf::INTERFACE_MEMBERS,
        )?;
        self.write_punctuation(b"}");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeAliasDeclaration
    fn emit_type_alias_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (modifiers, name, type_parameters, type_node) = (
            read.modifiers(),
            read.name().ok_or(Error::MissingNode("alias name"))?,
            read.type_parameter_list(),
            read.type_node().ok_or(Error::MissingNode("alias type"))?,
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"type");
        self.write_space();
        self.emit_binding_identifier(name)?;
        self.emit_type_parameters(node, type_parameters)?;
        self.write_space();
        self.write_punctuation(b"=");
        self.write_space();
        self.emit_type_node_outside_extends(type_node)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitEnumMember
    pub(super) fn emit_enum_member(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (name, initializer) = (read.name(), read.initializer());
        self.emit_property_name(name)?;
        let name_end = self
            .end_of(name)?
            .ok_or(Error::MissingNode("member name"))?;
        self.emit_initializer(initializer, name_end, node)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitEnumDeclaration
    fn emit_enum_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (modifiers, name, members) = (
            read.modifiers(),
            read.name().ok_or(Error::MissingNode("enum name"))?,
            read.member_list(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"enum");
        self.write_space();
        self.emit_binding_identifier(name)?;
        self.write_space();
        self.write_punctuation(b"{");
        self.emit_list(Self::emit_enum_member, node, members, lf::ENUM_MEMBERS)?;
        self.write_punctuation(b"}");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitModuleDeclaration
    fn emit_module_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_module_declaration()
            .ok_or(Error::MissingNode("module payload"))?;
        let (keyword, attributes) = (data.keyword(), data.attributes());
        self.emit_modifier_list(node, read.modifiers(), false)?;
        if keyword != K::GlobalKeyword {
            self.write_keyword(if keyword == K::NamespaceKeyword {
                b"namespace"
            } else {
                b"module"
            });
            self.write_space();
        }
        self.emit_module_name(read.name())?;
        let mut body = read.body();
        while let Some(inner) = body {
            let inner_read = self.node(inner)?;
            if inner_read.kind() != K::ModuleDeclaration {
                break;
            }
            self.write_punctuation(b".");
            self.emit_module_export_name(inner_read.name())?;
            body = inner_read.body();
        }
        if let Some(attributes) = attributes {
            self.write_space();
            self.write_keyword(b"with");
            self.write_space();
            self.emit_type_node(attributes, TypePrecedence::NonArray)?;
        }
        match body {
            None => self.write_trailing_semicolon(),
            Some(body) => {
                self.write_space();
                self.emit_block(body)?;
            }
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCaseBlock
    pub(super) fn emit_case_block(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let clauses = read
            .data_source()
            .as_case_block()
            .ok_or(Error::MissingNode("case block payload"))?
            .clauses();
        self.emit_token(
            K::OpenBraceToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        self.emit_list(
            Self::emit_case_or_default_clause,
            node,
            clauses,
            lf::CASE_BLOCK_CLAUSES,
        )?;
        let end = self.list_end(clauses, "case clauses")?;
        self.emit_token(K::CloseBraceToken, end, WriteKind::Punctuation, node);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportEqualsDeclaration
    fn emit_import_equals_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_import_equals_declaration()
            .ok_or(Error::MissingNode("import-equals payload"))?;
        let (is_type_only, module_reference) = (
            data.is_type_only(),
            data.module_reference()
                .ok_or(Error::MissingNode("module reference"))?,
        );
        let name = read.name().ok_or(Error::MissingNode("import name"))?;
        let modifiers = read.modifiers();
        self.emit_modifier_list(node, modifiers, false)?;
        let keyword_pos = greatest_end(i64::from(read.pos()), &[self.modifiers_end(modifiers)?]);
        let pos = self.emit_token(K::ImportKeyword, keyword_pos, WriteKind::Keyword, node);
        self.write_space();
        if is_type_only {
            self.emit_token(K::TypeKeyword, pos, WriteKind::Keyword, node);
            self.write_space();
        }
        self.emit_binding_identifier(name)?;
        self.write_space();
        let name_end = i64::from(self.node(name)?.end());
        self.emit_token(K::EqualsToken, name_end, WriteKind::Punctuation, node);
        self.write_space();
        // port: tsc/internal/printer/printer.go:Printer.emitModuleReference
        match self.known_kind(module_reference)? {
            K::Identifier => self.emit_identifier_reference(module_reference)?,
            K::QualifiedName => self.emit_qualified_name(module_reference)?,
            K::ExternalModuleReference => self.emit_external_module_reference(module_reference)?,
            kind => {
                return Err(Error::UnexpectedKind {
                    context: "unhandled ModuleReference",
                    kind: kind.into(),
                })
            }
        }
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    /// `greatestEnd` over a modifier list reads the list's own end.
    fn modifiers_end(&self, modifiers: Option<NodeListId>) -> Result<Option<i64>, Error> {
        match modifiers {
            Some(list) => Ok(Some(self.view.list(list)?.loc().end())),
            None => Ok(None),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExternalModuleReference
    pub(super) fn emit_external_module_reference(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let expression = self
            .node(node)?
            .expression()
            .ok_or(Error::MissingNode("module reference expression"))?;
        self.write_keyword(b"require");
        self.write_punctuation(b"(");
        self.emit_expression(expression, op::DISALLOW_COMMA)?;
        self.write_punctuation(b")");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportDeclaration
    fn emit_import_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let attributes = read
            .data_source()
            .as_import_declaration()
            .ok_or(Error::MissingNode("import payload"))?
            .attributes();
        let (modifiers, import_clause, module_specifier) = (
            read.modifiers(),
            read.import_clause(),
            read.module_specifier()
                .ok_or(Error::MissingNode("module specifier"))?,
        );
        self.emit_modifier_list(node, modifiers, false)?;
        let keyword_pos = greatest_end(i64::from(read.pos()), &[self.modifiers_end(modifiers)?]);
        self.emit_token(K::ImportKeyword, keyword_pos, WriteKind::Keyword, node);
        self.write_space();
        if let Some(import_clause) = import_clause {
            self.emit_import_clause(import_clause)?;
            self.write_space();
            let clause_end = i64::from(self.node(import_clause)?.end());
            self.emit_token(K::FromKeyword, clause_end, WriteKind::Keyword, node);
            self.write_space();
        }
        self.emit_expression(module_specifier, op::LOWEST)?;
        if let Some(attributes) = attributes {
            self.write_space();
            self.emit_import_attributes(attributes)?;
        }
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportClause
    pub(super) fn emit_import_clause(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_import_clause()
            .ok_or(Error::MissingNode("import clause payload"))?;
        let (phase_modifier, name, named_bindings) =
            (data.phase_modifier(), data.name(), data.named_bindings());
        if phase_modifier != K::Unknown {
            let keyword = phase_modifier.known().ok_or(Error::UnexpectedKind {
                context: "import phase modifier",
                kind: phase_modifier,
            })?;
            self.emit_token(keyword, i64::from(read.pos()), WriteKind::Keyword, node);
            self.write_space();
        }
        if let Some(name) = name {
            self.emit_binding_identifier(name)?;
            if named_bindings.is_some() {
                let name_end = i64::from(self.node(name)?.end());
                self.emit_token(K::CommaToken, name_end, WriteKind::Punctuation, node);
                self.write_space();
            }
        }
        // port: tsc/internal/printer/printer.go:Printer.emitNamedImportBindings
        if let Some(bindings) = named_bindings {
            match self.known_kind(bindings)? {
                K::NamespaceImport => self.emit_namespace_import(bindings)?,
                K::NamedImports => self.emit_named_imports_or_exports(bindings)?,
                kind => {
                    return Err(Error::UnexpectedKind {
                        context: "unhandled NamedImportBindings",
                        kind: kind.into(),
                    })
                }
            }
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNamespaceImport
    // port: tsc/internal/printer/printer.go:Printer.emitNamespaceExport
    pub(super) fn emit_namespace_import(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let name = read.name().ok_or(Error::MissingNode("namespace name"))?;
        let pos = self.emit_token(
            K::AsteriskToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        self.write_space();
        self.emit_token(K::AsKeyword, pos, WriteKind::Keyword, node);
        self.write_space();
        if read.kind() == K::NamespaceImport {
            self.emit_binding_identifier(name)?;
        } else {
            self.emit_module_export_name(Some(name))?;
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNamedImports
    // port: tsc/internal/printer/printer.go:Printer.emitNamedExports
    pub(super) fn emit_named_imports_or_exports(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let elements = self.node(node)?.element_list();
        self.write_punctuation(b"{");
        self.emit_list(
            Self::emit_import_or_export_specifier,
            node,
            elements,
            lf::NAMED_IMPORTS_OR_EXPORTS_ELEMENTS,
        )?;
        self.write_punctuation(b"}");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportSpecifier
    // port: tsc/internal/printer/printer.go:Printer.emitExportSpecifier
    pub(super) fn emit_import_or_export_specifier(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let is_import = read.kind() == K::ImportSpecifier;
        let (property_name, name) = (
            read.property_name(),
            read.name().ok_or(Error::MissingNode("specifier name"))?,
        );
        if read.is_type_only() {
            self.write_keyword(b"type");
            self.write_space();
        }
        if let Some(property_name) = property_name {
            self.emit_module_export_name(Some(property_name))?;
            self.write_space();
            let end = i64::from(self.node(property_name)?.end());
            self.emit_token(K::AsKeyword, end, WriteKind::Keyword, node);
            self.write_space();
        }
        if is_import {
            self.emit_binding_identifier(name)?;
        } else {
            self.emit_module_export_name(Some(name))?;
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExportAssignment
    fn emit_export_assignment(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let is_export_equals = read
            .data_source()
            .as_export_assignment()
            .ok_or(Error::MissingNode("export assignment payload"))?
            .is_export_equals();
        let expression = read
            .expression()
            .ok_or(Error::MissingNode("exported expression"))?;
        let next = self.emit_token(
            K::ExportKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        if is_export_equals {
            self.emit_token(K::EqualsToken, next, WriteKind::Operator, node);
        } else {
            self.emit_token(K::DefaultKeyword, next, WriteKind::Keyword, node);
        }
        self.write_space();
        let mut precedence = op::ASSIGNMENT;
        if !is_export_equals {
            // A class or function expression is parenthesized so it does not
            // read as an exported declaration.
            let leftmost = tsr_ast::get_leftmost_expression(self.view, expression, false)?;
            let kind = self.node(leftmost)?.kind();
            if kind == K::ClassExpression || kind == K::FunctionExpression {
                precedence = op::PARENTHESES;
            }
        }
        self.emit_expression(expression, precedence)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExportDeclaration
    fn emit_export_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_export_declaration()
            .ok_or(Error::MissingNode("export payload"))?;
        let (is_type_only, export_clause, attributes) =
            (data.is_type_only(), data.export_clause(), data.attributes());
        let (modifiers, module_specifier) = (read.modifiers(), read.module_specifier());
        self.emit_modifier_list(node, modifiers, false)?;
        let mut pos = self.emit_token(
            K::ExportKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        if is_type_only {
            pos = self.emit_token(K::TypeKeyword, pos, WriteKind::Keyword, node);
            self.write_space();
        }
        match export_clause {
            // port: tsc/internal/printer/printer.go:Printer.emitNamedExportBindings
            Some(clause) => match self.known_kind(clause)? {
                K::NamespaceExport => self.emit_namespace_import(clause)?,
                K::NamedExports => self.emit_named_imports_or_exports(clause)?,
                kind => {
                    return Err(Error::UnexpectedKind {
                        context: "unhandled NamedExportBindings",
                        kind: kind.into(),
                    })
                }
            },
            None => pos = self.emit_token(K::AsteriskToken, pos, WriteKind::Punctuation, node),
        }
        if let Some(module_specifier) = module_specifier {
            self.write_space();
            let from_pos = greatest_end(pos, &[self.end_of(export_clause)?]);
            self.emit_token(K::FromKeyword, from_pos, WriteKind::Keyword, node);
            self.write_space();
            self.emit_expression(module_specifier, op::LOWEST)?;
        }
        if let Some(attributes) = attributes {
            self.write_space();
            self.emit_import_attributes(attributes)?;
        }
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportAttributes
    pub(super) fn emit_import_attributes(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_import_attributes()
            .ok_or(Error::MissingNode("import attributes payload"))?;
        let (token, attributes) = (data.token(), data.attributes());
        let token = token.known().ok_or(Error::UnexpectedKind {
            context: "import attributes keyword",
            kind: token,
        })?;
        self.emit_token(token, i64::from(read.pos()), WriteKind::Keyword, node);
        self.write_space();
        self.emit_list(
            Self::emit_import_attribute,
            node,
            attributes,
            lf::IMPORT_ATTRIBUTES,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNamespaceExportDeclaration
    fn emit_namespace_export_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let name = read.name().ok_or(Error::MissingNode("namespace name"))?;
        let mut pos = self.emit_token(
            K::ExportKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        pos = self.emit_token(K::AsKeyword, pos, WriteKind::Keyword, node);
        self.write_space();
        self.emit_token(K::NamespaceKeyword, pos, WriteKind::Keyword, node);
        self.write_space();
        self.emit_binding_identifier(name)?;
        self.write_trailing_semicolon();
        self.exit_node(node);
        Ok(())
    }

    /// Snippet elements are an editor feature no caller here sets.
    // port: tsc/internal/printer/printer.go:Printer.emitStatement
    pub(super) fn emit_statement(&mut self, node: NodeId) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_statement_worker(node)
        })
    }

    fn emit_statement_worker(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Block => self.emit_block(node),
            K::EmptyStatement => {
                self.emit_empty_statement(node, false);
                Ok(())
            }
            K::VariableStatement => self.emit_variable_statement(node),
            K::ExpressionStatement => self.emit_expression_statement(node),
            K::IfStatement => self.emit_if_statement(node),
            K::DoStatement => self.emit_do_statement(node),
            K::WhileStatement => self.emit_while_or_with_statement(node, K::WhileKeyword),
            K::ForStatement => self.emit_for_statement(node),
            K::ForInStatement | K::ForOfStatement => self.emit_for_in_or_of_statement(node),
            K::ContinueStatement => self.emit_break_or_continue_statement(node, K::ContinueKeyword),
            K::BreakStatement => self.emit_break_or_continue_statement(node, K::BreakKeyword),
            K::ReturnStatement => self.emit_return_statement(node),
            K::WithStatement => self.emit_while_or_with_statement(node, K::WithKeyword),
            K::SwitchStatement => self.emit_switch_statement(node),
            K::LabeledStatement => self.emit_labeled_statement(node),
            K::ThrowStatement => self.emit_throw_statement(node),
            K::TryStatement => self.emit_try_statement(node),
            K::DebuggerStatement => self.emit_debugger_statement(node),
            K::NotEmittedStatement => {
                self.emit_nothing(node);
                Ok(())
            }
            K::FunctionDeclaration => self.emit_function_declaration(node),
            K::ClassDeclaration => self.emit_class(node),
            K::InterfaceDeclaration => self.emit_interface_declaration(node),
            K::TypeAliasDeclaration | K::JSTypeAliasDeclaration => {
                self.emit_type_alias_declaration(node)
            }
            K::EnumDeclaration => self.emit_enum_declaration(node),
            K::ModuleDeclaration => self.emit_module_declaration(node),
            K::MissingDeclaration => Ok(()),
            K::NamespaceExportDeclaration => self.emit_namespace_export_declaration(node),
            K::ImportEqualsDeclaration => self.emit_import_equals_declaration(node),
            K::ImportDeclaration => self.emit_import_declaration(node),
            K::ExportAssignment => self.emit_export_assignment(node),
            K::ExportDeclaration => self.emit_export_declaration(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled statement",
                kind: kind.into(),
            }),
        }
    }

    //
    // JSX
    //

    // port: tsc/internal/printer/printer.go:Printer.emitJsxElement
    // port: tsc/internal/printer/printer.go:Printer.emitJsxFragment
    pub(super) fn emit_jsx_element_or_fragment(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read.data_source();
        let (opening, children, closing) = if let Some(element) = data.as_jsx_element() {
            (
                element.opening_element(),
                element.children(),
                element.closing_element(),
            )
        } else {
            let fragment = data
                .as_jsx_fragment()
                .ok_or(Error::MissingNode("JSX payload"))?;
            (
                fragment.opening_fragment(),
                fragment.children(),
                fragment.closing_fragment(),
            )
        };
        let (opening, closing) = (
            opening.ok_or(Error::MissingNode("JSX opening"))?,
            closing.ok_or(Error::MissingNode("JSX closing"))?,
        );
        if read.kind() == K::JsxElement {
            self.emit_jsx_opening_element(opening)?;
        } else {
            self.emit_jsx_fragment_tag(opening, b"<");
        }
        self.emit_list(
            Self::emit_jsx_child,
            node,
            children,
            lf::JSX_ELEMENT_OR_FRAGMENT_CHILDREN,
        )?;
        if read.kind() == K::JsxElement {
            self.emit_jsx_closing_element(closing)?;
        } else {
            self.emit_jsx_fragment_tag(closing, b"</");
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxSelfClosingElement
    pub(super) fn emit_jsx_self_closing_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (tag_name, type_arguments, attributes) = (
            read.tag_name().ok_or(Error::MissingNode("JSX tag name"))?,
            read.type_argument_list(),
            read.attributes()
                .ok_or(Error::MissingNode("JSX attributes"))?,
        );
        self.write_punctuation(b"<");
        self.emit_jsx_tag_name(tag_name)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.write_space();
        self.emit_jsx_attributes(attributes)?;
        self.write_punctuation(b"/>");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxOpeningElement
    pub(super) fn emit_jsx_opening_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (tag_name, type_arguments, attributes) = (
            read.tag_name().ok_or(Error::MissingNode("JSX tag name"))?,
            read.type_argument_list(),
            read.attributes()
                .ok_or(Error::MissingNode("JSX attributes"))?,
        );
        self.write_punctuation(b"<");
        let psn = self.printer.options.preserve_source_newlines;
        let leading = if psn {
            self.get_leading_line_terminator_count(Some(node), Some(tag_name), lf::NONE)?
        } else {
            0
        };
        self.write_lines_and_indent(leading, false);
        self.emit_jsx_tag_name(tag_name)?;
        self.emit_type_arguments(node, type_arguments)?;
        if self.list_len(self.node(attributes)?.property_list())? > 0 {
            self.write_space();
        }
        self.emit_jsx_attributes(attributes)?;
        if psn {
            let trailing =
                self.get_closing_line_terminator_count(Some(node), Some(attributes), lf::NONE)?;
            self.write_line_repeat(trailing);
        }
        self.decrease_indent_if(leading > 0);
        self.write_punctuation(b">");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxClosingElement
    pub(super) fn emit_jsx_closing_element(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let tag_name = self
            .node(node)?
            .tag_name()
            .ok_or(Error::MissingNode("JSX tag name"))?;
        self.write_punctuation(b"</");
        self.emit_jsx_tag_name(tag_name)?;
        self.write_punctuation(b">");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxOpeningFragment
    // port: tsc/internal/printer/printer.go:Printer.emitJsxClosingFragment
    pub(super) fn emit_jsx_fragment_tag(&mut self, node: NodeId, open: &'static [u8]) {
        self.enter_node(node);
        self.write_punctuation(open);
        self.write_punctuation(b">");
        self.exit_node(node);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxText
    pub(super) fn emit_jsx_text(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let text = read
            .data_source()
            .as_jsx_text()
            .ok_or(Error::MissingNode("JSX text payload"))?
            .text();
        self.writer.write_literal(text);
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxAttributes
    pub(super) fn emit_jsx_attributes(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let properties = self.node(node)?.property_list();
        self.emit_list(
            Self::emit_jsx_attribute_like,
            node,
            properties,
            lf::JSX_ELEMENT_ATTRIBUTES,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxAttribute
    pub(super) fn emit_jsx_attribute(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (name, initializer) = (
            read.name()
                .ok_or(Error::MissingNode("JSX attribute name"))?,
            read.initializer(),
        );
        // port: tsc/internal/printer/printer.go:Printer.emitJsxAttributeName
        match self.known_kind(name)? {
            K::Identifier => self.emit_identifier_name(name)?,
            K::JsxNamespacedName => self.emit_jsx_namespaced_name(name)?,
            kind => {
                return Err(Error::UnexpectedKind {
                    context: "unhandled JsxAttributeName",
                    kind: kind.into(),
                })
            }
        }
        if let Some(initializer) = initializer {
            self.write_punctuation(b"=");
            // port: tsc/internal/printer/printer.go:Printer.emitJsxAttributeValue
            match self.known_kind(initializer)? {
                K::StringLiteral => self.emit_string_literal(initializer)?,
                K::JsxExpression => self.emit_jsx_expression(initializer)?,
                K::JsxElement | K::JsxFragment => self.emit_jsx_element_or_fragment(initializer)?,
                K::JsxSelfClosingElement => self.emit_jsx_self_closing_element(initializer)?,
                _ => self.emit_expression(initializer, op::LOWEST)?,
            }
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxSpreadAttribute
    pub(super) fn emit_jsx_spread_attribute(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let expression = self
            .node(node)?
            .expression()
            .ok_or(Error::MissingNode("JSX spread expression"))?;
        self.write_punctuation(b"{...");
        self.emit_expression(expression, op::LOWEST)?;
        self.write_punctuation(b"}");
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxAttributeLike
    fn emit_jsx_attribute_like(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::JsxAttribute => self.emit_jsx_attribute(node),
            K::JsxSpreadAttribute => self.emit_jsx_spread_attribute(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled JsxAttributeLike",
                kind: kind.into(),
            }),
        }
    }

    /// An empty expression is kept only for the comments inside it, which are
    /// not emitted here, so an empty one writes nothing.
    // port: tsc/internal/printer/printer.go:Printer.emitJsxExpression
    pub(super) fn emit_jsx_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let dot_dot_dot = read
            .data_source()
            .as_jsx_expression()
            .ok_or(Error::MissingNode("JSX expression payload"))?
            .dot_dot_dot_token();
        if let Some(expression) = read.expression() {
            let indented = self.current_source.is_some()
                && !tsr_ast::utilities::node_is_synthesized(&read)
                && self.get_lines_between_positions(i64::from(read.pos()), i64::from(read.end()))
                    != 0;
            self.increase_indent_if(indented);
            let end = self.emit_token(
                K::OpenBraceToken,
                i64::from(read.pos()),
                WriteKind::Punctuation,
                node,
            );
            self.emit_token_node(dot_dot_dot)?;
            self.emit_expression(expression, op::DISALLOW_COMMA)?;
            let close_pos = greatest_end(
                end,
                &[self.end_of(Some(expression))?, self.end_of(dot_dot_dot)?],
            );
            self.emit_token(K::CloseBraceToken, close_pos, WriteKind::Punctuation, node);
            self.decrease_indent_if(indented);
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxNamespacedName
    pub(super) fn emit_jsx_namespaced_name(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let namespace = read
            .data_source()
            .as_jsx_namespaced_name()
            .ok_or(Error::MissingNode("JSX namespaced name payload"))?
            .namespace()
            .ok_or(Error::MissingNode("JSX namespace"))?;
        let name = read.name().ok_or(Error::MissingNode("JSX name"))?;
        self.emit_identifier_name(namespace)?;
        self.write_punctuation(b":");
        self.emit_identifier_name(name)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxChild
    fn emit_jsx_child(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::JsxText => self.emit_jsx_text(node),
            K::JsxExpression => self.emit_jsx_expression(node),
            K::JsxElement | K::JsxFragment => self.emit_jsx_element_or_fragment(node),
            K::JsxSelfClosingElement => self.emit_jsx_self_closing_element(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled JsxChild",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJsxTagName
    fn emit_jsx_tag_name(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_reference(node),
            K::ThisKeyword => self.emit_keyword_expression(node),
            K::JsxNamespacedName => self.emit_jsx_namespaced_name(node),
            K::PropertyAccessExpression => self.emit_property_access_expression(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled JsxTagName",
                kind: kind.into(),
            }),
        }
    }

    //
    // Clauses
    //

    /// A clause whose one statement starts on the clause's own line is written
    /// on one line. Synthesized nodes count as being on the same line.
    // port: tsc/internal/printer/printer.go:Printer.emitCaseOrDefaultClauseStatements
    fn emit_case_or_default_clause_statements(
        &mut self,
        node: NodeId,
        statements: Option<NodeListId>,
        colon_pos: i64,
    ) -> Result<(), Error> {
        let nodes = match statements {
            Some(list) => self.list_nodes(list)?,
            None => Vec::new(),
        };
        let emit_as_single_statement = nodes.len() == 1 && {
            let clause = self.node(node)?;
            let statement = self.node(nodes[0])?;
            self.current_source.is_none()
                || tsr_ast::utilities::node_is_synthesized(&clause)
                || tsr_ast::utilities::node_is_synthesized(&statement)
                || self
                    .range_start_positions_are_on_same_line(Span::of(&clause), Span::of(&statement))
        };
        let mut format: ListFormat = lf::CASE_OR_DEFAULT_CLAUSE_STATEMENTS;
        if emit_as_single_statement {
            self.write_token_text(K::ColonToken, WriteKind::Punctuation, colon_pos);
            self.write_space();
            format &= !(lf::MULTI_LINE | lf::INDENTED);
        } else {
            self.emit_token(K::ColonToken, colon_pos, WriteKind::Punctuation, node);
        }
        self.emit_list(Self::emit_statement, node, statements, format)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCaseClause
    // port: tsc/internal/printer/printer.go:Printer.emitDefaultClause
    pub(super) fn emit_case_or_default_clause(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_case_or_default_clause()
            .ok_or(Error::MissingNode("clause payload"))?;
        let (expression, statements) = (data.expression(), data.statements());
        let colon_pos = if read.kind() == K::CaseClause {
            let expression = expression.ok_or(Error::MissingNode("case expression"))?;
            self.emit_token(
                K::CaseKeyword,
                i64::from(read.pos()),
                WriteKind::Keyword,
                node,
            );
            self.write_space();
            self.emit_expression(expression, op::LOWEST)?;
            i64::from(self.node(expression)?.end())
        } else {
            self.emit_token(
                K::DefaultKeyword,
                i64::from(read.pos()),
                WriteKind::Keyword,
                node,
            )
        };
        self.emit_case_or_default_clause_statements(node, statements, colon_pos)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitHeritageClause
    pub(super) fn emit_heritage_clause(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::MissingNode("heritage clause payload"))?;
        let (token, types) = (data.token(), data.types());
        let token = token.known().ok_or(Error::UnexpectedKind {
            context: "heritage clause keyword",
            kind: token,
        })?;
        self.write_space();
        self.emit_token(token, i64::from(read.pos()), WriteKind::Keyword, node);
        self.write_space();
        self.emit_list(
            Self::emit_heritage_clause_element,
            node,
            types,
            lf::HERITAGE_CLAUSE_TYPES,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitHeritageClauseElement
    fn emit_heritage_clause_element(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::ExpressionWithTypeArguments => self.emit_expression_with_type_arguments(node),
            K::TypeReference => self.emit_type_reference(node),
            kind => Err(Error::UnexpectedKind {
                context: "unhandled HeritageClauseElement",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCatchClause
    pub(super) fn emit_catch_clause(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_catch_clause()
            .ok_or(Error::MissingNode("catch payload"))?;
        let (variable_declaration, block) = (
            data.variable_declaration(),
            data.block().ok_or(Error::MissingNode("catch block"))?,
        );
        let open_paren_pos = self.emit_token(
            K::CatchKeyword,
            i64::from(read.pos()),
            WriteKind::Keyword,
            node,
        );
        self.write_space();
        if let Some(declaration) = variable_declaration {
            self.emit_token(
                K::OpenParenToken,
                open_paren_pos,
                WriteKind::Punctuation,
                node,
            );
            self.emit_variable_declaration(declaration)?;
            let end = i64::from(self.node(declaration)?.end());
            self.emit_token(K::CloseParenToken, end, WriteKind::Punctuation, node);
            self.write_space();
        }
        self.emit_block(block)?;
        self.exit_node(node);
        Ok(())
    }

    //
    // Property assignments
    //

    /// The comment upstream emits before the initializer is not emitted.
    // port: tsc/internal/printer/printer.go:Printer.emitPropertyAssignment
    pub(super) fn emit_property_assignment(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let (name, initializer) = (
            read.name(),
            read.initializer()
                .ok_or(Error::MissingNode("property initializer"))?,
        );
        self.emit_property_name(name)?;
        self.write_punctuation(b":");
        self.write_space();
        self.emit_expression(initializer, op::DISALLOW_COMMA)?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitShorthandPropertyAssignment
    pub(super) fn emit_shorthand_property_assignment(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let initializer = read
            .data_source()
            .as_shorthand_property_assignment()
            .ok_or(Error::MissingNode("shorthand payload"))?
            .object_assignment_initializer();
        self.emit_property_name(read.name())?;
        if let Some(initializer) = initializer {
            self.write_space();
            self.write_punctuation(b"=");
            self.write_space();
            self.emit_expression(initializer, op::DISALLOW_COMMA)?;
        }
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitSpreadAssignment
    pub(super) fn emit_spread_assignment(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        if let Some(expression) = read.expression() {
            self.emit_token(
                K::DotDotDotToken,
                i64::from(read.pos()),
                WriteKind::Punctuation,
                node,
            );
            self.emit_expression(expression, op::DISALLOW_COMMA)?;
        }
        self.exit_node(node);
        Ok(())
    }
}
