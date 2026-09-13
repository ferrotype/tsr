//! Consecutive unreachable statements share one native diagnostic span.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_core::Tristate;
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkSourceElementUnreachable
    pub(crate) fn check_source_element_unreachable(&mut self, node: NodeId) -> Result<bool, Error> {
        if !ts_ast::is_potentially_executable_node(self.ast(node)?, node)? {
            return Ok(false);
        }
        if self.query.reported_unreachable.contains(&node) {
            return Ok(true);
        }
        if !self.source_element_unreachable(node)? {
            return Ok(false);
        }
        self.query.reported_unreachable.insert(node);
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("unreachable source"))?;
        let mut end = node;
        if let Some(parent) = self.ast(node)?.node(node)?.parent() {
            let view = self.ast(parent)?;
            let read = view.node(parent)?;
            if read.can_have_statements() {
                let statements: Vec<_> = view
                    .node_slice(read.statements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                if let Some(index) = statements.iter().position(|&statement| statement == node) {
                    for &next in &statements[index + 1..] {
                        if !ts_ast::is_potentially_executable_node(self.ast(next)?, next)?
                            || !self.source_element_unreachable(next)?
                        {
                            break;
                        }
                        end = next;
                        self.query.reported_unreachable.insert(next);
                    }
                }
            }
        }
        let file = self.ast(source)?.source_file(source)?;
        let start = ts_scanner::skip_trivia(
            file.text().as_bytes(),
            i64::from(self.ast(node)?.node(node)?.pos()),
        );
        let end = i64::from(self.ast(end)?.node(end)?.end());
        let mut diagnostic = ts_ast::Diagnostic::new(
            Some(source),
            ts_core::TextRange::new(start, end),
            ts_diagnostics::Unreachable_code_detected,
            vec![],
        );
        if self.program()?.host.options().allow_unreachable_code == Tristate::FALSE {
            self.add_diagnostic(diagnostic)?;
        } else {
            diagnostic.category = ts_diagnostics::Category::Suggestion as i32;
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        Ok(true)
    }
    // port: tsc/internal/checker/checker.go:Checker.isSourceElementUnreachable
    fn source_element_unreachable(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::UNREACHABLE != 0 {
            let preserve = self.program()?.host.options().should_preserve_const_enums();
            return match read.kind().known() {
                Some(K::EnumDeclaration) => Ok(read.modifier_flags(self.ast(node)?)?
                    & ts_ast::modifier_flags::CONST
                    == 0
                    || preserve),
                Some(K::ModuleDeclaration) => {
                    ts_ast::is_instantiated_module(self.ast(node)?, node, preserve)
                        .map_err(Error::from)
                }
                _ => Ok(true),
            };
        }
        match self.node_flow(node)? {
            Some(flow) => Ok(!self.reachable_flow(node, flow)?),
            None => Ok(false),
        }
    }
}
