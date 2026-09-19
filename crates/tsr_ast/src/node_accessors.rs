//! Source-level Node methods share logical reads while owned setters stay explicit.
use crate::{
    AstView, Node, NodeAccess, NodeData, NodeId, NodeListId, NodeRead, NodeSlice, SyntaxKind as K,
};
use ts_arena::Error;

macro_rules! read_methods {
    () => {
        pub fn name(&self) -> Option<NodeId> {
            NodeAccess::name(self)
        }
        pub fn modifiers(&self) -> Option<NodeListId> {
            NodeAccess::modifiers(self)
        }
        pub fn body(&self) -> Option<NodeId> {
            NodeAccess::body(self)
        }
        pub fn parameter_list(&self) -> Option<NodeListId> {
            NodeAccess::parameter_list(self)
        }
        pub fn expression(&self) -> Option<NodeId> {
            NodeAccess::expression(self)
        }
        pub fn type_node(&self) -> Option<NodeId> {
            NodeAccess::type_node(self)
        }
        pub fn initializer(&self) -> Option<NodeId> {
            NodeAccess::initializer(self)
        }
        pub fn tag_name(&self) -> Option<NodeId> {
            NodeAccess::tag_name(self)
        }
        pub fn property_name(&self) -> Option<NodeId> {
            NodeAccess::property_name(self)
        }
        pub fn label(&self) -> Option<NodeId> {
            NodeAccess::label(self)
        }
        pub fn attributes(&self) -> Option<NodeId> {
            NodeAccess::attributes(self)
        }
        pub fn module_specifier(&self) -> Option<NodeId> {
            NodeAccess::module_specifier(self)
        }
        pub fn import_clause(&self) -> Option<NodeId> {
            NodeAccess::import_clause(self)
        }
        pub fn statement(&self) -> Option<NodeId> {
            NodeAccess::statement(self)
        }
        pub fn postfix_token(&self) -> Option<NodeId> {
            NodeAccess::postfix_token(self)
        }
        pub fn question_dot_token(&self) -> Option<NodeId> {
            NodeAccess::question_dot_token(self)
        }
        pub fn type_expression(&self) -> Option<NodeId> {
            NodeAccess::type_expression(self)
        }
        pub fn class_name(&self) -> Option<NodeId> {
            NodeAccess::class_name(self)
        }
        pub fn argument_list(&self) -> Option<NodeListId> {
            NodeAccess::argument_list(self)
        }
        pub fn type_argument_list(&self) -> Option<NodeListId> {
            NodeAccess::type_argument_list(self)
        }
        pub fn type_parameter_list(&self) -> Option<NodeListId> {
            NodeAccess::type_parameter_list(self)
        }
        pub fn member_list(&self) -> Option<NodeListId> {
            NodeAccess::member_list(self)
        }
        pub fn statement_list(&self) -> Option<NodeListId> {
            NodeAccess::statement_list(self)
        }
        pub fn comment_list(&self) -> Option<NodeListId> {
            NodeAccess::comment_list(self)
        }
        pub fn children_list(&self) -> Option<NodeListId> {
            NodeAccess::children_list(self)
        }
        pub fn property_list(&self) -> Option<NodeListId> {
            NodeAccess::property_list(self)
        }
        pub fn element_list(&self) -> Option<NodeListId> {
            NodeAccess::element_list(self)
        }
        pub fn kind_string(&self) -> String {
            NodeAccess::kind_string(self)
        }
        pub fn kind_value(&self) -> i16 {
            NodeAccess::kind_value(self)
        }
        pub fn property_name_or_name(&self) -> Option<NodeId> {
            NodeAccess::property_name_or_name(self)
        }
        pub fn can_have_statements(&self) -> bool {
            NodeAccess::can_have_statements(self)
        }
        pub fn is_type_only(&self) -> bool {
            NodeAccess::is_type_only(self)
        }
        pub fn modifier_flags(&self, view: AstView<'_>) -> Result<u32, Error> {
            NodeAccess::modifier_flags(self, view)
        }
        pub fn question_token(&self, view: AstView<'_>) -> Result<Option<NodeId>, Error> {
            NodeAccess::question_token(self, view)
        }
        pub fn parameters(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::parameters(self, view)
        }
        pub fn arguments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::arguments(self, view)
        }
        pub fn type_arguments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::type_arguments(self, view)
        }
        pub fn type_parameters(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::type_parameters(self, view)
        }
        pub fn members(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::members(self, view)
        }
        pub fn statements(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::statements(self, view)
        }
        pub fn modifier_nodes(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::modifier_nodes(self, view)
        }
        pub fn comments(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::comments(self, view)
        }
        pub fn properties(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::properties(self, view)
        }
        pub fn elements(&self, view: AstView<'_>) -> Result<NodeSlice, Error> {
            NodeAccess::elements(self, view)
        }
    };
}
impl Node {
    read_methods!();
}
impl NodeRead<'_> {
    read_methods!();
}

