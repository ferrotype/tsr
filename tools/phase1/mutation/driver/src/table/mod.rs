//! Table rows: one `(column, input)` pair per request, the Rust side of the
//! operation-table driver `tools/phase1/tables/go` (contract:
//! `docs/PHASE1-mutation-witnesses.md`, section 9).
//!
//! `setup` builds the input (parse or bind a source file, parse a tsconfig,
//! decode a constructed value) and observes, so mutants are off and nothing it
//! runs is reach; `column` calls the production port(s) the column names and
//! is the only production stage. The column's single value is digested as
//! `sha256(canonical(value))`, exactly as the Go driver digests its value.
//!
//! Columns live in one module per group (`<group>.rs`); each lists its column
//! ids in `COLUMNS` and builds them in `build`, which returns `None` for a
//! column of another group. A column's closure is the only code the column
//! stage runs: everything that is not the operation's own work (the walk,
//! container lists, config parsing, value decoding) belongs to `build`.
//!
//! Threads the operation starts (the concurrency group) are workers of the
//! row: `phase1_mutants` runs them in the column stage and takes their reach
//! with the row's, so the operation must join them before the closure returns.
use crate::jobs::{enter, Stage};
use crate::protocol::{hex, unhex, Session};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use tsr_ast::{
    AstView, BoundFile, BoundView, ChildVisitor, ExternalModuleIndicatorOptions, JsDocProvider,
    JsString, NodeId, NodeListId, NodeSlice, ParsedFile, SourceFileParseOptions, SymbolId,
};
use tsr_jsstring::SourceText;

mod class;
mod concurrency;
mod containers;
#[path = "core.rs"]
mod core_group;
mod diagnostics;
pub mod helpers;
mod modules;
mod positions;
mod runtime;
mod targets;
mod tsoptions;

/// A column's evaluation: the column stage calls it once.
pub type Column = Box<dyn FnOnce() -> Result<Value, String>>;

/// One group module: its column ids and its builder.
struct Group {
    columns: &'static [&'static str],
    build: fn(&str, &Value) -> Option<Result<Column, String>>,
}

const GROUPS: &[Group] = &[
    Group {
        columns: runtime::COLUMNS,
        build: runtime::build,
    },
    Group {
        columns: class::COLUMNS,
        build: class::build,
    },
    Group {
        columns: modules::COLUMNS,
        build: modules::build,
    },
    Group {
        columns: positions::COLUMNS,
        build: positions::build,
    },
    Group {
        columns: targets::COLUMNS,
        build: targets::build,
    },
    Group {
        columns: containers::COLUMNS,
        build: containers::build,
    },
    Group {
        columns: diagnostics::COLUMNS,
        build: diagnostics::build,
    },
    Group {
        columns: core_group::COLUMNS,
        build: core_group::build,
    },
    Group {
        columns: concurrency::COLUMNS,
        build: concurrency::build,
    },
    Group {
        columns: tsoptions::COLUMNS,
        build: tsoptions::build,
    },
];

pub fn check(value: &Value) -> Result<(), String> {
    crate::protocol::fields(value, "id column op input")?;
    let column = value["column"].as_str().ok_or("column")?;
    if !GROUPS.iter().any(|group| group.columns.contains(&column)) {
        return Err(format!("unknown table column {column:?}"));
    }
    if !value["input"].is_object() {
        return Err("input".into());
    }
    Ok(())
}

/// The size a table row is ordered by: its source bytes, or its input's length.
pub fn size(value: &Value) -> usize {
    value["input"]["source_hex"]
        .as_str()
        .map_or_else(|| value["input"].to_string().len(), |hex| hex.len() / 2)
}

/// Runs one row; the caller is already on the parser worker.
pub fn run(s: &Session, r: &Value) {
    let mut column: Option<Column> = None;
    if !s.stage("setup", || {
        column = Some(build(r["column"].as_str().ok_or("column")?, &r["input"])?);
        Ok(())
    }) {
        return;
    }
    let mut value = None;
    let evaluate = column.take().expect("setup built the column");
    if s.stage("column", || {
        value = Some(evaluate()?);
        Ok(())
    }) {
        enter(Stage::Observe);
        s.observe("column", "value", value.expect("column value"));
    }
}

