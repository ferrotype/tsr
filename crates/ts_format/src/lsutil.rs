//! The language-service helpers the formatter calls (`ls/lsutil/children.go`
//! and `ls/lsutil/asi.go`). They live here until that crate exists.

use crate::{Error, FormatFile};
use std::ops::ControlFlow;
use ts_arena::NodeId;
use ts_ast::{
    node_flags, utilities, utilities_middle, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K,
};
use ts_astnav::ChildVisit;

// port: tsc/internal/ls/lsutil/children.go:AssertHasRealPosition
fn assert_has_real_position(file: &FormatFile<'_, '_>, node: NodeId) -> Result<(), Error> {
    let read = file.node(node)?;
    if read.pos() < 0 || read.end() < 0 {
        return Err(Error::Assertion(
            "Node must have a real position for this operation.".into(),
        ));
    }
    Ok(())
}

/// The last visited child. It does not include tokens no child covers; for
/// those use [`get_last_child`] or [`get_last_token`].
// port: tsc/internal/ls/lsutil/children.go:GetLastVisitedChild
pub(crate) fn get_last_visited_child(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let mut last = None;
    for visit in file.navigator().visit_each_child_and_jsdoc(node)? {
        match visit {
            ChildVisit::Node(child) => {
                if file.node(child)?.flags() & node_flags::REPARSED == 0 {
                    last = Some(child);
                }
            }
            ChildVisit::List(nodes) => {
                for &child in nodes.iter().rev() {
                    if file.node(child)?.flags() & node_flags::REPARSED == 0 {
                        last = Some(child);
                        break;
                    }
                }
            }
        }
    }
    Ok(last)
}

/// Replaces `last(node.getChildren(sourceFile))`.
// port: tsc/internal/ls/lsutil/children.go:GetLastChild
pub(crate) fn get_last_child(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let last_child = get_last_visited_child(file, node)?;
    if last_child.is_none()
        && utilities_middle::is_js_doc_single_comment_node(file.view, &file.node(node)?)?
    {
        return Ok(None);
    }
    let start = match last_child {
        Some(child) => i64::from(file.node(child)?.end()),
        None => i64::from(file.node(node)?.pos()),
    };
    let end = i64::from(file.node(node)?.end());
    let view = file.view;
    let state = view.source_file(file.source)?;
    let mut scanner = ts_scanner::get_scanner_for_source_file(&state, start);
    let mut last_token = None;
    let mut position = start;
    while position < end {
        let (kind, full_start, token_end) = (
            scanner.token(),
            scanner.token_full_start(),
            scanner.token_end(),
        );
        let (Ok(pos), Ok(finish)) = (i32::try_from(full_start), i32::try_from(token_end)) else {
            return Err(ts_arena::Error::InvalidTokenRange.into());
        };
        last_token = Some(
            view.get_or_create_token(kind, pos, finish, node, scanner.token_flags())?
                .id(),
        );
        position = token_end;
        scanner.scan();
    }
    Ok(last_token.or(last_child))
}

// port: tsc/internal/ls/lsutil/children.go:GetLastToken
pub(crate) fn get_last_token(
    file: &mut FormatFile<'_, '_>,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let Some(node) = node else {
        return Ok(None);
    };
    let kind = file.node(node)?.kind();
    if ts_ast::is_token_kind(kind) || kind == K::Identifier {
        return Ok(None);
    }
    assert_has_real_position(file, node)?;
    let Some(last) = get_last_child(file, node)? else {
        return Ok(None);
    };
    if file.node(last)?.kind().raw() < K::FirstNode as i16 {
        Ok(Some(last))
    } else {
        get_last_token(file, Some(last))
    }
}

/// The first child `ForEachChild` reaches.
struct FirstChild<'v> {
    view: ts_ast::AstView<'v>,
    found: Option<NodeId>,
    error: Option<ts_arena::Error>,
}

