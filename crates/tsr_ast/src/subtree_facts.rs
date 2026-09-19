//! Pinned subtree facts, scope exclusions, and immutable atomic cache semantics.
use crate::NodeAccess;
use crate::{modifier_flags as m, node_flags, token_flags};
use crate::{AstView, NodeDataRead, NodeId, NodeKind, NodeListId, SubtreeContext, SyntaxKind};

pub type SubtreeFacts = u32;
pub mod subtree_flags {
    pub const TYPE_SCRIPT: u32 = 1 << 0;
    pub const JSX: u32 = 1 << 1;
    pub const ES_DECORATORS: u32 = 1 << 2;
    pub const USING: u32 = 1 << 3;
    pub const CLASS_STATIC_BLOCKS: u32 = 1 << 4;
    pub const ES_CLASS_FIELDS: u32 = 1 << 5;
    pub const LOGICAL_ASSIGNMENTS: u32 = 1 << 6;
    pub const NULLISH_COALESCING: u32 = 1 << 7;
    pub const OPTIONAL_CHAINING: u32 = 1 << 8;
    pub const MISSING_CATCH_CLAUSE_VARIABLE: u32 = 1 << 9;
    pub const ES_OBJECT_REST_OR_SPREAD: u32 = 1 << 10;
    pub const FOR_AWAIT_OR_ASYNC_GENERATOR: u32 = 1 << 11;
    pub const ANY_AWAIT: u32 = 1 << 12;
    pub const EXPONENTIATION_OPERATOR: u32 = 1 << 13;
    pub const LEXICAL_THIS: u32 = 1 << 14;
    pub const LEXICAL_SUPER: u32 = 1 << 15;
    pub const REST_OR_SPREAD: u32 = 1 << 16;
    pub const OBJECT_REST_OR_SPREAD: u32 = 1 << 17;
    pub const AWAIT: u32 = 1 << 18;
    pub const DYNAMIC_IMPORT: u32 = 1 << 19;
    pub const CLASS_FIELDS: u32 = 1 << 20;
    pub const DECORATORS: u32 = 1 << 21;
    pub const IDENTIFIER: u32 = 1 << 22;
    pub const PRIVATE_IDENTIFIER_IN_EXPRESSION: u32 = 1 << 23;
    pub const INVALID_TEMPLATE_ESCAPE: u32 = 1 << 24;
    pub const COMPUTED: u32 = 1 << 25;
    pub const NONE: u32 = 0;
    pub const ES_NEXT: u32 = ES_DECORATORS | USING;
    pub const ES2022: u32 = CLASS_STATIC_BLOCKS | ES_CLASS_FIELDS;
    pub const ES2021: u32 = LOGICAL_ASSIGNMENTS;
    pub const ES2020: u32 = NULLISH_COALESCING | OPTIONAL_CHAINING;
    pub const ES2019: u32 = MISSING_CATCH_CLAUSE_VARIABLE;
    pub const ES2018: u32 =
        ES_OBJECT_REST_OR_SPREAD | FOR_AWAIT_OR_ASYNC_GENERATOR | INVALID_TEMPLATE_ESCAPE;
    pub const ES2017: u32 = ANY_AWAIT;
    pub const ES2016: u32 = EXPONENTIATION_OPERATOR;
    pub const LEXICAL_THIS_OR_SUPER: u32 = LEXICAL_THIS | LEXICAL_SUPER;
}
use subtree_flags::{
    ANY_AWAIT, AWAIT, CLASS_FIELDS, COMPUTED, DECORATORS, DYNAMIC_IMPORT, ES_OBJECT_REST_OR_SPREAD,
    EXPONENTIATION_OPERATOR, FOR_AWAIT_OR_ASYNC_GENERATOR, IDENTIFIER, INVALID_TEMPLATE_ESCAPE,
    JSX, LEXICAL_SUPER, LEXICAL_THIS, LOGICAL_ASSIGNMENTS, MISSING_CATCH_CLAUSE_VARIABLE, NONE,
    NULLISH_COALESCING, OBJECT_REST_OR_SPREAD, OPTIONAL_CHAINING, PRIVATE_IDENTIFIER_IN_EXPRESSION,
    REST_OR_SPREAD, TYPE_SCRIPT, USING,
};

