use crate::{syntax::Syntax, LanguageService, Result};
use tsr_ast::{node_flags, span_map::FEATURE_FOLDING_RANGES, utilities, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

#[derive(Clone, Copy, Default)]
pub struct FoldingOptions {
    pub line_folding_only: bool,
    pub collapsed_text: bool,
}

// port: tsc/internal/ls/folding.go:parseRegionDelimiter
fn region_delimiter(line: &[u8]) -> Option<(bool, String)> {
    let text = String::from_utf8_lossy(line);
    let mut text = text
        .trim_start()
        .strip_prefix("//")?
        .trim()
        .trim_end_matches('\r')
        .strip_prefix('#')?;
    let start = !text.starts_with("end");
    if !start {
        text = &text[3..];
    }
    Some((start, text.strip_prefix("region")?.trim().into()))
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/folding.go:LanguageService.ProvideFoldingRange
    pub fn folding_ranges(
        &mut self,
        uri: &lsp::DocumentUri,
        options: FoldingOptions,
    ) -> Result<lsp::FoldingRangesOrNull> {
        let source = self.file(uri)?;
        let file = self.source(source)?;
        let files: Vec<_> = std::iter::once(source)
            .chain(file.supplemental_source_files()?.iter().flatten().copied())
            .collect();
        let mut result = Vec::new();
        for source in files {
            self.check_canceled()?;
            let mut syntax = Syntax::new(self.view(source)?, source)?;
            let mut ranges = self.node_outlining(&mut syntax, options)?;
            self.region_outlining(&mut syntax, options, &mut ranges)?;
            if options.line_folding_only {
                self.adjust_folding_end(&syntax, &mut ranges)?;
            }
            result.extend(ranges);
        }
        result.sort_by_key(|r| {
            (
                r.start_line,
                *r.start_character.as_deref().unwrap(),
                r.end_line,
                *r.end_character.as_deref().unwrap(),
            )
        });
        // Equal ranges may have different kinds or banners. Go deduplicates the
        // full key, preserving the first occurrence in this stable ordering.
        let mut seen = std::collections::HashSet::new();
        result.retain(|r| {
            seen.insert((
                r.start_line,
                r.start_character.clone(),
                r.end_line,
                r.end_character.clone(),
                r.kind.as_ref().map(|k| k.0.clone()),
                r.collapsed_text.clone(),
            ))
        });
        Ok(lsp::FoldingRangesOrNull {
            folding_ranges: Some(Box::new(
                result.into_iter().map(|r| Some(Box::new(r))).collect(),
            )),
        })
    }
    // port: tsc/internal/ls/folding.go:LanguageService.adjustFoldingEnd
    fn adjust_folding_end(
        &mut self,
        syntax: &Syntax<'_>,
        ranges: &mut [lsp::FoldingRange],
    ) -> Result<()> {
        for range in ranges {
            if let Some(&character) = range.end_character.as_deref().filter(|&&c| c > 0) {
                let positions = self.converters.from_lsp_position_for_source_file(
                    self.program,
                    syntax.source,
                    &lsp::Position {
                        line: range.end_line,
                        character,
                    },
                    FEATURE_FOLDING_RANGES,
                )?;
                if let Some(pos) = positions
                    .iter()
                    .find(|p| p.script == syntax.source && !p.mapped.fidelity.is_none())
                    .map(|p| p.mapped.position)
                    .filter(|&pos| pos > 0)
                {
                    if matches!(
                        syntax.file.text().as_bytes().get(pos as usize - 1),
                        Some(b'}' | b']' | b')' | b'`' | b'>')
                    ) && range.end_line > range.start_line
                    {
                        range.end_line -= 1;
                    }
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/folding.go:createFoldingRangeFromBounds
    fn fold(
        &mut self,
        source: NodeId,
        start: i64,
        end: i64,
        kind: &str,
        banner: &str,
        options: FoldingOptions,
    ) -> Result<Option<lsp::FoldingRange>> {
        let (range, fidelity) =
            self.range(source, TextRange::new(start, end), FEATURE_FOLDING_RANGES)?;
        if fidelity.is_none() {
            return Ok(None);
        }
        // port: tsc/internal/ls/folding.go:createFoldingRange
        Ok(Some(lsp::FoldingRange {
            start_line: range.start.line,
            start_character: Some(Box::new(range.start.character)),
            end_line: range.end.line,
            end_character: Some(Box::new(range.end.character)),
            kind: (!kind.is_empty()).then(|| Box::new(lsp::FoldingRangeKind(kind.into()))),
            collapsed_text: (options.collapsed_text && !banner.is_empty())
                .then(|| Box::new(banner.into())),
        }))
    }
    // port: tsc/internal/ls/folding.go:LanguageService.addNodeOutliningSpans
    fn node_outlining(
        &mut self,
        syntax: &mut Syntax<'_>,
        options: FoldingOptions,
    ) -> Result<Vec<lsp::FoldingRange>> {
        let view = syntax.view;
        let source = view.node(syntax.source)?;
        let statements: Vec<_> = view
            .node_slice(source.statements(view)?)?
            .iter()
            .flatten()
            .collect();
        let mut result = Vec::new();
        let mut current = 0;
        while current < statements.len() {
            if !utilities::is_any_import_syntax(&view.node(statements[current])?) {
                self.folding_visit(syntax, statements[current], 40, options, &mut result)?;
                current += 1;
                continue;
            }
            let first = current;
            while current < statements.len()
                && utilities::is_any_import_syntax(&view.node(statements[current])?)
            {
                self.folding_visit(syntax, statements[current], 40, options, &mut result)?;
                current += 1;
            }
            if current - 1 != first {
                if let Some(open) = syntax
                    .nav()
                    .find_child_of_kind(statements[first], K::ImportKeyword)?
                {
                    result.extend(self.fold(
                        syntax.source,
                        syntax.start(open)?,
                        i64::from(view.node(statements[current - 1])?.end()),
                        "imports",
                        "",
                        options,
                    )?);
                }
            }
        }
        if let Some(eof) = source
            .data_source()
            .as_source_file()
            .and_then(|data| data.end_of_file_token())
        {
            self.folding_visit(syntax, eof, 40, options, &mut result)?;
        }
        Ok(result)
    }
    // port: tsc/internal/ls/folding.go:LanguageService.addRegionOutliningSpans
    fn region_outlining(
        &mut self,
        syntax: &mut Syntax<'_>,
        options: FoldingOptions,
        result: &mut Vec<lsp::FoldingRange>,
    ) -> Result<()> {
        let mut regions = Vec::new();
        let file = syntax.view.source_file(syntax.source)?;
        let text = file.text().as_bytes();
        for &start in file.ecma_line_map() {
            self.check_canceled()?;
            let end = syntax.line_end(i64::from(start));
            let line = &text[start as usize..end as usize];
            let Some((is_start, name)) = region_delimiter(line) else {
                continue;
            };
            if syntax.in_comment(i64::from(start))? {
                continue;
            }
            if is_start {
                let pos =
                    line.windows(2).position(|w| w == b"//").unwrap() as i64 + i64::from(start);
                regions.push((
                    pos,
                    if name.is_empty() {
                        "#region".into()
                    } else {
                        name
                    },
                ));
            } else if let Some((pos, name)) = regions.pop() {
                result.extend(self.fold(syntax.source, pos, end, "region", &name, options)?);
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/folding.go:visitNode
    fn folding_visit(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        depth: usize,
        options: FoldingOptions,
        result: &mut Vec<lsp::FoldingRange>,
    ) -> Result<()> {
        // An explicit stack also bounds long call/else-if chains for which the
        // pin deliberately does not charge a nesting level.
        let mut stack = vec![(node, depth)];
        let view = syntax.view;
        while let Some((node, depth)) = stack.pop() {
            self.check_canceled()?;
            let read = view.node(node)?;
            if depth == 0 || read.flags() & node_flags::REPARSED != 0 {
                continue;
            }
            let kind = read.kind();
            if kind != K::BinaryExpression && tsr_ast::is_declaration(&read)
                || matches!(
                    kind.known(),
                    Some(
                        K::VariableStatement
                            | K::ReturnStatement
                            | K::CallExpression
                            | K::NewExpression
                            | K::EndOfFile
                    )
                )
            {
                self.fold_comments(syntax, i64::from(read.pos()), options, result)?;
            }
            if utilities::is_function_like(Some(&read)) {
                if let Some(parent) = read.parent() {
                    if let Some(binary) = view.node(parent)?.data_source().as_binary_expression() {
                        if let Some(left) = binary.left().filter(|id| {
                            view.node(*id)
                                .is_ok_and(|n| n.kind() == K::PropertyAccessExpression)
                        }) {
                            self.fold_comments(
                                syntax,
                                i64::from(view.node(left)?.pos()),
                                options,
                                result,
                            )?;
                        }
                    }
                }
            }
            let comment_list = match kind.known() {
                Some(K::Block) => read.data_source().as_block().and_then(|d| d.statements()),
                Some(K::ModuleBlock) => read
                    .data_source()
                    .as_module_block()
                    .and_then(|d| d.statements()),
                Some(K::ClassDeclaration | K::ClassExpression) => read
                    .data_source()
                    .as_class_declaration()
                    .and_then(|d| d.members()),
                Some(K::InterfaceDeclaration) => read
                    .data_source()
                    .as_interface_declaration()
                    .and_then(|d| d.members()),
                _ => None,
            };
            if let Some(list) = comment_list {
                self.fold_comments(syntax, view.list(list)?.loc().end(), options, result)?;
            }
            result.extend(self.outlining_span(syntax, node, options)?);
            let mut children = Vec::new();
            if kind == K::CallExpression {
                if let Some(expression) = read.expression() {
                    children.push((expression, depth));
                }
                for slice in [read.arguments(view)?, read.type_arguments(view)?] {
                    children.extend(
                        view.node_slice(slice)?
                            .iter()
                            .flatten()
                            .map(|id| (id, depth - 1)),
                    );
                }
            } else if let Some(data) = read.data_source().as_if_statement().filter(|d| {
                d.else_statement()
                    .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::IfStatement))
            }) {
                children.extend(read.expression().map(|id| (id, depth - 1)));
                children.extend(data.then_statement().map(|id| (id, depth - 1)));
                children.extend(data.else_statement().map(|id| (id, depth)));
            } else {
                children.extend(syntax.children(node)?.into_iter().map(|id| (id, depth - 1)));
            }
            stack.extend(children.into_iter().rev());
        }
        Ok(())
    }
    // port: tsc/internal/ls/folding.go:addOutliningForLeadingCommentsForPos
    fn fold_comments(
        &mut self,
        syntax: &Syntax<'_>,
        pos: i64,
        options: FoldingOptions,
        result: &mut Vec<lsp::FoldingRange>,
    ) -> Result<()> {
        let mut single = Vec::new();
        let flush = |service: &mut Self,
                     single: &mut Vec<TextRange>,
                     result: &mut Vec<lsp::FoldingRange>|
         -> Result<()> {
            if single.len() > 1 {
                result.extend(service.fold(
                    syntax.source,
                    single[0].pos(),
                    single.last().unwrap().end(),
                    "comment",
                    "",
                    options,
                )?);
            }
            single.clear();
            Ok(())
        };
        let text = syntax.file.text().as_bytes();
        for comment in tsr_scanner::get_leading_comment_ranges(text, pos) {
            self.check_canceled()?;
            if comment.kind == K::SingleLineCommentTrivia {
                if region_delimiter(&text[comment.loc.pos() as usize..comment.loc.end() as usize])
                    .is_some()
                {
                    flush(self, &mut single, result)?;
                } else {
                    single.push(comment.loc);
                }
            } else {
                flush(self, &mut single, result)?;
                result.extend(self.fold(
                    syntax.source,
                    comment.loc.pos(),
                    comment.loc.end(),
                    "comment",
                    "",
                    options,
                )?);
            }
        }
        flush(self, &mut single, result)
    }
    // port: tsc/internal/ls/folding.go:getOutliningSpanForNode
    fn outlining_span(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        options: FoldingOptions,
    ) -> Result<Option<lsp::FoldingRange>> {
        let view = syntax.view;
        let read = view.node(node)?;
        let parent = read.parent().map(|id| view.node(id)).transpose()?;
        let parent_kind = parent.as_ref().and_then(|n| n.kind().known());
        let start = syntax.start(node)?;
        let end = i64::from(read.end());
        let mut open_kind = K::OpenBraceToken;
        let mut full_start = true;
        let tokens_node = node;
        let mut explicit_open = None;
        match read.kind().known() {
            Some(K::Block) => {
                if utilities::is_function_like(parent.as_ref()) {
                    let parent = parent.as_ref().unwrap();
                    let params: Vec<_> = view
                        .node_slice(parent.parameters(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    if let (Some(first), Some(last)) = (params.first(), params.last()) {
                        if !syntax.same_line(
                            i64::from(view.node(*first)?.pos()),
                            i64::from(view.node(*last)?.end()),
                        ) {
                            explicit_open = syntax
                                .nav()
                                .find_child_of_kind(read.parent().unwrap(), K::OpenParenToken)?;
                        }
                    }
                } else if !matches!(
                    parent_kind,
                    Some(
                        K::DoStatement
                            | K::ForInStatement
                            | K::ForOfStatement
                            | K::ForStatement
                            | K::IfStatement
                            | K::WhileStatement
                            | K::WithStatement
                            | K::CatchClause
                            | K::TryStatement
                    )
                ) {
                    return self.fold(syntax.source, start, end, "", "", options);
                }
            }
            Some(
                K::ModuleBlock
                | K::ClassDeclaration
                | K::ClassExpression
                | K::InterfaceDeclaration
                | K::EnumDeclaration
                | K::CaseBlock
                | K::TypeLiteral
                | K::ObjectBindingPattern,
            ) => {}
            Some(K::TupleType) => {
                open_kind = K::OpenBracketToken;
                full_start = parent_kind != Some(K::TupleType);
            }
            Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression) => {
                if read.kind() == K::ArrayLiteralExpression {
                    open_kind = K::OpenBracketToken;
                }
                full_start = !matches!(
                    parent_kind,
                    Some(K::ArrayLiteralExpression | K::CallExpression)
                );
            }
            Some(K::ArrayBindingPattern) => {
                open_kind = K::OpenBracketToken;
                full_start = parent_kind != Some(K::BindingElement);
            }
            Some(K::CaseClause | K::DefaultClause) => {
                let list = read
                    .data_source()
                    .as_case_or_default_clause()
                    .and_then(|d| d.statements());
                if let Some(list) = list {
                    let list = view.list(list)?;
                    if !view.node_slice(list.nodes())?.is_empty() {
                        return self.fold(
                            syntax.source,
                            list.loc().pos(),
                            list.loc().end(),
                            "",
                            "",
                            options,
                        );
                    }
                }
                return Ok(None);
            }
            Some(K::ParenthesizedExpression) => {
                return if syntax.same_line(start, end) {
                    Ok(None)
                } else {
                    self.fold(syntax.source, start, end, "", "", options)
                };
            }
            Some(K::ArrowFunction) => {
                let Some(body) = read.body() else {
                    return Ok(None);
                };
                let body = view.node(body)?;
                return if matches!(
                    body.kind().known(),
                    Some(K::Block | K::ParenthesizedExpression)
                ) || syntax.same_line(i64::from(body.pos()), i64::from(body.end()))
                {
                    Ok(None)
                } else {
                    self.fold(
                        syntax.source,
                        i64::from(body.pos()),
                        i64::from(body.end()),
                        "",
                        "",
                        options,
                    )
                };
            }
            Some(K::TemplateExpression | K::NoSubstitutionTemplateLiteral) => {
                if read.kind() == K::NoSubstitutionTemplateLiteral
                    && read
                        .data_source()
                        .as_no_substitution_template_literal()
                        .unwrap()
                        .text()
                        .is_empty()
                {
                    return Ok(None);
                }
                return self.fold(syntax.source, start, end, "", "", options);
            }
            Some(K::JsxElement) => {
                let data = read.data_source().as_jsx_element().unwrap();
                let open = data
                    .opening_element()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let close = data
                    .closing_element()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let name = view
                    .node(open)?
                    .tag_name()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let text = tsr_scanner::get_text_of_node(view, name)?;
                let text = String::from_utf8_lossy(text.as_bytes());
                return self.fold(
                    syntax.source,
                    syntax.start(open)?,
                    i64::from(view.node(close)?.end()),
                    "",
                    &format!("<{text}>...</{text}>"),
                    options,
                );
            }
            Some(K::JsxFragment) => {
                let data = read.data_source().as_jsx_fragment().unwrap();
                let open = data
                    .opening_fragment()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let close = data
                    .closing_fragment()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                return self.fold(
                    syntax.source,
                    syntax.start(open)?,
                    i64::from(view.node(close)?.end()),
                    "",
                    "<>...</>",
                    options,
                );
            }
            Some(K::JsxSelfClosingElement | K::JsxOpeningElement) => {
                let Some(attributes) = read.attributes() else {
                    return Ok(None);
                };
                if view
                    .node_slice(view.node(attributes)?.properties(view)?)?
                    .is_empty()
                {
                    return Ok(None);
                }
                return self.fold(syntax.source, start, end, "", "", options);
            }
            Some(K::CallExpression) => {
                if view.node_slice(read.arguments(view)?)?.is_empty() {
                    return Ok(None);
                }
                open_kind = K::OpenParenToken;
            }
            Some(K::NamedImports | K::NamedExports | K::ImportAttributes) => {
                if syntax.children(node)?.is_empty() {
                    return Ok(None);
                }
                full_start = false;
            }
            _ => return Ok(None),
        }
        let close_kind = match open_kind {
            K::OpenParenToken => K::CloseParenToken,
            K::OpenBracketToken => K::CloseBracketToken,
            _ => K::CloseBraceToken,
        };
        let open = match explicit_open {
            Some(id) => Some(id),
            None => syntax.nav().find_child_of_kind(tokens_node, open_kind)?,
        };
        let close = syntax.nav().find_child_of_kind(tokens_node, close_kind)?;
        let (Some(open), Some(close)) = (open, close) else {
            return Ok(None);
        };
        if matches!(
            read.kind().known(),
            Some(K::CallExpression | K::NamedImports | K::NamedExports | K::ImportAttributes)
        ) && syntax.same_line(
            i64::from(view.node(open)?.pos()),
            i64::from(view.node(close)?.pos()),
        ) {
            return Ok(None);
        }
        let start = if full_start {
            i64::from(view.node(open)?.pos())
        } else {
            syntax.start(open)?
        };
        self.fold(
            syntax.source,
            start,
            i64::from(view.node(close)?.end()),
            "",
            "",
            options,
        )
    }
}
