use crate::{Parser, ParserFactory};
use std::sync::Arc;
use ts_ast::{
    modifier_flags, node_flags, Diagnostic, JsString, NodeId, NodeListId, NodeSlice,
    SyntaxKind as K,
};
use ts_core::TextRange;
use ts_diagnostics::{self as diagnostics, Message};

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.jsErrorAtRange
    pub(crate) fn js_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static Message,
        args: Vec<JsString>,
    ) {
        self.js_diagnostics.push(Diagnostic::new(
            None,
            self.skip_range_trivia(loc),
            message,
            args,
        ));
    }

    /// port: tsc/internal/parser/parser.go:Parser.checkJSDecoratorSyntax
    pub(crate) fn check_js_decorator_syntax(&mut self, node: NodeId) {
        let modifiers = self
            .node_modifiers(node)
            .map_or(NodeSlice::empty(), |list| {
                self.factory.read_list(list).nodes()
            });
        if modifiers.is_empty() {
            return;
        }
        let kind = self.factory.node(node).kind();
        if can_have_illegal_decorators(kind) {
            if let Some(index) = self.js_find_modifier(modifiers, 0, K::Decorator) {
                let modifier = self
                    .factory
                    .read_nodes(modifiers)
                    .at(index)
                    .expect("parsed modifier");
                self.js_error_at_range(
                    self.factory.node(modifier).range(),
                    diagnostics::Decorators_are_not_valid_here,
                    vec![],
                );
            }
            return;
        }
        if !can_have_decorators(kind) {
            return;
        }
        let Some(decorator) = self.js_find_modifier(modifiers, 0, K::Decorator) else {
            return;
        };
        if kind != K::ClassDeclaration {
            return;
        }
        let Some(export) = self.js_find_modifier(modifiers, 0, K::ExportKeyword) else {
            return;
        };
        let default = self.js_find_modifier(modifiers, 0, K::DefaultKeyword);
        if decorator > export && default.is_some_and(|default| decorator < default) {
            let modifier = self
                .factory
                .read_nodes(modifiers)
                .at(decorator)
                .expect("parsed decorator");
            self.js_error_at_range(
                self.factory.node(modifier).range(),
                diagnostics::Decorators_are_not_valid_here,
                vec![],
            );
        } else if decorator < export {
            if let Some(trailing) = self.js_find_modifier(modifiers, export, K::Decorator) {
                let trailing = self
                    .factory
                    .read_nodes(modifiers)
                    .at(trailing)
                    .expect("parsed decorator");
                let first = self
                    .factory
                    .read_nodes(modifiers)
                    .at(decorator)
                    .expect("parsed decorator");
                let mut diagnostic = Diagnostic::new(None, self.skip_range_trivia(self.factory.node(trailing).range()), diagnostics::Decorators_may_not_appear_after_export_or_export_default_if_they_also_appear_before_export, vec![]);
                diagnostic
                    .related_information
                    .push(Arc::new(Diagnostic::new(
                        None,
                        self.skip_range_trivia(self.factory.node(first).range()),
                        diagnostics::Decorator_used_before_export_here,
                        vec![],
                    )));
                self.js_diagnostics.push(diagnostic);
            }
        }
    }

    /// port: tsc/internal/parser/parser.go:Parser.checkJSSyntax
    pub(crate) fn check_js_syntax(&mut self, node: NodeId) -> NodeId {
        let data = self.factory.node(node);
        let flags = data.flags();
        let kind = data.kind();
        let loc = data.range();
        drop(data);
        if flags & node_flags::JAVA_SCRIPT_FILE == 0
            || flags & (node_flags::JS_DOC | node_flags::REPARSED) != 0
        {
            return node;
        }

        if matches!(
            kind.known(),
            Some(K::Parameter | K::PropertyDeclaration | K::MethodDeclaration)
        ) {
            let question = match self.factory.node(node).data() {
                ts_ast::NodeDataRead::ParameterDeclaration(data) => data.question_token(),
                ts_ast::NodeDataRead::PropertyDeclaration(data) => data.postfix_token(),
                ts_ast::NodeDataRead::MethodDeclaration(data) => data.postfix_token(),
                _ => unreachable!("parsed declaration payload"),
            };
            if let Some(question) = question {
                let question = self.factory.node(question);
                if question.flags() & node_flags::REPARSED == 0
                    && question.kind() == K::QuestionToken
                {
                    let loc = question.range();
                    drop(question);
                    self.js_error_at_range(
                        loc,
                        diagnostics::The_0_modifier_can_only_be_used_in_TypeScript_files,
                        vec![JsString::from_bytes(b"?".as_slice())],
                    );
                }
            }
        }
        match kind.known() {
            Some(
                K::Parameter
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionExpression
                | K::FunctionDeclaration
                | K::ArrowFunction
                | K::VariableDeclaration
                | K::IndexSignature,
            ) => {
                let function_like = !matches!(
                    kind.known(),
                    Some(K::Parameter | K::PropertyDeclaration | K::VariableDeclaration)
                );
                if function_like && self.js_syntax_body(node).is_none() {
                    self.js_error_at_range(
                        loc,
                        diagnostics::Signature_declarations_can_only_be_used_in_TypeScript_files,
                        vec![],
                    );
                } else if let Some(annotation) = self.js_syntax_annotation(node) {
                    let annotation = self.factory.node(annotation);
                    if annotation.flags() & node_flags::REPARSED == 0 {
                        let loc = annotation.range();
                        drop(annotation);
                        self.js_error_at_range(
                            loc,
                            diagnostics::Type_annotations_can_only_be_used_in_TypeScript_files,
                            vec![],
                        );
                    }
                }
            }
            Some(K::ImportDeclaration) => {
                let clause = self
                    .factory
                    .node(node)
                    .data_source()
                    .as_import_declaration()
                    .expect("import payload")
                    .import_clause();
                if clause.is_some_and(|clause| {
                    self.factory
                        .node(clause)
                        .data_source()
                        .as_import_clause()
                        .expect("import clause payload")
                        .phase_modifier()
                        == K::TypeKeyword
                }) {
                    self.js_error_at_range(
                        loc,
                        diagnostics::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        vec![JsString::from_bytes(b"import type".as_slice())],
                    );
                }
            }
            Some(K::ExportDeclaration | K::ImportSpecifier | K::ExportSpecifier) => {
                let (type_only, message) = match self.factory.node(node).data() {
                    ts_ast::NodeDataRead::ExportDeclaration(data) => {
                        (data.is_type_only(), b"export type".as_slice())
                    }
                    ts_ast::NodeDataRead::ImportSpecifier(data) => {
                        (data.is_type_only(), b"import...type".as_slice())
                    }
                    ts_ast::NodeDataRead::ExportSpecifier(data) => {
                        (data.is_type_only(), b"export...type".as_slice())
                    }
                    _ => unreachable!("parsed import/export payload"),
                };
                if type_only {
                    self.js_error_at_range(
                        loc,
                        diagnostics::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        vec![JsString::from_bytes(message)],
                    );
                }
            }
            Some(K::ImportEqualsDeclaration) => self.js_error_at_range(
                loc,
                diagnostics::X_import_can_only_be_used_in_TypeScript_files,
                vec![],
            ),
            Some(K::ExportAssignment) => {
                if self
                    .factory
                    .node(node)
                    .data_source()
                    .as_export_assignment()
                    .expect("export assignment payload")
                    .is_export_equals()
                {
                    self.js_error_at_range(
                        loc,
                        diagnostics::X_export_can_only_be_used_in_TypeScript_files,
                        vec![],
                    );
                }
            }
            Some(K::HeritageClause) => {
                if self
                    .factory
                    .node(node)
                    .data_source()
                    .as_heritage_clause()
                    .expect("heritage clause payload")
                    .token()
                    == K::ImplementsKeyword
                {
                    self.js_error_at_range(
                        loc,
                        diagnostics::X_implements_clauses_can_only_be_used_in_TypeScript_files,
                        vec![],
                    );
                }
            }
            Some(
                K::InterfaceDeclaration
                | K::ModuleDeclaration
                | K::EnumDeclaration
                | K::TypeAliasDeclaration,
            ) => {
                let name = self
                    .factory
                    .node(node)
                    .data()
                    .declaration_name_generated()
                    .expect("parsed declaration name");
                let loc = self.factory.node(name).range();
                if kind == K::TypeAliasDeclaration {
                    self.js_error_at_range(
                        loc,
                        diagnostics::Type_aliases_can_only_be_used_in_TypeScript_files,
                        vec![],
                    );
                } else {
                    let text = match kind.known() {
                        Some(K::InterfaceDeclaration) => {
                            JsString::from_bytes(b"interface".as_slice())
                        }
                        Some(K::EnumDeclaration) => JsString::from_bytes(b"enum".as_slice()),
                        _ => {
                            let keyword = self
                                .factory
                                .node(node)
                                .data_source()
                                .as_module_declaration()
                                .expect("module payload")
                                .keyword();
                            keyword
                                .known()
                                .map_or_else(JsString::default, crate::tokens::token_text)
                        }
                    };
                    self.js_error_at_range(
                        loc,
                        diagnostics::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        vec![text],
                    );
                }
            }
            Some(K::NonNullExpression) => self.js_error_at_range(
                loc,
                diagnostics::Non_null_assertions_can_only_be_used_in_TypeScript_files,
                vec![],
            ),
            Some(K::AsExpression | K::SatisfiesExpression) => {
                let annotation = self
                    .js_syntax_annotation(node)
                    .expect("parsed assertion type");
                let message = if kind == K::AsExpression {
                    diagnostics::Type_assertion_expressions_can_only_be_used_in_TypeScript_files
                } else {
                    diagnostics::Type_satisfaction_expressions_can_only_be_used_in_TypeScript_files
                };
                self.js_error_at_range(self.factory.node(annotation).range(), message, vec![]);
            }
            _ => {}
        }
        self.check_js_decorator_syntax(node);
        match kind.known() {
            Some(
                K::ClassDeclaration
                | K::ClassExpression
                | K::MethodDeclaration
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionExpression
                | K::FunctionDeclaration
                | K::ArrowFunction
                | K::VariableStatement
                | K::PropertyDeclaration,
            ) => {
                if let Some(list) = self.js_syntax_type_parameters(node) {
                    if self.js_list_has_non_reparsed_node(list) {
                        self.js_error_at_range(self.factory.read_list(list).loc(), diagnostics::Type_parameter_declarations_can_only_be_used_in_TypeScript_files, vec![]);
                    }
                }
                if let Some(list) = self.node_modifiers(node) {
                    let nodes = self.factory.read_list(list).nodes();
                    for i in 0..nodes.len() {
                        let modifier = self.factory.node(
                            self.factory
                                .read_nodes(nodes)
                                .at(i)
                                .expect("parsed modifier"),
                        );
                        if modifier.flags() & node_flags::REPARSED == 0
                            && modifier.kind() != K::Decorator
                            && ts_ast::modifier_to_flag(modifier.kind())
                                & modifier_flags::JAVA_SCRIPT
                                == 0
                        {
                            let loc = modifier.range();
                            let text = modifier
                                .kind()
                                .known()
                                .map_or_else(JsString::default, crate::tokens::token_text);
                            drop(modifier);
                            self.js_error_at_range(
                                loc,
                                diagnostics::The_0_modifier_can_only_be_used_in_TypeScript_files,
                                vec![text],
                            );
                        }
                    }
                }
            }
            Some(K::Parameter) => {
                if let Some(list) = self.node_modifiers(node) {
                    let nodes = self.factory.read_list(list).nodes();
                    if self.factory.read_nodes(nodes).iter().any(|id| {
                        ts_ast::is_modifier_kind(
                            self.factory.node(id.expect("parsed modifier")).kind(),
                        )
                    }) {
                        self.js_error_at_range(
                            self.factory.read_list(list).loc(),
                            diagnostics::Parameter_modifiers_can_only_be_used_in_TypeScript_files,
                            vec![],
                        );
                    }
                }
            }
            Some(
                K::CallExpression
                | K::NewExpression
                | K::ExpressionWithTypeArguments
                | K::JsxSelfClosingElement
                | K::JsxOpeningElement
                | K::TaggedTemplateExpression,
            ) => {
                if let Some(list) = self.js_syntax_type_arguments(node) {
                    if self.js_list_has_non_reparsed_node(list) {
                        self.js_error_at_range(
                            self.factory.read_list(list).loc(),
                            diagnostics::Type_arguments_can_only_be_used_in_TypeScript_files,
                            vec![],
                        );
                    }
                }
            }
            _ => {}
        }
        node
    }

    fn js_find_modifier(&self, nodes: NodeSlice, start: usize, kind: K) -> Option<usize> {
        self.factory
            .read_nodes(nodes.slice(start..nodes.len()).expect("modifier range"))
            .iter()
            .position(|id| self.factory.node(id.expect("parsed modifier")).kind() == kind)
            .map(|index| start + index)
    }
    fn js_list_has_non_reparsed_node(&self, list: NodeListId) -> bool {
        self.factory
            .read_nodes(self.factory.read_list(list).nodes())
            .iter()
            .any(|id| {
                self.factory.node(id.expect("parsed list element")).flags() & node_flags::REPARSED
                    == 0
            })
    }
    fn js_syntax_body(&self, node: NodeId) -> Option<NodeId> {
        use ts_ast::NodeDataRead as D;
        match self.factory.node(node).data() {
            D::MethodDeclaration(d) => d.body(),
            D::ConstructorDeclaration(d) => d.body(),
            D::GetAccessorDeclaration(d) => d.body(),
            D::SetAccessorDeclaration(d) => d.body(),
            D::FunctionDeclaration(d) => d.body(),
            D::FunctionExpression(d) => d.body(),
            D::ArrowFunction(d) => d.body(),
            _ => None,
        }
    }
    fn js_syntax_annotation(&self, node: NodeId) -> Option<NodeId> {
        use ts_ast::NodeDataRead as D;
        match self.factory.node(node).data() {
            D::ParameterDeclaration(d) => d.r#type(),
            D::PropertyDeclaration(d) => d.r#type(),
            D::MethodDeclaration(d) => d.r#type(),
            D::MethodSignatureDeclaration(d) => d.r#type(),
            D::ConstructorDeclaration(d) => d.r#type(),
            D::GetAccessorDeclaration(d) => d.r#type(),
            D::SetAccessorDeclaration(d) => d.r#type(),
            D::FunctionExpression(d) => d.r#type(),
            D::FunctionDeclaration(d) => d.r#type(),
            D::ArrowFunction(d) => d.r#type(),
            D::VariableDeclaration(d) => d.r#type(),
            D::IndexSignatureDeclaration(d) => d.r#type(),
            D::AsExpression(d) => d.r#type(),
            D::SatisfiesExpression(d) => d.r#type(),
            _ => None,
        }
    }
    fn js_syntax_type_parameters(&self, node: NodeId) -> Option<NodeListId> {
        use ts_ast::NodeDataRead as D;
        match self.factory.node(node).data() {
            D::ClassDeclaration(d) => d.type_parameters(),
            D::ClassExpression(d) => d.type_parameters(),
            D::MethodDeclaration(d) => d.type_parameters(),
            D::ConstructorDeclaration(d) => d.type_parameters(),
            D::GetAccessorDeclaration(d) => d.type_parameters(),
            D::SetAccessorDeclaration(d) => d.type_parameters(),
            D::FunctionExpression(d) => d.type_parameters(),
            D::FunctionDeclaration(d) => d.type_parameters(),
            D::ArrowFunction(d) => d.type_parameters(),
            _ => None,
        }
    }
    fn js_syntax_type_arguments(&self, node: NodeId) -> Option<NodeListId> {
        use ts_ast::NodeDataRead as D;
        match self.factory.node(node).data() {
            D::CallExpression(d) => d.type_arguments(),
            D::NewExpression(d) => d.type_arguments(),
            D::ExpressionWithTypeArguments(d) => d.type_arguments(),
            D::JsxSelfClosingElement(d) => d.type_arguments(),
            D::JsxOpeningElement(d) => d.type_arguments(),
            D::TaggedTemplateExpression(d) => d.type_arguments(),
            _ => None,
        }
    }
}

/// port: tsc/internal/ast/utilities.go:CanHaveIllegalDecorators
fn can_have_illegal_decorators(kind: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::FunctionDeclaration
                | K::Constructor
                | K::IndexSignature
                | K::ClassStaticBlockDeclaration
                | K::MissingDeclaration
                | K::VariableStatement
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::EnumDeclaration
                | K::ModuleDeclaration
                | K::ImportEqualsDeclaration
                | K::ImportDeclaration
                | K::JSImportDeclaration
                | K::NamespaceExportDeclaration
                | K::ExportDeclaration
                | K::ExportAssignment
        )
    )
}
/// port: tsc/internal/ast/utilities.go:CanHaveDecorators
fn can_have_decorators(kind: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::Parameter
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassExpression
                | K::ClassDeclaration
        )
    )
}
