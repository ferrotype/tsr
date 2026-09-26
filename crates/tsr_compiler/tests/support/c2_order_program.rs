//! ADR 0010 diagnostic driver. One source witness per fresh process; no corpus
//! authority depends on this executable or its diagnostic feature.
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_ast::SyntaxKind as K;
use tsr_checker::{CheckerOwner, UnionReduction};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    source: String,
    queries: Vec<String>,
    #[serde(default)]
    property: String,
    #[serde(default)]
    union: bool,
}

struct Children<'a> {
    view: tsr_ast::AstView<'a>,
    nodes: Vec<tsr_arena::NodeId>,
}
impl tsr_ast::ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: tsr_arena::NodeId) -> std::ops::ControlFlow<()> {
        self.nodes.push(node);
        std::ops::ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: tsr_ast::NodeListId) -> std::ops::ControlFlow<()> {
        self.visit_node_slice(self.view.list(nodes).expect("source list").nodes())
    }
    fn visit_node_slice(&mut self, nodes: tsr_ast::NodeSlice) -> std::ops::ControlFlow<()> {
        self.nodes.extend(
            self.view
                .node_slice(nodes)
                .expect("source edges")
                .iter()
                .flatten(),
        );
        std::ops::ControlFlow::Continue(())
    }
}

pub fn run(
    request: serde_json::Value,
    tracing: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    // Keep parsing, binding and checking on one native parser worker. Its
    // reentry contract prevents nested dispatch, so all actual births remain
    // on the trace thread without changing the production constructors.
    tsr_parser::on_parser_worker(|| run_worker(request, tracing))
}
fn run_worker(
    request: serde_json::Value,
    tracing: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let request: Request = serde_json::from_value(request)?;
    #[cfg(not(feature = "creation-trace"))]
    assert!(!tracing, "trace requested without diagnostic feature");
    #[cfg(feature = "creation-trace")]
    if tracing {
        tsr_ast::creation_trace::begin();
    }
    let counters = Counters::new();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", request.source.as_bytes());
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        no_lib: Tristate::TRUE,
        ..Default::default()
    };
    let program = Arc::new(Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                vec![JsString::from_bytes(b"/main.ts".as_slice())],
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        &counters,
    )?);
    let file = program.file(b"/main.ts").expect("loaded source");
    let source = file.source();
    let view = file.bound().view().ast();
    let mut names = HashMap::new();
    let mut pending = vec![source];
    while let Some(node) = pending.pop() {
        let read = view.node(node)?;
        if matches!(
            read.kind().known(),
            Some(K::VariableDeclaration | K::TypeAliasDeclaration)
        ) {
            if let Some(name) = read.name() {
                names.insert(view.node_text(name)?.as_bytes().to_vec(), name);
            }
        }
        let mut children = Children {
            view,
            nodes: Vec::new(),
        };
        let _ = read.for_each_child(&mut children);
        pending.extend(children.nodes.into_iter().rev());
    }
    let owner = Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(Generation::new(&counters), &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )?);
    let mut op = owner.operation()?;
    let diagnostics = op.semantic_diagnostics(source)?.iter().map(|d| {
        json!({"code":d.code,"pos":d.loc.pos(),"end":d.loc.end(),
            "text":String::from_utf8(tsr_compiler::diagnostic_writer::flattened(d,b"\n").expect("diagnostic message")).expect("native fixture Unicode")})
    }).collect::<Vec<_>>();
    let mut types = Vec::new();
    let mut displays = Vec::new();
    for name in request.queries {
        let node = *names.get(name.as_bytes()).expect("source query exists");
        let ty = op.get_type_at_location(node)?;
        types.push(ty);
        displays.push(String::from_utf8(
            op.type_to_string_at(ty, Some(node), 0)?.as_bytes().to_vec(),
        )?);
    }
    #[cfg(feature = "creation-trace")]
    let queried = op.trace_types(&types)?;
    #[cfg(not(feature = "creation-trace"))]
    let queried = serde_json::Value::Null;
    let mut order = serde_json::Value::Null;
    if !request.property.is_empty() {
        order = op.trace_property_order(&types, request.property.as_bytes())?;
    }
    let mut union = serde_json::Value::Null;
    if request.union {
        let ty = op.union_type_with(&types, UnionReduction::None)?;
        let order = op
            .constituents(ty)?
            .iter()
            .map(|member| {
                types
                    .iter()
                    .position(|input| input == member)
                    .expect("union retains source types")
            })
            .collect::<Vec<_>>();
        union = json!({"display":String::from_utf8(op.type_to_string_at(ty, None, 0)?.as_bytes().to_vec())?, "order":order});
    }
    #[cfg(feature = "creation-trace")]
    let trace = if tracing {
        tsr_ast::creation_trace::finish()
    } else {
        Vec::new()
    };
    #[cfg(not(feature = "creation-trace"))]
    let trace: Vec<serde_json::Value> = Vec::new();
    Ok(json!({
        "ordinary":{"diagnostics":diagnostics,"display":displays,"union":union,"property_order":order.get("order")},"order":order,"types":queried,"trace":trace}))
}