fn build(column: &str, input: &Value) -> Result<Column, String> {
    GROUPS
        .iter()
        .find_map(|group| (group.build)(column, input))
        .unwrap_or_else(|| Err(format!("unknown table column {column:?}")))
}

/// Error text of any displayable error (`map_err(text)` hands it over by value).
#[allow(clippy::needless_pass_by_value)]
pub fn text(error: impl ToString) -> String {
    error.to_string()
}

/// The value a column records for a panic its callers rely on (a class of
/// the spec's `panic_contract`), which the port returns through an explicit
/// check: Go's `Guard` records the same `{"panic": class}`.
#[allow(dead_code)] // for the columns whose spec declares a panic contract
pub fn panic_value(class: &str) -> Value {
    json!({ "panic": class })
}

/// Decodes a column's input (or a part of it) into `T`.
pub fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(text)
}

// ---------------------------------------------------------------------------
// Source and bound inputs: the facts oracle's parse and document order.

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<String>,
}

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }

    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error.to_string());
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
                self.error = Some(error.to_string());
                ControlFlow::Break(())
            }
        }
    }
}

/// The document order of `source` and `bound` inputs (Go's `Walk`), the facts
/// oracle's preorder: a node, then each child subtree in child-visitor order
/// (Go's `ForEachChild`). Reparsed nodes are in it where the parser attaches
/// them: reparsed declarations (JS `@typedef`, `@callback`, `@import`,
/// `@overload`) in the enclosing list just before the element whose JSDoc
/// holds them (`@typedef`, `@callback` and `@import` moved out to the nearest
/// statement list), or after the file's statements for the end-of-file
/// token's JSDoc; and reparsed parameters, types, type parameters and members
/// as ordinary children of their hosts. JSDoc comments and everything under
/// them are not in it.
pub fn walk(view: AstView<'_>, root: NodeId) -> Result<Vec<NodeId>, String> {
    walk_with(view, root, None)
}

/// The document order of `source_jsdoc` and `bound_jsdoc` inputs (Go's
/// `WalkJSDoc`): [`walk`], with a node's JSDoc comments (the parser's JSDoc
/// provider, which parses lazy JSDoc in setup) and their subtrees visited
/// before its children, the order of Go's `ForEachChildAndJSDoc`, except that
/// a comment is visited only under its parent: a reparsed `@typedef` or
/// `@callback` declaration lists its source statement's comment as its own
/// JSDoc, and that comment is visited once, under the statement. Every JSDoc
/// comment, tag and type expression then has one reference, and [`walk`]'s
/// order is a subsequence of it.
pub fn walk_jsdoc(view: AstView<'_>, root: NodeId) -> Result<Vec<NodeId>, String> {
    walk_with(
        view,
        root,
        Some(&mut tsr_parser::ParserJsDocProvider::default()),
    )
}

fn walk_with(
    view: AstView<'_>,
    root: NodeId,
    mut jsdoc: Option<&mut tsr_parser::ParserJsDocProvider>,
) -> Result<Vec<NodeId>, String> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        let node = view.node(id).map_err(text)?;
        nodes.push(id);
        let mut children = Children {
            view,
            nodes: Vec::new(),
            error: None,
        };
        if let Some(provider) = jsdoc.as_deref_mut() {
            for comment in provider.jsdoc(view, root, id).map_err(text)?.to_vec() {
                if view.node(comment).map_err(text)?.parent() == Some(id) {
                    children.nodes.push(comment);
                }
            }
        }
        let _ = node.for_each_child(&mut children);
        if let Some(error) = children.error {
            return Err(error);
        }
        pending.extend(children.nodes.into_iter().rev());
    }
    Ok(nodes)
}

/// A node's children in `ForEachChild` order: the projection of a node no
/// walk holds (factory output) by the references of its children.
pub fn children(view: AstView<'_>, id: NodeId) -> Result<Vec<NodeId>, String> {
    let mut children = Children {
        view,
        nodes: Vec::new(),
        error: None,
    };
    let _ = view.node(id).map_err(text)?.for_each_child(&mut children);
    match children.error {
        Some(error) => Err(error),
        None => Ok(children.nodes),
    }
}

enum Tree {
    Parsed(Box<ParsedFile>),
    Bound(BoundFile),
}

