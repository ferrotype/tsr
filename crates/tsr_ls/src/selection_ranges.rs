use crate::{syntax::Syntax, LanguageService, Result};
use std::collections::VecDeque;
use tsr_ast::{
    span_map::FEATURE_SELECTION_RANGES, utilities, JsDocProvider, NodeId, SyntaxKind as K,
};
use tsr_astnav::Visit;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

const MAX_SELECTION_DEPTH: usize = 1000;

// port: tsc/internal/ls/selectionranges.go:newSelectionRangeBuilder
struct RangeBuilder {
    root: Option<Box<lsp::SelectionRange>>,
    ranges: VecDeque<lsp::Range>,
    last: Option<lsp::Range>,
    position: i64,
}
impl RangeBuilder {
    // port: tsc/internal/ls/selectionranges.go:selectionRangeBuilder.push
    fn push(
        &mut self,
        service: &mut LanguageService<'_>,
        source: NodeId,
        a: i64,
        b: i64,
    ) -> Result<()> {
        if a == b || !(a <= self.position && self.position <= b) {
            return Ok(());
        }
        let (range, fidelity) =
            service.range(source, TextRange::new(a, b), FEATURE_SELECTION_RANGES)?;
        if fidelity.is_none() || self.last.as_ref() == Some(&range) {
            return Ok(());
        }
        self.last = Some(range.clone());
        if self.ranges.len() == MAX_SELECTION_DEPTH - 1 {
            self.ranges.pop_front();
        }
        self.ranges.push_back(range);
        Ok(())
    }
    // port: tsc/internal/ls/selectionranges.go:selectionRangeBuilder.build
    fn build(self) -> Option<Box<lsp::SelectionRange>> {
        self.ranges.into_iter().fold(self.root, |parent, range| {
            Some(Box::new(lsp::SelectionRange { range, parent }))
        })
    }
}