macro_rules! set_field {
    ($node:expr, $variant:ident, $field:ident, $value:expr) => {{
        let actual = $node.data().name();
        match $node.data_mut() {
            NodeData::$variant(data) => data.$field = $value,
            _ => panic!(
                "interface conversion: ast.nodeData is *ast.{actual}, not *ast.{}",
                stringify!($variant)
            ),
        }
    }};
}
impl Node {
    /// port: tsc/internal/ast/ast.go:MutableNode.SetExpression
    pub fn set_expression(&mut self, value: Option<NodeId>) {
        match self.kind().known() {
            Some(K::PropertyAccessExpression) => {
                set_field!(self, PropertyAccessExpression, expression, value);
            }
            Some(K::ElementAccessExpression) => {
                set_field!(self, ElementAccessExpression, expression, value);
            }
            Some(K::ParenthesizedExpression) => {
                set_field!(self, ParenthesizedExpression, expression, value);
            }
            Some(K::CallExpression) => set_field!(self, CallExpression, expression, value),
            Some(K::NewExpression) => set_field!(self, NewExpression, expression, value),
            Some(K::ExpressionWithTypeArguments) => {
                set_field!(self, ExpressionWithTypeArguments, expression, value);
            }
            Some(K::ComputedPropertyName) => {
                set_field!(self, ComputedPropertyName, expression, value);
            }
            Some(K::NonNullExpression) => set_field!(self, NonNullExpression, expression, value),
            Some(K::TypeAssertionExpression) => set_field!(self, TypeAssertion, expression, value),
            Some(K::AsExpression) => set_field!(self, AsExpression, expression, value),
            Some(K::SatisfiesExpression) => {
                set_field!(self, SatisfiesExpression, expression, value);
            }
            Some(K::TypeOfExpression) => set_field!(self, TypeOfExpression, expression, value),
            Some(K::SpreadAssignment) => set_field!(self, SpreadAssignment, expression, value),
            Some(K::SpreadElement) => set_field!(self, SpreadElement, expression, value),
            Some(K::TemplateSpan) => set_field!(self, TemplateSpan, expression, value),
            Some(K::DeleteExpression) => set_field!(self, DeleteExpression, expression, value),
            Some(K::VoidExpression) => set_field!(self, VoidExpression, expression, value),
            Some(K::AwaitExpression) => set_field!(self, AwaitExpression, expression, value),
            Some(K::YieldExpression) => set_field!(self, YieldExpression, expression, value),
            Some(K::PartiallyEmittedExpression) => {
                set_field!(self, PartiallyEmittedExpression, expression, value);
            }
            Some(K::IfStatement) => set_field!(self, IfStatement, expression, value),
            Some(K::DoStatement) => set_field!(self, DoStatement, expression, value),
            Some(K::WhileStatement) => set_field!(self, WhileStatement, expression, value),
            Some(K::WithStatement) => set_field!(self, WithStatement, expression, value),
            Some(K::ForInStatement | K::ForOfStatement) => {
                set_field!(self, ForInOrOfStatement, expression, value);
            }
            Some(K::SwitchStatement) => set_field!(self, SwitchStatement, expression, value),
            Some(K::CaseClause) => set_field!(self, CaseOrDefaultClause, expression, value),
            Some(K::ExpressionStatement) => {
                set_field!(self, ExpressionStatement, expression, value);
            }
            Some(K::ReturnStatement) => set_field!(self, ReturnStatement, expression, value),
            Some(K::ThrowStatement) => set_field!(self, ThrowStatement, expression, value),
            Some(K::ExternalModuleReference) => {
                set_field!(self, ExternalModuleReference, expression, value);
            }
            Some(K::ExportAssignment) => set_field!(self, ExportAssignment, expression, value),
            Some(K::Decorator) => set_field!(self, Decorator, expression, value),
            Some(K::JsxExpression) => set_field!(self, JsxExpression, expression, value),
            Some(K::JsxSpreadAttribute) => set_field!(self, JsxSpreadAttribute, expression, value),
            _ => panic!(
                "Unhandled case in mutableNode.SetExpression: {}",
                self.kind()
            ),
        }
    }
    /// port: tsc/internal/ast/ast.go:MutableNode.SetType
    pub fn set_type_node(&mut self, value: Option<NodeId>) {
        match self.kind().known() {
            Some(K::VariableDeclaration) => set_field!(self, VariableDeclaration, r#type, value),
            Some(K::Parameter) => set_field!(self, ParameterDeclaration, r#type, value),
            Some(K::PropertySignature) => {
                set_field!(self, PropertySignatureDeclaration, r#type, value);
            }
            Some(K::PropertyDeclaration) => set_field!(self, PropertyDeclaration, r#type, value),
            Some(K::PropertyAssignment) => set_field!(self, PropertyAssignment, r#type, value),
            Some(K::ShorthandPropertyAssignment) => {
                set_field!(self, ShorthandPropertyAssignment, r#type, value);
            }
            Some(K::TypePredicate) => set_field!(self, TypePredicateNode, r#type, value),
            Some(K::ParenthesizedType) => set_field!(self, ParenthesizedTypeNode, r#type, value),
            Some(K::TypeOperator) => set_field!(self, TypeOperatorNode, r#type, value),
            Some(K::MappedType) => set_field!(self, MappedTypeNode, r#type, value),
            Some(K::TypeAssertionExpression) => set_field!(self, TypeAssertion, r#type, value),
            Some(K::AsExpression) => set_field!(self, AsExpression, r#type, value),
            Some(K::SatisfiesExpression) => set_field!(self, SatisfiesExpression, r#type, value),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => {
                set_field!(self, TypeAliasDeclaration, r#type, value);
            }
            Some(K::NamedTupleMember) => set_field!(self, NamedTupleMember, r#type, value),
            Some(K::OptionalType) => set_field!(self, OptionalTypeNode, r#type, value),
            Some(K::RestType) => set_field!(self, RestTypeNode, r#type, value),
            Some(K::TemplateLiteralTypeSpan) => {
                set_field!(self, TemplateLiteralTypeSpan, r#type, value);
            }
            Some(K::JSDocTypeExpression) => set_field!(self, JSDocTypeExpression, r#type, value),
            Some(K::JSDocParameterTag | K::JSDocPropertyTag) => {
                set_field!(self, JSDocParameterOrPropertyTag, type_expression, value);
            }
            Some(K::JSDocNullableType) => set_field!(self, JSDocNullableType, r#type, value),
            Some(K::JSDocNonNullableType) => set_field!(self, JSDocNonNullableType, r#type, value),
            Some(K::JSDocOptionalType) => set_field!(self, JSDocOptionalType, r#type, value),
            Some(K::ExportAssignment) => set_field!(self, ExportAssignment, r#type, value),
            Some(K::BinaryExpression) => set_field!(self, BinaryExpression, r#type, value),
            _ => match self.data_mut() {
                NodeData::FunctionDeclaration(data) => data.r#type = value,
                NodeData::CallSignatureDeclaration(data) => data.r#type = value,
                NodeData::ConstructSignatureDeclaration(data) => data.r#type = value,
                NodeData::ConstructorDeclaration(data) => data.r#type = value,
                NodeData::GetAccessorDeclaration(data) => data.r#type = value,
                NodeData::SetAccessorDeclaration(data) => data.r#type = value,
                NodeData::IndexSignatureDeclaration(data) => data.r#type = value,
                NodeData::MethodSignatureDeclaration(data) => data.r#type = value,
                NodeData::MethodDeclaration(data) => data.r#type = value,
                NodeData::ArrowFunction(data) => data.r#type = value,
                NodeData::FunctionExpression(data) => data.r#type = value,
                NodeData::FunctionTypeNode(data) => data.r#type = value,
                NodeData::ConstructorTypeNode(data) => data.r#type = value,
                NodeData::JSDocSignature(data) => data.r#type = value,
                _ => panic!("Unhandled case in mutableNode.SetType: {}", self.kind()),
            },
        }
    }
    /// port: tsc/internal/ast/ast.go:MutableNode.SetInitializer
    pub fn set_initializer(&mut self, value: Option<NodeId>) {
        match self.kind().known() {
            Some(K::VariableDeclaration) => {
                set_field!(self, VariableDeclaration, initializer, value);
            }
            Some(K::Parameter) => set_field!(self, ParameterDeclaration, initializer, value),
            Some(K::BindingElement) => set_field!(self, BindingElement, initializer, value),
            Some(K::PropertyDeclaration) => {
                set_field!(self, PropertyDeclaration, initializer, value);
            }
            Some(K::PropertySignature) => {
                set_field!(self, PropertySignatureDeclaration, initializer, value);
            }
            Some(K::PropertyAssignment) => set_field!(self, PropertyAssignment, initializer, value),
            Some(K::EnumMember) => set_field!(self, EnumMember, initializer, value),
            Some(K::ForStatement) => set_field!(self, ForStatement, initializer, value),
            Some(K::ForInStatement | K::ForOfStatement) => {
                set_field!(self, ForInOrOfStatement, initializer, value);
            }
            Some(K::JsxAttribute) => set_field!(self, JsxAttribute, initializer, value),
            _ => panic!("Unhandled case in mutableNode.SetInitializer"),
        }
    }
    /// port: tsc/internal/ast/ast.go:MutableNode.SetModifiers
    pub fn set_modifiers(&mut self, value: Option<NodeListId>) {
        match self.data_mut() {
            NodeData::VariableStatement(data) => data.modifiers = value,
            NodeData::ParameterDeclaration(data) => data.modifiers = value,
            NodeData::MissingDeclaration(data) => data.modifiers = value,
            NodeData::FunctionDeclaration(data) => data.modifiers = value,
            NodeData::ClassDeclaration(data) => data.modifiers = value,
            NodeData::ClassExpression(data) => data.modifiers = value,
            NodeData::InterfaceDeclaration(data) => data.modifiers = value,
            NodeData::TypeAliasDeclaration(data) => data.modifiers = value,
            NodeData::EnumMember(data) => data.modifiers = value,
            NodeData::EnumDeclaration(data) => data.modifiers = value,
            NodeData::ImportDeclaration(data) => data.modifiers = value,
            NodeData::ExportAssignment(data) => data.modifiers = value,
            NodeData::NamespaceExportDeclaration(data) => data.modifiers = value,
            NodeData::ConstructorDeclaration(data) => data.modifiers = value,
            NodeData::GetAccessorDeclaration(data) => data.modifiers = value,
            NodeData::SetAccessorDeclaration(data) => data.modifiers = value,
            NodeData::IndexSignatureDeclaration(data) => data.modifiers = value,
            NodeData::MethodSignatureDeclaration(data) => data.modifiers = value,
            NodeData::MethodDeclaration(data) => data.modifiers = value,
            NodeData::PropertySignatureDeclaration(data) => data.modifiers = value,
            NodeData::PropertyDeclaration(data) => data.modifiers = value,
            NodeData::ClassStaticBlockDeclaration(data) => data.modifiers = value,
            NodeData::BinaryExpression(data) => data.modifiers = value,
            NodeData::ArrowFunction(data) => data.modifiers = value,
            NodeData::FunctionExpression(data) => data.modifiers = value,
            NodeData::PropertyAssignment(data) => data.modifiers = value,
            NodeData::ShorthandPropertyAssignment(data) => data.modifiers = value,
            NodeData::FunctionTypeNode(data) => data.modifiers = value,
            NodeData::ConstructorTypeNode(data) => data.modifiers = value,
            NodeData::ModuleDeclaration(data) => data.modifiers = value,
            NodeData::ImportEqualsDeclaration(data) => data.modifiers = value,
            NodeData::ExportDeclaration(data) => data.modifiers = value,
            NodeData::TypeParameterDeclaration(data) => data.modifiers = value,
            _ => {}
        }
    }
}
