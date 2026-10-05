use crate::{
    change_nodes::{line_start, Leading, NodeOptions, NodeTracker, Trailing},
    Result,
};
use tsr_ast::{utilities as ast, FactoryMethods, NodeId, NodeListId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_scanner::{skip_trivia_ex, SkipTriviaOptions};

impl NodeTracker<'_> {
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertNodeAfter
    pub fn insert_after(&mut self, source: NodeId, after: NodeId, new: NodeId) -> Result<()> {
        let syntax = self.syntax(source)?;
        let a = syntax.view.node(after)?;
        let b = self.ast.view().node(new)?;
        let need_semicolon = matches!(
            a.kind().known(),
            Some(K::PropertySignature | K::PropertyDeclaration)
        ) && tsr_ast::utilities_class::is_class_or_type_element(&b)
            && b.name().is_some_and(|n| {
                self.ast
                    .view()
                    .node(n)
                    .is_ok_and(|n| n.kind() == K::ComputedPropertyName)
            })
            || ast::is_statement_but_not_declaration(&a)
                && ast::is_statement_but_not_declaration(&b);
        let end = i64::from(a.end());
        if need_semicolon && syntax.file.text().as_bytes().get(a.end() as usize - 1) != Some(&b';')
        {
            self.raw
                .replace_text(source, TextRange::new(end, end), ";".into());
        }
        let options = match a.kind().known() {
            Some(K::Parameter) => NodeOptions::default(),
            Some(K::ClassDeclaration | K::ModuleDeclaration) => NodeOptions {
                prefix: self.newline.clone(),
                suffix: self.newline.clone(),
                ..Default::default()
            },
            Some(K::VariableDeclaration | K::StringLiteral | K::Identifier) => NodeOptions {
                prefix: ", ".into(),
                ..Default::default()
            },
            Some(K::PropertyAssignment) => NodeOptions {
                suffix: format!(",{}", self.newline),
                ..Default::default()
            },
            Some(K::ExportKeyword) => NodeOptions {
                prefix: " ".into(),
                ..Default::default()
            },
            _ => {
                assert!(
                    ast::is_statement(syntax.view, after)?
                        || tsr_ast::utilities_class::is_class_or_type_element(&a),
                    "unimplemented node type in changeTracker.getInsertNodeAfterOptions"
                );
                NodeOptions {
                    suffix: self.newline.clone(),
                    ..Default::default()
                }
            }
        };
        let mut options = options;
        if a.end() == syntax.view.node(source)?.end() && ast::is_statement(syntax.view, after)? {
            options.prefix.insert_str(0, &self.newline);
        }
        let end = self.adjusted_end(source, after, Trailing::None)?;
        self.insert_node(source, end, new, options);
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertNodeBefore
    pub fn insert_before(
        &mut self,
        source: NodeId,
        before: NodeId,
        new: NodeId,
        blank: bool,
        leading: Leading,
    ) -> Result<()> {
        let view = self.source_view(source)?;
        let read = view.node(before)?;
        let kind = read.kind();
        let suffix = if ast::is_statement(view, before)?
            || tsr_ast::utilities_class::is_class_or_type_element(&read)
        {
            if blank {
                self.newline.repeat(2)
            } else {
                self.newline.clone()
            }
        } else if kind == K::VariableDeclaration
            || kind == K::NamedImports
            || kind == K::StringLiteral
                && read
                    .parent()
                    .is_some_and(|p| view.node(p).is_ok_and(|p| p.kind() == K::ImportDeclaration))
        {
            ", ".into()
        } else if kind == K::Parameter {
            if self.ast.view().node(new)?.kind() == K::Parameter {
                ", "
            } else {
                ""
            }
            .into()
        } else if kind == K::ImportSpecifier {
            format!(",{}", if blank { &self.newline } else { " " })
        } else {
            panic!(
                "unimplemented node type in changeTracker.getOptionsForInsertNodeBefore: {kind:?}"
            )
        };
        let pos = self.adjusted_start(source, before, leading, false)?;
        self.insert_node(
            source,
            pos,
            new,
            NodeOptions {
                suffix,
                ..Default::default()
            },
        );
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.TryInsertTypeAnnotation
    pub fn insert_type_annotation(
        &mut self,
        source: NodeId,
        node: NodeId,
        type_node: NodeId,
    ) -> Result<bool> {
        let mut syntax = self.syntax(source)?;
        let read = syntax.view.node(node)?;
        let end_node = if ast::is_function_like(Some(&read)) {
            let close = syntax.nav().find_child_of_kind(node, K::CloseParenToken)?;
            if close.is_none() && read.kind() != K::ArrowFunction {
                return Ok(false);
            }
            close.or(syntax
                .view
                .node_slice(read.parameters(syntax.view)?)?
                .first()
                .flatten())
        } else {
            let postfix = match read.kind().known() {
                Some(K::VariableDeclaration) => read
                    .data_source()
                    .as_variable_declaration()
                    .and_then(|d| d.exclamation_token()),
                Some(K::PropertySignature) => read
                    .data_source()
                    .as_property_signature_declaration()
                    .and_then(|d| d.postfix_token()),
                Some(K::PropertyDeclaration) => read
                    .data_source()
                    .as_property_declaration()
                    .and_then(|d| d.postfix_token()),
                Some(K::Parameter) => read
                    .data_source()
                    .as_parameter_declaration()
                    .and_then(|d| d.question_token()),
                _ => None,
            };
            postfix.or(read.name())
        };
        let Some(end_node) = end_node else {
            return Ok(false);
        };
        self.insert_node(
            source,
            i64::from(syntax.view.node(end_node)?.end()),
            type_node,
            NodeOptions {
                prefix: ": ".into(),
                ..Default::default()
            },
        );
        Ok(true)
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.ParenthesizeArrowParameters
    pub fn parenthesize_arrow_parameters(&mut self, source: NodeId, arrow: NodeId) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        if syntax
            .nav()
            .find_child_of_kind(arrow, K::CloseParenToken)?
            .is_some()
        {
            return Ok(());
        }
        let params: Vec<_> = syntax
            .view
            .node_slice(syntax.view.node(arrow)?.parameters(syntax.view)?)?
            .iter()
            .flatten()
            .collect();
        if let (Some(&first), Some(&last)) = (params.first(), params.last()) {
            let start = syntax.start(first)?;
            let end = i64::from(syntax.view.node(last)?.end());
            self.raw
                .replace_text(source, TextRange::new(start, start), "(".into());
            self.raw
                .replace_text(source, TextRange::new(end, end), ")".into());
        }
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertModifierBefore
    pub fn insert_modifier_before(
        &mut self,
        source: NodeId,
        modifier: K,
        before: NodeId,
    ) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let pos = syntax.start(before)?;
        let token = self.ast.new_token(modifier.into());
        self.ast
            .node_mut(token)?
            .set_range(TextRange::new(pos, pos));
        self.ast
            .node_mut(token)?
            .set_parent(syntax.view.node(before)?.parent());
        self.insert_node(
            source,
            pos,
            token,
            NodeOptions {
                suffix: " ".into(),
                ..Default::default()
            },
        );
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertNodeInListAfter
    pub fn insert_in_list_after(
        &mut self,
        source: NodeId,
        after: NodeId,
        new: NodeId,
    ) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let mut file = tsr_format::FormatFile {
            view: syntax.view,
            source,
            jsdoc: &mut syntax.docs,
        };
        let Some(list_id) = tsr_format::get_containing_list(&mut file, after)? else {
            return Ok(());
        };
        let list = syntax.view.list(list_id)?;
        let nodes: Vec<_> = syntax
            .view
            .node_slice(list.nodes())?
            .iter()
            .flatten()
            .collect();
        let Some(index) = nodes.iter().position(|&n| n == after) else {
            return Ok(());
        };
        let ar = syntax.view.node(after)?;
        let end = i64::from(ar.end());
        let source_text = syntax.file.text().clone();
        let text = source_text.as_bytes();
        if index + 1 < nodes.len() {
            let next_token = syntax.nav().get_token_at_position(end)?;
            if crate::change_delete::is_separator(syntax.view, after, next_token)? {
                let token = syntax.view.node(next_token)?;
                let start = skip_trivia_ex(
                    text,
                    i64::from(syntax.view.node(nodes[index + 1])?.pos()),
                    Some(&SkipTriviaOptions {
                        stop_at_comments: true,
                        ..Default::default()
                    }),
                );
                let suffix = format!(
                    "{}{}",
                    if token.kind() == K::CommaToken {
                        ","
                    } else {
                        ";"
                    },
                    String::from_utf8_lossy(&text[token.end() as usize..start as usize])
                );
                self.insert_node(
                    source,
                    start,
                    new,
                    NodeOptions {
                        suffix,
                        ..Default::default()
                    },
                );
            }
            return Ok(());
        }
        let after_start = syntax.start(after)?;
        let after_line = line_start(&syntax.file, after_start);
        let mut separator = ",";
        let mut multiline = false;
        if nodes.len() > 1 {
            if let Some(token) = syntax.nav().find_preceding_token(i64::from(ar.pos()))? {
                if crate::change_delete::is_separator(syntax.view, after, token)?
                    && syntax.view.node(token)?.kind() == K::SemicolonToken
                {
                    separator = ";";
                }
            }
            let previous = syntax.start(nodes[index - 1])?;
            multiline = line_start(&syntax.file, previous) != after_line;
        }
        let has_comment = has_comments_before_line_break(text, end as usize);
        multiline |= has_comment || !syntax.same_line(list.loc().pos(), list.loc().end());
        if multiline {
            self.raw
                .replace_text(source, TextRange::new(end, end), separator.into());
            let file = tsr_format::FormatFile {
                view: syntax.view,
                source,
                jsdoc: &mut syntax.docs,
            };
            let indentation = tsr_format::find_first_non_whitespace_column(
                &file,
                after_line,
                after_start,
                &self.settings,
            )?;
            let mut pos = skip_trivia_ex(
                text,
                end,
                Some(&SkipTriviaOptions {
                    stop_after_line_break: true,
                    ..Default::default()
                }),
            );
            while pos != end
                && text
                    .get(pos as usize - 1)
                    .is_some_and(|&b| b == b'\r' || b == b'\n')
            {
                pos -= 1;
            }
            self.insert_node(
                source,
                pos,
                new,
                NodeOptions {
                    indentation: Some(indentation),
                    prefix: self.newline.clone(),
                    ..Default::default()
                },
            );
        } else {
            self.insert_node(
                source,
                end,
                new,
                NodeOptions {
                    prefix: format!("{separator} "),
                    ..Default::default()
                },
            );
        }
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.InsertMemberAtStart
    pub fn insert_member_at_start(
        &mut self,
        source: NodeId,
        container: NodeId,
        new: NodeId,
    ) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let view = syntax.view;
        let node = view.node(container)?;
        let Some(list) = members(view, container)? else {
            return Ok(());
        };
        let list = view.list(list)?;
        let members: Vec<_> = view.node_slice(list.nodes())?.iter().flatten().collect();
        let tab_size = if self.settings.editor.tab_size <= 0 {
            4
        } else {
            self.settings.editor.tab_size
        };
        let mut indentation = -1;
        let mut last = container;
        for &member in &members {
            let last_start = syntax.start(last)?;
            let start = syntax.start(member)?;
            if syntax.same_line(last_start, start) {
                indentation = -1;
                break;
            }
            let column = indentation_column(
                syntax.file.text().as_bytes(),
                line_start(&syntax.file, start),
                start,
                tab_size,
            );
            if column < 0 || indentation >= 0 && indentation != column {
                indentation = -1;
                break;
            }
            indentation = column;
            last = member;
        }
        if indentation < 0 {
            let start = syntax.start(container)?;
            indentation = indentation_column(
                syntax.file.text().as_bytes(),
                line_start(&syntax.file, start),
                start,
                tab_size,
            )
            .max(0)
                + if self.settings.editor.indent_size <= 0 {
                    4
                } else {
                    self.settings.editor.indent_size
                };
        }
        let previous = self.insertions_at_start.contains(&(source, container));
        if !previous {
            self.insertions_at_start.push((source, container));
        }
        let object = node.kind() == K::ObjectLiteralExpression;
        let json = syntax.file.script_kind == tsr_core::ScriptKind::JSON;
        let suffix = if object && (!members.is_empty() || !json) {
            ","
        } else if node.kind() == K::InterfaceDeclaration && members.is_empty() {
            ";"
        } else {
            ""
        };
        let prefix = if object && json && members.is_empty() && previous {
            format!(",{}", self.newline)
        } else {
            self.newline.clone()
        };
        self.insert_node(
            source,
            list.loc().pos(),
            new,
            NodeOptions {
                indentation: Some(indentation),
                prefix,
                suffix: suffix.into(),
                ..Default::default()
            },
        );
        Ok(())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.finishNodesWithInsertionsAtStart
    pub fn finish_insertions_at_start(&mut self) -> Result<()> {
        for (source, node) in self.insertions_at_start.clone() {
            let mut syntax = self.syntax(source)?;
            let (Some(open), Some(close)) = (
                syntax.nav().find_child_of_kind(node, K::OpenBraceToken)?,
                syntax.nav().find_child_of_kind(node, K::CloseBraceToken)?,
            ) else {
                continue;
            };
            let open = i64::from(syntax.view.node(open)?.end());
            let close = i64::from(syntax.view.node(close)?.end());
            let empty = if let Some(list) = members(syntax.view, node)? {
                syntax
                    .view
                    .node_slice(syntax.view.list(list)?.nodes())?
                    .is_empty()
            } else {
                true
            };
            let single = syntax.same_line(open, close);
            if empty && single && open != close - 1 {
                self.raw
                    .replace_text(source, TextRange::new(open, close - 1), String::new());
            }
            if single {
                self.raw.replace_text(
                    source,
                    TextRange::new(close - 1, close - 1),
                    self.newline.clone(),
                );
            }
        }
        Ok(())
    }
}
// port: tsc/internal/ls/change/trackerimpl.go:hasCommentsBeforeLineBreak
fn has_comments_before_line_break(text: &[u8], start: usize) -> bool {
    let mut remaining = &text[start..];
    while !remaining.is_empty() {
        let (ch, width) = tsr_jsstring::wtf8::decode_utf8(remaining);
        if !tsr_scanner::is_white_space_single_line(ch) {
            return ch == i32::from(b'/');
        }
        remaining = &remaining[width..];
    }
    false
}

fn members(view: tsr_ast::AstView<'_>, node: NodeId) -> Result<Option<NodeListId>> {
    let node = view.node(node)?;
    Ok(if node.kind() == K::ObjectLiteralExpression {
        node.property_list()
    } else {
        node.member_list()
    })
}
// port: tsc/internal/ls/change/tracker.go:findIndentationColumn
fn indentation_column(text: &[u8], line_start: i64, member_start: i64, tab_size: i64) -> i64 {
    let mut column = 0;
    for &ch in &text[line_start as usize..(member_start as usize).min(text.len())] {
        if ch == b'\r' || ch == b'\n' {
            return -1;
        }
        if !matches!(ch, b' ' | b'\t' | 0x0b | 0x0c) {
            return column;
        }
        column += if ch == b'\t' {
            tab_size - column % tab_size
        } else {
            1
        };
    }
    column
}

#[cfg(test)]
mod tests {
    use super::has_comments_before_line_break;

    #[test]
    fn trailing_comments_skip_single_line_unicode_space_only() {
        for whitespace in [
            "",
            " \t\u{b}\u{c}",
            "\u{85}",
            "\u{a0}",
            "\u{2003}",
            "\u{200b}",
            "\u{feff}",
        ] {
            let text = format!("name{whitespace}/*keep*/");
            assert!(has_comments_before_line_break(text.as_bytes(), 4));
        }
        for preceding in ["\n", "\r\n", "\u{2028}", "\u{2029}", "other"] {
            let text = format!("name {preceding} /*later*/");
            assert!(!has_comments_before_line_break(text.as_bytes(), 4));
        }
        assert!(!has_comments_before_line_break(b"name \xff /*later*/", 4));
        assert!(!has_comments_before_line_break(b"name", 4));
    }
}
