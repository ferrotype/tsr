//! Syntax predicates shared by value resolution and const-enum use checks.
//! Parent context distinguishes names in type syntax from evaluated expressions.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/ast/utilities.go:IsValidTypeOnlyAliasUseSite
    pub(crate) fn valid_type_only_alias_use_site(&self, node: NodeId) -> Result<bool, Error> {
        if self.ast(node)?.node(node)?.flags() & (nf::AMBIENT | nf::JS_DOC) != 0
            || ts_ast::is_part_of_type_query(self.ast(node)?, node)?
        {
            return Ok(true);
        }
        if self.ast(node)?.node(node)?.kind() == K::Identifier {
            let mut parent = self.ast(node)?.node(node)?.parent();
            while let Some(id) = parent {
                let read = self.ast(id)?.node(id)?;
                if !matches!(
                    read.kind().known(),
                    Some(K::PropertyAccessExpression | K::ExpressionWithTypeArguments)
                ) {
                    break;
                }
                parent = read.parent();
            }
            if let Some(parent) = parent {
                let read = self.ast(parent)?.node(parent)?;
                if read.kind() == K::HeritageClause {
                    let clause = read
                        .data_source()
                        .as_heritage_clause()
                        .ok_or(Error::MissingLink("type-only heritage"))?;
                    let owner = read.parent().ok_or(Error::MissingLink("heritage owner"))?;
                    if clause.token() == K::ImplementsKeyword
                        || self.ast(owner)?.node(owner)?.kind() == K::InterfaceDeclaration
                    {
                        return Ok(true);
                    }
                }
            }
        }
        let mut current = node;
        while matches!(
            self.ast(current)?.node(current)?.kind().known(),
            Some(K::Identifier | K::PropertyAccessExpression)
        ) {
            let Some(parent) = self.ast(current)?.node(current)?.parent() else {
                break;
            };
            current = parent;
        }
        if self.ast(current)?.node(current)?.kind() == K::ComputedPropertyName {
            let member = self
                .ast(current)?
                .node(current)?
                .parent()
                .ok_or(Error::MissingLink("computed member"))?;
            let read = self.ast(member)?.node(member)?;
            if read.modifier_flags(self.ast(member)?)? & mf::ABSTRACT != 0 {
                return Ok(true);
            }
            let owner = read
                .parent()
                .ok_or(Error::MissingLink("computed member owner"))?;
            if matches!(
                self.ast(owner)?.node(owner)?.kind().known(),
                Some(K::InterfaceDeclaration | K::TypeLiteral)
            ) {
                return Ok(true);
            }
        }
        let shorthand = if self.ast(node)?.node(node)?.kind() == K::Identifier {
            match self.ast(node)?.node(node)?.parent() {
                Some(parent) => {
                    self.ast(parent)?.node(parent)?.kind() == K::ShorthandPropertyAssignment
                        && self.ast(parent)?.node(parent)?.name() == Some(node)
                }
                None => false,
            }
        } else {
            false
        };
        Ok(!self.expression_node(node)? && !shorthand)
    }

    // port: tsc/internal/ast/utilities.go:IsExpressionNode
    pub(crate) fn expression_node(&self, mut node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::SuperKeyword
                | K::NullKeyword
                | K::TrueKeyword
                | K::FalseKeyword
                | K::RegularExpressionLiteral
                | K::ArrayLiteralExpression
                | K::ObjectLiteralExpression
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::CallExpression
                | K::NewExpression
                | K::TaggedTemplateExpression
                | K::AsExpression
                | K::TypeAssertionExpression
                | K::SatisfiesExpression
                | K::NonNullExpression
                | K::ParenthesizedExpression
                | K::FunctionExpression
                | K::ClassExpression
                | K::ArrowFunction
                | K::VoidExpression
                | K::DeleteExpression
                | K::TypeOfExpression
                | K::PrefixUnaryExpression
                | K::PostfixUnaryExpression
                | K::BinaryExpression
                | K::ConditionalExpression
                | K::SpreadElement
                | K::TemplateExpression
                | K::OmittedExpression
                | K::JsxElement
                | K::JsxSelfClosingElement
                | K::JsxFragment
                | K::YieldExpression
                | K::AwaitExpression,
            ) => true,
            Some(K::MetaProperty) => {
                if let Some(parent) = read.parent() {
                    let parent_read = self.ast(parent)?.node(parent)?;
                    if parent_read.kind() == K::CallExpression {
                        let expression = parent_read
                            .expression()
                            .ok_or(Error::MissingLink("import call"))?;
                        let expr_read = self.ast(expression)?.node(expression)?;
                        let import_call = expr_read.kind() == K::ImportKeyword
                            || if let Some(data) = expr_read.data_source().as_meta_property() {
                                if data.keyword_token() == K::ImportKeyword {
                                    let name = data
                                        .name()
                                        .ok_or(Error::MissingLink("import meta name"))?;
                                    self.ast(name)?.node_text(name)?.as_bytes() == b"defer"
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                        !import_call || expression != node
                    } else {
                        true
                    }
                } else {
                    true
                }
            }
            Some(K::ExpressionWithTypeArguments) => match read.parent() {
                Some(parent) => self.ast(parent)?.node(parent)?.kind() != K::HeritageClause,
                None => true,
            },
            Some(K::QualifiedName) => {
                while let Some(parent) = self.ast(node)?.node(node)?.parent() {
                    if self.ast(parent)?.node(parent)?.kind() != K::QualifiedName {
                        break;
                    }
                    node = parent;
                }
                self.type_only_name_expression_context(node)?
            }
            Some(K::PrivateIdentifier) => {
                if let Some(parent) = read.parent() {
                    let parent_read = self.ast(parent)?.node(parent)?;
                    if let Some(data) = parent_read.data_source().as_binary_expression() {
                        let token = data
                            .operator_token()
                            .ok_or(Error::MissingLink("private-name operator"))?;
                        data.left() == Some(node)
                            && self.ast(token)?.node(token)?.kind() == K::InKeyword
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            Some(K::Identifier) => {
                self.type_only_name_expression_context(node)? || self.in_expression_context(node)?
            }
            Some(
                K::NumericLiteral
                | K::BigIntLiteral
                | K::StringLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::ThisKeyword,
            ) => self.in_expression_context(node)?,
            _ => false,
        })
    }

    fn type_only_name_expression_context(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        Ok(match read.kind().known() {
            Some(
                K::TypeQuery
                | K::JSDocLink
                | K::JSDocLinkCode
                | K::JSDocLinkPlain
                | K::JSDocNameReference,
            ) => true,
            Some(K::JsxOpeningElement | K::JsxClosingElement | K::JsxSelfClosingElement) => {
                read.tag_name() == Some(node)
            }
            _ => false,
        })
    }

    // port: tsc/internal/ast/utilities.go:IsInExpressionContext
    pub(crate) fn in_expression_context(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        Ok(match read.kind().known() {
            Some(
                K::VariableDeclaration
                | K::Parameter
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::EnumMember
                | K::PropertyAssignment
                | K::BindingElement,
            ) => read.initializer() == Some(node),
            Some(
                K::ExpressionStatement
                | K::IfStatement
                | K::DoStatement
                | K::WhileStatement
                | K::ReturnStatement
                | K::WithStatement
                | K::SwitchStatement
                | K::CaseClause
                | K::DefaultClause
                | K::ThrowStatement
                | K::TypeAssertionExpression
                | K::AsExpression
                | K::TemplateSpan
                | K::ComputedPropertyName
                | K::SatisfiesExpression,
            ) => read.expression() == Some(node),
            Some(K::ForStatement) => {
                let data = read
                    .data_source()
                    .as_for_statement()
                    .ok_or(Error::MissingLink("expression-context for"))?;
                data.initializer() == Some(node)
                    && self.ast(node)?.node(node)?.kind() != K::VariableDeclarationList
                    || data.condition() == Some(node)
                    || data.incrementor() == Some(node)
            }
            Some(K::ForInStatement | K::ForOfStatement) => {
                let data = read
                    .data_source()
                    .as_for_in_or_of_statement()
                    .ok_or(Error::MissingLink("expression-context for-in/of"))?;
                data.initializer() == Some(node)
                    && self.ast(node)?.node(node)?.kind() != K::VariableDeclarationList
                    || data.expression() == Some(node)
            }
            Some(K::Decorator | K::JsxExpression | K::JsxSpreadAttribute | K::SpreadAssignment) => {
                true
            }
            Some(K::ExpressionWithTypeArguments) => {
                read.expression() == Some(node) && !self.is_part_of_type_node(parent)?
            }
            Some(K::ShorthandPropertyAssignment) => {
                read.data_source()
                    .as_shorthand_property_assignment()
                    .ok_or(Error::MissingLink("shorthand context"))?
                    .object_assignment_initializer()
                    == Some(node)
            }
            _ => self.expression_node(parent)?,
        })
    }
}
