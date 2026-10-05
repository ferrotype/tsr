use crate::{
    completion_context::ALL, completion_items, jsdoc_template as template, syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use tsr_ast::{span_map::FEATURE_COMPLETION, utilities as ast, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

pub(crate) enum JsDocCompletion {
    Code,
    Prose,
    List(lsp::CompletionList),
}

const TAG_NAMES: &[&str] = &[
    "abstract",
    "access",
    "alias",
    "argument",
    "async",
    "augments",
    "author",
    "borrows",
    "callback",
    "class",
    "classdesc",
    "constant",
    "constructor",
    "constructs",
    "copyright",
    "default",
    "deprecated",
    "description",
    "emits",
    "enum",
    "event",
    "example",
    "exports",
    "extends",
    "external",
    "field",
    "file",
    "fileoverview",
    "fires",
    "function",
    "generator",
    "global",
    "hideconstructor",
    "host",
    "ignore",
    "implements",
    "import",
    "inheritdoc",
    "inner",
    "instance",
    "interface",
    "kind",
    "lends",
    "license",
    "link",
    "linkcode",
    "linkplain",
    "listens",
    "member",
    "memberof",
    "method",
    "mixes",
    "module",
    "name",
    "namespace",
    "overload",
    "override",
    "package",
    "param",
    "private",
    "prop",
    "property",
    "protected",
    "public",
    "readonly",
    "requires",
    "returns",
    "satisfies",
    "see",
    "since",
    "static",
    "summary",
    "template",
    "this",
    "throws",
    "todo",
    "tutorial",
    "type",
    "typedef",
    "var",
    "variation",
    "version",
    "virtual",
    "yields",
];
impl LanguageService<'_> {
    // port: tsc/internal/ls/jsdoc_snippet.go:LanguageService.getJSDocSnippetCompletion
    pub(crate) fn jsdoc_snippet(
        &mut self,
        syntax: &mut Syntax<'_>,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionList>> {
        if options.enable_jsdoc == Some(false) {
            return Ok(None);
        }
        let Some((start, end)) = template::snippet_range(syntax, position as usize) else {
            return Ok(None);
        };
        let newline = options
            .newline
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("\n");
        let Some(text) = template::template(
            syntax,
            position as usize,
            options.generate_return.unwrap_or(true),
            newline,
        )?
        else {
            return Ok(None);
        };
        let text = if options.snippets {
            template::to_snippet(&text, newline)
        } else {
            text
        };
        let (range, fidelity) = self.range(
            syntax.source,
            TextRange::new(start as i64, end as i64),
            FEATURE_COMPLETION,
        )?;
        let edit = fidelity.is_exact().then(|| {
            Box::new(if options.insert_replace {
                lsp::TextEditOrInsertReplaceEdit {
                    insert_replace_edit: Some(Box::new(lsp::InsertReplaceEdit {
                        new_text: text.clone(),
                        insert: range.clone(),
                        replace: range.clone(),
                    })),
                    ..Default::default()
                }
            } else {
                lsp::TextEditOrInsertReplaceEdit {
                    text_edit: Some(Box::new(lsp::TextEdit {
                        new_text: text,
                        range,
                    })),
                    ..Default::default()
                }
            })
        });
        let mut list = lsp::CompletionList {
            items: vec![Some(Box::new(lsp::CompletionItem {
                label: "/** */".into(),
                kind: Some(Box::new(lsp::CompletionItemKind::TEXT)),
                detail: Some(Box::new(
                    String::from_utf8_lossy(
                        &tsr_diagnostics::JSDoc_comment.localize(&options.locale, &[]),
                    )
                    .into_owned(),
                )),
                sort_text: Some(Box::new("\0".into())),
                insert_text_format: options
                    .snippets
                    .then(|| Box::new(lsp::InsertTextFormat::SNIPPET)),
                text_edit: edit,
                commit_characters: options.commit_characters.then(|| Box::new(Vec::new())),
                ..Default::default()
            }))],
            ..Default::default()
        };
        self.completion_data(syntax.source, position, &mut list)?;
        Ok(Some(list))
    }
    /// Type expressions continue through the semantic collector; prose returns no list.
    pub(crate) fn jsdoc_completions(
        &mut self,
        checker: &mut tsr_checker::Operation<'_>,
        syntax: &mut Syntax<'_>,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<JsDocCompletion> {
        let token = syntax.nav().get_token_at_position(position)?;
        let mut current = Some(token);
        let mut doc = None;
        let mut tag = None;
        while let Some(id) = current {
            let read = syntax.view.node(id)?;
            if read.kind() == K::JSDoc {
                doc = Some(id);
                break;
            }
            if tsr_ast::utilities_middle::is_js_doc_tag(&read)
                && i64::from(read.pos()) <= position
                && position <= i64::from(read.end())
            {
                tag = Some(id);
            }
            current = read.parent();
        }
        let mut list = lsp::CompletionList::default();
        let mut name_only = None;
        let mut parameter_name = false;
        if doc.is_some() {
            let text = syntax.file.text().as_bytes();
            if position > 0 && text[position as usize - 1] == b'@' {
                name_only = Some(true);
            } else {
                let prefix =
                    &text[template::line_start(syntax, position as usize)..position as usize];
                let mut pos = 0;
                while pos < prefix.len() {
                    let (ch, size) = tsr_jsstring::wtf8::decode_utf8(&prefix[pos..]);
                    if size == 0
                        || !(tsr_jsstring::classify::is_white_space_single_line(ch)
                            || b"*/()|".contains(&(ch as u8)) && ch < 128)
                    {
                        break;
                    }
                    pos += size;
                }
                if pos == prefix.len() {
                    name_only = Some(false);
                }
            }
        }
        if name_only.is_none() {
            if let Some(tag) = tag {
                let read = syntax.view.node(tag)?;
                if let Some(name) = read.tag_name() {
                    let n = syntax.view.node(name)?;
                    if i64::from(n.pos()) <= position && position <= i64::from(n.end()) {
                        name_only = Some(true);
                    }
                }
                if name_only.is_none() {
                    if read.kind() == K::JSDocImportTag {
                        return Ok(JsDocCompletion::Code);
                    }
                    let expression = if read.kind() == K::JSDocTemplateTag {
                        read.data_source()
                            .as_js_doc_template_tag()
                            .and_then(|d| d.constraint())
                    } else if matches!(
                        read.kind().known(),
                        Some(
                            K::JSDocParameterTag
                                | K::JSDocPropertyTag
                                | K::JSDocReturnTag
                                | K::JSDocTypeTag
                                | K::JSDocTypedefTag
                                | K::JSDocThrowsTag
                                | K::JSDocSatisfiesTag
                        )
                    ) {
                        read.type_expression()
                    } else if matches!(
                        read.kind().known(),
                        Some(K::JSDocAugmentsTag | K::JSDocImplementsTag)
                    ) {
                        read.class_name()
                    } else {
                        None
                    };
                    if let Some(expression) = expression {
                        let e = syntax.view.node(expression)?;
                        if syntax.start(expression)? <= position && position <= i64::from(e.end()) {
                            return Ok(JsDocCompletion::Code);
                        }
                    }
                    if read.kind() == K::JSDocParameterTag
                        && read.name().is_none_or(|n| {
                            syntax.view.node(n).is_ok_and(|r| {
                                tsr_ast::node_is_missing(Some(&r))
                                    || i64::from(r.pos()) <= position
                                        && position <= i64::from(r.end())
                            })
                        })
                    {
                        parameter_name = true;
                        list.items = parameter_names(syntax, tag)?;
                    }
                }
            }
        }
        if let Some(name_only) = name_only {
            list.items = TAG_NAMES
                .iter()
                .map(|name| {
                    Some(Box::new(lsp::CompletionItem {
                        label: format!("{}{name}", if name_only { "" } else { "@" }),
                        kind: Some(Box::new(lsp::CompletionItemKind::KEYWORD)),
                        sort_text: Some(Box::new("11".into())),
                        ..Default::default()
                    }))
                })
                .collect();
        }
        if let Some(name_only) = name_only {
            list.items.extend(crate::jsdoc_parameters::completions(
                checker, syntax, position, name_only, options,
            )?);
        }
        if list.items.is_empty() && !parameter_name {
            return Ok(JsDocCompletion::Prose);
        }
        let (range, _) = self.range(
            syntax.source,
            TextRange::new(position, position),
            FEATURE_COMPLETION,
        )?;
        completion_items::defaults(&mut list, options, &range.start, None, ALL);
        self.completion_data(syntax.source, position, &mut list)?;
        Ok(JsDocCompletion::List(list))
    }
}
// port: tsc/internal/ls/completions.go:getJSDocParameterNameCompletions
fn parameter_names(
    syntax: &Syntax<'_>,
    tag: NodeId,
) -> Result<Vec<Option<Box<lsp::CompletionItem>>>> {
    let read = syntax.view.node(tag)?;
    let Some(name) = read.name().filter(|&id| {
        syntax
            .view
            .node(id)
            .is_ok_and(|r| r.kind() == K::Identifier)
    }) else {
        return Ok(Vec::new());
    };
    let prefix = syntax.view.node_text(name)?;
    let doc = read.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
    let doc_read = syntax.view.node(doc)?;
    let Some(function) = doc_read.parent() else {
        return Ok(Vec::new());
    };
    let function_read = syntax.view.node(function)?;
    if !ast::is_function_like(Some(&function_read)) {
        return Ok(Vec::new());
    }
    let mut used = std::collections::HashSet::new();
    if let Some(tags) = doc_read.data_source().as_js_doc().and_then(|d| d.tags()) {
        for id in syntax
            .view
            .node_slice(syntax.view.list(tags)?.nodes())?
            .iter()
            .flatten()
        {
            let r = syntax.view.node(id)?;
            if id != tag && r.kind() == K::JSDocParameterTag {
                if let Some(name) = r
                    .name()
                    .filter(|&n| syntax.view.node(n).is_ok_and(|n| n.kind() == K::Identifier))
                {
                    used.insert(syntax.view.node_text(name)?.as_bytes().to_vec());
                }
            }
        }
    }
    let mut items = Vec::new();
    for parameter in syntax
        .view
        .node_slice(function_read.parameters(syntax.view)?)?
        .iter()
        .flatten()
    {
        let Some(name) = syntax
            .view
            .node(parameter)?
            .name()
            .filter(|&n| syntax.view.node(n).is_ok_and(|n| n.kind() == K::Identifier))
        else {
            continue;
        };
        let text = syntax.view.node_text(name)?;
        if used.contains(text.as_bytes()) || !text.as_bytes().starts_with(prefix.as_bytes()) {
            continue;
        }
        items.push(Some(Box::new(lsp::CompletionItem {
            label: String::from_utf8_lossy(text.as_bytes()).into_owned(),
            kind: Some(Box::new(lsp::CompletionItemKind::VARIABLE)),
            sort_text: Some(Box::new("11".into())),
            ..Default::default()
        })));
    }
    Ok(items)
}
