//! Source text and diagnostic ranges for the binder's AST-facing scanner calls.
use crate::{normalize_jsdoc_type_source_text, skip_trivia, Scanner};
use tsr_arena::Error;
use tsr_ast::{
    node_flags, token_flags, AstView, NodeId, NodeListId, SourceFileState, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_jsstring::{scanner_positions, JsString};

/// port: tsc/internal/scanner/utilities.go:isJSDocTypeExpressionOrChild
fn is_jsdoc_type_expression_or_child(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if node.kind() == K::JSDocTypeExpression {
        return Ok(true);
    }
    if node.flags() & (node_flags::JS_DOC | node_flags::REPARSED) == 0 {
        return Ok(false);
    }
    let mut current = Some(id);
    while let Some(id) = current {
        let node = view.node(id)?;
        if tsr_ast::utilities::is_type_node(&node) {
            return Ok(true);
        }
        current = node.parent();
    }
    Ok(false)
}

/// port: tsc/internal/scanner/utilities.go:GetTextOfNodeFromSourceText
pub fn get_text_of_node_from_source_text(
    view: AstView<'_>,
    source_text: &[u8],
    id: Option<NodeId>,
    include_trivia: bool,
) -> Result<JsString, Error> {
    let node = id.map(|id| view.node(id)).transpose()?;
    if tsr_ast::node_is_missing(node.as_ref()) {
        return Ok(JsString::default());
    }
    let node = node.expect("nil source-text node");
    let id = id.expect("nil source-text node");
    let mut pos = i64::from(node.pos());
    if !include_trivia {
        pos = skip_trivia(source_text, pos);
    }
    let mut text = std::borrow::Cow::Borrowed(&source_text[pos as usize..node.end() as usize]);
    if is_jsdoc_type_expression_or_child(view, id)? {
        text = std::borrow::Cow::Owned(normalize_jsdoc_type_source_text(&text).into_owned());
    }
    if node.flags() & node_flags::REPARSER_TRANSFORMED_LITERAL != 0 {
        if node.kind() == K::StringLiteral {
            let flags = node
                .data_source()
                .as_string_literal()
                .expect("StringLiteral payload")
                .token_flags();
            let quote = if flags & token_flags::SINGLE_QUOTE != 0 {
                b'\''
            } else {
                b'"'
            };
            let mut quoted = Vec::with_capacity(text.len() + 2);
            quoted.push(quote);
            quoted.extend_from_slice(&text);
            quoted.push(quote);
            return Ok(JsString::from_bytes(quoted));
        } else if node.kind() == K::Identifier {
            return Ok(JsString::from_bytes(view.node_text(id)?.as_bytes()));
        }
        tsr_core::debug::fail_bad_syntax_kind(
            &node.kind_string(),
            &[tsr_core::debug::Argument::String(
                "Unexpected reparser-transformed node kind",
            )],
        );
    }
    Ok(JsString::from_bytes(text.as_ref()))
}

/// port: tsc/internal/scanner/utilities.go:GetSourceTextOfNodeFromSourceFile
pub fn get_source_text_of_node_from_source_file(
    view: AstView<'_>,
    source: NodeId,
    node: Option<NodeId>,
    include_trivia: bool,
) -> Result<JsString, Error> {
    get_text_of_node_from_source_text(
        view,
        view.source_file(source)?.text().as_bytes(),
        node,
        include_trivia,
    )
}

/// port: tsc/internal/scanner/utilities.go:GetTextOfNode
pub fn get_text_of_node(view: AstView<'_>, node: NodeId) -> Result<JsString, Error> {
    let source = tsr_ast::utilities::get_source_file_of_node(view, Some(node))?
        .expect("nil source file in GetTextOfNode");
    get_source_text_of_node_from_source_file(view, source, Some(node), false)
}

/// Concatenate JSDoc text and link source spellings, trimming only Unicode
/// White_Space at the end. JavaScript trivia also includes U+200B and U+FEFF;
/// Go's unicode.IsSpace used here does not.
/// port: tsc/internal/scanner/utilities.go:GetTextOfJSDocComment
pub fn get_text_of_jsdoc_comment(
    view: AstView<'_>,
    comment: Option<NodeListId>,
) -> Result<JsString, Error> {
    let Some(comment) = comment else {
        return Ok(JsString::default());
    };
    let nodes = view.node_slice(view.list(comment)?.nodes())?;
    let mut text = Vec::new();
    for node in nodes.iter() {
        let node = node.expect("runtime error: invalid memory address or nil pointer dereference");
        match view.node(node)?.kind().known() {
            Some(K::JSDocText) => text.extend_from_slice(view.node_text(node)?.as_bytes()),
            Some(K::JSDocLink | K::JSDocLinkCode | K::JSDocLinkPlain) => {
                text.extend_from_slice(get_text_of_node(view, node)?.as_bytes());
            }
            _ => {}
        }
    }
    loop {
        let (rune, width) = crate::utilities::decode_last_utf8(&text);
        if width == 0 || !char::from_u32(rune as u32).is_some_and(char::is_whitespace) {
            break;
        }
        text.truncate(text.len() - width);
    }
    Ok(JsString::from_bytes(text))
}

/// port: tsc/internal/scanner/utilities.go:DeclarationNameToString
pub fn declaration_name_to_string(
    view: AstView<'_>,
    name: Option<NodeId>,
) -> Result<JsString, Error> {
    if let Some(name) = name {
        let node = view.node(name)?;
        if node.pos() != node.end() {
            return get_text_of_node(view, name);
        }
    }
    Ok(JsString::from_bytes(b"(Missing)".as_slice()))
}

/// Source setup assigns pos directly, so negative positions preserve Scan's
/// bounds failure rather than ResetPos's separate input assertion.
/// port: tsc/internal/scanner/scanner.go:GetScannerForSourceFile
pub fn get_scanner_for_source_file(source: &SourceFileState, position: i64) -> Scanner<'_> {
    let mut scanner = Scanner::new();
    scanner.text = source.text().as_bytes();
    scanner.state.pos = position;
    scanner.end = scanner.text.len() as i64;
    scanner.language_variant = source.language_variant;
    scanner.scan();
    scanner
}