// The pin synthesizes SyntaxList nodes only to group mapped-type selections.
// These request-local groups carry the same ranges/children without publishing
// synthetic nodes into the source owner.
enum SelectionChild {
    Node(NodeId),
    Group(Vec<SelectionChild>),
}
impl SelectionChild {
    fn range(&self, syntax: &mut Syntax<'_>) -> Result<(i64, i64)> {
        match self {
            Self::Node(id) => Ok((
                syntax.nav().get_start_of_node(*id, true)?,
                i64::from(syntax.view.node(*id)?.end()),
            )),
            Self::Group(children) => Ok((
                children[0].range(syntax)?.0,
                children[children.len() - 1].range(syntax)?.1,
            )),
        }
    }
}
// port: tsc/internal/ls/selectionranges.go:groupChildren
fn group_children(
    children: Vec<SelectionChild>,
    predicate: impl Fn(NodeId) -> bool,
) -> Vec<SelectionChild> {
    let mut result = Vec::new();
    let mut group = Vec::new();
    for child in children {
        if matches!(&child, SelectionChild::Node(id) if predicate(*id)) {
            group.push(child);
        } else {
            if !group.is_empty() {
                result.push(SelectionChild::Group(std::mem::take(&mut group)));
            }
            result.push(child);
        }
    }
    if !group.is_empty() {
        result.push(SelectionChild::Group(group));
    }
    result
}
// port: tsc/internal/ls/selectionranges.go:getSelectionChildren
fn selection_children(syntax: &Syntax<'_>, node: NodeId) -> Result<Vec<SelectionChild>> {
    let children = syntax.token_children(node)?;
    if syntax.view.node(node)?.kind() != K::MappedType
        || children.len() < 2
        || syntax.view.node(children[0])?.kind() != K::OpenBraceToken
        || syntax.view.node(*children.last().unwrap())?.kind() != K::CloseBraceToken
    {
        return Ok(children.into_iter().map(SelectionChild::Node).collect());
    }
    let open = children[0];
    let close = *children.last().unwrap();
    let read = syntax.view.node(node)?;
    let data = read
        .data_source()
        .as_mapped_type_node()
        .ok_or(tsr_arena::Error::InvalidGraph)?;
    let readonly = data.readonly_token();
    let question = data.question_token();
    let grouped = group_children(
        children[1..children.len() - 1]
            .iter()
            .copied()
            .map(SelectionChild::Node)
            .collect(),
        |id| {
            Some(id) == readonly
                || Some(id) == question
                || matches!(
                    syntax.view.node(id).unwrap().kind().known(),
                    Some(K::ReadonlyKeyword | K::QuestionToken)
                )
        },
    );
    let mut grouped = group_children(grouped, |id| {
        matches!(
            syntax.view.node(id).unwrap().kind().known(),
            Some(K::OpenBracketToken | K::TypeParameter | K::CloseBracketToken)
        )
    });
    // port: tsc/internal/ls/selectionranges.go:splitChildren
    if let Some(pivot) = grouped.iter().position(|child| matches!(child, SelectionChild::Node(id) if syntax.view.node(*id).unwrap().kind() == K::ColonToken)) {
        let right = grouped.split_off(pivot+1);
        let colon = grouped.pop().unwrap();
        let mut split = Vec::new();
        if !grouped.is_empty() { split.push(SelectionChild::Group(grouped)); }
        split.push(colon);
        if !right.is_empty() { split.push(SelectionChild::Group(right)); }
        grouped = split;
    }
    Ok(vec![
        SelectionChild::Node(open),
        SelectionChild::Group(grouped),
        SelectionChild::Node(close),
    ])
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/selectionranges.go:LanguageService.ProvideSelectionRanges
    pub fn selection_ranges(
        &mut self,
        params: &lsp::SelectionRangeParams,
    ) -> Result<lsp::SelectionRangesOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let mut results = Vec::new();
        for position in &params.positions {
            self.check_canceled()?;
            let positions = self.converters.from_lsp_position_for_source_file(
                self.program,
                source,
                position,
                FEATURE_SELECTION_RANGES,
            )?;
            let [position] = positions.as_slice() else {
                return Ok(lsp::SelectionRangesOrNull::default());
            };
            if !position.mapped.fidelity.is_single_segment() {
                return Ok(lsp::SelectionRangesOrNull::default());
            }
            if let Some(range) =
                self.smart_selection_range(position.script, i64::from(position.mapped.position))?
            {
                results.push(Some(range));
            }
        }
        Ok(lsp::SelectionRangesOrNull {
            selection_ranges: Some(Box::new(results)),
        })
    }
    // port: tsc/internal/ls/selectionranges.go:getSmartSelectionRange
    fn smart_selection_range(
        &mut self,
        source: NodeId,
        pos: i64,
    ) -> Result<Option<Box<lsp::SelectionRange>>> {
        let view = self.view(source)?;
        let mut syntax = Syntax::new(view, source)?;
        let (full, _) = self.range(
            source,
            TextRange::new(
                i64::from(view.node(source)?.pos()),
                i64::from(view.node(source)?.end()),
            ),
            FEATURE_SELECTION_RANGES,
        )?;
        let root = syntax.file.content_mapper().is_empty().then(|| {
            Box::new(lsp::SelectionRange {
                range: full.clone(),
                parent: None,
            })
        });
        let mut ranges = RangeBuilder {
            last: root.as_ref().map(|r| r.range.clone()),
            root,
            ranges: VecDeque::with_capacity(MAX_SELECTION_DEPTH - 1),
            position: pos,
        };
        let mut current = Some(source);
        while let Some(parent) = current {
            self.check_canceled()?;
            let mut visits = syntax
                .docs
                .jsdoc(view, source, parent)?
                .iter()
                .copied()
                .map(Visit::Node)
                .collect::<Vec<_>>();
            visits.extend(tsr_astnav::visit_each_child(view, parent)?);
            let mut next = None;
            for visit in visits {
                let nodes = match visit {
                    Visit::Node(node) => vec![node],
                    Visit::List(list) => {
                        let nodes: Vec<_> = view
                            .node_slice(view.list(list)?.nodes())?
                            .iter()
                            .flatten()
                            .collect();
                        // Go visits modifiers through VisitModifiers, which
                        // does not invoke the selection-specific VisitNodes hook.
                        if view.node(parent)?.modifiers() != Some(list)
                            && !nodes.is_empty()
                            && !matches!(
                                view.node(parent)?.kind().known(),
                                Some(K::VariableDeclarationList | K::TemplateExpression)
                            )
                        {
                            let start = syntax.start(nodes[0])?;
                            let end = i64::from(view.node(*nodes.last().unwrap())?.end());
                            if start <= pos && pos < end {
                                ranges.push(self, source, start, end)?;
                            }
                        }
                        nodes
                    }
                };
                for node in nodes {
                    if next.is_some() {
                        continue;
                    }
                    let read = view.node(node)?;
                    let end = i64::from(read.end());
                    if let Some(comment) =
                        tsr_scanner::get_trailing_comment_ranges(syntax.file.text().as_bytes(), end)
                            .next()
                    {
                        if comment.kind == K::SingleLineCommentTrivia {
                            ranges.push(self, source, comment.loc.pos(), comment.loc.end())?;
                            let mut start = comment.loc.pos();
                            let text = syntax.file.text().as_bytes();
                            while start < comment.loc.end()
                                && text.get(start as usize) == Some(&b'/')
                            {
                                start += 1;
                            }
                            ranges.push(self, source, start, comment.loc.end())?;
                        }
                    }
                    let include_start = syntax.nav().get_start_of_node(node, true)?;
                    if !(include_start <= pos && pos < end) {
                        continue;
                    }
                    let parent_read = view.node(parent)?;
                    let start = syntax.start(node)?;
                    if read.kind() == K::Block
                        && utilities::is_function_like_declaration(Some(&parent_read))
                        && !syntax.same_line(start, end)
                    {
                        ranges.push(self, source, start, end)?;
                    }
                    if let Some(span) = parent_read.data_source().as_template_span() {
                        if let Some(literal) = span.literal() {
                            let a = i64::from(read.pos()) - 2;
                            let b = syntax.start(literal)? + 1;
                            if a >= 0 && b <= syntax.file.text().len() as i64 && a < b {
                                ranges.push(self, source, a, b)?;
                            }
                        }
                    }
                    let skip = matches!(
                        read.kind().known(),
                        Some(
                            K::Block
                                | K::TemplateSpan
                                | K::TemplateHead
                                | K::TemplateTail
                                | K::JSDocTypeExpression
                                | K::JSDocSignature
                                | K::JSDocTypeLiteral
                        )
                    ) || read.kind() == K::VariableDeclarationList
                        && parent_read.kind() == K::VariableStatement
                        || read.kind() == K::VariableDeclaration
                            && parent_read.kind() == K::VariableDeclarationList
                            && syntax.children(parent)?.len() == 1;
                    if !skip {
                        ranges.push(self, source, start, end)?;
                        if read.kind() == K::MappedType {
                            let mut children = selection_children(&syntax, node)?;
                            loop {
                                let mut selected = None;
                                for child in children {
                                    let (a, b) = child.range(&mut syntax)?;
                                    if a > pos {
                                        break;
                                    }
                                    let snap = pos < b
                                        || pos == b
                                            && i64::from(
                                                view.node(
                                                    syntax.nav().get_touching_property_name(pos)?,
                                                )?
                                                .pos(),
                                            ) < b;
                                    if snap {
                                        ranges.push(self, source, a, b)?;
                                        selected = Some(child);
                                        break;
                                    }
                                }
                                match selected {
                                    Some(SelectionChild::Group(group)) => children = group,
                                    _ => break,
                                }
                            }
                        }
                        if matches!(
                            read.kind().known(),
                            Some(
                                K::StringLiteral
                                    | K::TemplateExpression
                                    | K::NoSubstitutionTemplateLiteral
                            )
                        ) && start + 1 < end - 1
                        {
                            ranges.push(self, source, start + 1, end - 1)?;
                        }
                    }
                    next = Some(node);
                }
            }
            current = next;
        }
        Ok(ranges.build())
    }
}
