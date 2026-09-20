//! One semantic getter inventory for owned/checked nodes and scoped typed reads.
//! Storage adapters supply field/shape access; selection and panic order are shared.

macro_rules! reads {
    ($node_id:ty, $list_id:ty, $field:ident; $visibility:vis,) => {
        /// port: tsc/internal/ast/ast.go:Node.RawText
        $visibility fn raw_text(&self) -> &[u8] {
            match self.kind().known() {
                Some($crate::SyntaxKind::TemplateHead) => $field!(self, TemplateHead, as_template_head, raw_text),
                Some($crate::SyntaxKind::TemplateMiddle) => $field!(self, TemplateMiddle, as_template_middle, raw_text),
                Some($crate::SyntaxKind::TemplateTail) => $field!(self, TemplateTail, as_template_tail, raw_text),
                _ => panic!("Unhandled case in Node.RawText: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ParameterList
        $visibility fn parameter_list(&self) -> Option<$list_id> {
            self.function_fields()
                .expect("runtime error: invalid memory address or nil pointer dereference")
                .1
        }
        /// port: tsc/internal/ast/ast.go:Node.Expression
        $visibility fn expression(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::PropertyAccessExpression) => $field!(
                    self,
                    PropertyAccessExpression,
                    as_property_access_expression,
                    expression
                ),
                Some($crate::SyntaxKind::ElementAccessExpression) => $field!(
                    self,
                    ElementAccessExpression,
                    as_element_access_expression,
                    expression
                ),
                Some($crate::SyntaxKind::ParenthesizedExpression) => $field!(
                    self,
                    ParenthesizedExpression,
                    as_parenthesized_expression,
                    expression
                ),
                Some($crate::SyntaxKind::CallExpression) => $field!(self, CallExpression, as_call_expression, expression),
                Some($crate::SyntaxKind::NewExpression) => $field!(self, NewExpression, as_new_expression, expression),
                Some($crate::SyntaxKind::ExpressionWithTypeArguments) => {
                    $field!(
                        self,
                        ExpressionWithTypeArguments,
                        as_expression_with_type_arguments,
                        expression
                    )
                }
                Some($crate::SyntaxKind::ComputedPropertyName) => $field!(
                    self,
                    ComputedPropertyName,
                    as_computed_property_name,
                    expression
                ),
                Some($crate::SyntaxKind::NonNullExpression) => {
                    $field!(self, NonNullExpression, as_non_null_expression, expression)
                }
                Some($crate::SyntaxKind::TypeAssertionExpression) => {
                    $field!(self, TypeAssertion, as_type_assertion, expression)
                }
                Some($crate::SyntaxKind::AsExpression) => $field!(self, AsExpression, as_as_expression, expression),
                Some($crate::SyntaxKind::SatisfiesExpression) => $field!(
                    self,
                    SatisfiesExpression,
                    as_satisfies_expression,
                    expression
                ),
                Some($crate::SyntaxKind::TypeOfExpression) => {
                    $field!(self, TypeOfExpression, as_type_of_expression, expression)
                }
                Some($crate::SyntaxKind::SpreadAssignment) => {
                    $field!(self, SpreadAssignment, as_spread_assignment, expression)
                }
                Some($crate::SyntaxKind::SpreadElement) => $field!(self, SpreadElement, as_spread_element, expression),
                Some($crate::SyntaxKind::TemplateSpan) => $field!(self, TemplateSpan, as_template_span, expression),
                Some($crate::SyntaxKind::DeleteExpression) => {
                    $field!(self, DeleteExpression, as_delete_expression, expression)
                }
                Some($crate::SyntaxKind::VoidExpression) => $field!(self, VoidExpression, as_void_expression, expression),
                Some($crate::SyntaxKind::AwaitExpression) => {
                    $field!(self, AwaitExpression, as_await_expression, expression)
                }
                Some($crate::SyntaxKind::YieldExpression) => {
                    $field!(self, YieldExpression, as_yield_expression, expression)
                }
                Some($crate::SyntaxKind::PartiallyEmittedExpression) => {
                    $field!(
                        self,
                        PartiallyEmittedExpression,
                        as_partially_emitted_expression,
                        expression
                    )
                }
                Some($crate::SyntaxKind::IfStatement) => $field!(self, IfStatement, as_if_statement, expression),
                Some($crate::SyntaxKind::DoStatement) => $field!(self, DoStatement, as_do_statement, expression),
                Some($crate::SyntaxKind::WhileStatement) => $field!(self, WhileStatement, as_while_statement, expression),
                Some($crate::SyntaxKind::WithStatement) => $field!(self, WithStatement, as_with_statement, expression),
                Some($crate::SyntaxKind::ForInStatement | $crate::SyntaxKind::ForOfStatement) => {
                    $field!(
                        self,
                        ForInOrOfStatement,
                        as_for_in_or_of_statement,
                        expression
                    )
                }
                Some($crate::SyntaxKind::SwitchStatement) => {
                    $field!(self, SwitchStatement, as_switch_statement, expression)
                }
                Some($crate::SyntaxKind::CaseClause) => $field!(
                    self,
                    CaseOrDefaultClause,
                    as_case_or_default_clause,
                    expression
                ),
                Some($crate::SyntaxKind::ExpressionStatement) => $field!(
                    self,
                    ExpressionStatement,
                    as_expression_statement,
                    expression
                ),
                Some($crate::SyntaxKind::ReturnStatement) => {
                    $field!(self, ReturnStatement, as_return_statement, expression)
                }
                Some($crate::SyntaxKind::ThrowStatement) => $field!(self, ThrowStatement, as_throw_statement, expression),
                Some($crate::SyntaxKind::ExternalModuleReference) => $field!(
                    self,
                    ExternalModuleReference,
                    as_external_module_reference,
                    expression
                ),
                Some($crate::SyntaxKind::ExportAssignment) => {
                    $field!(self, ExportAssignment, as_export_assignment, expression)
                }
                Some($crate::SyntaxKind::Decorator) => $field!(self, Decorator, as_decorator, expression),
                Some($crate::SyntaxKind::JsxExpression) => $field!(self, JsxExpression, as_jsx_expression, expression),
                Some($crate::SyntaxKind::JsxSpreadAttribute) => $field!(
                    self,
                    JsxSpreadAttribute,
                    as_jsx_spread_attribute,
                    expression
                ),
                _ => panic!("Unhandled case in Node.Expression: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Type
        $visibility fn type_node(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::VariableDeclaration) => {
                    $field!(self, VariableDeclaration, as_variable_declaration, r#type)
                }
                Some($crate::SyntaxKind::Parameter) => {
                    $field!(self, ParameterDeclaration, as_parameter_declaration, r#type)
                }
                Some($crate::SyntaxKind::PropertySignature) => $field!(
                    self,
                    PropertySignatureDeclaration,
                    as_property_signature_declaration,
                    r#type
                ),
                Some($crate::SyntaxKind::PropertyDeclaration) => {
                    $field!(self, PropertyDeclaration, as_property_declaration, r#type)
                }
                Some($crate::SyntaxKind::PropertyAssignment) => {
                    $field!(self, PropertyAssignment, as_property_assignment, r#type)
                }
                Some($crate::SyntaxKind::ShorthandPropertyAssignment) => {
                    $field!(
                        self,
                        ShorthandPropertyAssignment,
                        as_shorthand_property_assignment,
                        r#type
                    )
                }
                Some($crate::SyntaxKind::TypePredicate) => {
                    $field!(self, TypePredicateNode, as_type_predicate_node, r#type)
                }
                Some($crate::SyntaxKind::ParenthesizedType) => $field!(
                    self,
                    ParenthesizedTypeNode,
                    as_parenthesized_type_node,
                    r#type
                ),
                Some($crate::SyntaxKind::TypeOperator) => $field!(self, TypeOperatorNode, as_type_operator_node, r#type),
                Some($crate::SyntaxKind::MappedType) => $field!(self, MappedTypeNode, as_mapped_type_node, r#type),
                Some($crate::SyntaxKind::TypeAssertionExpression) => {
                    $field!(self, TypeAssertion, as_type_assertion, r#type)
                }
                Some($crate::SyntaxKind::AsExpression) => $field!(self, AsExpression, as_as_expression, r#type),
                Some($crate::SyntaxKind::SatisfiesExpression) => {
                    $field!(self, SatisfiesExpression, as_satisfies_expression, r#type)
                }
                Some($crate::SyntaxKind::TypeAliasDeclaration | $crate::SyntaxKind::JSTypeAliasDeclaration) => {
                    $field!(
                        self,
                        TypeAliasDeclaration,
                        as_type_alias_declaration,
                        r#type
                    )
                }
                Some($crate::SyntaxKind::NamedTupleMember) => {
                    $field!(self, NamedTupleMember, as_named_tuple_member, r#type)
                }
                Some($crate::SyntaxKind::OptionalType) => $field!(self, OptionalTypeNode, as_optional_type_node, r#type),
                Some($crate::SyntaxKind::RestType) => $field!(self, RestTypeNode, as_rest_type_node, r#type),
                Some($crate::SyntaxKind::TemplateLiteralTypeSpan) => $field!(
                    self,
                    TemplateLiteralTypeSpan,
                    as_template_literal_type_span,
                    r#type
                ),
                Some($crate::SyntaxKind::JSDocTypeExpression) => {
                    $field!(self, JSDocTypeExpression, as_js_doc_type_expression, r#type)
                }
                Some($crate::SyntaxKind::JSDocParameterTag | $crate::SyntaxKind::JSDocPropertyTag) => {
                    $field!(
                        self,
                        JSDocParameterOrPropertyTag,
                        as_js_doc_parameter_or_property_tag,
                        type_expression
                    )
                }
                Some($crate::SyntaxKind::JSDocNullableType) => {
                    $field!(self, JSDocNullableType, as_js_doc_nullable_type, r#type)
                }
                Some($crate::SyntaxKind::JSDocNonNullableType) => $field!(
                    self,
                    JSDocNonNullableType,
                    as_js_doc_non_nullable_type,
                    r#type
                ),
                Some($crate::SyntaxKind::JSDocOptionalType) => {
                    $field!(self, JSDocOptionalType, as_js_doc_optional_type, r#type)
                }
                Some($crate::SyntaxKind::ExportAssignment) => {
                    $field!(self, ExportAssignment, as_export_assignment, r#type)
                }
                Some($crate::SyntaxKind::BinaryExpression) => {
                    $field!(self, BinaryExpression, as_binary_expression, r#type)
                }
                _ => self.function_fields().and_then(|fields| fields.2),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Initializer
        $visibility fn initializer(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::VariableDeclaration) => $field!(
                    self,
                    VariableDeclaration,
                    as_variable_declaration,
                    initializer
                ),
                Some($crate::SyntaxKind::Parameter) => $field!(
                    self,
                    ParameterDeclaration,
                    as_parameter_declaration,
                    initializer
                ),
                Some($crate::SyntaxKind::BindingElement) => {
                    $field!(self, BindingElement, as_binding_element, initializer)
                }
                Some($crate::SyntaxKind::PropertyDeclaration) => $field!(
                    self,
                    PropertyDeclaration,
                    as_property_declaration,
                    initializer
                ),
                Some($crate::SyntaxKind::PropertySignature) => $field!(
                    self,
                    PropertySignatureDeclaration,
                    as_property_signature_declaration,
                    initializer
                ),
                Some($crate::SyntaxKind::PropertyAssignment) => $field!(
                    self,
                    PropertyAssignment,
                    as_property_assignment,
                    initializer
                ),
                Some($crate::SyntaxKind::EnumMember) => $field!(self, EnumMember, as_enum_member, initializer),
                Some($crate::SyntaxKind::ForStatement) => $field!(self, ForStatement, as_for_statement, initializer),
                Some($crate::SyntaxKind::ForInStatement | $crate::SyntaxKind::ForOfStatement) => {
                    $field!(
                        self,
                        ForInOrOfStatement,
                        as_for_in_or_of_statement,
                        initializer
                    )
                }
                Some($crate::SyntaxKind::JsxAttribute) => $field!(self, JsxAttribute, as_jsx_attribute, initializer),
                _ => panic!("Unhandled case in Node.Initializer"),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.TagName
        $visibility fn tag_name(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JsxOpeningElement) => {
                    $field!(self, JsxOpeningElement, as_jsx_opening_element, tag_name)
                }
                Some($crate::SyntaxKind::JsxClosingElement) => {
                    $field!(self, JsxClosingElement, as_jsx_closing_element, tag_name)
                }
                Some($crate::SyntaxKind::JsxSelfClosingElement) => $field!(
                    self,
                    JsxSelfClosingElement,
                    as_jsx_self_closing_element,
                    tag_name
                ),
                Some($crate::SyntaxKind::JSDocUnknownTag) => {
                    $field!(self, JSDocUnknownTag, as_js_doc_unknown_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocAugmentsTag) => {
                    $field!(self, JSDocAugmentsTag, as_js_doc_augments_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocImplementsTag) => {
                    $field!(self, JSDocImplementsTag, as_js_doc_implements_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocDeprecatedTag) => {
                    $field!(self, JSDocDeprecatedTag, as_js_doc_deprecated_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocPublicTag) => $field!(self, JSDocPublicTag, as_js_doc_public_tag, tag_name),
                Some($crate::SyntaxKind::JSDocPrivateTag) => {
                    $field!(self, JSDocPrivateTag, as_js_doc_private_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocProtectedTag) => {
                    $field!(self, JSDocProtectedTag, as_js_doc_protected_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocReadonlyTag) => {
                    $field!(self, JSDocReadonlyTag, as_js_doc_readonly_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocOverrideTag) => {
                    $field!(self, JSDocOverrideTag, as_js_doc_override_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocCallbackTag) => {
                    $field!(self, JSDocCallbackTag, as_js_doc_callback_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocOverloadTag) => {
                    $field!(self, JSDocOverloadTag, as_js_doc_overload_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocParameterTag | $crate::SyntaxKind::JSDocPropertyTag) => {
                    $field!(
                        self,
                        JSDocParameterOrPropertyTag,
                        as_js_doc_parameter_or_property_tag,
                        tag_name
                    )
                }
                Some($crate::SyntaxKind::JSDocReturnTag) => $field!(self, JSDocReturnTag, as_js_doc_return_tag, tag_name),
                Some($crate::SyntaxKind::JSDocThisTag) => $field!(self, JSDocThisTag, as_js_doc_this_tag, tag_name),
                Some($crate::SyntaxKind::JSDocTypeTag) => $field!(self, JSDocTypeTag, as_js_doc_type_tag, tag_name),
                Some($crate::SyntaxKind::JSDocTemplateTag) => {
                    $field!(self, JSDocTemplateTag, as_js_doc_template_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocTypedefTag) => {
                    $field!(self, JSDocTypedefTag, as_js_doc_typedef_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocSeeTag) => $field!(self, JSDocSeeTag, as_js_doc_see_tag, tag_name),
                Some($crate::SyntaxKind::JSDocSatisfiesTag) => {
                    $field!(self, JSDocSatisfiesTag, as_js_doc_satisfies_tag, tag_name)
                }
                Some($crate::SyntaxKind::JSDocThrowsTag) => $field!(self, JSDocThrowsTag, as_js_doc_throws_tag, tag_name),
                Some($crate::SyntaxKind::JSDocImportTag) => $field!(self, JSDocImportTag, as_js_doc_import_tag, tag_name),
                _ => panic!("Unhandled case in Node.TagName: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.PropertyName
        $visibility fn property_name(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ImportSpecifier) => {
                    $field!(self, ImportSpecifier, as_import_specifier, property_name)
                }
                Some($crate::SyntaxKind::ExportSpecifier) => {
                    $field!(self, ExportSpecifier, as_export_specifier, property_name)
                }
                Some($crate::SyntaxKind::BindingElement) => {
                    $field!(self, BindingElement, as_binding_element, property_name)
                }
                _ => None,
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Label
        $visibility fn label(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::LabeledStatement) => {
                    $field!(self, LabeledStatement, as_labeled_statement, label)
                }
                Some($crate::SyntaxKind::BreakStatement) => $field!(self, BreakStatement, as_break_statement, label),
                Some($crate::SyntaxKind::ContinueStatement) => {
                    $field!(self, ContinueStatement, as_continue_statement, label)
                }
                _ => panic!("Unhandled case in Node.Label: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Attributes
        $visibility fn attributes(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JsxOpeningElement) => {
                    $field!(self, JsxOpeningElement, as_jsx_opening_element, attributes)
                }
                Some($crate::SyntaxKind::JsxSelfClosingElement) => $field!(
                    self,
                    JsxSelfClosingElement,
                    as_jsx_self_closing_element,
                    attributes
                ),
                Some($crate::SyntaxKind::ModuleDeclaration) => {
                    $field!(self, ModuleDeclaration, as_module_declaration, attributes)
                }
                _ => panic!("Unhandled case in Node.Attributes: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ModuleSpecifier
        $visibility fn module_specifier(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ImportDeclaration | $crate::SyntaxKind::JSImportDeclaration) => {
                    $field!(
                        self,
                        ImportDeclaration,
                        as_import_declaration,
                        module_specifier
                    )
                }
                Some($crate::SyntaxKind::ExportDeclaration) => $field!(
                    self,
                    ExportDeclaration,
                    as_export_declaration,
                    module_specifier
                ),
                Some($crate::SyntaxKind::JSDocImportTag) => {
                    $field!(self, JSDocImportTag, as_js_doc_import_tag, module_specifier)
                }
                _ => panic!("Unhandled case in Node.ModuleSpecifier: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ImportClause
        $visibility fn import_clause(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ImportDeclaration | $crate::SyntaxKind::JSImportDeclaration) => {
                    $field!(
                        self,
                        ImportDeclaration,
                        as_import_declaration,
                        import_clause
                    )
                }
                Some($crate::SyntaxKind::JSDocImportTag) => {
                    $field!(self, JSDocImportTag, as_js_doc_import_tag, import_clause)
                }
                _ => panic!("Unhandled case in Node.ImportClause: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Statement
        $visibility fn statement(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::DoStatement) => $field!(self, DoStatement, as_do_statement, statement),
                Some($crate::SyntaxKind::WhileStatement) => $field!(self, WhileStatement, as_while_statement, statement),
                Some($crate::SyntaxKind::ForStatement) => $field!(self, ForStatement, as_for_statement, statement),
                Some($crate::SyntaxKind::ForInStatement | $crate::SyntaxKind::ForOfStatement) => {
                    $field!(
                        self,
                        ForInOrOfStatement,
                        as_for_in_or_of_statement,
                        statement
                    )
                }
                Some($crate::SyntaxKind::WithStatement) => $field!(self, WithStatement, as_with_statement, statement),
                Some($crate::SyntaxKind::LabeledStatement) => {
                    $field!(self, LabeledStatement, as_labeled_statement, statement)
                }
                _ => panic!("Unhandled case in Node.Statement: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.PostfixToken
        $visibility fn postfix_token(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::MethodDeclaration) => $field!(
                    self,
                    MethodDeclaration,
                    as_method_declaration,
                    postfix_token
                ),
                Some($crate::SyntaxKind::ShorthandPropertyAssignment) => {
                    $field!(
                        self,
                        ShorthandPropertyAssignment,
                        as_shorthand_property_assignment,
                        postfix_token
                    )
                }
                Some($crate::SyntaxKind::MethodSignature) => $field!(
                    self,
                    MethodSignatureDeclaration,
                    as_method_signature_declaration,
                    postfix_token
                ),
                Some($crate::SyntaxKind::PropertySignature) => $field!(
                    self,
                    PropertySignatureDeclaration,
                    as_property_signature_declaration,
                    postfix_token
                ),
                Some($crate::SyntaxKind::PropertyAssignment) => $field!(
                    self,
                    PropertyAssignment,
                    as_property_assignment,
                    postfix_token
                ),
                Some($crate::SyntaxKind::PropertyDeclaration) => $field!(
                    self,
                    PropertyDeclaration,
                    as_property_declaration,
                    postfix_token
                ),
                Some($crate::SyntaxKind::EnumMember) => $field!(self, EnumMember, as_enum_member, postfix_token),
                Some($crate::SyntaxKind::GetAccessor) => $field!(
                    self,
                    GetAccessorDeclaration,
                    as_get_accessor_declaration,
                    postfix_token
                ),
                Some($crate::SyntaxKind::SetAccessor) => $field!(
                    self,
                    SetAccessorDeclaration,
                    as_set_accessor_declaration,
                    postfix_token
                ),
                _ => None,
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.QuestionDotToken
        $visibility fn question_dot_token(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ElementAccessExpression) => {
                    $field!(
                        self,
                        ElementAccessExpression,
                        as_element_access_expression,
                        question_dot_token
                    )
                }
                Some($crate::SyntaxKind::PropertyAccessExpression) => {
                    $field!(
                        self,
                        PropertyAccessExpression,
                        as_property_access_expression,
                        question_dot_token
                    )
                }
                Some($crate::SyntaxKind::CallExpression) => {
                    $field!(self, CallExpression, as_call_expression, question_dot_token)
                }
                Some($crate::SyntaxKind::TaggedTemplateExpression) => {
                    $field!(
                        self,
                        TaggedTemplateExpression,
                        as_tagged_template_expression,
                        question_dot_token
                    )
                }
                _ => panic!("Unhandled case in Node.QuestionDotToken: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.TypeExpression
        $visibility fn type_expression(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JSDocParameterTag | $crate::SyntaxKind::JSDocPropertyTag) => {
                    $field!(
                        self,
                        JSDocParameterOrPropertyTag,
                        as_js_doc_parameter_or_property_tag,
                        type_expression
                    )
                }
                Some($crate::SyntaxKind::JSDocReturnTag) => {
                    $field!(self, JSDocReturnTag, as_js_doc_return_tag, type_expression)
                }
                Some($crate::SyntaxKind::JSDocTypeTag) => {
                    $field!(self, JSDocTypeTag, as_js_doc_type_tag, type_expression)
                }
                Some($crate::SyntaxKind::JSDocTypedefTag) => $field!(
                    self,
                    JSDocTypedefTag,
                    as_js_doc_typedef_tag,
                    type_expression
                ),
                Some($crate::SyntaxKind::JSDocCallbackTag) => $field!(
                    self,
                    JSDocCallbackTag,
                    as_js_doc_callback_tag,
                    type_expression
                ),
                Some($crate::SyntaxKind::JSDocSatisfiesTag) => $field!(
                    self,
                    JSDocSatisfiesTag,
                    as_js_doc_satisfies_tag,
                    type_expression
                ),
                Some($crate::SyntaxKind::JSDocThrowsTag) => {
                    $field!(self, JSDocThrowsTag, as_js_doc_throws_tag, type_expression)
                }
                _ => panic!("Unhandled case in Node.TypeExpression: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ClassName
        $visibility fn class_name(&self) -> Option<$node_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JSDocAugmentsTag) => {
                    $field!(self, JSDocAugmentsTag, as_js_doc_augments_tag, class_name)
                }
                Some($crate::SyntaxKind::JSDocImplementsTag) => $field!(
                    self,
                    JSDocImplementsTag,
                    as_js_doc_implements_tag,
                    class_name
                ),
                _ => panic!("Unhandled case in Node.ClassName: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ArgumentList
        $visibility fn argument_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::CallExpression) => $field!(self, CallExpression, as_call_expression, arguments),
                Some($crate::SyntaxKind::NewExpression) => $field!(self, NewExpression, as_new_expression, arguments),
                _ => panic!("Unhandled case in Node.Arguments: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.TypeArgumentList
        $visibility fn type_argument_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::CallExpression) => {
                    $field!(self, CallExpression, as_call_expression, type_arguments)
                }
                Some($crate::SyntaxKind::NewExpression) => {
                    $field!(self, NewExpression, as_new_expression, type_arguments)
                }
                Some($crate::SyntaxKind::TaggedTemplateExpression) => {
                    $field!(
                        self,
                        TaggedTemplateExpression,
                        as_tagged_template_expression,
                        type_arguments
                    )
                }
                Some($crate::SyntaxKind::TypeReference) => $field!(
                    self,
                    TypeReferenceNode,
                    as_type_reference_node,
                    type_arguments
                ),
                Some($crate::SyntaxKind::ExpressionWithTypeArguments) => {
                    $field!(
                        self,
                        ExpressionWithTypeArguments,
                        as_expression_with_type_arguments,
                        type_arguments
                    )
                }
                Some($crate::SyntaxKind::ImportType) => {
                    $field!(self, ImportTypeNode, as_import_type_node, type_arguments)
                }
                Some($crate::SyntaxKind::TypeQuery) => $field!(self, TypeQueryNode, as_type_query_node, type_arguments),
                Some($crate::SyntaxKind::JsxOpeningElement) => $field!(
                    self,
                    JsxOpeningElement,
                    as_jsx_opening_element,
                    type_arguments
                ),
                Some($crate::SyntaxKind::JsxSelfClosingElement) => $field!(
                    self,
                    JsxSelfClosingElement,
                    as_jsx_self_closing_element,
                    type_arguments
                ),
                _ => panic!("Unhandled case in Node.TypeArguments"),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.TypeParameterList
        $visibility fn type_parameter_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ClassDeclaration) => $field!(
                    self,
                    ClassDeclaration,
                    as_class_declaration,
                    type_parameters
                ),
                Some($crate::SyntaxKind::ClassExpression) => {
                    $field!(self, ClassExpression, as_class_expression, type_parameters)
                }
                Some($crate::SyntaxKind::InterfaceDeclaration) => $field!(
                    self,
                    InterfaceDeclaration,
                    as_interface_declaration,
                    type_parameters
                ),
                Some($crate::SyntaxKind::TypeAliasDeclaration | $crate::SyntaxKind::JSTypeAliasDeclaration) => {
                    $field!(
                        self,
                        TypeAliasDeclaration,
                        as_type_alias_declaration,
                        type_parameters
                    )
                }
                Some($crate::SyntaxKind::JSDocTemplateTag) => $field!(
                    self,
                    JSDocTemplateTag,
                    as_js_doc_template_tag,
                    type_parameters
                ),
                _ => {
                    self.function_fields()
                        .expect("Unhandled case in Node.TypeParameterList")
                        .0
                }
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.MemberList
        $visibility fn member_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ClassDeclaration) => {
                    $field!(self, ClassDeclaration, as_class_declaration, members)
                }
                Some($crate::SyntaxKind::ClassExpression) => $field!(self, ClassExpression, as_class_expression, members),
                Some($crate::SyntaxKind::InterfaceDeclaration) => $field!(
                    self,
                    InterfaceDeclaration,
                    as_interface_declaration,
                    members
                ),
                Some($crate::SyntaxKind::EnumDeclaration) => $field!(self, EnumDeclaration, as_enum_declaration, members),
                Some($crate::SyntaxKind::TypeLiteral) => $field!(self, TypeLiteralNode, as_type_literal_node, members),
                Some($crate::SyntaxKind::MappedType) => $field!(self, MappedTypeNode, as_mapped_type_node, members),
                _ => panic!("Unhandled case in Node.MemberList: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.StatementList
        $visibility fn statement_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::SourceFile) => $field!(self, SourceFile, as_source_file, statements),
                Some($crate::SyntaxKind::Block) => $field!(self, Block, as_block, statements),
                Some($crate::SyntaxKind::ModuleBlock) => $field!(self, ModuleBlock, as_module_block, statements),
                Some($crate::SyntaxKind::CaseClause | $crate::SyntaxKind::DefaultClause) => {
                    $field!(
                        self,
                        CaseOrDefaultClause,
                        as_case_or_default_clause,
                        statements
                    )
                }
                _ => panic!("Unhandled case in Node.StatementList: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.CommentList
        $visibility fn comment_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JSDoc) => $field!(self, JSDoc, as_js_doc, comment),
                Some($crate::SyntaxKind::JSDocUnknownTag) => {
                    $field!(self, JSDocUnknownTag, as_js_doc_unknown_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocAugmentsTag) => {
                    $field!(self, JSDocAugmentsTag, as_js_doc_augments_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocImplementsTag) => {
                    $field!(self, JSDocImplementsTag, as_js_doc_implements_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocDeprecatedTag) => {
                    $field!(self, JSDocDeprecatedTag, as_js_doc_deprecated_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocPublicTag) => $field!(self, JSDocPublicTag, as_js_doc_public_tag, comment),
                Some($crate::SyntaxKind::JSDocPrivateTag) => {
                    $field!(self, JSDocPrivateTag, as_js_doc_private_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocProtectedTag) => {
                    $field!(self, JSDocProtectedTag, as_js_doc_protected_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocReadonlyTag) => {
                    $field!(self, JSDocReadonlyTag, as_js_doc_readonly_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocOverrideTag) => {
                    $field!(self, JSDocOverrideTag, as_js_doc_override_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocCallbackTag) => {
                    $field!(self, JSDocCallbackTag, as_js_doc_callback_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocOverloadTag) => {
                    $field!(self, JSDocOverloadTag, as_js_doc_overload_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocParameterTag | $crate::SyntaxKind::JSDocPropertyTag) => {
                    $field!(
                        self,
                        JSDocParameterOrPropertyTag,
                        as_js_doc_parameter_or_property_tag,
                        comment
                    )
                }
                Some($crate::SyntaxKind::JSDocReturnTag) => $field!(self, JSDocReturnTag, as_js_doc_return_tag, comment),
                Some($crate::SyntaxKind::JSDocThisTag) => $field!(self, JSDocThisTag, as_js_doc_this_tag, comment),
                Some($crate::SyntaxKind::JSDocTypeTag) => $field!(self, JSDocTypeTag, as_js_doc_type_tag, comment),
                Some($crate::SyntaxKind::JSDocTemplateTag) => {
                    $field!(self, JSDocTemplateTag, as_js_doc_template_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocTypedefTag) => {
                    $field!(self, JSDocTypedefTag, as_js_doc_typedef_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocSeeTag) => $field!(self, JSDocSeeTag, as_js_doc_see_tag, comment),
                Some($crate::SyntaxKind::JSDocSatisfiesTag) => {
                    $field!(self, JSDocSatisfiesTag, as_js_doc_satisfies_tag, comment)
                }
                Some($crate::SyntaxKind::JSDocThrowsTag) => $field!(self, JSDocThrowsTag, as_js_doc_throws_tag, comment),
                Some($crate::SyntaxKind::JSDocImportTag) => $field!(self, JSDocImportTag, as_js_doc_import_tag, comment),
                _ => panic!("Unhandled case in Node.CommentList: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.Children
        $visibility fn children_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::JsxElement) => $field!(self, JsxElement, as_jsx_element, children),
                Some($crate::SyntaxKind::JsxFragment) => $field!(self, JsxFragment, as_jsx_fragment, children),
                _ => panic!("Unhandled case in Node.Children: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.PropertyList
        $visibility fn property_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::ObjectLiteralExpression) => $field!(
                    self,
                    ObjectLiteralExpression,
                    as_object_literal_expression,
                    properties
                ),
                Some($crate::SyntaxKind::JsxAttributes) => $field!(self, JsxAttributes, as_jsx_attributes, properties),
                _ => panic!("Unhandled case in Node.PropertyList: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.ElementList
        $visibility fn element_list(&self) -> Option<$list_id> {
            match self.kind().known() {
                Some($crate::SyntaxKind::NamedImports) => $field!(self, NamedImports, as_named_imports, elements),
                Some($crate::SyntaxKind::NamedExports) => $field!(self, NamedExports, as_named_exports, elements),
                Some($crate::SyntaxKind::ObjectBindingPattern | $crate::SyntaxKind::ArrayBindingPattern) => {
                    $field!(self, BindingPattern, as_binding_pattern, elements)
                }
                Some($crate::SyntaxKind::ArrayLiteralExpression) => $field!(
                    self,
                    ArrayLiteralExpression,
                    as_array_literal_expression,
                    elements
                ),
                Some($crate::SyntaxKind::TupleType) => $field!(self, TupleTypeNode, as_tuple_type_node, elements),
                _ => panic!("Unhandled case in Node.ElementList: {}", self.kind()),
            }
        }
        /// port: tsc/internal/ast/ast.go:Node.KindString
        $visibility fn kind_string(&self) -> String {
            self.kind().to_string()
        }
        /// port: tsc/internal/ast/ast.go:Node.KindValue
        $visibility fn kind_value(&self) -> i16 {
            self.kind().raw()
        }
        /// port: tsc/internal/ast/ast.go:Node.PropertyNameOrName
        $visibility fn property_name_or_name(&self) -> Option<$node_id> {
            self.property_name().or_else(|| self.name())
        }
        /// port: tsc/internal/ast/ast.go:Node.CanHaveStatements
        $visibility fn can_have_statements(&self) -> bool {
            matches!(
                self.kind().known(),
                Some($crate::SyntaxKind::SourceFile | $crate::SyntaxKind::Block | $crate::SyntaxKind::ModuleBlock | $crate::SyntaxKind::CaseClause | $crate::SyntaxKind::DefaultClause)
            )
        }
        /// port: tsc/internal/ast/ast.go:Node.IsTypeOnly
        $visibility fn is_type_only(&self) -> bool {
            match self.kind().known() {
                Some($crate::SyntaxKind::ImportEqualsDeclaration) => $field!(
                    self,
                    ImportEqualsDeclaration,
                    as_import_equals_declaration,
                    is_type_only
                ),
                Some($crate::SyntaxKind::ImportSpecifier) => {
                    $field!(self, ImportSpecifier, as_import_specifier, is_type_only)
                }
                Some($crate::SyntaxKind::ImportClause) => {
                    $field!(self, ImportClause, as_import_clause, phase_modifier) == $crate::SyntaxKind::TypeKeyword
                }
                Some($crate::SyntaxKind::ExportDeclaration) => {
                    $field!(self, ExportDeclaration, as_export_declaration, is_type_only)
                }
                Some($crate::SyntaxKind::ExportSpecifier) => {
                    $field!(self, ExportSpecifier, as_export_specifier, is_type_only)
                }
                _ => false,
            }
        }
    };
}
pub(crate) use reads;

macro_rules! shape_reads {
    ($node_id:ty, $list_id:ty, $shape:ident; $visibility:vis,) => {
        /// port: tsc/internal/ast/ast.go:Node.Modifiers
        $visibility fn modifiers(&self) -> Option<$list_id> {
            $shape!(self;
                VariableStatement as_variable_statement => |data| data.modifiers(),
                ParameterDeclaration as_parameter_declaration => |data| data.modifiers(),
                MissingDeclaration as_missing_declaration => |data| data.modifiers(),
                FunctionDeclaration as_function_declaration => |data| data.modifiers(),
                ClassDeclaration as_class_declaration => |data| data.modifiers(),
                ClassExpression as_class_expression => |data| data.modifiers(),
                InterfaceDeclaration as_interface_declaration => |data| data.modifiers(),
                TypeAliasDeclaration as_type_alias_declaration => |data| data.modifiers(),
                EnumMember as_enum_member => |data| data.modifiers(),
                EnumDeclaration as_enum_declaration => |data| data.modifiers(),
                ImportDeclaration as_import_declaration => |data| data.modifiers(),
                ExportAssignment as_export_assignment => |data| data.modifiers(),
                NamespaceExportDeclaration as_namespace_export_declaration => |data| data.modifiers(),
                ConstructorDeclaration as_constructor_declaration => |data| data.modifiers(),
                GetAccessorDeclaration as_get_accessor_declaration => |data| data.modifiers(),
                SetAccessorDeclaration as_set_accessor_declaration => |data| data.modifiers(),
                IndexSignatureDeclaration as_index_signature_declaration => |data| data.modifiers(),
                MethodSignatureDeclaration as_method_signature_declaration => |data| data.modifiers(),
                MethodDeclaration as_method_declaration => |data| data.modifiers(),
                PropertySignatureDeclaration as_property_signature_declaration => |data| data.modifiers(),
                PropertyDeclaration as_property_declaration => |data| data.modifiers(),
                ClassStaticBlockDeclaration as_class_static_block_declaration => |data| data.modifiers(),
                BinaryExpression as_binary_expression => |data| data.modifiers(),
                ArrowFunction as_arrow_function => |data| data.modifiers(),
                FunctionExpression as_function_expression => |data| data.modifiers(),
                PropertyAssignment as_property_assignment => |data| data.modifiers(),
                ShorthandPropertyAssignment as_shorthand_property_assignment => |data| data.modifiers(),
                FunctionTypeNode as_function_type_node => |data| data.modifiers(),
                ConstructorTypeNode as_constructor_type_node => |data| data.modifiers(),
                ModuleDeclaration as_module_declaration => |data| data.modifiers(),
                ImportEqualsDeclaration as_import_equals_declaration => |data| data.modifiers(),
                ExportDeclaration as_export_declaration => |data| data.modifiers(),
                TypeParameterDeclaration as_type_parameter_declaration => |data| data.modifiers(),
            )
        }
        /// port: tsc/internal/ast/ast.go:Node.Body
        $visibility fn body(&self) -> Option<$node_id> {
            $shape!(self;
                FunctionDeclaration as_function_declaration => |data| data.body(),
                ConstructorDeclaration as_constructor_declaration => |data| data.body(),
                GetAccessorDeclaration as_get_accessor_declaration => |data| data.body(),
                SetAccessorDeclaration as_set_accessor_declaration => |data| data.body(),
                MethodDeclaration as_method_declaration => |data| data.body(),
                ClassStaticBlockDeclaration as_class_static_block_declaration => |data| data.body(),
                ArrowFunction as_arrow_function => |data| data.body(),
                FunctionExpression as_function_expression => |data| data.body(),
                ModuleDeclaration as_module_declaration => |data| data.body(),
            )
        }
        $visibility fn function_fields(&self) -> Option<(Option<$list_id>, Option<$list_id>, Option<$node_id>)> {
            $shape!(self;
                FunctionDeclaration as_function_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                CallSignatureDeclaration as_call_signature_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                ConstructSignatureDeclaration as_construct_signature_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                ConstructorDeclaration as_constructor_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                GetAccessorDeclaration as_get_accessor_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                SetAccessorDeclaration as_set_accessor_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                IndexSignatureDeclaration as_index_signature_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                MethodSignatureDeclaration as_method_signature_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                MethodDeclaration as_method_declaration => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                ArrowFunction as_arrow_function => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                FunctionExpression as_function_expression => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                FunctionTypeNode as_function_type_node => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                ConstructorTypeNode as_constructor_type_node => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
                JSDocSignature as_js_doc_signature => |data| Some((data.type_parameters(), data.parameters(), data.r#type())),
            )
        }
    };
}
pub(crate) use shape_reads;

// IsLocalsContainer is a source predicate, not an inline-storage capability:
// the pinned predicate includes TryStatement and SwitchStatement as well.
macro_rules! locals_container_shapes {
    ($select:ident, $node:expr) => {
        $select!($node; SourceFile, ForStatement, ForInOrOfStatement, SwitchStatement, CaseBlock, TryStatement, CatchClause, Block, FunctionDeclaration, ClassDeclaration, ClassExpression, TypeAliasDeclaration, CallSignatureDeclaration, ConstructSignatureDeclaration, ConstructorDeclaration, GetAccessorDeclaration, SetAccessorDeclaration, IndexSignatureDeclaration, MethodSignatureDeclaration, MethodDeclaration, ClassStaticBlockDeclaration, ArrowFunction, FunctionExpression, ConditionalTypeNode, MappedTypeNode, FunctionTypeNode, ConstructorTypeNode, JSDocSignature, ModuleDeclaration)
    };
}
pub(crate) use locals_container_shapes;
