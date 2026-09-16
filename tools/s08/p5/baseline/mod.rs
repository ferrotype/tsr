//! The pinned tsbaseline type/symbol walker, over production checker APIs.
//! This harness records queries as executed, rather than replaying the oracle's
//! query list. Types are walked before symbols on the same checker operation.
mod classify;
mod decorate;
mod query;

use serde_json::{json, Value};
use std::ops::ControlFlow;
use ts_arena::{Error as ArenaError, NodeId};
use ts_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K};
use ts_checker::{Operation, TypeRef};
use ts_compiler::Program;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Row {
    line: usize,
    source_text: Vec<u8>,
    display: Vec<u8>,
}

pub struct InputFile<'a> {
    pub name: &'a [u8],
    pub content: &'a [u8],
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}

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
        let reparsed = node.flags() & ts_ast::node_flags::REPARSED != 0;
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
#[derive(Default)]
pub struct Trace {
    pub queries: Vec<Value>,
    pub active: Value,
    pub collect_type_strings: bool,
}

struct Walker<'a, 'operation> {
    program: &'a Program,
    op: &'a mut Operation<'operation>,
    had_errors: bool,
    trace: &'a mut Trace,
    type_strings: Vec<(Value, TypeRef)>,
}
impl Walker<'_, '_> {
    fn baseline(&mut self, files: &[InputFile<'_>], header: &[u8], symbols: bool) -> Result<Value> {
        let mut output = Vec::new();
        for input in files {
            let path = ts_tspath::to_path(
                input.name,
                self.program.current_directory(),
                self.program.host().use_case_sensitive_file_names(),
            );
            let source = self
                .program
                .file(path.as_bytes())
                .ok_or("baseline source absent from program")?
                .source();
            let mut rows = Vec::new();
            for id in nodes(self.program, source)? {
                let view = ast(self.program, id)?;
                let node = view.node(id)?;
                self.trace.active = self.stamp(source, id, "ClassifyNode")?;
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
                    // Native GetTypeCheckerForFile selects the existing
                    // checker; it does not force a skipped source check.
                    if let Some(row) = self.write(source, id, symbols)? {
                        rows.push(row);
                    }
                }
            }
            if symbols && rows.iter().any(|row| row.display.is_empty()) {
                break;
            }
            output.extend_from_slice(&decorate::file(input.name, input.content, &rows)?);
        }
        if output.is_empty() {
            return Ok(json!({"state":"no_content"}));
        }
        let mut full = b"//// [".to_vec();
        full.extend_from_slice(header);
        full.extend_from_slice(b"] ////\r\n\r\n");
        full.extend_from_slice(&output);
        Ok(json!({"state":"content","text_hex":hex(&full)}))
    }
}

pub fn generate(
    program: &Program,
    op: &mut Operation<'_>,
    files: &[InputFile<'_>],
    header: &[u8],
    had_errors: bool,
    trace: &mut Trace,
) -> Value {
    let mut walker = Walker {
        program,
        op,
        had_errors,
        trace,
        type_strings: Vec::new(),
    };
    let result = (|| {
        let types = walker.baseline(files, header, false)?;
        let symbols = walker.baseline(files, header, true)?;
        Ok::<_, Box<dyn std::error::Error>>((types, symbols))
    })();
    match result {
        Ok((types, symbols)) => {
            let mut result = json!({"state":"executed","types":types,"symbols":symbols,"queries":walker.trace.queries});
            if walker.trace.collect_type_strings {
                let display = (|| -> Result<Vec<Value>> {
                    let mut rows = Vec::new();
                    for (mut row, typ) in walker.type_strings {
                        walker.trace.active = row.clone();
                        let text = walker.op.type_to_string(typ,
                            ts_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                                | ts_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE)?;
                        row["text_hex"] = json!(hex(text.as_bytes()));
                        rows.push(row);
                    }
                    Ok(rows)
                })();
                result["public_type_strings"] = match display {
                    Ok(rows) => json!({"state":"executed","queries":rows}),
                    Err(error) => {
                        json!({"state":"failed","reason":error.to_string(),"active_query":walker.trace.active})
                    }
                };
            }
            result
        }
        Err(error) => {
            json!({"state":"failed","class":"walker_error","reason":error.to_string(),"active_query":walker.trace.active,"queries":walker.trace.queries})
        }
    }
}
