use crate::{
    change_nodes::{line_start, Leading, NodeTracker, Trailing},
    Result,
};
use tsr_ast::NodeId;
use tsr_core::TextRange;
use tsr_scanner::{
    get_leading_comment_ranges as leading, get_trailing_comment_ranges as trailing, skip_trivia_ex,
    SkipTriviaOptions,
};

impl NodeTracker<'_> {
    // port: tsc/internal/ls/change/trackerimpl.go:Tracker.GetAdjustedRange
    pub fn adjusted_range(
        &self,
        source: NodeId,
        first: NodeId,
        last: NodeId,
        leading: Leading,
        trailing: Trailing,
    ) -> Result<TextRange> {
        Ok(TextRange::new(
            self.adjusted_start(source, first, leading, false)?,
            self.adjusted_end(source, last, trailing)?,
        ))
    }
    // port: tsc/internal/ls/change/trackerimpl.go:Tracker.getAdjustedStartPosition
    pub fn adjusted_start(
        &self,
        source: NodeId,
        node: NodeId,
        option: Leading,
        has_trailing_comment: bool,
    ) -> Result<i64> {
        let mut syntax = self.syntax(source)?;
        let read = syntax.view.node(node)?;
        let source_text = syntax.file.text().clone();
        let text = source_text.as_bytes();
        if option == Leading::JsDoc {
            if let Some(comment) =
                tsr_parser::get_jsdoc_comment_ranges(&self.ast, Vec::new(), node, text).first()
            {
                return Ok(line_start(&syntax.file, comment.loc.pos()));
            }
        }
        let start = syntax.start(node)?;
        let start_line = line_start(&syntax.file, start);
        if option == Leading::Exclude {
            return Ok(start);
        }
        if option == Leading::StartLine {
            return Ok(if read.range().contains_inclusive(start_line) {
                start_line
            } else {
                start
            });
        }
        let full_start = i64::from(read.pos());
        if full_start == start {
            return Ok(start);
        }
        let lines = syntax.file.ecma_line_map();
        let full_line = lines
            .partition_point(|&p| i64::from(p) <= full_start)
            .saturating_sub(1);
        if start_line == i64::from(lines[full_line]) {
            return Ok(if option == Leading::IncludeAll {
                full_start
            } else {
                start
            });
        }
        if has_trailing_comment {
            if let Some(comment) = leading(text, full_start)
                .next()
                .or_else(|| trailing(text, full_start).next())
            {
                return Ok(skip_trivia_ex(
                    text,
                    comment.loc.end(),
                    Some(&SkipTriviaOptions {
                        stop_after_line_break: true,
                        stop_at_comments: true,
                        ..Default::default()
                    }),
                ));
            }
        }
        let next = i64::from(lines[full_line + usize::from(full_start > 0)]);
        Ok(line_start(
            &syntax.file,
            skip_trivia_ex(
                text,
                next,
                Some(&SkipTriviaOptions {
                    stop_at_comments: true,
                    ..Default::default()
                }),
            ),
        ))
    }
    // port: tsc/internal/ls/change/trackerimpl.go:Tracker.getAdjustedEndPosition
    pub fn adjusted_end(&self, source: NodeId, node: NodeId, option: Trailing) -> Result<i64> {
        let syntax = self.syntax(source)?;
        let end = i64::from(syntax.view.node(node)?.end());
        if option == Trailing::Exclude {
            return Ok(end);
        }
        let source_text = syntax.file.text().clone();
        let text = source_text.as_bytes();
        if option == Trailing::ExcludeWhitespace {
            return Ok(trailing(text, end)
                .chain(leading(text, end))
                .last()
                .map_or(end, |c| if c.loc.end() == 0 { end } else { c.loc.end() }));
        }
        if option == Trailing::Include {
            let node_line = line_start(&syntax.file, end);
            for comment in trailing(text, end) {
                if comment.kind == tsr_ast::SyntaxKind::SingleLineCommentTrivia
                    || line_start(&syntax.file, comment.loc.pos()) > node_line
                {
                    break;
                }
                if line_start(&syntax.file, comment.loc.end()) > node_line {
                    return Ok(skip_trivia_ex(
                        text,
                        comment.loc.end(),
                        Some(&SkipTriviaOptions {
                            stop_after_line_break: true,
                            stop_at_comments: true,
                            ..Default::default()
                        }),
                    ));
                }
            }
        }
        let new_end = skip_trivia_ex(
            text,
            end,
            Some(&SkipTriviaOptions {
                stop_after_line_break: true,
                ..Default::default()
            }),
        );
        Ok(
            if new_end != end
                && (option == Trailing::Include
                    || text
                        .get(new_end as usize - 1)
                        .is_some_and(|&b| b == b'\r' || b == b'\n'))
            {
                new_end
            } else {
                end
            },
        )
    }
}
