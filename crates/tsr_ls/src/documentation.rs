//! JSDoc lookup, including overloads, inherited members and alias targets.
use crate::{hover_display::call_or_new, syntax::Syntax, LanguageService, Result};
use std::collections::HashSet;
use tsr_ast::{utilities as ast, JsDocProvider, NodeId, NodeListId, SymbolId, SyntaxKind as K};
use tsr_checker::{Operation, SymbolRef};
use tsr_core::TextRange;

pub(crate) fn list(view: tsr_ast::AstView<'_>, list: Option<NodeListId>) -> Result<Vec<NodeId>> {
    let Some(list) = list else {
        return Ok(Vec::new());
    };
    Ok(view
        .node_slice(view.list(list)?.nodes())?
        .iter()
        .flatten()
        .collect())
}
pub(crate) fn code(out: &mut String, language: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let mut ticks = "```".to_owned();
    while text.contains(&ticks) {
        ticks.push('`');
    }
    out.push_str(&ticks);
    out.push_str(language);
    out.push('\n');
    out.push_str(text);
    out.push('\n');
    out.push_str(&ticks);
    out.push('\n');
}
fn quoted(out: &mut String, text: &str, quote: bool) {
    if quote && !text.contains('`') {
        out.push('`');
        out.push_str(text);
        out.push('`');
    } else {
        out.push_str(text);
    }
}
fn markdown_link(out: &mut String, text: &str, uri: &str, quote: bool) {
    out.push('[');
    quoted(out, text, quote);
    out.push_str("](");
    out.push_str(uri);
    out.push(')');
}
fn comment_prefix(text: &str) -> &str {
    text.trim_start_matches(' ')
        .strip_prefix('|')
        .unwrap_or(text.trim_start_matches(' '))
        .trim_start_matches(' ')
}
// port: tsc/internal/ls/hover.go:getEntityNameString
pub(crate) fn entity_name(view: tsr_ast::AstView<'_>, name: NodeId) -> Result<String> {
    let mut out = String::new();
    let mut stack = vec![Some(name)];
    while let Some(node) = stack.pop() {
        let Some(node) = node else {
            out.push('.');
            continue;
        };
        let read = view.node(node)?;
        match read.kind().known() {
            Some(K::Identifier) => {
                out.push_str(&String::from_utf8_lossy(view.node_text(node)?.as_bytes()));
            }
            Some(K::QualifiedName) => {
                let d = read.data_source().as_qualified_name().unwrap();
                stack.push(d.right());
                stack.push(None);
                stack.push(d.left());
            }
            Some(K::PropertyAccessExpression) => {
                stack.push(read.name());
                stack.push(None);
                stack.push(read.expression());
            }
            Some(K::ParenthesizedExpression | K::ExpressionWithTypeArguments) => {
                if let Some(e) = read.expression() {
                    stack.push(Some(e));
                }
            }
            Some(K::JSDocNameReference) => {
                if let Some(n) = read.name() {
                    stack.push(Some(n));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/jsdoc.go:getJSDoc
    fn last_jsdoc(&self, node: NodeId) -> Result<Option<NodeId>> {
        let view = self.view(node)?;
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        Ok(tsr_parser::ParserJsDocProvider::default()
            .jsdoc(view, source, node)?
            .last()
            .copied())
    }
    // port: tsc/internal/ls/jsdoc.go:getJSDocOrTag
    fn jsdoc_or_tag(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
        seen: &mut HashSet<SymbolId>,
    ) -> Result<Option<NodeId>> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.jsdoc_or_tag_inner(checker, node, seen)
        })
    }
    fn jsdoc_or_tag_inner(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
        seen: &mut HashSet<SymbolId>,
    ) -> Result<Option<NodeId>> {
        self.check_canceled()?;
        if let Some(doc) = self.last_jsdoc(node)? {
            return Ok(Some(doc));
        }
        let view = self.view(node)?;
        let read = view.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        match read.kind().known() {
            Some(K::Parameter | K::TypeParameter) => {
                let Some(name) = read.name() else {
                    return Ok(None);
                };
                if let Some(doc) = self.jsdoc_or_tag(checker, parent, seen)? {
                    if let Some(data) = self.view(doc)?.node(doc)?.data_source().as_js_doc() {
                        let tags = list(self.view(doc)?, data.tags())?;
                        if ast::is_binding_pattern(&view.node(name)?) && read.kind() == K::Parameter
                        {
                            let params: Vec<_> = view
                                .node_slice(view.node(parent)?.parameters(view)?)?
                                .iter()
                                .flatten()
                                .collect();
                            let index = params.iter().position(|p| *p == node);
                            if let Some(index) = index {
                                return Ok(tags
                                    .into_iter()
                                    .filter(|t| {
                                        self.view(*t)
                                            .and_then(|v| {
                                                Ok(v.node(*t)?.kind() == K::JSDocParameterTag)
                                            })
                                            .unwrap_or(false)
                                    })
                                    .nth(index));
                            }
                        } else {
                            let text = view.node_text(name)?;
                            for tag in tags {
                                let v = self.view(tag)?;
                                let t = v.node(tag)?;
                                if t.kind() == K::JSDocParameterTag && read.kind() == K::Parameter {
                                    if let Some(n) = t.name() {
                                        if v.node(n)?.kind() == K::Identifier
                                            && v.node_text(n)?.as_bytes() == text.as_bytes()
                                        {
                                            return Ok(Some(tag));
                                        }
                                    }
                                } else if t.kind() == K::JSDocTemplateTag
                                    && read.kind() == K::TypeParameter
                                {
                                    for tp in v.node_slice(t.type_parameters(v)?)?.iter().flatten()
                                    {
                                        if let Some(n) = v.node(tp)?.name() {
                                            if v.node(n)?.kind() == K::Identifier
                                                && v.node_text(n)?.as_bytes() == text.as_bytes()
                                            {
                                                return Ok(Some(tag));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                return Ok(None);
            }
            Some(K::VariableDeclaration)
                if view.node(parent)?.kind() == K::VariableDeclarationList =>
            {
                let parent_read = view.node(parent)?;
                let data = parent_read
                    .data_source()
                    .as_variable_declaration_list()
                    .unwrap();
                if list(view, data.declarations())?.first() == Some(&node) {
                    if let Some(grand) = view.node(parent)?.parent() {
                        return self.jsdoc_or_tag(checker, grand, seen);
                    }
                }
            }
            Some(K::FunctionExpression | K::ArrowFunction | K::ClassExpression)
                if matches!(
                    view.node(parent)?.kind().known(),
                    Some(K::VariableDeclaration | K::PropertyDeclaration | K::PropertyAssignment)
                ) && view.node(parent)?.initializer() == Some(node) =>
            {
                return self.jsdoc_or_tag(checker, parent, seen)
            }
            Some(K::BindingElement) if view.node(parent)?.kind() == K::ObjectBindingPattern => {
                if let Some(name) = read.property_name().or(read.name()) {
                    if view.node(name)?.kind() == K::Identifier {
                        let ty = checker.get_type_at_location(parent)?;
                        if let Some(prop) =
                            checker.get_property_of_type(ty, view.node_text(name)?.as_bytes())?
                        {
                            let decls: Vec<_> = checker
                                .symbol_declarations(prop)?
                                .iter()
                                .flatten()
                                .collect();
                            for decl in decls {
                                if let Some(doc) = self.last_jsdoc(decl)? {
                                    return Ok(Some(doc));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(symbol) = self.bound_symbol(checker, node)? {
            if matches!(
                read.kind().known(),
                Some(
                    K::FunctionDeclaration
                        | K::MethodDeclaration
                        | K::MethodSignature
                        | K::Constructor
                        | K::ConstructSignature
                )
            ) {
                let first = checker
                    .symbol_declarations(symbol)?
                    .iter()
                    .flatten()
                    .find(|d| {
                        checker
                            .node(*d)
                            .is_ok_and(|n| ast::is_function_like(Some(&n)))
                    });
                if let Some(first) = first.filter(|d| *d != node) {
                    if let Some(doc) = self.jsdoc_or_tag(checker, first, seen)? {
                        return Ok(Some(doc));
                    }
                }
            }
            if matches!(
                view.node(parent)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration)
            ) {
                if let Some(class) = self.bound_symbol(checker, parent)? {
                    let class_type = checker.get_declared_type_of_symbol(class)?;
                    let bases = if ast::get_combined_modifier_flags(view, node)?
                        & tsr_ast::modifier_flags::STATIC
                        != 0
                    {
                        let base = checker.get_base_constructor_type_of_class(class_type)?;
                        vec![checker.get_apparent_type(base)?]
                    } else {
                        checker.get_base_types(class_type)?
                    };
                    let name = checker.symbol(symbol)?.name_to_owned();
                    for base in bases {
                        if let Some(prop) = checker.get_property_of_type(base, name.as_bytes())? {
                            if let Some(decl) = checker.symbol(prop)?.value_declaration() {
                                if seen.insert(prop.id()) {
                                    if let Some(doc) = self.jsdoc_or_tag(checker, decl, seen)? {
                                        return Ok(Some(doc));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ls/hover.go:getDocumentationForSymbol
    pub(crate) fn documentation(
        &mut self,
        checker: &mut Operation<'_>,
        symbol: Option<SymbolRef>,
        node: NodeId,
        declaration: Option<NodeId>,
        markdown: bool,
        comment_only: bool,
    ) -> Result<String> {
        let view = self.view(node)?;
        if let Some(call) = call_or_new(view, node)? {
            let sig = checker.get_resolved_signature(call)?;
            if let Some(decl) = checker.signature_declaration(sig)? {
                if matches!(
                    checker.node(decl.id())?.kind().known(),
                    Some(K::CallSignature | K::ConstructSignature)
                ) {
                    let docs =
                        self.declaration_documentation(checker, decl.id(), markdown, comment_only)?;
                    if !docs.is_empty() {
                        return Ok(docs);
                    }
                }
            }
        }
        if let Some(sym) = symbol {
            let roots = checker.get_root_symbols(sym)?;
            if roots.len() > 1 {
                let mut parts = Vec::new();
                for root in roots {
                    let mut decls: Vec<_> = checker
                        .symbol_declarations(root)?
                        .iter()
                        .flatten()
                        .collect();
                    if decls.is_empty() {
                        decls.extend(checker.symbol(root)?.value_declaration());
                    }
                    for decl in decls {
                        let docs =
                            self.declaration_documentation(checker, decl, markdown, comment_only)?;
                        if !docs.is_empty() && !parts.contains(&docs) {
                            parts.push(docs);
                        }
                    }
                }
                if !parts.is_empty() {
                    return Ok(parts.join("\n"));
                }
            }
        }
        if let Some(decl) = declaration {
            let docs = self.declaration_documentation(checker, decl, markdown, comment_only)?;
            if !docs.is_empty() {
                return Ok(docs);
            }
        }
        if let Some(sym) = symbol {
            if checker.symbol(sym)?.flags() & tsr_ast::symbol_flags::ALIAS != 0 {
                let target = checker.get_aliased_symbol(sym)?;
                if !checker.is_unknown_symbol(target)? {
                    let mut candidates = vec![target];
                    if let Some(export) = checker.symbol(target)?.export_symbol() {
                        candidates.push(checker.symbol_ref(export)?);
                    }
                    for candidate in candidates {
                        let decl = checker.symbol(candidate)?.value_declaration().or_else(|| {
                            checker
                                .symbol_declarations(candidate)
                                .ok()?
                                .iter()
                                .flatten()
                                .next()
                        });
                        if let Some(decl) = decl {
                            let docs = self.declaration_documentation(
                                checker,
                                decl,
                                markdown,
                                comment_only,
                            )?;
                            if !docs.is_empty() {
                                return Ok(docs);
                            }
                        }
                    }
                }
            }
        }
        Ok(String::new())
    }
    // port: tsc/internal/ls/hover.go:getDocumentationFromDeclaration
    pub(crate) fn declaration_documentation(
        &mut self,
        checker: &mut Operation<'_>,
        declaration: NodeId,
        markdown: bool,
        comment_only: bool,
    ) -> Result<String> {
        self.declaration_documentation_for_feature(
            checker,
            declaration,
            markdown,
            comment_only,
            tsr_ast::span_map::FEATURE_HOVER,
        )
    }
    pub(crate) fn declaration_documentation_for_feature(
        &mut self,
        checker: &mut Operation<'_>,
        declaration: NodeId,
        markdown: bool,
        comment_only: bool,
        feature: i32,
    ) -> Result<String> {
        let Some(doc) = self.jsdoc_or_tag(checker, declaration, &mut HashSet::new())? else {
            return Ok(String::new());
        };
        let view = self.view(doc)?;
        let tags = if let Some(d) = view.node(doc)?.data_source().as_js_doc() {
            list(view, d.tags())?
        } else {
            Vec::new()
        };
        if self.view(declaration)?.node(declaration)?.flags() & tsr_ast::node_flags::REPARSED == 0
            && tags.iter().any(|t| {
                view.node(*t).is_ok_and(|n| {
                    matches!(
                        n.kind().known(),
                        Some(K::JSDocTypedefTag | K::JSDocCallbackTag)
                    )
                })
            })
        {
            return Ok(String::new());
        }
        let comments: Vec<_> = view
            .node_slice(view.node(doc)?.comments(view)?)?
            .iter()
            .flatten()
            .collect();
        let mut out = String::new();
        self.write_comments(checker, &mut out, comments, markdown, feature)?;
        if comment_only {
            return Ok(out);
        }
        for tag in tags {
            let read = view.node(tag)?;
            if matches!(
                read.kind().known(),
                Some(K::JSDocTypeTag | K::JSDocTypedefTag | K::JSDocCallbackTag)
            ) {
                continue;
            }
            out.push_str("\n\n");
            out.push_str(if markdown { "*@" } else { "@" });
            if let Some(name) = read.tag_name() {
                out.push_str(&String::from_utf8_lossy(view.node_text(name)?.as_bytes()));
            }
            if markdown {
                out.push('*');
            }
            let mut optional_name = |name: Option<NodeId>| -> Result<()> {
                if let Some(name) = name {
                    out.push(' ');
                    quoted(&mut out, &entity_name(view, name)?, true);
                }
                Ok(())
            };
            match read.kind().known() {
                Some(K::JSDocParameterTag | K::JSDocPropertyTag) => optional_name(read.name())?,
                Some(K::JSDocAugmentsTag) => optional_name(read.class_name())?,
                Some(K::JSDocTemplateTag) => {
                    let params: Vec<_> = view
                        .node_slice(read.type_parameters(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    for (i, tp) in params.iter().enumerate() {
                        if i != 0 {
                            out.push(',');
                        }
                        if let Some(name) = view.node(*tp)?.name() {
                            out.push(' ');
                            quoted(&mut out, &entity_name(view, name)?, true);
                        }
                    }
                }
                _ => {}
            }
            let comments: Vec<_> = view
                .node_slice(read.comments(view)?)?
                .iter()
                .flatten()
                .collect();
            if read.kind() == K::JSDocUnknownTag
                && read
                    .tag_name()
                    .is_some_and(|n| view.node_text(n).is_ok_and(|t| t.as_bytes() == b"example"))
            {
                let text = tsr_scanner::get_text_of_jsdoc_comment(view, read.comment_list())?;
                let mut text = String::from_utf8_lossy(text.as_bytes()).into_owned();
                if text.starts_with("<caption>") {
                    if let Some(end) = text.find("</caption>") {
                        out.push_str(" — ");
                        out.push_str(&text[9..end]);
                        text = text[end + 10..].to_owned();
                        loop {
                            let s1 = text.trim_start_matches([' ', '\t']);
                            let s2 = s1.trim_start_matches(['\r', '\n']);
                            if s1.len() == s2.len() {
                                break;
                            }
                            text = s2.to_owned();
                        }
                    }
                }
                out.push('\n');
                if text.len() > 6
                    && text.starts_with("```")
                    && text.ends_with("```")
                    && text.contains('\n')
                {
                    out.push_str(&text);
                    out.push('\n');
                } else {
                    code(&mut out, "tsx", &text);
                }
            } else if let Some(name_expr) = read
                .data_source()
                .as_js_doc_see_tag()
                .and_then(|d| d.name_expression())
            {
                out.push_str(" — ");
                if let Some(name) = view.node(name_expr)?.name() {
                    self.write_name_link(checker, &mut out, name, "", false, markdown, feature)?;
                }
                if !comments.is_empty() {
                    out.push(' ');
                    self.write_comments(checker, &mut out, comments, markdown, feature)?;
                }
            } else if let Some(ty) = read
                .data_source()
                .as_js_doc_throws_tag()
                .and_then(|d| d.type_expression())
            {
                out.push_str(" — ");
                out.push_str(&self.node_text(ty)?);
                if !comments.is_empty() {
                    out.push(' ');
                    self.write_comments(checker, &mut out, comments, markdown, feature)?;
                }
            } else if !comments.is_empty() {
                out.push(' ');
                if view.node(comments[0])?.kind() != K::JSDocText
                    || !view.node_text(comments[0])?.as_bytes().starts_with(b"-")
                {
                    out.push_str("— ");
                }
                self.write_comments(checker, &mut out, comments, markdown, feature)?;
            }
        }
        Ok(out)
    }
    fn node_text(&self, node: NodeId) -> Result<String> {
        let view = self.view(node)?;
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let mut syntax = Syntax::new(view, source)?;
        let start = syntax.start(node)? as usize;
        let end = view.node(node)?.end() as usize;
        Ok(String::from_utf8_lossy(&syntax.file.text().as_bytes()[start..end]).into_owned())
    }
    // port: tsc/internal/ls/hover.go:writeComments
    fn write_comments(
        &mut self,
        checker: &mut Operation<'_>,
        out: &mut String,
        comments: Vec<NodeId>,
        markdown: bool,
        feature: i32,
    ) -> Result<()> {
        for comment in comments {
            let view = self.view(comment)?;
            let read = view.node(comment)?;
            match read.kind().known() {
                Some(K::JSDocText) => out.push_str(&String::from_utf8_lossy(
                    view.node_text(comment)?.as_bytes(),
                )),
                Some(K::JSDocLink | K::JSDocLinkPlain | K::JSDocLinkCode) => {
                    let quote = read.kind() == K::JSDocLinkCode;
                    let text = String::from_utf8_lossy(view.node_text(comment)?.as_bytes())
                        .trim_matches(' ')
                        .to_owned();
                    if let Some(name) = read.name() {
                        if view.node(name)?.kind() == K::Identifier
                            && matches!(view.node_text(name)?.as_bytes(), b"http" | b"https")
                            && text.starts_with("://")
                        {
                            let link = format!(
                                "{}{text}",
                                String::from_utf8_lossy(view.node_text(name)?.as_bytes())
                            );
                            let (uri, label) = if let Some(pos) = link.find([' ', '|']) {
                                let label = comment_prefix(&link[pos..]);
                                (
                                    &link[..pos],
                                    if label.is_empty() {
                                        &link[..pos]
                                    } else {
                                        label
                                    },
                                )
                            } else {
                                (link.as_str(), link.as_str())
                            };
                            if markdown {
                                markdown_link(out, label, uri, quote);
                            } else {
                                out.push_str(label);
                                if label != uri {
                                    out.push_str(" (");
                                    out.push_str(uri);
                                    out.push(')');
                                }
                            }
                        } else {
                            self.write_name_link(
                                checker, out, name, &text, quote, markdown, feature,
                            )?;
                        }
                    } else {
                        quoted(out, &text, quote && markdown);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/hover.go:writeNameLink
    #[allow(
        clippy::too_many_arguments,
        reason = "The pinned link arguments plus the feature-specific source mapper"
    )]
    fn write_name_link(
        &mut self,
        checker: &mut Operation<'_>,
        out: &mut String,
        name: NodeId,
        text: &str,
        quote: bool,
        markdown: bool,
        feature: i32,
    ) -> Result<()> {
        let decls = self.declarations_at(checker, name)?;
        let view = self.view(name)?;
        if let Some(&decl) = decls.first() {
            let v = self.view(decl)?;
            let node = v.node(decl)?.name().unwrap_or(decl);
            let source = ast::get_source_file_of_node(v, Some(decl))?
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let mut syntax = Syntax::new(v, source)?;
            let (range, fidelity) = self.range(
                source,
                TextRange::new(syntax.start(node)?, i64::from(v.node(node)?.end())),
                feature,
            )?;
            let prefix = if text.starts_with("()") { 2 } else { 0 };
            let label = comment_prefix(&text[prefix..]);
            let label = if label.is_empty() {
                format!("{}{}", entity_name(view, name)?, &text[..prefix])
            } else {
                label.to_owned()
            };
            if markdown && fidelity.is_single_segment() {
                let uri = tsr_lsproto::DocumentUri::from_file_name(
                    syntax.file.original_file_name()?.as_bytes(),
                );
                let target = format!(
                    "{}#{},{}-{},{}",
                    uri.0,
                    range.start.line + 1,
                    range.start.character + 1,
                    range.end.line + 1,
                    range.end.character + 1
                );
                markdown_link(out, &label, &target, quote);
            } else {
                quoted(out, &label, false);
            }
        } else {
            let label = format!(
                "{}{}{text}",
                entity_name(view, name)?,
                if text.is_empty() { "" } else { " " }
            );
            quoted(out, &label, quote && markdown);
        }
        Ok(())
    }
}