impl FirstChild<'_> {
    fn take(&mut self, nodes: Result<Option<NodeId>, ts_arena::Error>) -> ControlFlow<()> {
        match nodes {
            Ok(Some(node)) => {
                self.found = Some(node);
                ControlFlow::Break(())
            }
            Ok(None) => ControlFlow::Continue(()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

impl ChildVisitor for FirstChild<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.found = Some(node);
        ControlFlow::Break(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        let first = self
            .view
            .list(nodes)
            .and_then(|list| self.view.node_slice(list.nodes()))
            .map(|slice| slice.iter().flatten().next());
        self.take(first)
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        let first = self
            .view
            .node_slice(nodes)
            .map(|slice| slice.iter().flatten().next());
        self.take(first)
    }
}

// port: tsc/internal/ls/lsutil/children.go:GetFirstToken
pub(crate) fn get_first_token(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let (kind, pos, end, flags) = {
        let read = file.node(node)?;
        (
            read.kind(),
            i64::from(read.pos()),
            i64::from(read.end()),
            read.flags(),
        )
    };
    if kind == K::Identifier || ts_ast::is_token_kind(kind) {
        return Ok(None);
    }
    assert_has_real_position(file, node)?;
    // Upstream tests the flags of the node, not of the child, so a reparsed
    // node has no first child here.
    let first_child = if flags & node_flags::REPARSED != 0 {
        None
    } else {
        let mut visitor = FirstChild {
            view: file.view,
            found: None,
            error: None,
        };
        let _ = file.node(node)?.for_each_child(&mut visitor);
        if let Some(error) = visitor.error {
            return Err(error.into());
        }
        visitor.found
    };
    let token_end_position = match first_child {
        Some(child) => i64::from(file.node(child)?.pos()),
        None => end,
    };
    if pos < token_end_position {
        let view = file.view;
        let state = view.source_file(file.source)?;
        let scanner = ts_scanner::get_scanner_for_source_file(&state, pos);
        let (Ok(start), Ok(finish)) = (
            i32::try_from(scanner.token_full_start()),
            i32::try_from(scanner.token_end()),
        ) else {
            return Err(ts_arena::Error::InvalidTokenRange.into());
        };
        let token =
            view.get_or_create_token(scanner.token(), start, finish, node, scanner.token_flags())?;
        return Ok(Some(token.id()));
    }
    let Some(first_child) = first_child else {
        return Ok(None);
    };
    if file.node(first_child)?.kind().raw() < K::FirstNode as i16 {
        return Ok(Some(first_child));
    }
    get_first_token(file, first_child)
}

// port: tsc/internal/ls/lsutil/asi.go:SyntaxRequiresTrailingCommaOrSemicolonOrASI
fn syntax_requires_trailing_comma_or_semicolon_or_asi(kind: Option<K>) -> bool {
    matches!(
        kind,
        Some(
            K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::PropertySignature
                | K::MethodSignature
        )
    )
}

// port: tsc/internal/ls/lsutil/asi.go:SyntaxRequiresTrailingFunctionBlockOrSemicolonOrASI
fn syntax_requires_trailing_function_block_or_semicolon_or_asi(kind: Option<K>) -> bool {
    matches!(
        kind,
        Some(
            K::FunctionDeclaration
                | K::Constructor
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
        )
    )
}

// port: tsc/internal/ls/lsutil/asi.go:SyntaxRequiresTrailingModuleBlockOrSemicolonOrASI
fn syntax_requires_trailing_module_block_or_semicolon_or_asi(kind: Option<K>) -> bool {
    kind == Some(K::ModuleDeclaration)
}

// port: tsc/internal/ls/lsutil/asi.go:SyntaxRequiresTrailingSemicolonOrASI
fn syntax_requires_trailing_semicolon_or_asi(kind: Option<K>) -> bool {
    matches!(
        kind,
        Some(
            K::VariableStatement
                | K::ExpressionStatement
                | K::DoStatement
                | K::ContinueStatement
                | K::BreakStatement
                | K::ReturnStatement
                | K::ThrowStatement
                | K::DebuggerStatement
                | K::PropertyDeclaration
                | K::TypeAliasDeclaration
                | K::ImportDeclaration
                | K::ImportEqualsDeclaration
                | K::ExportDeclaration
                | K::NamespaceExportDeclaration
                | K::ExportAssignment
        )
    )
}

// port: tsc/internal/ls/lsutil/asi.go:SyntaxMayBeASICandidate
fn syntax_may_be_asi_candidate(kind: Option<K>) -> bool {
    syntax_requires_trailing_comma_or_semicolon_or_asi(kind)
        || syntax_requires_trailing_function_block_or_semicolon_or_asi(kind)
        || syntax_requires_trailing_module_block_or_semicolon_or_asi(kind)
        || syntax_requires_trailing_semicolon_or_asi(kind)
}

// port: tsc/internal/ls/lsutil/asi.go:PositionIsASICandidate
pub(crate) fn position_is_asi_candidate(
    pos: i64,
    context: NodeId,
    file: &mut FormatFile<'_, '_>,
) -> Result<bool, Error> {
    let ancestor = utilities::find_ancestor_or_quit(file.view, Some(context), |ancestor| {
        if i64::from(ancestor.end()) != pos {
            return utilities::FindAncestorResult::QUIT;
        }
        utilities::to_find_ancestor_result(syntax_may_be_asi_candidate(ancestor.kind().known()))
    })?;
    match ancestor {
        Some(ancestor) => node_is_asi_candidate(ancestor, file),
        None => Ok(false),
    }
}

// port: tsc/internal/ls/lsutil/asi.go:NodeIsASICandidate
fn node_is_asi_candidate(node: NodeId, file: &mut FormatFile<'_, '_>) -> Result<bool, Error> {
    let last_token = get_last_token(file, Some(node))?;
    let last_kind = match last_token {
        Some(token) => file.node(token)?.kind().known(),
        None => None,
    };
    if last_kind == Some(K::SemicolonToken) {
        return Ok(false);
    }
    let kind = file.node(node)?.kind().known();
    if syntax_requires_trailing_comma_or_semicolon_or_asi(kind) {
        if last_kind == Some(K::CommaToken) {
            return Ok(false);
        }
    } else if syntax_requires_trailing_module_block_or_semicolon_or_asi(kind) {
        if let Some(last_child) = get_last_child(file, node)? {
            if file.node(last_child)?.kind() == K::ModuleBlock {
                return Ok(false);
            }
        }
    } else if syntax_requires_trailing_function_block_or_semicolon_or_asi(kind) {
        let last_child = get_last_child(file, node)?;
        if last_child.is_some() && utilities::is_function_block(file.view, last_child)? {
            return Ok(false);
        }
    } else if !syntax_requires_trailing_semicolon_or_asi(kind) {
        return Ok(false);
    }
    // See the comment in the parser's `parseDoStatement`.
    if kind == Some(K::DoStatement) {
        return Ok(true);
    }
    let top = utilities::find_ancestor(file.view, Some(node), |ancestor| {
        ancestor.parent().is_none()
    })?
    .ok_or(ts_arena::Error::InvalidGraph)?;
    let Some(next) = file.navigator().find_next_token(node, top)? else {
        return Ok(true);
    };
    if file.node(next)?.kind() == K::CloseBraceToken {
        return Ok(true);
    }
    let start_line = file.line_of(i64::from(file.node(node)?.end()))?;
    let next_start = file.navigator().get_start_of_node(next, false)?;
    Ok(start_line != file.line_of(next_start)?)
}