/// A parsed (and, for bound inputs, bound) source file with its nodes in
/// document order ([`walk`], or [`walk_jsdoc`] for the `_jsdoc` input kinds).
/// A node's reference in a value is its index in `nodes`, and no other node
/// has one.
pub struct Parsed {
    tree: Tree,
    root: NodeId,
    /// Whether `nodes` is [`walk_jsdoc`]'s order.
    pub jsdoc: bool,
    pub nodes: Vec<NodeId>,
    index: HashMap<NodeId, usize>,
}

impl Parsed {
    fn new(tree: Tree, root: NodeId, jsdoc: bool) -> Result<Self, String> {
        let mut parsed = Self {
            tree,
            root,
            jsdoc,
            nodes: Vec::new(),
            index: HashMap::new(),
        };
        parsed.nodes = if jsdoc {
            walk_jsdoc(parsed.view(), root)?
        } else {
            walk(parsed.view(), root)?
        };
        for (at, id) in parsed.nodes.iter().enumerate() {
            if let Some(first) = parsed.index.insert(*id, at) {
                return Err(format!("the walk reaches node {first} again at {at}"));
            }
        }
        Ok(parsed)
    }

    pub fn view(&self) -> AstView<'_> {
        match &self.tree {
            Tree::Parsed(parsed) => parsed.view(),
            Tree::Bound(bound) => bound.view().ast(),
        }
    }

    /// The source file node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The bound view of a bound input.
    /// The parsed file's builder, the factory of a column whose operation
    /// builds nodes over the input's storage.
    pub fn builder_mut(&mut self) -> Result<&mut tsr_ast::AstBuilder, String> {
        match &mut self.tree {
            Tree::Parsed(parsed) => Ok(parsed.builder_mut()),
            Tree::Bound(_) => Err("a bound input has no builder".into()),
        }
    }

    pub fn bound(&self) -> Option<BoundView<'_>> {
        match &self.tree {
            Tree::Parsed(_) => None,
            Tree::Bound(bound) => Some(bound.view()),
        }
    }

    /// Null for no node, else the node's index in `nodes` (Go's `Parsed.Ref`).
    /// A node outside the walk has no reference and is an error: a shared
    /// placeholder such as -1 would let a port that returns the wrong JSDoc or
    /// synthesized node match Go. Go stops its driver on such a node, so a
    /// Rust error here is a difference from native, never a match.
    pub fn node_ref(&self, node: Option<NodeId>) -> Result<Value, String> {
        let Some(node) = node else {
            return Ok(Value::Null);
        };
        match self.index.get(&node) {
            Some(at) => Ok(json!(at)),
            None => Err(format!(
                "node {node:?} is outside the {}; project it by its own fields",
                if self.jsdoc {
                    "JSDoc walk"
                } else {
                    "walk (a JSDoc node needs a source_jsdoc or bound_jsdoc input)"
                }
            )),
        }
    }

    /// The reference list of nodes (Go's `Parsed.Refs`).
    /// Go's `RefOrFields`: the reference of a walked node, else
    /// `["fields", kind, pos, end]`.
    pub fn node_ref_or_fields(&self, node: Option<NodeId>) -> Result<Value, String> {
        let Some(node) = node else {
            return Ok(Value::Null);
        };
        if let Some(at) = self.index.get(&node) {
            return Ok(json!(at));
        }
        let read = self.view().node(node).map_err(text)?;
        Ok(json!(["fields", read.kind().raw(), read.pos(), read.end()]))
    }

    pub fn refs(&self, nodes: impl IntoIterator<Item = NodeId>) -> Result<Value, String> {
        nodes
            .into_iter()
            .map(|node| self.node_ref(Some(node)))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    }

    /// The symbol of a node of a bound input (Go's `Node.Symbol`).
    pub fn node_symbol(&self, node: NodeId) -> Result<Option<SymbolId>, String> {
        let bound = self.bound().ok_or("the input is not bound")?;
        Ok(bound
            .node_binding(node)
            .map_err(text)?
            .and_then(|binding| binding.symbol))
    }

    /// `[first declaration reference or null, name as hex]`, null for no
    /// symbol: Go's `Parsed.SymbolKey`.
    pub fn symbol_key(&self, symbol: Option<SymbolId>) -> Result<Value, String> {
        let Some(id) = symbol else {
            return Ok(Value::Null);
        };
        let bound = self.bound().ok_or("the input is not bound")?;
        let symbol = bound.symbol(id).map_err(text)?;
        let first = bound
            .result()
            .declarations()
            .get(symbol.declarations())
            .map_err(text)?
            .iter()
            .flatten()
            .next();
        Ok(json!([self.node_ref(first)?, hex(symbol.name_bytes())]))
    }
}

