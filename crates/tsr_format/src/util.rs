//! AST helpers of the formatter (`format/util.go`).

use crate::{Error, FormatFile};
use tsr_arena::NodeId;
use tsr_ast::{NodeListId, SyntaxKind as K};
use tsr_core::TextRange;

// port: tsc/internal/format/util.go:rangeIsOnOneLine
pub(crate) fn range_is_on_one_line(
    range: TextRange,
    file: &FormatFile<'_, '_>,
) -> Result<bool, Error> {
    Ok(file.line_of(range.pos())? == file.line_of(range.end())?)
}

// port: tsc/internal/format/util.go:getOpenTokenForList
pub(crate) fn get_open_token_for_list(
    file: &FormatFile<'_, '_>,
    node: NodeId,
    list: NodeListId,
) -> Result<K, Error> {
    let node = file.node(node)?;
    let list = Some(list);
    Ok(match node.kind().known() {
        Some(
            K::Constructor
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::MethodDeclaration
            | K::MethodSignature
            | K::ArrowFunction
            | K::CallSignature
            | K::ConstructSignature
            | K::FunctionType
            | K::ConstructorType
            | K::GetAccessor
            | K::SetAccessor,
        ) => {
            if node.type_parameter_list() == list {
                K::LessThanToken
            } else if node.parameter_list() == list {
                K::OpenParenToken
            } else {
                K::Unknown
            }
        }
        Some(K::CallExpression | K::NewExpression) => {
            if node.type_argument_list() == list {
                K::LessThanToken
            } else if node.argument_list() == list {
                K::OpenParenToken
            } else {
                K::Unknown
            }
        }
        Some(
            K::ClassDeclaration
            | K::ClassExpression
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration,
        ) => {
            if node.type_parameter_list() == list {
                K::LessThanToken
            } else {
                K::Unknown
            }
        }
        Some(
            K::TypeReference
            | K::TaggedTemplateExpression
            | K::TypeQuery
            | K::ExpressionWithTypeArguments
            | K::ImportType,
        ) => {
            if node.type_argument_list() == list {
                K::LessThanToken
            } else {
                K::Unknown
            }
        }
        Some(K::TypeLiteral) => K::OpenBraceToken,
        _ => K::Unknown,
    })
}

/// Upstream notes that more pairs could be handled, brackets notably.
// port: tsc/internal/format/util.go:getCloseTokenForOpenToken
pub(crate) fn get_close_token_for_open_token(kind: K) -> K {
    match kind {
        K::OpenParenToken => K::CloseParenToken,
        K::LessThanToken => K::GreaterThanToken,
        K::OpenBraceToken => K::CloseBraceToken,
        _ => K::Unknown,
    }
}

// port: tsc/internal/format/util.go:GetLineStartPositionForPosition
pub fn get_line_start_position_for_position(
    position: i64,
    file: &FormatFile<'_, '_>,
) -> Result<i64, Error> {
    file.line_start(file.line_of(position)?)
}

/// Checking the kind makes sure the token was typed in the expected context,
/// not inside a comment for instance.
// port: tsc/internal/format/util.go:findImmediatelyPrecedingTokenOfKind
pub(crate) fn find_immediately_preceding_token_of_kind(
    end: i64,
    expected: K,
    file: &mut FormatFile<'_, '_>,
) -> Result<Option<NodeId>, Error> {
    let Some(token) = file.navigator().find_preceding_token(end)? else {
        return Ok(None);
    };
    let node = file.node(token)?;
    Ok((node.kind() == expected && i64::from(node.end()) == end).then_some(token))
}

/// The highest node enclosing `node` at the same list level whose end does not
/// exceed the node's. Typing the closing brace of a `while` formats the whole
/// statement, not the declaration before it.
// port: tsc/internal/format/util.go:findOutermostNodeWithinListLevel
pub(crate) fn find_outermost_node_within_list_level(
    file: &FormatFile<'_, '_>,
    node: NodeId,
) -> Result<NodeId, Error> {
    let end = file.node(node)?.end();
    let mut current = node;
    while let Some(parent) = file.node(current)?.parent() {
        if file.node(parent)?.end() != end || is_list_element(file, parent, current)? {
            break;
        }
        current = parent;
    }
    Ok(current)
}

fn contained_by(
    file: &FormatFile<'_, '_>,
    node: NodeId,
    list: Option<NodeListId>,
) -> Result<bool, Error> {
    let Some(list) = list else {
        return Ok(false);
    };
    let (inner, outer) = (file.node(node)?.range(), file.view.list(list)?.loc());
    Ok(inner.pos() >= outer.pos() && inner.end() <= outer.end())
}

/// Whether the node is an element of some list of its parent, as a member of a
/// class is.
// port: tsc/internal/format/util.go:isListElement
pub(crate) fn is_list_element(
    file: &FormatFile<'_, '_>,
    parent: NodeId,
    node: NodeId,
) -> Result<bool, Error> {
    let read = file.node(parent)?;
    match read.kind().known() {
        Some(K::ClassDeclaration | K::InterfaceDeclaration) => {
            contained_by(file, node, read.member_list())
        }
        Some(K::ModuleDeclaration) => {
            let Some(body) = read.body() else {
                return Ok(false);
            };
            let body = file.node(body)?;
            if body.kind() != K::ModuleBlock {
                return Ok(false);
            }
            contained_by(file, node, body.statement_list())
        }
        Some(K::SourceFile | K::Block | K::ModuleBlock) => {
            contained_by(file, node, read.statement_list())
        }
        Some(K::CatchClause) => {
            let block = read
                .data_source()
                .as_catch_clause()
                .and_then(|clause| clause.block())
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            contained_by(file, node, file.node(block)?.statement_list())
        }
        _ => Ok(false),
    }
}

// port: tsc/internal/format/util.go:isMemberListElement
pub(crate) fn is_member_list_element(
    file: &FormatFile<'_, '_>,
    parent: NodeId,
    node: NodeId,
) -> Result<bool, Error> {
    let read = file.node(parent)?;
    match read.kind().known() {
        Some(
            K::ClassDeclaration
            | K::ClassExpression
            | K::InterfaceDeclaration
            | K::EnumDeclaration
            | K::TypeLiteral
            | K::MappedType,
        ) => contained_by(file, node, read.member_list()),
        _ => Ok(false),
    }
}
