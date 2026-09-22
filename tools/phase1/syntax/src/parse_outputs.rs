//! `parseOutputs` group (F4a plan task 5, the missing witnesses). The parser
//! outputs E1's encoder never observes -- comment directives, pragmas, the
//! check-js directive, UsesUriStyleNodeCoreModules and reparsed clones -- read
//! from the production parse, and the lazy JSDoc contract read through the
//! production providers: `AstView::source_eager_jsdoc` (EagerJSDoc) and
//! `tsr_parser::ParserJsDocProvider` (JSDoc). Shapes follow the probe; a node
//! is [kind, pos, end, ordinal] with first-seen ordinals.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use tsr_arena::NodeId;
use tsr_ast::{node_flags, AstView, JsDocProvider, SourceFileParseOptions};
use tsr_jsstring::SourceText;

use crate::api::{subject, Outcome};
use crate::astnav::preorder;

const SUBJECT: &str = "parseOutputs";

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == SUBJECT).then(|| match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error),
    })
}

fn text(value: &tsr_ast::JsString) -> String {
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

struct Nodes<'a> {
    view: AstView<'a>,
    ordinals: HashMap<NodeId, usize>,
}

impl Nodes<'_> {
    fn node(&mut self, id: NodeId) -> Result<Value, String> {
        let next = self.ordinals.len();
        let ordinal = *self.ordinals.entry(id).or_insert(next);
        let node = self.view.node(id).map_err(|e| format!("{e:?}"))?;
        Ok(json!([node.kind().raw(), node.pos(), node.end(), ordinal]))
    }

    fn list(&mut self, ids: &[NodeId]) -> Result<Value, String> {
        ids.iter()
            .map(|id| self.node(*id))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    }
}

fn side_fields(nodes: &mut Nodes<'_>, root: NodeId) -> Result<Value, String> {
    let view = nodes.view;
    let source = view.source_file(root).map_err(|e| format!("{e:?}"))?;
    let directives: Vec<Value> = source
        .comment_directives()
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .map(|d| json!([d.kind as i32, d.loc.pos(), d.loc.end()]))
        .collect();
    let pragmas: Vec<Value> = source
        .pragmas()
        .map_err(|e| format!("{e:?}"))?
        .iter()
        .map(|p| {
            // BTreeMap order is the probe's sorted-name order.
            let args: Vec<Value> = p
                .args
                .iter()
                .map(|(name, arg)| {
                    json!([
                        text(name),
                        text(&arg.name),
                        text(&arg.value),
                        arg.loc.pos(),
                        arg.loc.end()
                    ])
                })
                .collect();
            json!([
                text(&p.name),
                p.range.kind.raw(),
                p.range.loc.pos(),
                p.range.loc.end(),
                p.range.has_trailing_new_line,
                args
            ])
        })
        .collect();
    let check_js = source.check_js_directive.map_or(Value::Null, |d| {
        json!([
            d.enabled,
            d.range.kind.raw(),
            d.range.loc.pos(),
            d.range.loc.end(),
            d.range.has_trailing_new_line
        ])
    });
    let in_tree: HashSet<NodeId> = preorder(view, root)?.into_iter().collect();
    let mut clones = Vec::new();
    for clone in &source.reparsed_clones {
        let node = view.node(*clone).map_err(|e| format!("{e:?}"))?;
        let parent = match node.parent() {
            Some(parent) => i32::from(
                view.node(parent)
                    .map_err(|e| format!("{e:?}"))?
                    .kind()
                    .raw(),
            ),
            None => -1,
        };
        clones.push(json!([
            node.kind().raw(),
            node.pos(),
            node.end(),
            node.flags(),
            parent,
            in_tree.contains(clone)
        ]));
    }
    let uri = source.uses_uri_style_node_core_modules.0;
    Ok(json!([directives, pragmas, check_js, uri, clones]))
}

fn lazy_jsdoc(nodes: &mut Nodes<'_>, root: NodeId) -> Result<Value, String> {
    let view = nodes.view;
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut rows = Vec::new();
    for (index, node) in preorder(view, root)?.into_iter().enumerate() {
        let flags = view.node(node).map_err(|e| format!("{e:?}"))?.flags();
        if flags & node_flags::HAS_JS_DOC == 0 {
            continue;
        }
        let eager = |view: AstView<'_>| -> Result<Vec<NodeId>, String> {
            Ok(view
                .source_eager_jsdoc(root, node)
                .map_err(|e| format!("{e:?}"))?
                .map(|roots| roots.to_vec())
                .unwrap_or_default())
        };
        let eager_before = eager(view)?;
        let eager_before = nodes.list(&eager_before)?;
        let first = provider
            .jsdoc(view, root, node)
            .map_err(|e| format!("{e:?}"))?
            .to_vec();
        let lazy_first = nodes.list(&first)?;
        let second = provider
            .jsdoc(view, root, node)
            .map_err(|e| format!("{e:?}"))?
            .to_vec();
        let lazy_second = nodes.list(&second)?;
        let eager_after = eager(view)?;
        let eager_after = nodes.list(&eager_after)?;
        let mut trees = Vec::new();
        for jsdoc in &first {
            let order = preorder(view, *jsdoc)?;
            trees.push(nodes.list(&order)?);
        }
        rows.push(json!([
            index,
            eager_before,
            lazy_first,
            lazy_second,
            eager_after,
            trees
        ]));
    }
    Ok(Value::Array(rows))
}

fn run(request: &Value) -> Result<Value, String> {
    let file_spec = request.get("file").ok_or("request has no file")?;
    let name = file_spec["name"].as_str().ok_or("file has no name")?;
    let source_text = file_spec["text"].as_str().ok_or("file has no text")?;
    let kind = file_spec["script_kind"]
        .as_i64()
        .ok_or("file has no script kind")?;
    let file = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(source_text.as_bytes().to_vec()),
        tsr_core::ScriptKind(i32::try_from(kind).map_err(|e| e.to_string())?),
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(name.as_bytes()),
            path: tsr_ast::JsString::from_bytes(name.as_bytes()),
            ..Default::default()
        },
    )
    .publish_unbound();
    let root = file.root().ok_or("parsed file has no root")?;
    let mut nodes = Nodes {
        view: file.view(),
        ordinals: HashMap::new(),
    };
    let mut ordered = Vec::new();
    for action in request
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        ordered.push(match action.get("op").and_then(Value::as_str) {
            Some("side_fields") => side_fields(&mut nodes, root)?,
            Some("lazy_jsdoc") => lazy_jsdoc(&mut nodes, root)?,
            other => return Err(format!("unknown parseOutputs action {other:?}")),
        });
    }
    Ok(json!({"ordered": ordered, "nodes_seen": nodes.ordinals.len()}))
}