/// The position of a node's first token. A missing node keeps its position,
/// because skipping trivia from it would run on to the next token, and a JSDoc
/// node or JSX text stops at a comment, which JSX text cannot really contain.
/// port: tsc/internal/scanner/scanner.go:GetTokenPosOfNode
pub fn get_token_pos_of_node(
    view: AstView<'_>,
    source: NodeId,
    id: NodeId,
    include_jsdoc: bool,
    jsdoc: &mut dyn tsr_ast::JsDocProvider,
) -> Result<i64, Error> {
    let node = view.node(id)?;
    if tsr_ast::node_is_missing(Some(&node)) {
        return Ok(i64::from(node.pos()));
    }
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    if tsr_ast::utilities_middle::is_js_doc_node(&node) || node.kind() == K::JsxText {
        return Ok(crate::skip_trivia_ex(
            text,
            i64::from(node.pos()),
            Some(&crate::SkipTriviaOptions {
                stop_after_line_break: false,
                stop_at_comments: true,
                in_jsdoc: false,
            }),
        ));
    }
    if include_jsdoc {
        let roots = jsdoc.jsdoc(view, source, id)?;
        if let Some(&first) = roots.first() {
            return get_token_pos_of_node(view, source, first, false, jsdoc);
        }
    }
    Ok(crate::skip_trivia_ex(
        text,
        i64::from(node.pos()),
        Some(&crate::SkipTriviaOptions {
            stop_after_line_break: false,
            stop_at_comments: false,
            in_jsdoc: node.flags() & node_flags::JS_DOC != 0,
        }),
    ))
}

/// port: tsc/internal/scanner/scanner.go:ScanTokenAtPosition
pub fn scan_token_at_position(
    view: AstView<'_>,
    source: NodeId,
    position: i64,
) -> Result<K, Error> {
    Ok(get_scanner_for_source_file(&*view.source_file(source)?, position).token())
}

/// port: tsc/internal/scanner/scanner.go:GetRangeOfTokenAtPosition
pub fn get_range_of_token_at_position(
    view: AstView<'_>,
    source: NodeId,
    position: i64,
) -> Result<TextRange, Error> {
    Ok(get_scanner_for_source_file(&*view.source_file(source)?, position).token_range())
}

/// port: tsc/internal/scanner/scanner.go:GetECMAEndLinePosition
pub fn get_ecma_end_line_position(source: &SourceFileState, line: isize) -> i64 {
    let mut pos = source.ecma_line_map()[line as usize] as usize;
    loop {
        let (rune, width) = tsr_jsstring::wtf8::decode_utf8(&source.text().as_bytes()[pos..]);
        if width == 0 || crate::utilities::is_line_break(rune) {
            return pos as i64 - 1;
        }
        pos += width;
    }
}

/// port: tsc/internal/scanner/scanner.go:getErrorRangeForArrowFunction
fn get_error_range_for_arrow_function(
    view: AstView<'_>,
    source: NodeId,
    id: NodeId,
) -> Result<TextRange, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let node = view.node(id)?;
    let pos = skip_trivia(text, i64::from(node.pos()));
    if let Some(body) = node.body() {
        let body = view.node(body)?;
        if body.kind() == K::Block {
            let starts = file.ecma_line_map();
            let start_line =
                scanner_positions::compute_line_of_position(starts, body.pos() as isize);
            let end_line = scanner_positions::compute_line_of_position(starts, body.end() as isize);
            if start_line < end_line {
                return Ok(TextRange::new(
                    pos,
                    get_ecma_end_line_position(&file, start_line) + 1,
                ));
            }
        }
    }
    Ok(TextRange::new(pos, i64::from(node.end())))
}