impl AstView<'_> {
    // port: tsc/internal/ast/ast.go:Node.SubtreeFacts
    pub fn subtree_facts(self, id: NodeId) -> SubtreeFacts {
        Facts { view: self }.facts(id)
    }
    // port: tsc/internal/ast/ast.go:Node.propagateSubtreeFacts
    pub fn propagate_subtree_facts(self, id: Option<NodeId>) -> SubtreeFacts {
        Facts { view: self }.propagate_node(id)
    }
    pub fn contains_object_rest_or_spread(self, id: NodeId) -> bool {
        Facts { view: self }.contains_object_rest_or_spread(id)
    }
}
struct Facts<'a> {
    view: AstView<'a>,
}
impl Facts<'_> {
    // port: tsc/internal/ast/ast.go:NodeDefault.SubtreeFacts
    fn facts(&mut self, id: NodeId) -> u32 {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.facts_worker(id))
    }
    // port: tsc/internal/ast/ast.go:NodeDefault.subtreeFactsWorker
    fn facts_worker(&mut self, id: NodeId) -> u32 {
        let view = self.view;
        let node = view
            .node(id)
            .expect("subtree node belongs to retained storage");
        if node.data().uses_subtree_cache() {
            self.cached(&node)
        } else {
            self.compute(&node)
        }
    }
    // port: tsc/internal/ast/ast.go:CompositeBase.subtreeFactsWorker
    fn cached(&mut self, node: &(impl NodeAccess + ?Sized)) -> u32 {
        let mut facts = node.cached_subtree_facts();
        if facts & COMPUTED == 0 {
            facts |= self.compute(node) | COMPUTED;
            node.store_subtree_facts(facts);
        }
        facts & !COMPUTED
    }
    fn modifier_flags(&self, list: Option<NodeListId>) -> u32 {
        list.map_or(0, |id| {
            self.view
                .list(id)
                .expect("subtree modifier list")
                .modifier_flags()
        })
    }
    fn kind(&self, id: Option<NodeId>) -> NodeKind {
        self.view
            .node(id.expect("nil node in subtree kind predicate"))
            .expect("subtree node")
            .kind()
    }
    fn is_this_identifier(&self, id: NodeId) -> bool {
        let node = self.view.node(id).expect("subtree identifier");
        if node.kind() != SyntaxKind::Identifier {
            return false;
        }
        let NodeDataRead::Identifier(data) = node.data() else {
            panic!("invalid Identifier payload")
        };
        data.text() == b"this"
    }
    #[allow(clippy::match_same_arms)] // Keep each pinned override independently auditable.
    fn compute(&mut self, node: &(impl NodeAccess + ?Sized)) -> u32 {
        use NodeDataRead as D;
        use SyntaxKind as K;
        if node.data().is_type_syntax() {
            return type_syntax_facts();
        }
        if let Some(facts) = node.data().compute_subtree_facts_generated(self) {
            return facts;
        }
        match node.data() {
            // port: tsc/internal/ast/ast.go:PrivateIdentifier.computeSubtreeFacts
            D::PrivateIdentifier(_) => CLASS_FIELDS,
            // port: tsc/internal/ast/ast.go:Decorator.computeSubtreeFacts
            D::Decorator(d) => self.propagate_node(d.expression()) | TYPE_SCRIPT | DECORATORS,
            // port: tsc/internal/ast/ast.go:ReturnStatement.computeSubtreeFacts
            D::ReturnStatement(d) => {
                self.propagate_node(d.expression()) | FOR_AWAIT_OR_ASYNC_GENERATOR
            }
            // port: tsc/internal/ast/ast.go:VariableDeclaration.computeSubtreeFacts
            D::VariableDeclaration(d) => {
                self.propagate_node(d.name())
                    | eraseable_node(d.exclamation_token())
                    | eraseable_node(d.r#type())
                    | self.propagate_node(d.initializer())
            }
            // port: tsc/internal/ast/ast.go:BindingElement.computeSubtreeFacts
            D::BindingElement(d) => {
                self.propagate_node(d.property_name())
                    | self.propagate_node(d.name())
                    | self.propagate_node(d.initializer())
                    | (if d.dot_dot_dot_token().is_some() {
                        REST_OR_SPREAD
                    } else {
                        NONE
                    })
            }
            // port: tsc/internal/ast/ast.go:EnumMember.computeSubtreeFacts
            D::EnumMember(d) => {
                self.propagate_node(d.name()) | self.propagate_node(d.initializer()) | TYPE_SCRIPT
            }
            // port: tsc/internal/ast/ast.go:ExportAssignment.computeSubtreeFacts
            D::ExportAssignment(d) => {
                self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.r#type())
                    | self.propagate_node(d.expression())
                    | (if d.is_export_equals() {
                        TYPE_SCRIPT
                    } else {
                        NONE
                    })
            }
            // port: tsc/internal/ast/ast.go:ExportDeclaration.computeSubtreeFacts
            D::ExportDeclaration(d) => {
                self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.export_clause())
                    | self.propagate_node(d.module_specifier())
                    | self.propagate_node(d.attributes())
                    | (if d.is_type_only() { TYPE_SCRIPT } else { NONE })
            }
            // port: tsc/internal/ast/ast.go:PropertyDeclaration.computeSubtreeFacts
            D::PropertyDeclaration(d) => {
                self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.name())
                    | eraseable_node(d.postfix_token())
                    | eraseable_node(d.r#type())
                    | self.propagate_node(d.initializer())
                    | CLASS_FIELDS
            }
            // port: tsc/internal/ast/ast.go:ClassStaticBlockDeclaration.computeSubtreeFacts
            D::ClassStaticBlockDeclaration(d) => {
                self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.body())
                    | CLASS_FIELDS
            }
            // port: tsc/internal/ast/ast.go:BigIntLiteral.computeSubtreeFacts
            D::BigIntLiteral(_) => NONE,
            // port: tsc/internal/ast/ast.go:Identifier.computeSubtreeFacts
            D::Identifier(_) => IDENTIFIER,
            // port: tsc/internal/ast/ast.go:YieldExpression.computeSubtreeFacts
            D::YieldExpression(d) => {
                self.propagate_node(d.expression()) | FOR_AWAIT_OR_ASYNC_GENERATOR
            }
            // port: tsc/internal/ast/ast.go:AsExpression.computeSubtreeFacts
            D::AsExpression(d) => self.propagate_node(d.expression()) | TYPE_SCRIPT,
            // port: tsc/internal/ast/ast.go:SatisfiesExpression.computeSubtreeFacts
            D::SatisfiesExpression(d) => self.propagate_node(d.expression()) | TYPE_SCRIPT,
            // port: tsc/internal/ast/ast.go:NewExpression.computeSubtreeFacts
            D::NewExpression(d) => {
                self.propagate_node(d.expression())
                    | eraseable_list(d.type_arguments())
                    | self.propagate_list(d.arguments())
            }
            // port: tsc/internal/ast/ast.go:MetaProperty.computeSubtreeFacts
            D::MetaProperty(d) => self.propagate_node(d.name()) & !IDENTIFIER,
            // port: tsc/internal/ast/ast.go:NonNullExpression.computeSubtreeFacts
            D::NonNullExpression(d) => self.propagate_node(d.expression()) | TYPE_SCRIPT,
            // port: tsc/internal/ast/ast.go:SpreadElement.computeSubtreeFacts
            D::SpreadElement(d) => self.propagate_node(d.expression()) | REST_OR_SPREAD,
            // port: tsc/internal/ast/ast.go:TaggedTemplateExpression.computeSubtreeFacts
            D::TaggedTemplateExpression(d) => {
                self.propagate_node(d.tag())
                    | self.propagate_node(d.question_dot_token())
                    | eraseable_list(d.type_arguments())
                    | self.propagate_node(d.template())
            }
            // port: tsc/internal/ast/ast.go:SpreadAssignment.computeSubtreeFacts
            D::SpreadAssignment(d) => {
                self.propagate_node(d.expression())
                    | ES_OBJECT_REST_OR_SPREAD
                    | OBJECT_REST_OR_SPREAD
            }
            // port: tsc/internal/ast/ast.go:PropertyAssignment.computeSubtreeFacts
            D::PropertyAssignment(d) => {
                self.propagate_node(d.name())
                    | self.propagate_node(d.r#type())
                    | self.propagate_node(d.initializer())
            }
            // port: tsc/internal/ast/ast.go:ShorthandPropertyAssignment.computeSubtreeFacts
            D::ShorthandPropertyAssignment(d) => {
                self.propagate_node(d.name())
                    | self.propagate_node(d.r#type())
                    | self.propagate_node(d.object_assignment_initializer())
                    | TYPE_SCRIPT
            }
            // port: tsc/internal/ast/ast.go:AwaitExpression.computeSubtreeFacts
            D::AwaitExpression(d) => {
                self.propagate_node(d.expression())
                    | AWAIT
                    | ANY_AWAIT
                    | FOR_AWAIT_OR_ASYNC_GENERATOR
            }
            // port: tsc/internal/ast/ast.go:TypeAssertion.computeSubtreeFacts
            D::TypeAssertion(d) => self.propagate_node(d.expression()) | TYPE_SCRIPT,
            // port: tsc/internal/ast/ast.go:ExpressionWithTypeArguments.computeSubtreeFacts
            D::ExpressionWithTypeArguments(d) => {
                self.propagate_node(d.expression()) | eraseable_list(d.type_arguments())
            }
            // port: tsc/internal/ast/ast.go:JsxElement.computeSubtreeFacts
            D::JsxElement(d) => {
                self.propagate_node(d.opening_element())
                    | self.propagate_list(d.children())
                    | self.propagate_node(d.closing_element())
                    | JSX
            }
            // port: tsc/internal/ast/ast.go:JsxAttributes.computeSubtreeFacts
            D::JsxAttributes(d) => self.propagate_list(d.properties()) | JSX,
            // port: tsc/internal/ast/ast.go:JsxNamespacedName.computeSubtreeFacts
            D::JsxNamespacedName(d) => {
                self.propagate_node(d.namespace()) | self.propagate_node(d.name()) | JSX
            }
            // port: tsc/internal/ast/ast.go:JsxOpeningElement.computeSubtreeFacts
            D::JsxOpeningElement(d) => {
                self.propagate_node(d.tag_name())
                    | eraseable_list(d.type_arguments())
                    | self.propagate_node(d.attributes())
                    | JSX
            }
            // port: tsc/internal/ast/ast.go:JsxSelfClosingElement.computeSubtreeFacts
            D::JsxSelfClosingElement(d) => {
                self.propagate_node(d.tag_name())
                    | eraseable_list(d.type_arguments())
                    | self.propagate_node(d.attributes())
                    | JSX
            }
            // port: tsc/internal/ast/ast.go:JsxFragment.computeSubtreeFacts
            D::JsxFragment(d) => self.propagate_list(d.children()) | JSX,
            // port: tsc/internal/ast/ast.go:JsxOpeningFragment.computeSubtreeFacts
            D::JsxOpeningFragment(_) => JSX,
            // port: tsc/internal/ast/ast.go:JsxClosingFragment.computeSubtreeFacts
            D::JsxClosingFragment(_) => JSX,
            // port: tsc/internal/ast/ast.go:JsxAttribute.computeSubtreeFacts
            D::JsxAttribute(d) => {
                self.propagate_node(d.name()) | self.propagate_node(d.initializer()) | JSX
            }
            // port: tsc/internal/ast/ast.go:JsxSpreadAttribute.computeSubtreeFacts
            D::JsxSpreadAttribute(d) => self.propagate_node(d.expression()) | JSX,
            // port: tsc/internal/ast/ast.go:JsxClosingElement.computeSubtreeFacts
            D::JsxClosingElement(d) => self.propagate_node(d.tag_name()) | JSX,
            // port: tsc/internal/ast/ast.go:JsxExpression.computeSubtreeFacts
            D::JsxExpression(d) => self.propagate_node(d.expression()) | JSX,
            // port: tsc/internal/ast/ast.go:JsxText.computeSubtreeFacts
            D::JsxText(_) => JSX,
            // port: tsc/internal/ast/ast.go:SourceFile.computeSubtreeFacts
            D::SourceFile(d) => self.propagate_list(d.statements()),
            // port: tsc/internal/ast/ast.go:ForInOrOfStatement.computeSubtreeFacts
            D::ForInOrOfStatement(d) => {
                self.propagate_node(d.initializer())
                    | self.propagate_node(d.expression())
                    | self.propagate_node(d.statement())
                    | (if d.await_modifier().is_some() {
                        FOR_AWAIT_OR_ASYNC_GENERATOR
                    } else {
                        NONE
                    })
            }
            // port: tsc/internal/ast/ast.go:VariableDeclarationList.computeSubtreeFacts
            D::VariableDeclarationList(d) => {
                self.propagate_list(d.declarations())
                    | (if node.flags() & node_flags::USING != 0 {
                        USING
                    } else {
                        NONE
                    })
            }
            // port: tsc/internal/ast/ast.go:Token.computeSubtreeFacts
            D::Token(_) => match node.kind().known() {
                Some(K::UsingKeyword) => USING,
                Some(
                    K::PublicKeyword
                    | K::PrivateKeyword
                    | K::ProtectedKeyword
                    | K::ReadonlyKeyword
                    | K::AbstractKeyword
                    | K::DeclareKeyword
                    | K::ConstKeyword
                    | K::AnyKeyword
                    | K::NumberKeyword
                    | K::BigIntKeyword
                    | K::NeverKeyword
                    | K::ObjectKeyword
                    | K::InKeyword
                    | K::OutKeyword
                    | K::OverrideKeyword
                    | K::StringKeyword
                    | K::BooleanKeyword
                    | K::SymbolKeyword
                    | K::VoidKeyword
                    | K::UnknownKeyword
                    | K::UndefinedKeyword
                    | K::ExportKeyword,
                ) => TYPE_SCRIPT,
                Some(K::AccessorKeyword) => CLASS_FIELDS,
                Some(K::AsyncKeyword) => ANY_AWAIT,
                Some(K::SuperKeyword) => LEXICAL_SUPER,
                Some(K::ThisKeyword) => LEXICAL_THIS,
                Some(K::AsteriskAsteriskToken | K::AsteriskAsteriskEqualsToken) => {
                    EXPONENTIATION_OPERATOR
                }
                Some(K::QuestionQuestionToken) => NULLISH_COALESCING,
                Some(K::QuestionDotToken) => OPTIONAL_CHAINING,
                Some(
                    K::QuestionQuestionEqualsToken
                    | K::BarBarEqualsToken
                    | K::AmpersandAmpersandEqualsToken,
                ) => LOGICAL_ASSIGNMENTS,
                _ => NONE,
            },
            // port: tsc/internal/ast/ast.go:KeywordExpression.computeSubtreeFacts
            D::KeywordExpression(_) => match node.kind().known() {
                Some(K::ThisKeyword) => LEXICAL_THIS,
                Some(K::SuperKeyword) => LEXICAL_SUPER,
                _ => NONE,
            },
            // port: tsc/internal/ast/ast.go:CatchClause.computeSubtreeFacts
            D::CatchClause(d) => {
                self.propagate_node(d.variable_declaration())
                    | self.propagate_node(d.block())
                    | if d.variable_declaration().is_none() {
                        MISSING_CATCH_CLAUSE_VARIABLE
                    } else {
                        NONE
                    }
            }
            // port: tsc/internal/ast/ast.go:VariableStatement.computeSubtreeFacts
            D::VariableStatement(d) => {
                if self.modifier_flags(d.modifiers()) & m::AMBIENT != 0 {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.declaration_list())
                }
            }
            // port: tsc/internal/ast/ast.go:BindingPattern.computeSubtreeFacts
            D::BindingPattern(d) => match node.kind().known() {
                Some(K::ObjectBindingPattern) => {
                    self.list_with(d.elements(), Self::object_binding_element)
                }
                Some(K::ArrayBindingPattern) => self.list_with(d.elements(), Self::binding_element),
                _ => NONE,
            },
            // port: tsc/internal/ast/ast.go:ParameterDeclaration.computeSubtreeFacts
            D::ParameterDeclaration(d) => {
                if d.name().is_some_and(|id| self.is_this_identifier(id)) {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.name())
                        | eraseable_node(d.question_token())
                        | eraseable_node(d.r#type())
                        | self.propagate_node(d.initializer())
                }
            }
            // port: tsc/internal/ast/ast.go:FunctionDeclaration.computeSubtreeFacts
            D::FunctionDeclaration(d) => {
                if d.body().is_none() || self.modifier_flags(d.modifiers()) & m::AMBIENT != 0 {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.asterisk_token())
                        | self.propagate_node(d.name())
                        | eraseable_list(d.type_parameters())
                        | self.propagate_list(d.parameters())
                        | eraseable_node(d.r#type())
                        | eraseable_node(d.full_signature())
                        | self.propagate_node(d.body())
                        | async_facts(
                            self.modifier_flags(d.modifiers()),
                            d.asterisk_token().is_some(),
                        )
                }
            }
            D::ClassDeclaration(d) => self.class(
                d.modifiers(),
                d.name(),
                d.type_parameters(),
                d.heritage_clauses(),
                d.members(),
            ),
            D::ClassExpression(d) => self.class(
                d.modifiers(),
                d.name(),
                d.type_parameters(),
                d.heritage_clauses(),
                d.members(),
            ),
            // port: tsc/internal/ast/ast.go:HeritageClause.computeSubtreeFacts
            D::HeritageClause(d) => match d.token().known() {
                Some(K::ExtendsKeyword) => self.propagate_list(d.types()),
                Some(K::ImplementsKeyword) => TYPE_SCRIPT,
                _ => NONE,
            },
            // port: tsc/internal/ast/ast.go:EnumDeclaration.computeSubtreeFacts
            D::EnumDeclaration(d) => {
                if self.modifier_flags(d.modifiers()) & m::AMBIENT != 0 {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.name())
                        | self.propagate_list(d.members())
                        | TYPE_SCRIPT
                }
            }
            // port: tsc/internal/ast/ast.go:ModuleDeclaration.computeSubtreeFacts
            D::ModuleDeclaration(d) => {
                if self.modifier_flags(d.modifiers()) & m::AMBIENT != 0 {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.name())
                        | self.propagate_node(d.body())
                        | TYPE_SCRIPT
                }
            }
            // port: tsc/internal/ast/ast.go:ImportEqualsDeclaration.computeSubtreeFacts
            D::ImportEqualsDeclaration(d) => {
                if d.is_type_only() || self.kind(d.module_reference()) != K::ExternalModuleReference
                {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.name())
                        | self.propagate_node(d.module_reference())
                }
            }
            // port: tsc/internal/ast/ast.go:ImportSpecifier.computeSubtreeFacts
            D::ImportSpecifier(d) => {
                if d.is_type_only() {
                    TYPE_SCRIPT
                } else {
                    self.propagate_node(d.property_name()) | self.propagate_node(d.name())
                }
            }
            // port: tsc/internal/ast/ast.go:ImportClause.computeSubtreeFacts
            D::ImportClause(d) => {
                if d.phase_modifier() == K::TypeKeyword {
                    TYPE_SCRIPT
                } else {
                    self.propagate_node(d.name()) | self.propagate_node(d.named_bindings())
                }
            }
            // port: tsc/internal/ast/ast.go:ExportSpecifier.computeSubtreeFacts
            D::ExportSpecifier(d) => {
                if d.is_type_only() {
                    TYPE_SCRIPT
                } else {
                    self.propagate_node(d.property_name()) | self.propagate_node(d.name())
                }
            }
            // port: tsc/internal/ast/ast.go:ConstructorDeclaration.computeSubtreeFacts
            D::ConstructorDeclaration(d) => {
                if d.body().is_none() {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | eraseable_list(d.type_parameters())
                        | self.propagate_list(d.parameters())
                        | eraseable_node(d.r#type())
                        | eraseable_node(d.full_signature())
                        | self.propagate_node(d.body())
                }
            }
            D::GetAccessorDeclaration(d) => self.accessor(
                d.modifiers(),
                d.name(),
                d.type_parameters(),
                d.parameters(),
                d.r#type(),
                d.full_signature(),
                d.body(),
            ),
            D::SetAccessorDeclaration(d) => self.accessor(
                d.modifiers(),
                d.name(),
                d.type_parameters(),
                d.parameters(),
                d.r#type(),
                d.full_signature(),
                d.body(),
            ),
            // port: tsc/internal/ast/ast.go:MethodDeclaration.computeSubtreeFacts
            D::MethodDeclaration(d) => {
                if d.body().is_none() {
                    TYPE_SCRIPT
                } else {
                    self.propagate_modifiers(d.modifiers())
                        | self.propagate_node(d.asterisk_token())
                        | self.propagate_node(d.name())
                        | eraseable_node(d.postfix_token())
                        | eraseable_list(d.type_parameters())
                        | self.propagate_list(d.parameters())
                        | self.propagate_node(d.body())
                        | eraseable_node(d.r#type())
                        | eraseable_node(d.full_signature())
                        | async_facts(
                            self.modifier_flags(d.modifiers()),
                            d.asterisk_token().is_some(),
                        )
                }
            }
            // port: tsc/internal/ast/ast.go:NoSubstitutionTemplateLiteral.computeSubtreeFacts
            D::NoSubstitutionTemplateLiteral(d) => template(d.template_flags()),
            // port: tsc/internal/ast/ast.go:TemplateHead.computeSubtreeFacts
            D::TemplateHead(d) => template(d.template_flags()),
            // port: tsc/internal/ast/ast.go:TemplateMiddle.computeSubtreeFacts
            D::TemplateMiddle(d) => template(d.template_flags()),
            // port: tsc/internal/ast/ast.go:TemplateTail.computeSubtreeFacts
            D::TemplateTail(d) => template(d.template_flags()),
            // port: tsc/internal/ast/ast.go:BinaryExpression.computeSubtreeFacts
            D::BinaryExpression(d) => {
                let mut facts = self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.left())
                    | self.propagate_node(d.r#type())
                    | self.propagate_node(d.operator_token())
                    | self.propagate_node(d.right());
                let operator = self.kind(d.operator_token());
                if operator == K::InKeyword && self.kind(d.left()) == K::PrivateIdentifier {
                    facts |= CLASS_FIELDS | PRIVATE_IDENTIFIER_IN_EXPRESSION;
                }
                if operator == K::EqualsToken
                    && matches!(
                        self.kind(d.left()).known(),
                        Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression)
                    )
                    && self.contains_object_rest_or_spread(d.left().expect("binary left"))
                {
                    facts |= OBJECT_REST_OR_SPREAD;
                }
                facts
            }
            // port: tsc/internal/ast/ast.go:ArrowFunction.computeSubtreeFacts
            D::ArrowFunction(d) => {
                self.propagate_modifiers(d.modifiers())
                    | eraseable_list(d.type_parameters())
                    | self.propagate_list(d.parameters())
                    | eraseable_node(d.r#type())
                    | eraseable_node(d.full_signature())
                    | self.propagate_node(d.body())
                    | if self.modifier_flags(d.modifiers()) & m::ASYNC != 0 {
                        ANY_AWAIT
                    } else {
                        NONE
                    }
            }
            // port: tsc/internal/ast/ast.go:FunctionExpression.computeSubtreeFacts
            D::FunctionExpression(d) => {
                self.propagate_modifiers(d.modifiers())
                    | self.propagate_node(d.asterisk_token())
                    | self.propagate_node(d.name())
                    | eraseable_list(d.type_parameters())
                    | self.propagate_list(d.parameters())
                    | eraseable_node(d.r#type())
                    | eraseable_node(d.full_signature())
                    | self.propagate_node(d.body())
                    | async_facts(
                        self.modifier_flags(d.modifiers()),
                        d.asterisk_token().is_some(),
                    )
            }
            // port: tsc/internal/ast/ast.go:PropertyAccessExpression.computeSubtreeFacts
            D::PropertyAccessExpression(d) => {
                let private = if self.kind(d.name()) == K::Identifier {
                    NONE
                } else {
                    PRIVATE_IDENTIFIER_IN_EXPRESSION
                };
                self.propagate_node(d.expression())
                    | self.propagate_node(d.question_dot_token())
                    | self.propagate_node(d.name())
                    | private
            }
            // port: tsc/internal/ast/ast.go:CallExpression.computeSubtreeFacts
            D::CallExpression(d) => {
                self.propagate_node(d.expression())
                    | self.propagate_node(d.question_dot_token())
                    | eraseable_list(d.type_arguments())
                    | self.propagate_list(d.arguments())
                    | if self.kind(d.expression()) == K::ImportKeyword {
                        DYNAMIC_IMPORT
                    } else {
                        NONE
                    }
            }
            _ => {
                if node.data().uses_subtree_cache() {
                    unimplemented_composite()
                } else {
                    default_facts()
                }
            }
        }
    }
    // port: tsc/internal/ast/ast.go:ClassLikeBase.computeSubtreeFacts
    fn class(
        &mut self,
        modifiers: Option<NodeListId>,
        name: Option<NodeId>,
        types: Option<NodeListId>,
        heritage: Option<NodeListId>,
        members: Option<NodeListId>,
    ) -> u32 {
        if self.modifier_flags(modifiers) & m::AMBIENT != 0 {
            TYPE_SCRIPT
        } else {
            self.propagate_modifiers(modifiers)
                | self.propagate_node(name)
                | eraseable_list(types)
                | self.propagate_list(heritage)
                | self.propagate_list(members)
        }
    }
    // port: tsc/internal/ast/ast.go:AccessorDeclarationBase.computeSubtreeFacts
    #[allow(clippy::too_many_arguments)] // Shared Go base fields, without copying boxed payloads.
    fn accessor(
        &mut self,
        modifiers: Option<NodeListId>,
        name: Option<NodeId>,
        types: Option<NodeListId>,
        parameters: Option<NodeListId>,
        result: Option<NodeId>,
        signature: Option<NodeId>,
        body: Option<NodeId>,
    ) -> u32 {
        if body.is_none() {
            TYPE_SCRIPT
        } else {
            self.propagate_modifiers(modifiers)
                | self.propagate_node(name)
                | eraseable_list(types)
                | self.propagate_list(parameters)
                | eraseable_node(result)
                | eraseable_node(signature)
                | self.propagate_node(body)
        }
    }
    // port: tsc/internal/ast/subtreefacts.go:propagateObjectBindingElementSubtreeFacts
    fn object_binding_element(&mut self, node: Option<NodeId>) -> u32 {
        let mut facts = self.propagate_node(node);
        if facts & REST_OR_SPREAD != 0 {
            facts = (facts & !REST_OR_SPREAD) | OBJECT_REST_OR_SPREAD | ES_OBJECT_REST_OR_SPREAD;
        }
        facts
    }
    // port: tsc/internal/ast/subtreefacts.go:propagateBindingElementSubtreeFacts
    fn binding_element(&mut self, node: Option<NodeId>) -> u32 {
        self.propagate_node(node) & !REST_OR_SPREAD
    }
    // port: tsc/internal/ast/subtreefacts.go:propagateNodeListSubtreeFacts
    fn list_with(
        &mut self,
        list: Option<NodeListId>,
        propagate: fn(&mut Self, Option<NodeId>) -> u32,
    ) -> u32 {
        let Some(list) = list else { return NONE };
        let view = self.view;
        let nodes = view.list(list).expect("subtree list").nodes();
        let nodes = view.node_slice(nodes).expect("subtree list backing");
        let mut facts = NONE;
        for node in nodes.iter() {
            facts |= propagate(self, node);
        }
        facts
    }
}
// port: tsc/internal/ast/subtreefacts.go:propagateEraseableSyntaxListSubtreeFacts
fn eraseable_list(list: Option<NodeListId>) -> u32 {
    if list.is_some() {
        TYPE_SCRIPT
    } else {
        NONE
    }
}
// port: tsc/internal/ast/subtreefacts.go:propagateEraseableSyntaxSubtreeFacts
fn eraseable_node(node: Option<NodeId>) -> u32 {
    if node.is_some() {
        TYPE_SCRIPT
    } else {
        NONE
    }
}
// port: tsc/internal/ast/ast.go:NodeDefault.computeSubtreeFacts
fn default_facts() -> u32 {
    NONE
}
// port: tsc/internal/ast/ast.go:TypeSyntaxBase.computeSubtreeFacts
fn type_syntax_facts() -> u32 {
    TYPE_SCRIPT
}
fn template(flags: i32) -> u32 {
    if flags & token_flags::CONTAINS_INVALID_ESCAPE != 0 {
        INVALID_TEMPLATE_ESCAPE
    } else {
        NONE
    }
}
fn async_facts(modifiers: u32, generator: bool) -> u32 {
    if modifiers & m::ASYNC == 0 {
        NONE
    } else if generator {
        FOR_AWAIT_OR_ASYNC_GENERATOR
    } else {
        ANY_AWAIT
    }
}

