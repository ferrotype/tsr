//! JSDoc template construction. Temporary reparses retain their own AST owner;
//! only the generated text leaves this module.
use crate::{syntax::Syntax, Result};
use tsr_ast::{utilities as ast, JsDocProvider, NodeId, SyntaxKind as K};
use tsr_jsstring::{
    classify::{is_white_space_like, is_white_space_single_line},
    wtf8::decode_utf8,
};

pub(crate) fn skip_space(text: &[u8], mut position: usize, single_line: bool) -> usize {
    while position < text.len() {
        let (ch, size) = decode_utf8(&text[position..]);
        if size == 0
            || !(if single_line {
                is_white_space_single_line(ch)
            } else {
                is_white_space_like(ch)
            })
        {
            break;
        }
        position += size;
    }
    position
}
fn trim_right(text: &[u8]) -> &[u8] {
    let mut end = 0;
    let mut pos = 0;
    while pos < text.len() {
        let (ch, size) = decode_utf8(&text[pos..]);
        if size == 0 {
            break;
        }
        pos += size;
        if !is_white_space_single_line(ch) {
            end = pos;
        }
    }
    &text[..end]
}
// port: tsc/internal/ls/jsdoc_snippet.go:isJSDocSnippetPrefix
fn valid_prefix(prefix: &[u8]) -> bool {
    let trimmed = trim_right(prefix);
    if trimmed.ends_with(b"/**") {
        return true;
    }
    let start = skip_space(prefix, 0, true);
    trimmed
        .get(start..)
        .is_some_and(|s| s.len() >= 3 && s[0] == b'/' && s[1..].iter().all(|&b| b == b'*'))
}
// port: tsc/internal/ls/jsdoc_snippet.go:getJSDocSnippetPrefixStart
fn prefix_start(prefix: &[u8]) -> Option<usize> {
    let trimmed = trim_right(prefix);
    let mut i = trimmed.len();
    while i > 0 && trimmed[i - 1] == b'*' {
        i -= 1;
        if i > 0 && trimmed[i - 1] == b'/' {
            return Some(i - 1);
        }
    }
    trimmed.ends_with(b"/").then(|| trimmed.len() - 1)
}
// port: tsc/internal/ls/jsdoc_snippet.go:isJSDocSnippetSuffix
fn valid_suffix(suffix: &[u8]) -> bool {
    let trimmed = trim_right(&suffix[skip_space(suffix, 0, true)..]);
    trimmed.is_empty()
        || trimmed.ends_with(b"/") && trimmed[..trimmed.len() - 1].iter().all(|&b| b == b'*')
}
// port: tsc/internal/ls/jsdoc_snippet.go:getJSDocSnippetSuffixEnd
fn suffix_end(suffix: &[u8]) -> Option<usize> {
    let mut pos = skip_space(suffix, 0, true);
    while suffix.get(pos) == Some(&b'*') {
        pos += 1;
    }
    (suffix.get(pos) == Some(&b'/')).then_some(pos + 1)
}
pub(crate) fn line_start(syntax: &Syntax<'_>, position: usize) -> usize {
    let starts = syntax.file.ecma_line_map();
    starts[starts
        .partition_point(|&p| p as usize <= position)
        .saturating_sub(1)] as usize
}
pub(crate) fn snippet_range(syntax: &Syntax<'_>, position: usize) -> Option<(usize, usize)> {
    let text = syntax.file.text().as_bytes();
    let start = line_start(syntax, position);
    let end = syntax.line_end(position as i64) as usize;
    let prefix = text.get(start..position)?;
    let suffix = text.get(position..end)?;
    if !valid_prefix(prefix) || !valid_suffix(suffix) {
        return None;
    }
    Some((
        prefix_start(prefix).map_or(position, |p| start + p),
        suffix_end(suffix).map_or(position, |p| position + p),
    ))
}
fn nonempty_doc(syntax: &Syntax<'_>, doc: Option<NodeId>) -> Result<bool> {
    let Some(doc) = doc else {
        return Ok(false);
    };
    let read = syntax.view.node(doc)?;
    let d = read
        .data_source()
        .as_js_doc()
        .ok_or(tsr_arena::Error::InvalidGraph)?;
    for list in [d.comment(), d.tags()].into_iter().flatten() {
        if !syntax
            .view
            .node_slice(syntax.view.list(list)?.nodes())?
            .is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
// port: tsc/internal/ls/jsdoc_snippet.go:getDocCommentTemplateAtPosition
pub(crate) fn template(
    syntax: &mut Syntax<'_>,
    position: usize,
    returns: bool,
    newline: &str,
) -> Result<Option<String>> {
    let mut token = syntax.nav().get_token_at_position(position as i64)?;
    let mut node = Some(token);
    let mut doc = None;
    while let Some(id) = node {
        let read = syntax.view.node(id)?;
        if read.kind() == K::JSDoc {
            doc = Some(id);
            break;
        }
        node = read.parent();
    }
    let text = syntax.file.text().as_bytes();
    let prefix = &text[line_start(syntax, position)..position];
    let has_doc = trim_right(prefix).ends_with(b"/**");
    let suffix = &text[position..syntax.line_end(position as i64) as usize];
    let closing = suffix_end(suffix);
    if nonempty_doc(syntax, doc)? {
        if has_doc && closing.is_none() {
            let mut text = text.to_vec();
            text.splice(position..position, b" */".iter().copied());
            let parsed = tsr_parser::parse_source_file(
                tsr_jsstring::SourceText::from_bytes(text),
                syntax.file.script_kind,
                syntax.file.parse_options().clone(),
            );
            let root = parsed.root();
            let file = parsed.publish_unbound();
            return template(
                &mut Syntax::new(file.view(), root)?,
                position,
                returns,
                newline,
            );
        }
        return Ok(None);
    }
    if doc.is_none() && has_doc {
        let next = skip_space(text, position + closing.unwrap_or(0), false);
        token = syntax.nav().get_token_at_position(next as i64)?;
    }
    let token_start = syntax.start(token)?;
    if doc.is_none() && !has_doc && token_start < position as i64 {
        return Ok(None);
    }
    let Some((owner, parameters, has_return)) = owner_info(syntax, token, returns)? else {
        return Ok(None);
    };
    let docs = syntax.docs.jsdoc(syntax.view, syntax.source, owner)?;
    let last = docs.last().copied();
    if syntax.start(owner)? < position as i64 || last.is_some() && doc.is_some() && last != doc {
        return Ok(None);
    }
    let indentation = {
        let text = syntax.file.text().as_bytes();
        let start = line_start(syntax, position);
        String::from_utf8_lossy(&text[start..skip_space(text, start, true).min(position)])
            .into_owned()
    };
    let mut tags = String::new();
    for (i, parameter) in parameters.iter().enumerate() {
        let p = syntax.view.node(*parameter)?;
        let name = match p.name() {
            Some(id) if syntax.view.node(id)?.kind() == K::Identifier => {
                String::from_utf8_lossy(syntax.view.node_text(id)?.as_bytes()).into_owned()
            }
            _ => format!("param{i}"),
        };
        let ty = if syntax.file.is_js() {
            if p.data_source()
                .as_parameter_declaration()
                .and_then(|p| p.dot_dot_dot_token())
                .is_some()
            {
                "{...any} "
            } else {
                "{any} "
            }
        } else {
            ""
        };
        tags.push_str(&format!("{indentation} * @param {ty}{name}{newline}"));
    }
    if has_return {
        tags.push_str(&format!("{indentation} * @returns{newline}"));
    }
    let has_tags = if let Some(doc) = last {
        syntax
            .view
            .node(doc)?
            .data_source()
            .as_js_doc()
            .and_then(|d| d.tags())
            .is_some_and(|list| {
                syntax
                    .view
                    .list(list)
                    .and_then(|l| syntax.view.node_slice(l.nodes()))
                    .is_ok_and(|l| !l.is_empty())
            })
    } else {
        false
    };
    Ok(Some(if tags.is_empty() || has_tags {
        "/** */".into()
    } else {
        let end = if token_start == position as i64 {
            format!("{newline}{indentation}")
        } else {
            String::new()
        };
        format!("/**{newline}{indentation} * {newline}{tags}{indentation} */{end}")
    }))
}
fn has_return(syntax: &Syntax<'_>, id: NodeId, enabled: bool) -> Result<bool> {
    if !enabled {
        return Ok(false);
    }
    let read = syntax.view.node(id)?;
    if read.kind() == K::FunctionType {
        return Ok(true);
    }
    if read.kind() == K::ArrowFunction
        && read.body().is_some_and(|b| {
            tsr_ast::utilities_positions::is_expression(syntax.view, b).unwrap_or(false)
        })
    {
        return Ok(true);
    }
    if ast::is_function_like_declaration(Some(&read)) {
        if let Some(body) = read
            .body()
            .filter(|&b| syntax.view.node(b).is_ok_and(|r| r.kind() == K::Block))
        {
            return Ok(tsr_ast::utilities_containers::for_each_return_statement(
                syntax.view,
                body,
                &mut |_| true,
            )?);
        }
    }
    Ok(false)
}
fn assignment_function(syntax: &Syntax<'_>, mut id: NodeId) -> Result<Option<NodeId>> {
    while syntax.view.node(id)?.kind() == K::ParenthesizedExpression {
        id = syntax
            .view
            .node(id)?
            .expression()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
    }
    let read = syntax.view.node(id)?;
    Ok(match read.kind().known() {
        Some(K::FunctionExpression | K::ArrowFunction) => Some(id),
        Some(K::ClassExpression) => syntax
            .view
            .node_slice(read.members(syntax.view)?)?
            .iter()
            .flatten()
            .find(|&m| {
                syntax
                    .view
                    .node(m)
                    .is_ok_and(|r| r.kind() == K::Constructor)
            }),
        _ => None,
    })
}
type OwnerInfo = (NodeId, Vec<NodeId>, bool);
fn owner_info(syntax: &Syntax<'_>, token: NodeId, returns: bool) -> Result<Option<OwnerInfo>> {
    let mut current = Some(token);
    while let Some(id) = current {
        let (owner, quit) = owner_worker(syntax, id, returns)?;
        if owner.is_some() || quit {
            return Ok(owner);
        }
        current = syntax.view.node(id)?.parent();
    }
    Ok(None)
}
fn owner_worker(
    syntax: &Syntax<'_>,
    id: NodeId,
    returns: bool,
) -> Result<(Option<OwnerInfo>, bool)> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let n = syntax.view.node(id)?;
        let mut function = None;
        let mut owner = Some(id);
        match n.kind().known() {
            Some(
                K::FunctionDeclaration
                | K::FunctionExpression
                | K::MethodDeclaration
                | K::Constructor
                | K::MethodSignature
                | K::ArrowFunction,
            ) => function = Some(id),
            Some(K::PropertyAssignment | K::ExpressionStatement) => {
                let inner = if n.kind() == K::PropertyAssignment {
                    n.initializer()
                } else {
                    n.expression()
                };
                return inner.map_or(Ok((None, false)), |inner| {
                    owner_worker(syntax, inner, returns)
                });
            }
            Some(
                K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::EnumDeclaration
                | K::EnumMember
                | K::TypeAliasDeclaration,
            ) => {}
            Some(K::PropertySignature) => {
                function = n.type_node().filter(|&t| {
                    syntax
                        .view
                        .node(t)
                        .is_ok_and(|r| r.kind() == K::FunctionType)
                });
            }
            Some(K::VariableStatement) => {
                let list = n
                    .data_source()
                    .as_variable_statement()
                    .and_then(|s| s.declaration_list())
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let read = syntax.view.node(list)?;
                let declarations: Vec<_> = syntax
                    .view
                    .node_slice(
                        syntax
                            .view
                            .list(
                                read.data_source()
                                    .as_variable_declaration_list()
                                    .and_then(|d| d.declarations())
                                    .ok_or(tsr_arena::Error::InvalidGraph)?,
                            )?
                            .nodes(),
                    )?
                    .iter()
                    .flatten()
                    .collect();
                if let [decl] = declarations.as_slice() {
                    if let Some(initializer) = syntax.view.node(*decl)?.initializer() {
                        function = assignment_function(syntax, initializer)?;
                    }
                }
            }
            Some(K::SourceFile) => return Ok((None, true)),
            Some(K::ModuleDeclaration) => {
                if n.parent().is_some_and(|p| {
                    syntax
                        .view
                        .node(p)
                        .is_ok_and(|r| r.kind() == K::ModuleDeclaration)
                }) {
                    owner = None;
                }
            }
            Some(K::BinaryExpression) => {
                if tsr_ast::get_assignment_declaration_kind(syntax.view, id)?
                    == tsr_ast::JSDeclarationKind::None
                {
                    return Ok((None, true));
                }
                function = n
                    .data_source()
                    .as_binary_expression()
                    .and_then(|d| d.right())
                    .filter(|&r| {
                        syntax
                            .view
                            .node(r)
                            .is_ok_and(|r| ast::is_function_like(Some(&r)))
                    });
            }
            Some(K::PropertyDeclaration) => {
                function = n.initializer().filter(|&i| {
                    syntax.view.node(i).is_ok_and(|r| {
                        matches!(
                            r.kind().known(),
                            Some(K::FunctionExpression | K::ArrowFunction)
                        )
                    })
                });
                if function.is_none() {
                    owner = None;
                }
            }
            _ => owner = None,
        }

        Ok((
            if let Some(owner) = owner {
                Some(if let Some(function) = function {
                    (
                        owner,
                        syntax
                            .view
                            .node_slice(syntax.view.node(function)?.parameters(syntax.view)?)?
                            .iter()
                            .flatten()
                            .collect(),
                        has_return(syntax, function, returns)?,
                    )
                } else {
                    (owner, Vec::new(), false)
                })
            } else {
                None
            },
            false,
        ))
    })
}
// port: tsc/internal/ls/jsdoc_snippet.go:templateToSnippet
pub(crate) fn to_snippet(template: &str, newline: &str) -> String {
    if template == "/** */" {
        return format!("/**{newline} * $0{newline} */");
    }
    let escaped = template.replace('$', "\\$");
    let lines: Vec<_> = escaped
        .split(newline)
        .map(|line| {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if trimmed.starts_with('/') {
                trimmed.to_owned()
            } else if trimmed.starts_with('*') {
                format!(" {trimmed}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    let mut index = 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if i > 0
                && lines[i - 1].starts_with("/**")
                && trimmed.starts_with('*')
                && trimmed[1..].bytes().all(|b| b == b' ' || b == b'\t')
            {
                return format!("{line}$0");
            }
            if trimmed == "* @returns" {
                let result = format!("{line} ${{{index}}}");
                index += 1;
                return result;
            }
            if let Some(param) = trimmed.strip_prefix("* @param ") {
                let prefix = if line.starts_with(' ') { " " } else { "" };
                let mut ty = String::new();
                let mut name = param;
                if let Some(t) = param
                    .strip_prefix('{')
                    .and_then(|s| s.find('}').map(|end| &param[..end + 2]))
                {
                    name = param[t.len()..].trim_start_matches([' ', '\t']);
                    if matches!(t, "{any}" | "{*}") {
                        ty = format!("{{${{{index}:*}}}} ");
                        index += 1;
                    } else {
                        ty = format!(" {t} ");
                    }
                }
                let out = format!("{prefix}* @param {ty}{name} ${{{index}}}");
                index += 1;
                return out;
            }
            line.clone()
        })
        .collect::<Vec<_>>()
        .join(newline)
}