/// This lookup intentionally inspects already materialized JSDoc only.
/// port: tsc/internal/scanner/scanner.go:findOriginatingJSDocSatisfiesTag
fn find_originating_jsdoc_satisfies_tag(
    view: AstView<'_>,
    source: NodeId,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    let typ = node
        .data_source()
        .as_satisfies_expression()
        .expect("SatisfiesExpression payload")
        .r#type()
        .expect("nil satisfies target type");
    let target = view.node(typ)?;
    if target.flags() & node_flags::REPARSED == 0 {
        return Ok(None);
    }
    let mut current = node.parent();
    while let Some(id) = current {
        let node = view.node(id)?;
        current = node.parent();
        if node.flags() & node_flags::HAS_JS_DOC == 0 {
            continue;
        }
        let mut first = None;
        if let Some(roots) = view.source_eager_jsdoc(source, id)? {
            for &doc in roots.iter() {
                let node = view.node(doc)?;
                if let Some(tags) = node
                    .data_source()
                    .as_js_doc()
                    .expect("JSDoc payload")
                    .tags()
                {
                    for tag in view.node_slice(view.list(tags)?.nodes())?.iter() {
                        let tag = tag.expect("nil JSDoc tag");
                        let node = view.node(tag)?;
                        if node.kind() != K::JSDocSatisfiesTag {
                            continue;
                        }
                        if first.is_none() {
                            first = Some(tag);
                        }
                        if let Some(expression) = node
                            .data_source()
                            .as_js_doc_satisfies_tag()
                            .expect("JSDocSatisfiesTag payload")
                            .type_expression()
                        {
                            if let Some(typ) = view.node(expression)?.type_node() {
                                if view.node(typ)?.range() == target.range() {
                                    return Ok(Some(tag));
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok(first);
    }
    Ok(None)
}

/// port: tsc/internal/scanner/scanner.go:GetErrorRangeForNode
pub fn get_error_range_for_node(
    view: AstView<'_>,
    source: NodeId,
    id: NodeId,
) -> Result<TextRange, Error> {
    let file = view.source_file(source)?;
    let text = file.text().as_bytes();
    let node = view.node(id)?;
    let mut error_node = Some(id);
    match node.kind().known() {
        Some(K::SourceFile) => {
            let pos = skip_trivia(text, 0);
            if pos == text.len() as i64 {
                return Ok(TextRange::new(0, 0));
            }
            return get_range_of_token_at_position(view, source, pos);
        }
        Some(K::FunctionDeclaration | K::MethodDeclaration)
            if node.flags() & node_flags::REPARSED != 0 => {}
        Some(
            K::FunctionDeclaration
            | K::MethodDeclaration
            | K::VariableDeclaration
            | K::BindingElement
            | K::ClassDeclaration
            | K::InterfaceDeclaration
            | K::ModuleDeclaration
            | K::EnumDeclaration
            | K::EnumMember
            | K::FunctionExpression
            | K::GetAccessor
            | K::SetAccessor
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::NamespaceImport,
        ) => error_node = tsr_ast::get_name_of_declaration(view, Some(id))?,
        Some(K::ClassExpression) => error_node = node.name(),
        Some(K::ArrowFunction) => return get_error_range_for_arrow_function(view, source, id),
        Some(K::CaseClause | K::DefaultClause) => {
            let start = skip_trivia(text, i64::from(node.pos()));
            let mut end = node.end();
            let statements = view.node_slice(node.statements(view)?)?;
            if let Some(first) = statements.first() {
                end = view.node(first.expect("nil first case statement"))?.pos();
            }
            return Ok(TextRange::new(start, i64::from(end)));
        }
        Some(K::ReturnStatement | K::YieldExpression) => {
            return get_range_of_token_at_position(
                view,
                source,
                skip_trivia(text, i64::from(node.pos())),
            );
        }
        Some(K::SatisfiesExpression) => {
            if let Some(tag) = find_originating_jsdoc_satisfies_tag(view, source, id)? {
                let name = view.node(tag)?.tag_name().expect("nil satisfies tag name");
                return get_range_of_token_at_position(
                    view,
                    source,
                    skip_trivia(text, i64::from(view.node(name)?.pos())),
                );
            }
            let expression = node
                .data_source()
                .as_satisfies_expression()
                .expect("SatisfiesExpression payload")
                .expression()
                .expect("nil satisfies expression");
            return get_range_of_token_at_position(
                view,
                source,
                skip_trivia(text, i64::from(view.node(expression)?.end())),
            );
        }
        Some(K::Constructor) if node.flags() & node_flags::REPARSED == 0 => {
            let mut scanner = get_scanner_for_source_file(&file, i64::from(node.pos()));
            let start = scanner.token_start();
            while !matches!(
                scanner.token(),
                K::ConstructorKeyword | K::StringLiteral | K::EndOfFile
            ) {
                scanner.scan();
            }
            return Ok(TextRange::new(start, scanner.token_end()));
        }
        _ => {}
    }
    let Some(error_node) = error_node else {
        return get_range_of_token_at_position(view, source, i64::from(node.pos()));
    };
    let error_node = view.node(error_node)?;
    let mut pos = i64::from(error_node.pos());
    if !tsr_ast::node_is_missing(Some(&error_node)) && error_node.kind() != K::JsxText {
        pos = skip_trivia(text, pos);
    }
    Ok(TextRange::new(pos, i64::from(error_node.end())))
}

#[cfg(test)]
#[path = "binder_helpers_tests.rs"]
mod tests;