fn parse_file(input: &Value) -> Result<ParsedFile, String> {
    crate::protocol::fields(input, "filename path jsx force script_kind source_hex")?;
    let source = SourceText::from_loaded_bytes(unhex(&input["source_hex"])?);
    let options = SourceFileParseOptions {
        file_name: JsString::from_bytes(input["filename"].as_str().ok_or("filename")?.as_bytes()),
        path: JsString::from_bytes(input["path"].as_str().ok_or("path")?.as_bytes()),
        external_module_indicator_options: ExternalModuleIndicatorOptions {
            jsx: input["jsx"].as_bool().ok_or("jsx")?,
            force: input["force"].as_bool().ok_or("force")?,
        },
    };
    let kind = input["script_kind"].as_i64().ok_or("script_kind")?;
    Ok(tsr_parser::parse_source_file(
        source,
        tsr_core::ScriptKind(i32::try_from(kind).map_err(text)?),
        options,
    ))
}

/// The setup of a `source` column: parse, then [`walk`] (Go's `ParseSource`).
pub fn parse_source(input: &Value) -> Result<Parsed, String> {
    parse_input(input, false, false)
}

/// The setup of a `bound` column: parse, bind, then [`walk`] (Go's
/// `BindSource`).
pub fn bind_source(input: &Value) -> Result<Parsed, String> {
    parse_input(input, true, false)
}

/// The setup of a `source_jsdoc` column: parse, then [`walk_jsdoc`] (Go's
/// `ParseSourceJSDoc`).
pub fn parse_source_jsdoc(input: &Value) -> Result<Parsed, String> {
    parse_input(input, false, true)
}

/// The setup of a `bound_jsdoc` column: parse, bind, then [`walk_jsdoc`]
/// (Go's `BindSourceJSDoc`).
pub fn bind_source_jsdoc(input: &Value) -> Result<Parsed, String> {
    parse_input(input, true, true)
}

fn parse_input(input: &Value, bind: bool, jsdoc: bool) -> Result<Parsed, String> {
    let parsed = parse_file(input)?;
    let root = parsed.root();
    if !bind {
        return Parsed::new(Tree::Parsed(Box::new(parsed)), root, jsdoc);
    }
    let file = parsed.publish_unbound();
    let bound = tsr_binder::bind_source_file(&file, root).map_err(text)?;
    Parsed::new(Tree::Bound(bound), root, jsdoc)
}

// ---------------------------------------------------------------------------
// Config inputs: tsoptionstest's VFS host and
// ParseJsonSourceFileConfigFileContent over <currentDirectory>/tsconfig.json.

struct Host {
    fs: Arc<dyn tsr_vfs::FileSystem>,
    cwd: JsString,
}

impl tsr_tsoptions::ParseConfigHost for Host {
    fn fs(&self) -> &dyn tsr_vfs::FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported("table configs do not extend"))
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported(
            "table configs have no content mappers",
        ))
    }
}

/// The setup of a config column (Go's `ParseConfig`): the parsed command line
/// and the column's own `args`.
/// The VFS host of a config input (Go's `tsoptionstest.NewVFSParseConfigHost`)
/// and its args, for columns that parse a command line rather than the
/// tsconfig.
pub fn config_host(input: &Value) -> Result<(impl tsr_tsoptions::ParseConfigHost, Value), String> {
    crate::protocol::fields(input, "files currentDirectory caseSensitive jsonText args")?;
    let current = input["currentDirectory"]
        .as_str()
        .ok_or("currentDirectory")?
        .as_bytes()
        .to_vec();
    let case_sensitive = input["caseSensitive"].as_bool().ok_or("caseSensitive")?;
    let mut builder = tsr_vfs::MemoryBuilder::new(&current, case_sensitive);
    for (path, content) in input["files"].as_object().ok_or("files")? {
        builder.insert_physical(
            path.as_bytes(),
            content.as_str().ok_or("file text")?.as_bytes().to_vec(),
        );
    }
    Ok((
        Host {
            fs: Arc::new(builder.finish()),
            cwd: JsString::from_bytes(current),
        },
        input["args"].clone(),
    ))
}

