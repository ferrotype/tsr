use crate::{LanguageService, Result};
use tsr_ast::{node_flags, span_map::FEATURE_LINKED_EDITING, SyntaxKind as K};
use tsr_astnav::Navigator;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    // port: tsc/internal/ls/linkedediting.go:LanguageService.ProvideLinkedEditingRange
    pub fn linked_editing(
        &mut self,
        params: &lsp::LinkedEditingRangeParams,
    ) -> Result<lsp::LinkedEditingRangesOrNull> {
        self.check_canceled()?;
        let source = self.file(&params.text_document.uri)?;
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_LINKED_EDITING,
        )?;
        let [position] = positions.as_slice() else {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        };
        if !position.mapped.fidelity.is_exact() {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        }
        let source = position.script;
        let position = i64::from(position.mapped.position);
        let view = self.view(source)?;
        let mut docs = tsr_parser::ParserJsDocProvider::default();
        let mut nav = Navigator::new(view, source, &mut docs);
        let Some(token) = nav.find_preceding_token(position)? else {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        };
        let Some(parent) = view.node(token)?.parent() else {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        };
        if view.node(parent)?.kind() == K::SourceFile {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        }
        let grandparent = view.node(parent)?.parent();
        let (open_range, close_range) = if let Some(fragment) = grandparent.filter(|id| {
            view.node(*id)
                .is_ok_and(|node| node.kind() == K::JsxFragment)
        }) {
            let read = view.node(fragment)?;
            let data = read
                .data_source()
                .as_jsx_fragment()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let open = data
                .opening_fragment()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let close = data
                .closing_fragment()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            if (view.node(open)?.flags() | view.node(close)?.flags())
                & node_flags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR
                != 0
            {
                return Ok(lsp::LinkedEditingRangesOrNull::default());
            }
            let open_pos = nav.get_start_of_node(open, false)? + 1;
            let close_pos = nav.get_start_of_node(close, false)? + 2;
            if position != open_pos && position != close_pos {
                return Ok(lsp::LinkedEditingRangesOrNull::default());
            }
            (
                TextRange::new(open_pos, open_pos),
                TextRange::new(close_pos, close_pos),
            )
        } else {
            let mut tag = Some(parent);
            while let Some(id) = tag {
                if matches!(
                    view.node(id)?.kind().known(),
                    Some(K::JsxOpeningElement | K::JsxClosingElement)
                ) {
                    break;
                }
                tag = view.node(id)?.parent();
            }
            let Some(tag) = tag else {
                return Ok(lsp::LinkedEditingRangesOrNull::default());
            };
            let element = view
                .node(tag)?
                .parent()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let read = view.node(element)?;
            let data = read
                .data_source()
                .as_jsx_element()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let open = data
                .opening_element()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let close = data
                .closing_element()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let open_name = view
                .node(open)?
                .tag_name()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let close_name = view
                .node(close)?
                .tag_name()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let a = nav.get_start_of_node(open_name, false)?;
            let b = i64::from(view.node(open_name)?.end());
            let c = nav.get_start_of_node(close_name, false)?;
            let d = i64::from(view.node(close_name)?.end());
            if a == nav.get_start_of_node(open, false)?
                || c == nav.get_start_of_node(close, false)?
                || b == i64::from(view.node(open)?.end())
                || d == i64::from(view.node(close)?.end())
                || !(a <= position && position <= b || c <= position && position <= d)
            {
                return Ok(lsp::LinkedEditingRangesOrNull::default());
            }
            if tsr_scanner::get_text_of_node(view, open_name)?
                != tsr_scanner::get_text_of_node(view, close_name)?
            {
                return Ok(lsp::LinkedEditingRangesOrNull::default());
            }
            (TextRange::new(a, b), TextRange::new(c, d))
        };
        let (open, a) = self.range(source, open_range, FEATURE_LINKED_EDITING)?;
        let (close, b) = self.range(source, close_range, FEATURE_LINKED_EDITING)?;
        if !a.is_exact() || !b.is_exact() {
            return Ok(lsp::LinkedEditingRangesOrNull::default());
        }
        Ok(lsp::LinkedEditingRangesOrNull {
            linked_editing_ranges: Some(Box::new(lsp::LinkedEditingRanges {
                ranges: vec![open, close],
                word_pattern: Some(Box::new("[a-zA-Z0-9:\\-\\._$]*".into())),
            })),
        })
    }
}
