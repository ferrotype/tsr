use crate::Result;
use tsr_ast::{AstView, NodeId, SourceFileRead};
use tsr_astnav::{Navigator, Visit};

pub(crate) struct Syntax<'a> {
    pub view: AstView<'a>,
    pub source: NodeId,
    pub file: SourceFileRead<'a>,
    pub docs: tsr_parser::ParserJsDocProvider,
}
impl<'a> Syntax<'a> {
    pub fn new(view: AstView<'a>, source: NodeId) -> Result<Self> {
        Ok(Self {
            view,
            source,
            file: view.source_file(source)?,
            docs: tsr_parser::ParserJsDocProvider::default(),
        })
    }
    pub fn nav(&mut self) -> Navigator<'a, '_> {
        Navigator::new(self.view, self.source, &mut self.docs)
    }
    pub fn start(&mut self, node: NodeId) -> Result<i64> {
        Ok(self.nav().get_start_of_node(node, false)?)
    }
    // port: tsc/internal/ls/findallreferences.go:getRangeOfNode
    pub fn reference_range(
        &mut self,
        node: NodeId,
        end_node: Option<NodeId>,
    ) -> Result<tsr_core::TextRange> {
        let mut start = self.start(node)?;
        let mut end = i64::from(self.view.node(end_node.unwrap_or(node))?.end());
        if tsr_ast::utilities::is_string_literal_like(&self.view.node(node)?) && end - start > 2 {
            assert!(
                end_node.is_none(),
                "endNode is not nil for stringLiteralLike"
            );
            start += 1;
            end -= 1;
        }
        if let Some(end_node) = end_node {
            let read = self.view.node(end_node)?;
            if read.kind() == tsr_ast::SyntaxKind::CaseBlock {
                end = i64::from(read.pos());
            }
        }
        Ok(tsr_core::TextRange::new(start, end))
    }
    pub fn children(&self, node: NodeId) -> Result<Vec<NodeId>> {
        let mut result = Vec::new();
        for child in tsr_astnav::visit_each_child(self.view, node)? {
            match child {
                Visit::Node(id) => result.push(id),
                Visit::List(list) => result.extend(
                    self.view
                        .node_slice(self.view.list(list)?.nodes())?
                        .iter()
                        .flatten(),
                ),
            }
        }
        Ok(result)
    }
    pub fn same_line(&self, a: i64, b: i64) -> bool {
        self.file
            .ecma_line_map()
            .partition_point(|&p| i64::from(p) <= a)
            == self
                .file
                .ecma_line_map()
                .partition_point(|&p| i64::from(p) <= b)
    }
    // port: tsc/internal/ls/completions.go:getLineEndOfPosition
    pub fn line_end(&self, pos: i64) -> i64 {
        let starts = self.file.ecma_line_map();
        let next = starts.partition_point(|&p| i64::from(p) <= pos);
        let end = starts
            .get(next)
            .map_or(self.file.text().len() as i64, |&p| i64::from(p) - 1);
        let text = self.file.text().as_bytes();
        if end > 0
            && text.get(end as usize) == Some(&b'\n')
            && text.get(end as usize - 1) == Some(&b'\r')
        {
            end - 1
        } else {
            end
        }
    }
    // port: tsc/internal/ls/utilities.go:isInComment
    pub fn in_comment(&mut self, position: i64) -> Result<bool> {
        let preceding = self.nav().find_preceding_token(position)?;
        let mut token = self.nav().get_token_at_position(position)?;
        let mut ancestor = Some(token);
        while let Some(id) = ancestor {
            let read = self.view.node(id)?;
            if read.kind() == tsr_ast::SyntaxKind::JSDoc {
                token = read.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
                break;
            }
            ancestor = read.parent();
        }
        if self.start(token)? <= position && position < i64::from(self.view.node(token)?.end()) {
            return Ok(false);
        }
        let text = self.file.text().as_bytes();
        let mut ranges = Vec::new();
        if let Some(previous) = preceding {
            ranges.extend(tsr_scanner::get_trailing_comment_ranges(
                text,
                i64::from(self.view.node(previous)?.end()),
            ));
        }
        ranges.extend(tsr_scanner::get_leading_comment_ranges(
            text,
            i64::from(self.view.node(token)?.pos()),
        ));
        Ok(ranges.iter().any(|range| {
            range.loc.pos() < position && position < range.loc.end()
                || position == range.loc.end()
                    && (range.kind == tsr_ast::SyntaxKind::SingleLineCommentTrivia
                        || position == text.len() as i64)
        }))
    }
    // port: tsc/internal/ls/utilities.go:getChildrenFromNonJSDocNode
    pub fn token_children(&self, node: NodeId) -> Result<Vec<NodeId>> {
        let children = self.children(node)?;
        if children.is_empty() {
            return Ok(children);
        }
        let mut result = Vec::new();
        let mut pos = i64::from(self.view.node(node)?.pos());
        for child in children {
            self.scan_between(
                node,
                pos,
                i64::from(self.view.node(child)?.pos()),
                &mut result,
            )?;
            result.push(child);
            pos = i64::from(self.view.node(child)?.end());
        }
        self.scan_between(
            node,
            pos,
            i64::from(self.view.node(node)?.end()),
            &mut result,
        )?;
        Ok(result)
    }
    fn scan_between(
        &self,
        parent: NodeId,
        mut start: i64,
        end: i64,
        result: &mut Vec<NodeId>,
    ) -> Result<()> {
        let mut scanner = tsr_scanner::get_scanner_for_source_file(&self.file, start);
        while start < end {
            let finish = scanner.token_end();
            if finish <= start {
                return Err(tsr_arena::Error::InvalidTokenRange.into());
            }
            result.push(
                self.view
                    .get_or_create_token(
                        scanner.token(),
                        scanner.token_full_start() as i32,
                        finish as i32,
                        parent,
                        scanner.token_flags(),
                    )?
                    .id(),
            );
            start = finish;
            scanner.scan();
        }
        Ok(())
    }
}