pub fn parse_config(input: &Value) -> Result<(tsr_tsoptions::ParsedCommandLine, Value), String> {
    crate::protocol::fields(input, "files currentDirectory caseSensitive jsonText args")?;
    let current = input["currentDirectory"]
        .as_str()
        .ok_or("currentDirectory")?
        .as_bytes()
        .to_vec();
    let case_sensitive = input["caseSensitive"].as_bool().ok_or("caseSensitive")?;
    let mut builder = tsr_vfs::MemoryBuilder::new(&current, case_sensitive);
    for (path, content) in input["files"].as_object().ok_or("files")? {
        builder.insert_physical(
            path.as_bytes(),
            content.as_str().ok_or("file text")?.as_bytes().to_vec(),
        );
    }
    let host = Host {
        fs: Arc::new(builder.finish()),
        cwd: JsString::from_bytes(current.clone()),
    };
    let name = tsr_tspath::combine(&current, &[b"tsconfig.json"]);
    let source = tsr_tsoptions::TsConfigSourceFile::parse(
        JsString::from_bytes(name.clone()),
        tsr_tspath::to_path(&name, &current, case_sensitive),
        SourceText::from_loaded_bytes(
            input["jsonText"]
                .as_str()
                .ok_or("jsonText")?
                .as_bytes()
                .to_vec(),
        ),
    );
    let parsed = tsr_tsoptions::parse_json_source_file_config_file_content(
        source,
        &host,
        &current,
        &tsr_core::CompilerOptions::default(),
        &tsr_tsoptions::ConfigValue::Null,
        &name,
    )
    .map_err(|error| format!("config parse: {error:?}"))?;
    Ok((parsed, input["args"].clone()))
}

#[cfg(test)]
mod tests {
    use super::GROUPS;
    use crate::jobs::{run_row, Request, Rows};
    use crate::protocol::{is_production, Oracle};
    use crate::Driver;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use tsr_ast::{node_flags, JsDocProvider, NodeId, SyntaxKind};

    fn request(column: &str, input: &Value) -> Request {
        let value = json!({"id": "row", "column": column, "op": "table", "input": input});
        Request {
            index: 0,
            id: "row".into(),
            sha256: String::new(),
            source_bytes: super::size(&value),
            value,
        }
    }

    #[test]
    fn a_row_observes_its_setup_and_digests_the_column_value() {
        assert!(!is_production("setup") && is_production("column"));
        let input = json!({"s": [1, 2, 3], "cases": [{"start": 1, "count": 1, "items": [9]}]});
        let row = run_row(
            &Driver(Oracle::Table),
            &request("core.Splice", &input),
            true,
            0,
        );
        let output = row.output;
        assert!(output.error.is_none(), "{:?}", output.error);
        assert_eq!(output.outcomes["setup"], "ok");
        assert_eq!(output.outcomes["column"], "ok");
        let value = json!([[1, 9, 3]]);
        let mut canonical = Vec::new();
        crate::canonical::write(&mut canonical, &value, false).unwrap();
        assert_eq!(
            output.digests["column"],
            crate::jobs::sha256_hex(&canonical),
            "the digest is sha256(canonical(value)) with no newline"
        );
        assert!(Driver(Oracle::Table).dumps_kills());
        assert!(!Driver(Oracle::Table).observe_counts());
        // A malformed input fails setup; the column never runs and has no digest.
        let row = run_row(
            &Driver(Oracle::Table),
            &request("core.Splice", &json!({"s": "x"})),
            false,
            0,
        );
        assert_eq!(row.output.outcomes["setup"], "error");
        assert_eq!(row.output.outcomes["column"], "not_run");
        assert!(row.output.digests.is_empty());
    }

