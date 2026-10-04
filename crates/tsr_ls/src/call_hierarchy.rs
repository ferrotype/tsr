use crate::{
    call_declarations as decl,
    call_sites::Site,
    definition::ancestor,
    references::{ReferenceOptions, SearchState},
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::{HashMap, HashSet};
use tsr_ast::{
    span_map::FEATURE_CALL_HIERARCHY, utilities as ast, utilities_middle as middle,
    utilities_targets as targets, NodeId, SyntaxKind as K,
};
use tsr_checker::Operation;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

fn location_key(item: &lsp::CallHierarchyItem) -> (String, u32, u32, u32, u32) {
    let r = &item.selection_range;
    (
        item.uri.0.clone(),
        r.start.line,
        r.start.character,
        r.end.line,
        r.end.character,
    )
}
pub(crate) fn range_key(r: &lsp::Range) -> (u32, u32, u32, u32) {
    (r.start.line, r.start.character, r.end.line, r.end.character)
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/callhierarchy.go:LanguageService.callHierarchyDeclarations
    fn call_declarations(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        position: &lsp::Position,
        allow_source: bool,
    ) -> Result<Vec<NodeId>> {
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            position,
            FEATURE_CALL_HIERARCHY,
        )?;
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for mapped in positions {
            if !mapped.mapped.fidelity.is_single_segment() {
                continue;
            }
            let source = mapped.script;
            let view = self.view(source)?;
            let node = if mapped.mapped.position == 0 {
                source
            } else {
                Syntax::new(view, source)?
                    .nav()
                    .get_touching_property_name(i64::from(mapped.mapped.position))?
            };
            if !allow_source && node == source {
                continue;
            }
            for decl in decl::resolve(self, c, node)? {
                if seen.insert(decl) {
                    result.push(decl);
                }
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/callhierarchy.go:LanguageService.ProvidePrepareCallHierarchy
    pub fn prepare_call_hierarchy(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::CallHierarchyPrepareParams,
    ) -> Result<lsp::CallHierarchyItemsOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for decl in self.call_declarations(c, source, &params.position, false)? {
            self.check_canceled()?;
            if let Some(item) = self.call_item(c, decl)? {
                if seen.insert(location_key(&item)) {
                    items.push(Some(Box::new(item)));
                }
            }
        }
        Ok(lsp::CallHierarchyItemsOrNull {
            call_hierarchy_items: (!items.is_empty()).then(|| Box::new(items)),
        })
    }
    fn incoming_sites(&mut self, c: &mut Operation<'_>, declaration: NodeId) -> Result<Vec<Site>> {
        let view = self.view(declaration)?;
        if matches!(
            view.node(declaration)?.kind().known(),
            Some(K::SourceFile | K::ModuleDeclaration | K::ClassStaticBlockDeclaration)
        ) {
            return Ok(Vec::new());
        }
        let Some(node) = decl::reference(view, declaration)? else {
            return Ok(Vec::new());
        };
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let start = Syntax::new(view, source)?.start(node)?;
        if self
            .range(source, TextRange::new(start, start), FEATURE_CALL_HIERARCHY)?
            .1
            .is_none()
        {
            return Ok(Vec::new());
        }
        let files = self.program.files().iter().map(|f| f.source()).collect();
        let groups = SearchState::new(
            self,
            c,
            files,
            ReferenceOptions {
                adjust: true,
                ..Default::default()
            },
        )
        .for_node(node, start)?;
        let mut sites = Vec::new();
        for group in groups {
            for entry in group.entries {
                let Some(node) = entry.node else { continue };
                let view = self.view(node)?;
                if !targets::is_call_or_new_expression_target(view, node, true, true)?
                    && !targets::is_tagged_template_tag(view, node, true, true)?
                    && !targets::is_decorator_target(view, node, true, true)?
                    && !targets::is_jsx_opening_like_element_tag_name(view, node, true, true)?
                    && !middle::is_right_side_of_property_access(view, node)?
                    && !middle::is_argument_expression_of_element_access(view, node)?
                {
                    continue;
                }
                let declaration =
                    ancestor(view, Some(node), |id| decl::valid(view, id))?.unwrap_or(entry.source);
                sites.push(Site {
                    declaration,
                    source: entry.source,
                    range: TextRange::new(
                        Syntax::new(view, entry.source)?.start(node)?,
                        i64::from(view.node(node)?.end()),
                    ),
                });
            }
        }
        Ok(sites)
    }
    fn grouped_calls(
        &mut self,
        c: &mut Operation<'_>,
        sites: Vec<Site>,
    ) -> Result<Vec<(lsp::CallHierarchyItem, Vec<lsp::Range>)>> {
        let mut grouped: Vec<(NodeId, Vec<Site>)> = Vec::new();
        let mut indices = HashMap::new();
        for site in sites {
            let next = grouped.len();
            let index = *indices.entry(site.declaration).or_insert(next);
            if index == next {
                grouped.push((site.declaration, Vec::new()));
            }
            grouped[index].1.push(site);
        }
        let mut result = Vec::new();
        for (decl, sites) in grouped {
            let mut ranges = Vec::new();
            for site in sites {
                let (range, f) = self.range(site.source, site.range, FEATURE_CALL_HIERARCHY)?;
                if !f.is_none() {
                    ranges.push(range);
                }
            }
            if let Some(item) = self.call_item(c, decl)? {
                if !ranges.is_empty() {
                    ranges.sort_by_key(range_key);
                    result.push((item, ranges));
                }
            }
        }
        result.sort_by(|(a, ar), (b, br)| {
            a.uri
                .0
                .cmp(&b.uri.0)
                .then_with(|| range_key(&ar[0]).cmp(&range_key(&br[0])))
        });
        Ok(result)
    }
    fn hierarchy_calls(
        &mut self,
        c: &mut Operation<'_>,
        item: &lsp::CallHierarchyItem,
        incoming: bool,
    ) -> Result<Vec<(lsp::CallHierarchyItem, Vec<lsp::Range>)>> {
        let Some(file) = self.program.source_file(item.uri.file_name().as_bytes()) else {
            return Ok(Vec::new());
        };
        let declarations =
            self.call_declarations(c, file.source(), &item.selection_range.start, true)?;
        let mut calls: Vec<(lsp::CallHierarchyItem, Vec<lsp::Range>)> = Vec::new();
        let mut seen = HashMap::new();
        for decl in declarations {
            self.check_canceled()?;
            let sites = if incoming {
                self.incoming_sites(c, decl)?
            } else {
                let n = c.node(decl)?;
                if n.flags() & tsr_ast::node_flags::AMBIENT != 0 || n.kind() == K::MethodSignature {
                    continue;
                }
                self.call_sites(c, decl)?
            };
            for (item, ranges) in self.grouped_calls(c, sites)? {
                if let Some(&index) = seen.get(&location_key(&item)) {
                    let entry: &mut (lsp::CallHierarchyItem, Vec<lsp::Range>) = &mut calls[index];
                    for range in ranges {
                        if !entry.1.contains(&range) {
                            entry.1.push(range);
                        }
                    }
                } else {
                    seen.insert(location_key(&item), calls.len());
                    calls.push((item, ranges));
                }
            }
        }
        Ok(calls)
    }
    // port: tsc/internal/ls/callhierarchy.go:LanguageService.ProvideCallHierarchyIncomingCalls
    pub fn incoming_calls(
        &mut self,
        c: &mut Operation<'_>,
        item: &lsp::CallHierarchyItem,
    ) -> Result<lsp::CallHierarchyIncomingCallsOrNull> {
        let calls = self.hierarchy_calls(c, item, true)?;
        let calls = calls
            .into_iter()
            .map(|(item, ranges)| {
                Some(Box::new(lsp::CallHierarchyIncomingCall {
                    from: Some(Box::new(item)),
                    from_ranges: ranges,
                }))
            })
            .collect::<Vec<_>>();
        Ok(lsp::CallHierarchyIncomingCallsOrNull {
            call_hierarchy_incoming_calls: (!calls.is_empty()).then(|| Box::new(calls)),
        })
    }
    // port: tsc/internal/ls/callhierarchy.go:LanguageService.ProvideCallHierarchyOutgoingCalls
    pub fn outgoing_calls(
        &mut self,
        c: &mut Operation<'_>,
        item: &lsp::CallHierarchyItem,
    ) -> Result<lsp::CallHierarchyOutgoingCallsOrNull> {
        let calls = self.hierarchy_calls(c, item, false)?;
        let calls = calls
            .into_iter()
            .map(|(item, ranges)| {
                Some(Box::new(lsp::CallHierarchyOutgoingCall {
                    to: Some(Box::new(item)),
                    from_ranges: ranges,
                }))
            })
            .collect::<Vec<_>>();
        Ok(lsp::CallHierarchyOutgoingCallsOrNull {
            call_hierarchy_outgoing_calls: (!calls.is_empty()).then(|| Box::new(calls)),
        })
    }
}
