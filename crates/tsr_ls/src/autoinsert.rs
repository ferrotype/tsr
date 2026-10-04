use crate::{syntax::Syntax, LanguageService, Result};
use tsr_ast::{
    node_flags, span_map::FEATURE_AUTO_INSERT, utilities_tail::tag_names_are_equivalent, NodeId,
    SyntaxKind as K,
};
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    // port: tsc/internal/ls/autoinsert.go:LanguageService.ProvideOnAutoInsert
    pub fn auto_insert(
        &mut self,
        params: &lsp::VSOnAutoInsertParams,
        enabled: bool,
    ) -> Result<lsp::VSOnAutoInsertResponseItemOrNull> {
        self.check_canceled()?;
        if !enabled || params.vs_ch != ">" {
            return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
        }
        let source = self.file(&params.vs_text_document.uri)?;
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.vs_position,
            FEATURE_AUTO_INSERT,
        )?;
        let [position] = positions.as_slice() else {
            return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
        };
        if !position.mapped.fidelity.is_exact() {
            return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
        }
        let mut syntax = Syntax::new(self.view(position.script)?, position.script)?;
        let Some(token) = syntax
            .nav()
            .find_preceding_token(i64::from(position.mapped.position))?
        else {
            return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
        };
        let read = syntax.view.node(token)?;
        let Some(parent) = read.parent() else {
            return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
        };
        let parent_read = syntax.view.node(parent)?;
        let element =
            if read.kind() == K::GreaterThanToken && parent_read.kind() == K::JsxOpeningElement {
                parent_read.parent()
            } else if read.kind() == K::JsxText && parent_read.kind() == K::JsxElement {
                Some(parent)
            } else {
                None
            };
        let closing = if let Some(element) = element {
            if !unclosed_tag(&syntax, element)? {
                return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
            }
            let name = opening_name(&syntax, element)?;
            let text = tsr_ast::utilities_targets::entity_name_to_string(
                syntax.view,
                name,
                Some(&|id| {
                    tsr_scanner::get_text_of_node(syntax.view, id)
                        .map(|text| text.as_bytes().to_vec())
                }),
            )?;
            format!("</{}>", String::from_utf8_lossy(&text))
        } else {
            let fragment = if read.kind() == K::GreaterThanToken
                && parent_read.kind() == K::JsxOpeningFragment
            {
                parent_read.parent()
            } else if read.kind() == K::JsxText && parent_read.kind() == K::JsxFragment {
                Some(parent)
            } else {
                None
            };
            let Some(fragment) = fragment else {
                return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
            };
            if !unclosed_fragment(&syntax, fragment)? {
                return Ok(lsp::VSOnAutoInsertResponseItemOrNull::default());
            }
            "</>".into()
        };
        Ok(lsp::VSOnAutoInsertResponseItemOrNull {
            vs_on_auto_insert_response_item: Some(Box::new(lsp::VSOnAutoInsertResponseItem {
                vs_text_edit_format: lsp::InsertTextFormat::SNIPPET,
                vs_text_edit: Some(Box::new(lsp::TextEdit {
                    range: lsp::Range {
                        start: params.vs_position.clone(),
                        end: params.vs_position.clone(),
                    },
                    new_text: format!("$0{}", closing.replace('$', "\\$")),
                })),
            })),
        })
    }
}
fn opening_name(syntax: &Syntax<'_>, element: NodeId) -> Result<NodeId> {
    let read = syntax.view.node(element)?;
    let data = read
        .data_source()
        .as_jsx_element()
        .ok_or(tsr_arena::Error::InvalidGraph)?;
    Ok(syntax
        .view
        .node(
            data.opening_element()
                .ok_or(tsr_arena::Error::InvalidGraph)?,
        )?
        .tag_name()
        .ok_or(tsr_arena::Error::InvalidGraph)?)
}
// port: tsc/internal/ls/autoinsert.go:isUnclosedTag
fn unclosed_tag(syntax: &Syntax<'_>, mut element: NodeId) -> Result<bool> {
    loop {
        let name = opening_name(syntax, element)?;
        let read = syntax.view.node(element)?;
        let close = read
            .data_source()
            .as_jsx_element()
            .ok_or(tsr_arena::Error::InvalidGraph)?
            .closing_element()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let close_name = syntax
            .view
            .node(close)?
            .tag_name()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        if !tag_names_are_equivalent(syntax.view, name, close_name)? {
            return Ok(true);
        }
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        if syntax.view.node(parent)?.kind() != K::JsxElement
            || !tag_names_are_equivalent(syntax.view, name, opening_name(syntax, parent)?)?
        {
            return Ok(false);
        }
        element = parent;
    }
}
// port: tsc/internal/ls/autoinsert.go:isUnclosedFragment
fn unclosed_fragment(syntax: &Syntax<'_>, mut fragment: NodeId) -> Result<bool> {
    loop {
        let read = syntax.view.node(fragment)?;
        let close = read
            .data_source()
            .as_jsx_fragment()
            .ok_or(tsr_arena::Error::InvalidGraph)?
            .closing_fragment()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        if syntax.view.node(close)?.flags() & node_flags::THIS_NODE_HAS_ERROR != 0 {
            return Ok(true);
        }
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        if syntax.view.node(parent)?.kind() != K::JsxFragment {
            return Ok(false);
        }
        fragment = parent;
    }
}
