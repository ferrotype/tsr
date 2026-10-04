//! Syntax decisions shared by completion collection and item resolution.
use crate::{
    completion_keywords::{class_keyword, Filter},
    syntax::Syntax,
    Result,
};
use tsr_ast::{utilities as ast, utilities_positions as positions, NodeId, SyntaxKind as K};

pub(crate) struct Context {
    pub location: NodeId,
    pub previous: Option<NodeId>,
    pub token: Option<NodeId>,
    pub member: Option<(NodeId, NodeId)>,
    pub container: Option<(Container, NodeId)>,
    pub type_only: bool,
    pub filter: Filter,
    pub new_identifier: bool,
    pub commit: &'static [&'static str],
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Container {
    Object,
    Binding,
    Class,
    Interface,
    Imports,
    Exports,
    Constructor,
    Jsx,
}
pub(crate) const ALL: &[&str] = &[".", ",", ";"];
const NO_COMMA: &[&str] = &[".", ";"];

// port: tsc/internal/ls/completions.go:getRelevantTokens
pub(crate) fn relevant(
    syntax: &mut Syntax<'_>,
    position: i64,
) -> Result<(Option<NodeId>, Option<NodeId>)> {
    let previous = syntax.nav().find_preceding_token(position)?;
    if let Some(previous) = previous {
        let read = syntax.view.node(previous)?;
        if position <= i64::from(read.end())
            && (ast::is_member_name(&read)
                || matches!(read.kind().known(), Some(k) if (K::BreakKeyword..=K::OfKeyword).contains(&k)))
        {
            return Ok((
                syntax.nav().find_preceding_token(i64::from(read.pos()))?,
                Some(previous),
            ));
        }
    }
    Ok((previous, previous))
}

// port: tsc/internal/ls/completions.go:keywordForNode
fn keyword(syntax: &Syntax<'_>, id: NodeId) -> Result<K> {
    let node = syntax.view.node(id)?;
    Ok(if node.kind() == K::Identifier {
        {
            let k = tsr_scanner::string_to_token(syntax.view.node_text(id)?.as_bytes());
            if k == K::Unknown {
                K::Identifier
            } else {
                k
            }
        }
    } else {
        node.kind().known().unwrap_or(K::Unknown)
    })
}

// port: tsc/internal/ls/completions.go:computeCommitCharactersAndIsNewIdentifier
fn commits(
    syntax: &Syntax<'_>,
    token: Option<NodeId>,
    position: i64,
) -> Result<(bool, &'static [&'static str])> {
    let Some(token) = token else {
        return Ok((false, ALL));
    };
    let node = syntax.view.node(token)?;
    let parent = node.parent().unwrap_or(syntax.source);
    let read = syntax.view.node(parent)?;
    let kind = read.kind().known().unwrap_or(K::Unknown);
    let token_kind = keyword(syntax, token)?;
    Ok(match token_kind {
        K::CommaToken | K::OpenParenToken
            if matches!(kind, K::CallExpression | K::NewExpression) =>
        {
            let expression = read.expression().unwrap_or(parent);
            (
                true,
                if syntax.same_line(i64::from(syntax.view.node(expression)?.end()), position) {
                    ALL
                } else {
                    NO_COMMA
                },
            )
        }
        K::CommaToken => match kind {
            K::BinaryExpression => (true, NO_COMMA),
            K::Constructor | K::FunctionType | K::ObjectLiteralExpression => (true, &[]),
            K::ArrayLiteralExpression => (true, ALL),
            _ => (false, ALL),
        },
        K::OpenParenToken => match kind {
            K::ParenthesizedExpression => (true, NO_COMMA),
            K::Constructor | K::ParenthesizedType => (true, &[]),
            _ => (false, ALL),
        },
        K::OpenBracketToken => (
            matches!(
                kind,
                K::ArrayLiteralExpression
                    | K::IndexSignature
                    | K::TupleType
                    | K::ComputedPropertyName
            ),
            ALL,
        ),
        K::ModuleKeyword | K::NamespaceKeyword | K::ImportKeyword => (true, &[]),
        K::DotToken if kind == K::ModuleDeclaration => (true, &[]),
        K::OpenBraceToken if matches!(kind, K::ClassDeclaration | K::ObjectLiteralExpression) => {
            (true, &[])
        }
        K::EqualsToken => (
            matches!(kind, K::VariableDeclaration | K::BinaryExpression),
            ALL,
        ),
        K::TemplateHead => (kind == K::TemplateExpression, ALL),
        K::TemplateMiddle => (kind == K::TemplateSpan, ALL),
        K::AsyncKeyword
            if matches!(kind, K::MethodDeclaration | K::ShorthandPropertyAssignment) =>
        {
            (true, &[])
        }
        K::AsteriskToken if kind == K::MethodDeclaration => (true, &[]),
        _ if class_keyword(token_kind) => (true, &[]),
        _ => (false, ALL),
    })
}