impl SubtreeContext for Facts<'_> {
    // port: tsc/internal/ast/subtreefacts.go:propagateSubtreeFacts
    #[allow(clippy::match_same_arms)] // Equal current masks remain distinct source overrides.
    fn propagate_node(&mut self, id: Option<NodeId>) -> u32 {
        let Some(id) = id else { return NONE };
        let view = self.view;
        let node = view.node(id).expect("subtree propagated node");
        if node.data().is_type_syntax() {
            return propagate_type_syntax();
        }
        let facts = self.facts(id);
        use NodeDataRead as D;
        let function = COMPUTED | LEXICAL_THIS | LEXICAL_SUPER | AWAIT | OBJECT_REST_OR_SPREAD;
        let property = COMPUTED | LEXICAL_THIS | LEXICAL_SUPER;
        match node.data() {
            // port: tsc/internal/ast/ast.go:CatchClause.propagateSubtreeFacts
            D::CatchClause(_) => facts & !(COMPUTED | OBJECT_REST_OR_SPREAD),
            // port: tsc/internal/ast/ast.go:VariableDeclarationList.propagateSubtreeFacts
            D::VariableDeclarationList(_) => facts & !(COMPUTED | OBJECT_REST_OR_SPREAD),
            // port: tsc/internal/ast/ast.go:BindingPattern.propagateSubtreeFacts
            D::BindingPattern(_) => facts & !(COMPUTED | REST_OR_SPREAD),
            // port: tsc/internal/ast/ast.go:ParameterDeclaration.propagateSubtreeFacts
            D::ParameterDeclaration(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:FunctionDeclaration.propagateSubtreeFacts
            D::FunctionDeclaration(_) => facts & !function,
            // port: tsc/internal/ast/ast.go:FunctionExpression.propagateSubtreeFacts
            D::FunctionExpression(_) => facts & !function,
            // port: tsc/internal/ast/ast.go:ClassDeclaration.propagateSubtreeFacts
            D::ClassDeclaration(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:ClassExpression.propagateSubtreeFacts
            D::ClassExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:ModuleDeclaration.propagateSubtreeFacts
            D::ModuleDeclaration(_) => facts & !property,
            // port: tsc/internal/ast/ast.go:ConstructorDeclaration.propagateSubtreeFacts
            D::ConstructorDeclaration(_) => facts & !function,
            // port: tsc/internal/ast/ast.go:AccessorDeclarationBase.propagateSubtreeFacts
            D::GetAccessorDeclaration(d) => facts & !function | self.propagate_node(d.name()),
            D::SetAccessorDeclaration(d) => facts & !function | self.propagate_node(d.name()),
            // port: tsc/internal/ast/ast.go:MethodDeclaration.propagateSubtreeFacts
            D::MethodDeclaration(d) => facts & !function | self.propagate_node(d.name()),
            // port: tsc/internal/ast/ast.go:PropertyDeclaration.propagateSubtreeFacts
            D::PropertyDeclaration(d) => facts & !property | self.propagate_node(d.name()),
            // port: tsc/internal/ast/ast.go:ArrowFunction.propagateSubtreeFacts
            D::ArrowFunction(_) => facts & !(COMPUTED | AWAIT | OBJECT_REST_OR_SPREAD),
            // port: tsc/internal/ast/ast.go:AsExpression.propagateSubtreeFacts
            D::AsExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:SatisfiesExpression.propagateSubtreeFacts
            D::SatisfiesExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:PropertyAccessExpression.propagateSubtreeFacts
            D::PropertyAccessExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:ElementAccessExpression.propagateSubtreeFacts
            D::ElementAccessExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:CallExpression.propagateSubtreeFacts
            D::CallExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:NewExpression.propagateSubtreeFacts
            D::NewExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:ArrayLiteralExpression.propagateSubtreeFacts
            D::ArrayLiteralExpression(_) => facts & !COMPUTED,
            // port: tsc/internal/ast/ast.go:ObjectLiteralExpression.propagateSubtreeFacts
            D::ObjectLiteralExpression(_) => facts & !(COMPUTED | OBJECT_REST_OR_SPREAD),
            // port: tsc/internal/ast/ast.go:TypeAssertion.propagateSubtreeFacts
            D::TypeAssertion(_) => facts & !COMPUTED,
            _ => propagate_default(facts),
        }
    }
    fn propagate_list(&mut self, id: Option<NodeListId>) -> u32 {
        self.list_with(id, Self::propagate_node)
    }
    // port: tsc/internal/ast/subtreefacts.go:propagateModifierListSubtreeFacts
    fn propagate_modifiers(&mut self, id: Option<NodeListId>) -> u32 {
        self.propagate_list(id)
    }
}
// port: tsc/internal/ast/ast.go:NodeDefault.propagateSubtreeFacts
fn propagate_default(facts: u32) -> u32 {
    facts & !COMPUTED
}
// port: tsc/internal/ast/ast.go:TypeSyntaxBase.propagateSubtreeFacts
fn propagate_type_syntax() -> u32 {
    TYPE_SCRIPT
}
// port: tsc/internal/ast/ast.go:CompositeBase.computeSubtreeFacts
fn unimplemented_composite() -> u32 {
    panic!("not implemented")
}

impl Facts<'_> {
    // port: tsc/internal/ast/utilities.go:ContainsObjectRestOrSpread
    fn contains_object_rest_or_spread(&mut self, id: NodeId) -> bool {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            if self.facts(id) & OBJECT_REST_OR_SPREAD != 0 {
                return true;
            }
            if self.facts(id) & ES_OBJECT_REST_OR_SPREAD != 0 {
                let elements = self.elements_of_pattern(id);
                let view = self.view;
                let elements = view.node_slice(elements).expect("assignment elements");
                for element in elements.iter() {
                    if let Some(target) = self.target_of_element(element) {
                        if is_assignment_pattern(self.kind(Some(target))) {
                            if self.facts(target) & OBJECT_REST_OR_SPREAD != 0 {
                                return true;
                            }
                            if self.facts(target) & ES_OBJECT_REST_OR_SPREAD != 0
                                && self.contains_object_rest_or_spread(target)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        })
    }
    // port: tsc/internal/ast/utilities.go:GetElementsOfBindingOrAssignmentPattern
    fn elements_of_pattern(&self, id: NodeId) -> crate::NodeSlice {
        let node = self.view.node(id).expect("binding or assignment pattern");
        let list = match node.kind().known() {
            Some(SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern) => node
                .data_source()
                .as_binding_pattern()
                .expect("BindingPattern payload")
                .elements(),
            Some(SyntaxKind::ArrayLiteralExpression) => node
                .data_source()
                .as_array_literal_expression()
                .expect("ArrayLiteralExpression payload")
                .elements(),
            Some(SyntaxKind::ObjectLiteralExpression) => node
                .data_source()
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression payload")
                .properties(),
            _ => return crate::NodeSlice::empty(),
        };
        list.map_or_else(crate::NodeSlice::empty, |list| {
            self.view.list(list).expect("pattern list").nodes()
        })
    }
    fn is_left_hand_side(&self, id: Option<NodeId>) -> bool {
        let mut id = id.expect("nil left-hand-side expression");
        loop {
            let node = self.view.node(id).expect("left-hand-side expression");
            if node.kind() != SyntaxKind::PartiallyEmittedExpression {
                return is_left_hand_side_expression_kind(node.kind());
            }
            id = node
                .data_source()
                .as_partially_emitted_expression()
                .expect("PartiallyEmittedExpression payload")
                .expression()
                .expect("nil partially emitted expression");
        }
    }
    // port: tsc/internal/ast/utilities.go:GetTargetOfBindingOrAssignmentElement
    fn target_of_element(&self, id: Option<NodeId>) -> Option<NodeId> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let id = id.expect("nil binding or assignment element");
            let node = self.view.node(id).expect("binding or assignment element");
            use SyntaxKind as K;
            match node.kind().known() {
                Some(
                    K::VariableDeclaration
                    | K::Parameter
                    | K::BindingElement
                    | K::ShorthandPropertyAssignment,
                ) => node.data().declaration_name_generated(),
                Some(K::PropertyAssignment) => self.target_of_element(
                    node.data_source()
                        .as_property_assignment()
                        .expect("PropertyAssignment payload")
                        .initializer(),
                ),
                Some(K::SpreadAssignment) => self.target_of_element(
                    node.data_source()
                        .as_spread_assignment()
                        .expect("SpreadAssignment payload")
                        .expression(),
                ),
                Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => None,
                Some(K::BinaryExpression) => {
                    let data = node
                        .data_source()
                        .as_binary_expression()
                        .expect("BinaryExpression payload");
                    if self.kind(data.operator_token()) == K::EqualsToken
                        && self.is_left_hand_side(data.left())
                    {
                        self.target_of_element(data.left())
                    } else {
                        Some(id)
                    }
                }
                Some(K::SpreadElement) => self.target_of_element(
                    node.data_source()
                        .as_spread_element()
                        .expect("SpreadElement payload")
                        .expression(),
                ),
                _ => Some(id),
            }
        })
    }
}
// port: tsc/internal/ast/utilities.go:IsAssignmentPattern
fn is_assignment_pattern(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(SyntaxKind::ArrayLiteralExpression | SyntaxKind::ObjectLiteralExpression)
    )
}

// port: tsc/internal/ast/utilities.go:isLeftHandSideExpressionKind
pub fn is_left_hand_side_expression_kind(kind: NodeKind) -> bool {
    use SyntaxKind as K;
    matches!(
        kind.known(),
        Some(
            K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::NewExpression
                | K::CallExpression
                | K::JsxElement
                | K::JsxSelfClosingElement
                | K::JsxFragment
                | K::TaggedTemplateExpression
                | K::ArrayLiteralExpression
                | K::ParenthesizedExpression
                | K::ObjectLiteralExpression
                | K::ClassExpression
                | K::FunctionExpression
                | K::Identifier
                | K::PrivateIdentifier
                | K::RegularExpressionLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::StringLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateExpression
                | K::FalseKeyword
                | K::NullKeyword
                | K::ThisKeyword
                | K::TrueKeyword
                | K::SuperKeyword
                | K::NonNullExpression
                | K::ExpressionWithTypeArguments
                | K::MetaProperty
                | K::ImportKeyword
                | K::MissingDeclaration
        )
    )
}