    #[test]
    fn requests_name_a_declared_column_and_an_object_input() {
        let ok = json!({"id": "r", "column": "core.Splice", "op": "table", "input": {}});
        assert!(super::check(&ok).is_ok());
        let unknown = json!({"id": "r", "column": "no.such", "op": "table", "input": {}});
        assert!(super::check(&unknown)
            .unwrap_err()
            .contains("unknown table column"));
        let extra = json!({"id": "r", "column": "core.Splice", "op": "table", "input": {}, "x": 1});
        assert!(super::check(&extra).is_err());
        let source = json!({"input": {"source_hex": "616263"}});
        assert_eq!(super::size(&source), 3);
        assert_eq!(
            super::size(&json!({"input": {"s": []}})),
            "{\"s\":[]}".len()
        );
    }

    fn source(filename: &str, script_kind: i64, text: &str) -> Value {
        json!({"filename": filename, "path": filename, "jsx": false, "force": false,
               "script_kind": script_kind, "source_hex": crate::protocol::hex(text.as_bytes())})
    }

    fn kinds(parsed: &super::Parsed) -> Vec<(i16, i32, i32, u32)> {
        let view = parsed.view();
        parsed
            .nodes
            .iter()
            .map(|id| {
                let node = view.node(*id).unwrap();
                (node.kind().raw(), node.pos(), node.end(), node.flags())
            })
            .collect()
    }

    #[test]
    fn references_are_walk_indices_and_a_node_outside_the_walk_has_none() {
        // A TypeScript file (lazy JSDoc) and a JavaScript one (eager JSDoc,
        // reparsed @type and @typedef).
        for input in [
            source(
                "/t.ts",
                3,
                "/** @deprecated use g */\nfunction f(/** a */ a: number) {}\n",
            ),
            source(
                "/t.js",
                1,
                "/** @typedef {{a: string}} T */\n/** @type {T} */\nvar x;\n",
            ),
        ] {
            let plain = super::parse_source(&input).unwrap();
            let with_jsdoc = super::parse_source_jsdoc(&input).unwrap();
            assert!(!plain.jsdoc && with_jsdoc.jsdoc);
            let (plain_rows, jsdoc_rows) = (kinds(&plain), kinds(&with_jsdoc));
            // The JSDoc walk adds exactly the comment subtrees (a node with a
            // JSDoc ancestor-or-self), each once, and the plain walk is the
            // rest of it in order.
            let view = with_jsdoc.view();
            let in_comment = |id: NodeId| {
                let mut at = Some(id);
                while let Some(node) = at.map(|id| view.node(id).unwrap()) {
                    if node.kind().known() == Some(SyntaxKind::JSDoc) {
                        return true;
                    }
                    at = node.parent();
                }
                false
            };
            let rest: Vec<_> = with_jsdoc
                .nodes
                .iter()
                .zip(&jsdoc_rows)
                .filter(|(id, _)| !in_comment(**id))
                .map(|(_, row)| *row)
                .collect();
            assert_eq!(rest, plain_rows);
            assert!(jsdoc_rows
                .iter()
                .any(|row| row.0 == SyntaxKind::JSDoc as i16));
            // Every node, JSDoc included, has its index in the JSDoc walk...
            for (at, id) in with_jsdoc.nodes.iter().enumerate() {
                assert_eq!(with_jsdoc.node_ref(Some(*id)).unwrap(), json!(at));
            }
            // ...and none in the plain walk: a reference to it is an error,
            // never a shared placeholder.
            let mut provider = tsr_parser::ParserJsDocProvider::default();
            let view = plain.view();
            let mut refused = 0;
            for id in &plain.nodes {
                for doc in provider.jsdoc(view, plain.root(), *id).unwrap().to_vec() {
                    let error = plain.node_ref(Some(doc)).unwrap_err();
                    assert!(error.contains("outside the walk"), "{error}");
                    assert!(plain.refs([*id, doc]).is_err());
                    refused += 1;
                }
            }
            assert!(refused > 0);
            assert_eq!(plain.node_ref(None).unwrap(), Value::Null);
            assert_eq!(plain.refs([]).unwrap(), json!([]));
        }
    }

    fn view_parent(parsed: &super::Parsed, id: NodeId) -> Option<NodeId> {
        parsed.view().node(id).unwrap().parent()
    }

