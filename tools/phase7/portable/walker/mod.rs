//! The pinned observation schedule, shared by native and public API consumers.
//! Source lookup, checker selection and baseline decoration belong to the caller.
//! One call borrows an already-held operation; owned rows contain no handles.
mod classify;
mod query;
pub use super::walker_api::observation_rows as rows;

use super::walker_api::{Operation, Program, TypeRef};
use serde_json::Value;
use std::ops::ControlFlow;
use tsr_arena::{Error as ArenaError, NodeId};
use tsr_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
use rows::{Display, Row, SymbolDeclaration};

/// Excludes observer/decoration work from the caller's measurement interval.
pub trait Timing {
    fn pause(&mut self) {}
    fn resume(&mut self) {}
}
/// No measurement interval: the test runner's walks.
pub struct NoTiming;
impl Timing for NoTiming {}

fn ast(program: &Program, node: NodeId) -> Result<AstView<'_>> {
    for file in program.files() {
        match file.bound().view().ast().for_node_owner(node) {
            Ok(view) => {
                view.node(node)?;
                return Ok(view);
            }
            Err(ArenaError::WrongOwner) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(ArenaError::WrongOwner.into())
}

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<ArenaError>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        match self.view.list(id) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

// tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:forEachASTNode
fn nodes(program: &Program, root: NodeId) -> Result<Vec<NodeId>> {
    let mut result = Vec::new();
    let mut work = vec![root];
    while let Some(id) = work.pop() {
        let view = ast(program, id)?;
        let node = view.node(id)?;
        let reparsed = node.flags() & tsr_ast::node_flags::REPARSED != 0;
        let assertion = matches!(
            node.kind().known(),
            Some(K::AsExpression | K::SatisfiesExpression)
        );
        let parent_assertion = node
            .parent()
            .map(|p| {
                view.node(p).map(|p| {
                    matches!(
                        p.kind().known(),
                        Some(K::AsExpression | K::SatisfiesExpression)
                    )
                })
            })
            .transpose()?
            .unwrap_or(false);
        let assertion_expression = if parent_assertion {
            view.node(node.parent().ok_or("assertion parent")?)?
                .expression()
                == Some(id)
        } else {
            false
        };
        if !reparsed || assertion || assertion_expression {
            if !reparsed || parent_assertion {
                result.push(id);
            }
            let mut children = Children {
                view,
                nodes: vec![],
                error: None,
            };
            let _ = node.for_each_child(&mut children);
            if let Some(error) = children.error {
                return Err(error.into());
            }
            work.extend(children.nodes.into_iter().rev());
        }
    }
    Ok(result)
}

/// Kept outside the checker operation so a panicking unwind retires the owner
/// while retaining the last query and all completed observations.
pub struct Trace {
    pub queries: Vec<Value>,
    pub active: Value,
    pub collect_type_strings: bool,
    /// Inventory runs record every query as JSON; the checkerbench child only
    /// counts operations so JSON transport stays out of its checker interval.
    pub record_queries: bool,
    pub counts: std::collections::BTreeMap<&'static str, u64>,
    /// Retain every `GetTypeAtLocation` result as a root of the retained
    /// checkpoint (`data/s08/type-footprint.json`, roots).
    pub retain_types: bool,
    pub retained_types: Vec<TypeRef>,
}
impl Default for Trace {
    fn default() -> Self {
        Self {
            queries: Vec::new(),
            active: Value::Null,
            collect_type_strings: false,
            record_queries: true,
            counts: std::collections::BTreeMap::new(),
            retain_types: false,
            retained_types: Vec::new(),
        }
    }
}
impl Trace {
    pub(crate) fn count(&mut self, operation: &'static str) {
        *self.counts.entry(operation).or_insert(0) += 1;
    }
}

pub struct Collector<'a, 'operation> {
    program: &'a Program,
    op: &'a mut Operation<'operation>,
    had_errors: bool,
    trace: &'a mut Trace,
    timing: &'a mut dyn Timing,
    type_strings: &'a mut Vec<(Value, TypeRef, usize)>,
    owner_key: usize,
}

impl<'a, 'operation> Collector<'a, 'operation> {
    /// Borrow an already-selected file operation. `owner_key` is an opaque
    /// caller key for later TypeToString dispatch, never a transported ID.
    /// Deferred TypeRefs are scoped handles, not independently retained roots.
    pub fn new(
        program: &'a Program,
        op: &'a mut Operation<'operation>,
        had_errors: bool,
        trace: &'a mut Trace,
        timing: &'a mut dyn Timing,
        type_strings: &'a mut Vec<(Value, TypeRef, usize)>,
        owner_key: usize,
    ) -> Self {
        Self {
            program,
            op,
            had_errors,
            trace,
            timing,
            type_strings,
            owner_key,
        }
    }

    /// Types and symbols are separate calls so the host keeps its failure and
    /// subtest schedule. Traversal/query order is the pinned native schedule.
    pub fn collect(&mut self, source: NodeId, symbols: bool) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for id in nodes(self.program, source)? {
            let view = ast(self.program, id)?;
            let node = view.node(id)?;
            if self.trace.record_queries {
                self.trace.active = self.stamp(source, id, "ClassifyNode")?;
            }
            let selected = self.op.is_expression_node(id)?
                || node.kind() == K::Identifier
                || classify::declaration_name(view, id)?
                || node.kind() == K::QualifiedName
                    && classify::heritage_name(view, id)?
                    && (symbols
                        || node
                            .parent()
                            .map(|p| view.node(p).map(|p| p.kind() == K::QualifiedName))
                            .transpose()?
                            .unwrap_or(false));
            if selected {
                if let Some(row) = self.write(source, id, symbols)? {
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }
}