impl Context {
    pub fn collect(syntax: &mut Syntax<'_>, position: i64) -> Result<Self> {
        let (token, previous) = relevant(syntax, position)?;
        let mut location = syntax.nav().get_touching_property_name(position)?;
        let (mut new_identifier, mut commit) = commits(syntax, token, position)?;
        let mut member = None;
        if let Some(token) = token {
            let read = syntax.view.node(token)?;
            if matches!(read.kind().known(), Some(K::DotToken | K::QuestionDotToken)) {
                if let Some(parent) = read.parent() {
                    let read = syntax.view.node(parent)?;
                    let expression = match read.kind().known() {
                        Some(K::PropertyAccessExpression) => read.expression(),
                        Some(K::QualifiedName) => read
                            .data_source()
                            .as_qualified_name()
                            .and_then(|n| n.left()),
                        Some(K::ModuleDeclaration) => read.name(),
                        Some(K::ImportType) => Some(parent),
                        _ => None,
                    };
                    member = expression.map(|expression| (parent, expression));
                }
            }
        }
        let mut type_only = positions::is_part_of_type_node(syntax.view, location)?;
        if let Some((access, _)) = member {
            type_only |= positions::is_part_of_type_node(syntax.view, access)?;
        }
        if let Some(id) = token {
            let read = syntax.view.node(id)?;
            let parent = syntax.view.node(read.parent().unwrap_or(syntax.source))?;
            type_only |= match read.kind().known() {
                Some(K::ColonToken) => {
                    matches!(
                        parent.kind().known(),
                        Some(
                            K::PropertyDeclaration
                                | K::PropertySignature
                                | K::Parameter
                                | K::VariableDeclaration
                        )
                    ) || ast::is_function_like_kind(parent.kind())
                }
                Some(K::EqualsToken) => matches!(
                    parent.kind().known(),
                    Some(K::TypeAliasDeclaration | K::TypeParameter)
                ),
                Some(K::AsKeyword) => parent.kind() == K::AsExpression,
                Some(K::LessThanToken) => matches!(
                    parent.kind().known(),
                    Some(K::TypeReference | K::TypeAssertionExpression)
                ),
                Some(K::ExtendsKeyword) => parent.kind() == K::TypeParameter,
                Some(K::SatisfiesKeyword) => parent.kind() == K::SatisfiesExpression,
                Some(K::OpenBracketToken | K::CommaToken) => parent.kind() == K::TupleType,
                _ => false,
            };
            if matches!(
                read.kind().known(),
                Some(K::TypeOfKeyword | K::AssertsKeyword)
            ) {
                type_only = false;
            }
        }
        let mut filter = if type_only { Filter::Type } else { Filter::All };
        let mut container = None;
        if let Some(id) = token {
            let read = syntax.view.node(id)?;
            let parent = read.parent().unwrap_or(syntax.source);
            let pr = syntax.view.node(parent)?;
            let mut target = None;
            if matches!(read.kind().known(), Some(K::OpenBraceToken | K::CommaToken)) {
                target = Some(parent);
            } else if matches!(
                read.kind().known(),
                Some(K::SemicolonToken | K::CloseBraceToken)
            ) {
                if ast::is_class_like(&syntax.view.node(location)?)
                    || matches!(
                        syntax.view.node(location)?.kind().known(),
                        Some(K::InterfaceDeclaration | K::TypeLiteral)
                    )
                {
                    target = Some(location);
                }
            } else if class_keyword(keyword(syntax, id)?) {
                target = pr.parent();
            }
            if let Some(target) = target {
                container = match syntax.view.node(target)?.kind().known() {
                    Some(K::ObjectLiteralExpression) => Some((Container::Object, target)),
                    Some(K::ObjectBindingPattern) => Some((Container::Binding, target)),
                    Some(K::ClassDeclaration | K::ClassExpression) => {
                        Some((Container::Class, target))
                    }
                    Some(K::InterfaceDeclaration | K::TypeLiteral) => {
                        Some((Container::Interface, target))
                    }
                    Some(K::NamedImports) => Some((Container::Imports, target)),
                    Some(K::NamedExports) => Some((Container::Exports, target)),
                    _ => None,
                };
            }
            if matches!(read.kind().known(), Some(K::OpenParenToken | K::CommaToken))
                && pr.kind() == K::Constructor
            {
                container = Some((Container::Constructor, parent));
            }
            if type_only
                && matches!(
                    pr.kind().known(),
                    Some(K::AsExpression | K::TypeAssertionExpression)
                )
            {
                filter = Filter::TypeAssertion;
            }
        }
        if member.is_none() {
            let mut jsx_token = token;
            if let Some(id) = jsx_token {
                if let Some(parent) = syntax.view.node(id)?.parent() {
                    if syntax.view.node(parent)?.kind() == K::PropertyAccessExpression {
                        jsx_token = Some(parent);
                    }
                }
            }
            if let Some(opening) = crate::completion_jsx::container(syntax, jsx_token)? {
                container = Some((Container::Jsx, opening));
            }
        }
        let mut node = token;
        let mut child = None;
        while let Some(id) = node {
            let read = syntax.view.node(id)?;
            if ast::is_class_like(&read) {
                break;
            }
            if ast::is_function_like_declaration(Some(&read))
                && read.body() == child
                && child.is_some()
            {
                if !type_only {
                    filter = Filter::FunctionBody;
                }
                break;
            }
            child = node;
            node = read.parent();
        }
        if let Some((kind, _)) = container {
            filter = match kind {
                Container::Class => Filter::Class,
                Container::Interface => Filter::Interface,
                Container::Constructor => Filter::ConstructorParameter,
                Container::Imports | Container::Exports => Filter::TypeKeyword,
                _ => Filter::None,
            };
            new_identifier = matches!(
                kind,
                Container::Class | Container::Interface | Container::Constructor
            );
            commit = if new_identifier { &[] } else { ALL };
        }
        if let Some(token) = token {
            let read = syntax.view.node(token)?;
            if matches!(
                read.kind().known(),
                Some(K::LessThanToken | K::LessThanSlashToken)
            ) && read.parent().is_some_and(|p| {
                syntax.view.node(p).is_ok_and(|n| {
                    matches!(
                        n.kind().known(),
                        Some(
                            K::JsxOpeningElement
                                | K::JsxSelfClosingElement
                                | K::JsxClosingElement
                                | K::BinaryExpression
                        )
                    )
                })
            }) {
                location = token;
            }
        }
        Ok(Self {
            location,
            previous,
            token,
            member,
            container,
            type_only,
            filter,
            new_identifier,
            commit,
        })
    }
    pub fn blocked(&self, syntax: &Syntax<'_>, position: i64) -> Result<bool> {
        let Some(token) = self.token else {
            return Ok(false);
        };
        let read = syntax.view.node(token)?;
        let parent = read.parent().unwrap_or(syntax.source);
        let pr = syntax.view.node(parent)?;
        let kind = pr.kind().known().unwrap_or(K::Unknown);
        let tk = keyword(syntax, token)?;
        if read.kind() == K::JsxText || read.kind() == K::BigIntLiteral {
            return Ok(true);
        }
        if read.kind() == K::NumericLiteral
            && syntax.file.text().as_bytes().get(read.end() as usize - 1) == Some(&b'.')
        {
            return Ok(true);
        }
        let function = ast::is_function_like_kind(pr.kind()) && kind != K::Constructor;
        let certain = match tk {
            K::CommaToken => {
                matches!(
                    kind,
                    K::VariableDeclaration
                        | K::VariableDeclarationList
                        | K::VariableStatement
                        | K::EnumDeclaration
                        | K::InterfaceDeclaration
                        | K::ArrayBindingPattern
                        | K::TypeAliasDeclaration
                ) || function
            }
            K::DotToken | K::OpenBracketToken => kind == K::ArrayBindingPattern,
            K::ColonToken => kind == K::BindingElement,
            K::OpenParenToken => kind == K::CatchClause || function,
            K::OpenBraceToken => kind == K::EnumDeclaration,
            K::LessThanToken => {
                matches!(
                    kind,
                    K::ClassDeclaration
                        | K::ClassExpression
                        | K::InterfaceDeclaration
                        | K::TypeAliasDeclaration
                ) || ast::is_function_like_kind(pr.kind())
            }
            K::DotDotDotToken => kind == K::Parameter,
            K::AsKeyword => matches!(
                kind,
                K::ImportSpecifier | K::ExportSpecifier | K::NamespaceImport
            ),
            K::ClassKeyword
            | K::EnumKeyword
            | K::InterfaceKeyword
            | K::FunctionKeyword
            | K::VarKeyword
            | K::ImportKeyword
            | K::LetKeyword
            | K::ConstKeyword
            | K::InferKeyword => true,
            K::TypeKeyword => kind != K::ImportSpecifier,
            K::AsteriskToken => ast::is_function_like(Some(&pr)) && kind != K::MethodDeclaration,
            _ => false,
        };
        if certain {
            return Ok(true);
        }
        if read.kind() == K::Identifier && !syntax.same_line(i64::from(read.end()), position) {
            let mut node = Some(parent);
            while let Some(id) = node {
                let read = syntax.view.node(id)?;
                if read.kind() == K::VariableDeclaration {
                    return Ok(false);
                }
                node = read.parent();
            }
        }
        Ok(positions::is_declaration_name(syntax.view, token)?
            && !matches!(kind, K::ShorthandPropertyAssignment | K::JsxAttribute)
            && !(matches!(
                kind,
                K::ClassDeclaration
                    | K::ClassExpression
                    | K::InterfaceDeclaration
                    | K::TypeParameter
            ) && (self.token != self.previous || position > i64::from(read.end()))))
    }
}