    #[test]
    fn reparsed_nodes_are_in_the_plain_walk() {
        let input = source(
            "/t.js",
            1,
            "/** @typedef {{a: string}} T */\n/** @type {T} */\nvar x;\n",
        );
        let plain = super::parse_source(&input).unwrap();
        let rows = kinds(&plain);
        // The reparsed @typedef declaration stands just before its host
        // statement; the reparsed @type annotation is a child of `x`.
        let kind = |at: usize| rows[at].0;
        assert_eq!(kind(1), SyntaxKind::JSTypeAliasDeclaration as i16);
        assert_ne!(rows[1].3 & node_flags::REPARSED, 0);
        let statement = (2..rows.len())
            .find(|at| kind(*at) == SyntaxKind::VariableStatement as i16)
            .unwrap();
        assert!(rows[statement..]
            .iter()
            .any(|row| row.0 == SyntaxKind::TypeReference as i16
                && row.3 & node_flags::REPARSED != 0));
        assert!(rows.iter().all(|row| row.0 != SyntaxKind::JSDoc as i16));
        // The JSDoc walk visits the @typedef comment once, under the statement
        // (its parent), not under the reparsed declaration that shares it.
        let with_jsdoc = super::parse_source_jsdoc(&input).unwrap();
        let jsdoc_rows = kinds(&with_jsdoc);
        let comments: Vec<_> = (0..jsdoc_rows.len())
            .filter(|at| jsdoc_rows[*at].0 == SyntaxKind::JSDoc as i16)
            .collect();
        assert_eq!(comments.len(), 2);
        let host = (0..jsdoc_rows.len())
            .find(|at| jsdoc_rows[*at].0 == SyntaxKind::VariableStatement as i16)
            .unwrap();
        assert_eq!(comments[0], host + 1);
        for at in comments {
            let parent = view_parent(&with_jsdoc, with_jsdoc.nodes[at]);
            assert_eq!(with_jsdoc.node_ref(parent).unwrap(), json!(host));
        }
        // The bound JSDoc walk keys symbols by their declarations' indices in
        // that walk; binding declares no JSDoc node.
        let bound = super::bind_source_jsdoc(&input).unwrap();
        let mut keyed = 0;
        for id in &bound.nodes {
            if let Some(symbol) = bound.node_symbol(*id).unwrap() {
                let key = bound.symbol_key(Some(symbol)).unwrap();
                assert!(key[0].is_u64(), "{key}");
                keyed += 1;
            }
        }
        assert!(keyed >= 2);
    }

    /// Stage-2 packages port into these homes (the closure contract's list) and
    /// call the ports from their column modules, so every home is a public
    /// module: a private one would need a crate-root edit or a re-export per
    /// port. This test compiles only while that holds.
    #[test]
    #[allow(unused_imports)]
    fn every_stage_two_port_home_is_a_public_module() {
        use tsr_ast::{
            diagnostic_api as _, precedence as _, source_file_tables as _, utilities_class as _,
            utilities_containers as _, utilities_modules as _, utilities_positions as _,
            utilities_targets as _,
        };
        use tsr_core::{
            bfs as _, context as _, linkstore as _, semaphore as _, slices_ext as _, stack as _,
            text_change_ext as _, workgroup as _,
        };
        use tsr_tsoptions::{
            affects as _, enum_maps as _, parsed_commandline_ext as _, show_config as _,
        };
        // TC5's precedence ports sit beside the existing ones.
        let precedence = tsr_ast::precedence::get_operator_precedence(
            tsr_ast::SyntaxKind::BinaryExpression.into(),
            tsr_ast::SyntaxKind::PlusToken.into(),
            tsr_ast::precedence::operator_precedence_flags::NONE,
        );
        assert_eq!(
            precedence,
            tsr_ast::precedence::operator_precedence::ADDITIVE
        );
    }

    #[test]
    fn every_declared_column_is_built_by_its_own_group_only() {
        let mut seen = HashSet::new();
        for (index, group) in GROUPS.iter().enumerate() {
            for column in group.columns {
                assert!(seen.insert(*column), "{column} is declared twice");
                for (other_index, other) in GROUPS.iter().enumerate() {
                    let built = (other.build)(column, &json!({})).is_some();
                    assert_eq!(
                        built,
                        other_index == index,
                        "{column} of group {index} built by group {other_index}"
                    );
                }
            }
        }
        for group in GROUPS {
            assert!((group.build)("no.such.column", &json!({})).is_none());
        }
    }
}
