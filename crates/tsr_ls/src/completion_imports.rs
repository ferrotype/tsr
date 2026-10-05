//! Import-statement completion contexts, including parser-recovery ranges.
use crate::{syntax::Syntax, Result};
use tsr_ast::{utilities as ast, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

#[derive(Default)]
pub(crate) struct ImportStatement {
    pub keyword: Option<K>,
    pub keyword_only: bool,
    pub new_identifier: bool,
    pub replacement: Option<TextRange>,
    pub specifier_type_only: bool,
    pub top_level_type_only: bool,
}
// port: tsc/internal/ls/completions.go:isModuleSpecifierMissingOrEmpty
fn missing(syntax: &Syntax<'_>, node: Option<NodeId>) -> Result<bool> {
    let Some(mut node) = node else {
        return Ok(true);
    };
    let read = syntax.view.node(node)?;
    if tsr_ast::node_is_missing(Some(&read)) {
        return Ok(true);
    }
    if read.kind() == K::ExternalModuleReference {
        let Some(expression) = read.expression() else {
            return Ok(true);
        };
        node = expression;
    }
    Ok(!ast::is_string_literal_like(&syntax.view.node(node)?)
        || syntax.view.node_text(node)?.as_bytes().is_empty())
}
// port: tsc/internal/ls/completions.go:couldBeTypeOnlyImportSpecifier
fn type_specifier(syntax: &Syntax<'_>, node: NodeId, token: NodeId) -> Result<bool> {
    let read = syntax.view.node(node)?;
    Ok(read.kind() == K::ImportSpecifier
        && (read.is_type_only()
            || read.name() == Some(token)
                && (syntax.view.node(token)?.kind() == K::TypeKeyword
                    || syntax.view.node_text(token)?.as_bytes() == b"type")))
}
// port: tsc/internal/ls/completions.go:getPotentiallyInvalidImportSpecifier
fn invalid_specifier(syntax: &mut Syntax<'_>, bindings: Option<NodeId>) -> Result<Option<NodeId>> {
    let Some(bindings) = bindings else {
        return Ok(None);
    };
    let read = syntax.view.node(bindings)?;
    if read.kind() != K::NamedImports {
        return Ok(None);
    }
    for id in syntax
        .view
        .node_slice(read.elements(syntax.view)?)?
        .iter()
        .flatten()
    {
        let read = syntax.view.node(id)?;
        if read.property_name().is_some() {
            continue;
        }
        let Some(name) = read.name() else {
            continue;
        };
        if tsr_ast::utilities_tail::is_non_contextual_keyword(
            tsr_scanner::string_to_token(syntax.view.node_text(name)?.as_bytes()).into(),
        ) {
            let pos = i64::from(syntax.view.node(name)?.pos());
            let preceding = syntax.nav().find_preceding_token(pos)?;
            if preceding.is_none_or(|id| {
                syntax
                    .view
                    .node(id)
                    .is_ok_and(|r| r.kind() != K::CommaToken)
            }) {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}
// port: tsc/internal/ls/completions.go:canCompleteFromNamedBindings
fn can_complete(syntax: &mut Syntax<'_>, bindings: NodeId) -> Result<bool> {
    let read = syntax.view.node(bindings)?;
    let clause = syntax
        .view
        .node(read.parent().ok_or(tsr_arena::Error::InvalidGraph)?)?;
    let declaration = syntax
        .view
        .node(clause.parent().ok_or(tsr_arena::Error::InvalidGraph)?)?;
    if !missing(syntax, declaration.module_specifier())? || clause.name().is_some() {
        return Ok(false);
    }
    if read.kind() == K::NamedImports {
        let invalid = invalid_specifier(syntax, Some(bindings))?;
        let list = syntax.view.node_slice(read.elements(syntax.view)?)?;
        let valid = list
            .iter()
            .position(|e| e == invalid && invalid.is_some())
            .unwrap_or(list.len());
        return Ok(valid < 2);
    }
    Ok(true)
}
// port: tsc/internal/ls/completions.go:LanguageService.getSingleLineReplacementSpanForImportCompletionNode
fn replacement(syntax: &mut Syntax<'_>, mut node: NodeId) -> Result<Option<TextRange>> {
    let mut ancestor = Some(node);
    while let Some(id) = ancestor {
        let read = syntax.view.node(id)?;
        if matches!(
            read.kind().known(),
            Some(K::ImportDeclaration | K::ImportEqualsDeclaration | K::JSDocImportTag)
        ) {
            node = id;
            break;
        }
        ancestor = read.parent();
    }
    let read = syntax.view.node(node)?;
    let start = syntax.start(node)?;
    if syntax.same_line(start, i64::from(read.end())) {
        return Ok(Some(TextRange::new(start, i64::from(read.end()))));
    }
    assert!(
        !matches!(
            read.kind().known(),
            Some(K::ImportKeyword | K::ImportSpecifier)
        ),
        "single-line import token has a multiline range"
    );
    let split = if matches!(
        read.kind().known(),
        Some(K::ImportDeclaration | K::JSDocImportTag)
    ) {
        let bindings = read
            .import_clause()
            .and_then(|c| syntax.view.node(c).ok())
            .and_then(|c| {
                c.data_source()
                    .as_import_clause()
                    .and_then(|c| c.named_bindings())
            });
        invalid_specifier(syntax, bindings)?.or(read.module_specifier())
    } else {
        read.data_source()
            .as_import_equals_declaration()
            .and_then(|d| d.module_reference())
    };
    let Some(split) = split else {
        return Ok(None);
    };
    let end = i64::from(syntax.view.node(split)?.pos());
    Ok(syntax
        .same_line(start, end)
        .then_some(TextRange::new(start, end)))
}
// port: tsc/internal/ls/completions.go:LanguageService.getImportStatementCompletionInfo
pub(crate) fn context(syntax: &mut Syntax<'_>, token: Option<NodeId>) -> Result<ImportStatement> {
    let mut result = ImportStatement::default();
    let Some(token) = token else {
        return Ok(result);
    };
    let t = syntax.view.node(token)?;
    let Some(parent) = t.parent() else {
        return Ok(result);
    };
    let p = syntax.view.node(parent)?;
    let mut candidate = None;
    if p.kind() == K::ImportEqualsDeclaration {
        let last = tsr_format::get_last_token(
            &mut tsr_format::FormatFile {
                view: syntax.view,
                source: syntax.source,
                jsdoc: &mut syntax.docs,
            },
            Some(parent),
        )?;
        if t.kind() == K::Identifier && last != Some(token) {
            result.keyword = Some(K::FromKeyword);
            result.keyword_only = true;
        } else {
            if t.kind() != K::TypeKeyword {
                result.keyword = Some(K::TypeKeyword);
            }
            if missing(
                syntax,
                p.data_source()
                    .as_import_equals_declaration()
                    .and_then(|d| d.module_reference()),
            )? {
                candidate = Some(parent);
            }
        }
    } else if type_specifier(syntax, parent, token)?
        && p.parent().is_some()
        && can_complete(syntax, p.parent().unwrap())?
    {
        candidate = Some(parent);
    } else if matches!(p.kind().known(), Some(K::NamedImports | K::NamespaceImport)) {
        let clause = p.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
        if !syntax.view.node(clause)?.is_type_only()
            && matches!(
                t.kind().known(),
                Some(K::OpenBraceToken | K::ImportKeyword | K::CommaToken)
            )
        {
            result.keyword = Some(K::TypeKeyword);
        }
        if can_complete(syntax, parent)? {
            if matches!(t.kind().known(), Some(K::CloseBraceToken | K::Identifier)) {
                result.keyword_only = true;
                result.keyword = Some(K::FromKeyword);
            } else {
                candidate = syntax.view.node(clause)?.parent();
            }
        }
    } else if p.kind() == K::ExportDeclaration && t.kind() == K::AsteriskToken
        || p.kind() == K::NamedExports && t.kind() == K::CloseBraceToken
    {
        result.keyword_only = true;
        result.keyword = Some(K::FromKeyword);
    } else if t.kind() == K::ImportKeyword {
        if p.kind() == K::SourceFile {
            result.keyword = Some(K::TypeKeyword);
            candidate = Some(token);
        } else if p.kind() == K::ImportDeclaration {
            result.keyword = Some(K::TypeKeyword);
            if missing(syntax, p.module_specifier())? {
                candidate = Some(parent);
            }
        }
    }
    if let Some(candidate) = candidate {
        result.new_identifier = true;
        result.replacement = replacement(syntax, candidate)?;
        result.specifier_type_only = type_specifier(syntax, candidate, token)?;
        let c = syntax.view.node(candidate)?;
        result.top_level_type_only = if c.kind() == K::ImportDeclaration {
            c.import_clause()
                .is_some_and(|c| syntax.view.node(c).is_ok_and(|c| c.is_type_only()))
        } else {
            c.kind() == K::ImportEqualsDeclaration && c.is_type_only()
        };
    } else {
        result.new_identifier = result.keyword == Some(K::TypeKeyword);
    }
    Ok(result)
}
// port: tsc/internal/ls/completions.go:getInsertTextAndReplacementSpanForImportCompletion
pub(crate) fn insert_text(
    info: &ImportStatement,
    kind: lsp::ImportKind,
    name: &str,
    module: &str,
    snippet: bool,
    semicolons: bool,
    single_quote: bool,
) -> String {
    fn escape(s: &str) -> String {
        s.replace('$', "\\$")
    }
    let quoted = tsr_autoimport::edits::quote_module(module, single_quote);
    let quoted = escape(&quoted);
    let name = escape(name);
    let tab = if snippet { "$1" } else { "" };
    let suffix = if semicolons { ";" } else { "" };
    let top = if info.top_level_type_only {
        " type "
    } else {
        " "
    };
    match kind {
        lsp::ImportKind::COMMON_JS => format!("import{top}{name}{tab} = require({quoted}){suffix}"),
        lsp::ImportKind::DEFAULT => format!("import{top}{name}{tab} from {quoted}{suffix}"),
        lsp::ImportKind::NAMESPACE => format!("import{top}* as {name} from {quoted}{suffix}"),
        lsp::ImportKind::NAMED => format!(
            "import{top}{{ {}{name}{tab} }} from {quoted}{suffix}",
            if info.specifier_type_only {
                "type "
            } else {
                ""
            }
        ),
        _ => panic!("unhandled import kind"),
    }
}
