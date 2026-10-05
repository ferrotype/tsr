use crate::{
    change_nodes::{Leading, NodeTracker, Trailing},
    Result,
};
use std::collections::HashSet;
use tsr_ast::{FactoryMethods, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_scanner::{skip_trivia_ex, SkipTriviaOptions};

impl NodeTracker<'_> {
    pub fn containing_list(&self, source: NodeId, node: NodeId) -> Result<Vec<NodeId>> {
        let mut syntax = self.syntax(source)?;
        let mut file = tsr_format::FormatFile {
            view: syntax.view,
            source,
            jsdoc: &mut syntax.docs,
        };
        let Some(list) = tsr_format::get_containing_list(&mut file, node)? else {
            return Ok(Vec::new());
        };
        Ok(syntax
            .view
            .node_slice(syntax.view.list(list)?.nodes())?
            .iter()
            .flatten()
            .collect())
    }
    // port: tsc/internal/ls/change/tracker.go:Tracker.finishDeleteDeclarations
    pub fn finish_deletions(&mut self) -> Result<()> {
        let mut in_lists = HashSet::new();
        for (source, node) in self.deleted.clone() {
            let view = self.source_view(source)?;
            let inner = view.node(node)?.range();
            let mut contained = false;
            for &(other_source, other) in &self.deleted {
                if source != other_source || node == other {
                    continue;
                }
                let outer = view.node(other)?.range();
                if outer.pos() < inner.pos() && inner.end() < outer.end() {
                    contained = true;
                    break;
                }
            }
            if !contained {
                self.delete_declaration(source, node, &mut in_lists)?;
            }
        }
        let mut last_nodes: Vec<_> = in_lists.iter().copied().collect();
        last_nodes.sort();
        for (source, node) in last_nodes {
            let list = self.containing_list(source, node)?;
            if list.last() != Some(&node) {
                continue;
            }
            if let Some(i) = (0..list.len() - 1)
                .rev()
                .find(|&i| !in_lists.contains(&(source, list[i])))
            {
                let start = i64::from(self.source_view(source)?.node(list[i])?.end());
                let end = self.list_delete_start(source, list[i + 1])?;
                self.raw
                    .replace_text(source, TextRange::new(start, end), String::new());
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/change/delete.go:deleteDeclaration
    fn delete_declaration(
        &mut self,
        source: NodeId,
        node: NodeId,
        in_lists: &mut HashSet<(NodeId, NodeId)>,
    ) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let view = syntax.view;
        let read = view.node(node)?;
        let parent = read.parent();
        match read.kind().known() {
            Some(K::Parameter) => {
                let p = parent.expect("parameter parent");
                if view.node(p)?.kind() == K::ArrowFunction
                    && self.containing_list(source, node)?.len() == 1
                    && syntax
                        .nav()
                        .find_child_of_kind(p, K::OpenParenToken)?
                        .is_none()
                {
                    let range = self.adjusted_range(
                        source,
                        node,
                        node,
                        Leading::IncludeAll,
                        Trailing::Include,
                    )?;
                    self.raw.replace_text(source, range, "()".into());
                    Ok(())
                } else {
                    self.delete_list_node(source, node, in_lists)
                }
            }
            Some(K::ImportDeclaration | K::ImportEqualsDeclaration) => {
                let mut first = false;
                if let Some(import) = syntax.file.imports()?.first().copied().flatten() {
                    first = view.node(import)?.parent() == Some(node);
                }
                for stmt in view
                    .node_slice(view.node(source)?.statements(view)?)?
                    .iter()
                    .flatten()
                {
                    if tsr_ast::utilities::is_any_import_syntax(&view.node(stmt)?) {
                        first |= stmt == node;
                        break;
                    }
                }
                let leading = if first {
                    Leading::Exclude
                } else if self.has_jsdoc(source, node)? {
                    Leading::JsDoc
                } else {
                    Leading::StartLine
                };
                self.delete_node(source, node, leading, Trailing::Include)
            }
            Some(K::BindingElement) => {
                let p = parent.expect("binding parent");
                if view.node(p)?.kind() == K::ArrayBindingPattern
                    && self.containing_list(source, node)?.last() != Some(&node)
                {
                    self.delete_node(source, node, Leading::IncludeAll, Trailing::Exclude)
                } else {
                    self.delete_list_node(source, node, in_lists)
                }
            }
            Some(K::VariableDeclaration) => {
                let p = parent.expect("declaration parent");
                if view.node(p)?.kind() == K::CatchClause {
                    let open = syntax
                        .nav()
                        .find_child_of_kind(p, K::OpenParenToken)?
                        .expect("catch open paren");
                    let close = syntax
                        .nav()
                        .find_child_of_kind(p, K::CloseParenToken)?
                        .expect("catch close paren");
                    return self.delete_range(
                        source,
                        open,
                        close,
                        Leading::IncludeAll,
                        Trailing::Include,
                    );
                }
                if self.containing_list(source, node)?.len() != 1 {
                    return self.delete_list_node(source, node, in_lists);
                }
                let gp = view.node(p)?.parent().expect("declaration list parent");
                match view.node(gp)?.kind().known() {
                    Some(K::ForInStatement | K::ForOfStatement) => {
                        let list = crate::completion_snippets::list(&mut self.ast, &[])?;
                        let empty = self.ast.new_object_literal_expression(Some(list), false);
                        self.replace_node(source, node, empty, None)
                    }
                    Some(K::ForStatement) => {
                        self.delete_node(source, p, Leading::IncludeAll, Trailing::Include)
                    }
                    Some(K::VariableStatement) => self.delete_node(
                        source,
                        gp,
                        if self.has_jsdoc(source, gp)? {
                            Leading::JsDoc
                        } else {
                            Leading::StartLine
                        },
                        Trailing::Include,
                    ),
                    kind => panic!("Unexpected grandparent kind: {kind:?}"),
                }
            }
            Some(K::TypeParameter) => self.delete_list_node(source, node, in_lists),
            Some(K::ImportSpecifier) => {
                if self.containing_list(source, node)?.len() == 1 {
                    self.delete_import_binding(source, parent.expect("import parent"))
                } else {
                    self.delete_list_node(source, node, in_lists)
                }
            }
            Some(K::NamespaceImport) => self.delete_import_binding(source, node),
            Some(K::SemicolonToken) => {
                self.delete_node(source, node, Leading::IncludeAll, Trailing::Exclude)
            }
            Some(K::TypeKeyword | K::FunctionKeyword) => {
                self.delete_node(source, node, Leading::Exclude, Trailing::Include)
            }
            Some(K::ClassDeclaration | K::FunctionDeclaration) => self.delete_node(
                source,
                node,
                if self.has_jsdoc(source, node)? {
                    Leading::JsDoc
                } else {
                    Leading::StartLine
                },
                Trailing::Include,
            ),
            _ => {
                if let Some(p) = parent {
                    if view.node(p)?.kind() == K::ImportClause && view.node(p)?.name() == Some(node)
                    {
                        return self.delete_default_import(source, p);
                    }
                    if view.node(p)?.kind() == K::CallExpression
                        && view
                            .node_slice(view.node(p)?.arguments(view)?)?
                            .iter()
                            .flatten()
                            .any(|n| n == node)
                    {
                        return self.delete_list_node(source, node, in_lists);
                    }
                }
                self.delete_node(source, node, Leading::IncludeAll, Trailing::Include)
            }
        }
    }
    fn has_jsdoc(&self, source: NodeId, node: NodeId) -> Result<bool> {
        let syntax = self.syntax(source)?;
        Ok(!tsr_parser::get_jsdoc_comment_ranges(
            &self.ast,
            Vec::new(),
            node,
            syntax.file.text().as_bytes(),
        )
        .is_empty())
    }
    // port: tsc/internal/ls/change/delete.go:deleteDefaultImport
    fn delete_default_import(&mut self, source: NodeId, clause: NodeId) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let read = syntax.view.node(clause)?;
        let data = read
            .data_source()
            .as_import_clause()
            .expect("import clause");
        if data.named_bindings().is_none() {
            return self.delete_node(
                source,
                read.parent().expect("import declaration"),
                Leading::IncludeAll,
                Trailing::Include,
            );
        }
        let name = read.name().expect("default name");
        let start = syntax.start(name)?;
        let end = i64::from(syntax.view.node(name)?.end());
        let token = syntax.nav().get_token_at_position(end)?;
        if syntax.view.node(token)?.kind() == K::CommaToken {
            let end = skip_trivia_ex(
                syntax.file.text().as_bytes(),
                i64::from(syntax.view.node(token)?.end()),
                Some(&SkipTriviaOptions {
                    stop_at_comments: true,
                    ..Default::default()
                }),
            );
            self.raw
                .replace_text(source, TextRange::new(start, end), String::new());
            Ok(())
        } else {
            self.delete_node(source, name, Leading::IncludeAll, Trailing::Include)
        }
    }
    // port: tsc/internal/ls/change/delete.go:deleteImportBinding
    fn delete_import_binding(&mut self, source: NodeId, node: NodeId) -> Result<()> {
        let mut syntax = self.syntax(source)?;
        let read = syntax.view.node(node)?;
        let clause = read.parent().expect("import clause");
        if syntax.view.node(clause)?.name().is_some() {
            let previous = syntax
                .nav()
                .get_token_at_position(i64::from(read.pos()) - 1)?;
            let start = syntax.start(previous)?;
            self.raw.replace_text(
                source,
                TextRange::new(start, i64::from(read.end())),
                String::new(),
            );
            Ok(())
        } else {
            self.delete_node(
                source,
                syntax
                    .view
                    .node(clause)?
                    .parent()
                    .expect("import declaration"),
                Leading::IncludeAll,
                Trailing::Include,
            )
        }
    }
    // port: tsc/internal/ls/change/delete.go:deleteNodeInList
    fn delete_list_node(
        &mut self,
        source: NodeId,
        node: NodeId,
        deleted: &mut HashSet<(NodeId, NodeId)>,
    ) -> Result<()> {
        let list = self.containing_list(source, node)?;
        let index = list
            .iter()
            .position(|&n| n == node)
            .expect("node should be in containing list");
        if list.len() == 1 {
            return self.delete_node(source, node, Leading::IncludeAll, Trailing::Include);
        }
        assert!(deleted.insert((source, node)), "Deleting a node twice");
        let start = self.list_delete_start(source, node)?;
        let end = if index == list.len() - 1 {
            self.adjusted_end(source, node, Trailing::None)?
        } else {
            self.list_delete_end(
                source,
                node,
                index.checked_sub(1).map(|i| list[i]),
                list[index + 1],
            )?
        };
        self.raw
            .replace_text(source, TextRange::new(start, end), String::new());
        Ok(())
    }
    // port: tsc/internal/ls/change/delete.go:Tracker.startPositionToDeleteNodeInList
    pub fn list_delete_start(&self, source: NodeId, node: NodeId) -> Result<i64> {
        Ok(skip_trivia_ex(
            self.syntax(source)?.file.text().as_bytes(),
            self.adjusted_start(source, node, Leading::IncludeAll, false)?,
            Some(&SkipTriviaOptions {
                stop_at_comments: true,
                ..Default::default()
            }),
        ))
    }
    // port: tsc/internal/ls/change/delete.go:Tracker.endPositionToDeleteNodeInList
    fn list_delete_end(
        &self,
        source: NodeId,
        node: NodeId,
        previous: Option<NodeId>,
        next: NodeId,
    ) -> Result<i64> {
        let mut syntax = self.syntax(source)?;
        let end = self.list_delete_start(source, next)?;
        if previous.is_none()
            || syntax.same_line(self.adjusted_end(source, node, Trailing::Include)?, end)
        {
            return Ok(end);
        }
        let next_start = syntax.start(next)?;
        let Some(token) = syntax.nav().find_preceding_token(next_start)? else {
            return Ok(end);
        };
        if is_separator(syntax.view, node, token)? {
            let start = syntax.start(node)?;
            if let Some(prev) = syntax.nav().find_preceding_token(start)? {
                if is_separator(syntax.view, previous.unwrap(), prev)? {
                    let pos = skip_trivia_ex(
                        syntax.file.text().as_bytes(),
                        i64::from(syntax.view.node(token)?.end()),
                        Some(&SkipTriviaOptions {
                            stop_after_line_break: true,
                            stop_at_comments: true,
                            ..Default::default()
                        }),
                    );
                    let a = syntax.start(prev)?;
                    let b = syntax.start(token)?;
                    let text = syntax.file.text().as_bytes();
                    if syntax.same_line(a, b) {
                        return Ok(
                            if pos > 0
                                && text
                                    .get(pos as usize - 1)
                                    .is_some_and(|&b| b == b'\r' || b == b'\n')
                            {
                                pos - 1
                            } else {
                                pos
                            },
                        );
                    }
                    if text
                        .get(pos as usize)
                        .is_some_and(|&b| b == b'\r' || b == b'\n')
                    {
                        return Ok(pos);
                    }
                }
            }
        }
        Ok(end)
    }
}
pub(crate) fn is_separator(
    view: tsr_ast::AstView<'_>,
    node: NodeId,
    candidate: NodeId,
) -> Result<bool> {
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(false);
    };
    let kind = view.node(candidate)?.kind();
    Ok(kind == K::CommaToken
        || kind == K::SemicolonToken && view.node(parent)?.kind() == K::ObjectLiteralExpression)
}
