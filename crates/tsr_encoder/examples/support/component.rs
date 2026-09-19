//! S06 component adapter; parse requests require the parser's E1 adapter.
#[path = "codec.rs"]
mod codec;
#[path = "../../../ts_ast/examples/support/factory.rs"]
mod factory;
use crate::protocol;
use protocol::{fields, hex, unhex, Session};
use serde_json::{json, Value};
use std::ops::ControlFlow;
use std::sync::Arc;
use ts_arena::Counters;
use ts_ast::{AstView, ChildVisitor, NodeId, NodeKind, NodeListId, NodeSlice};

pub fn validate(request: &Value) -> Result<(), String> {
    let op = request["op"].as_str().ok_or("invalid operation")?;
    fields(
        request,
        match op {
            "decode" => "version id primary op wire_hex entrypoint",
            "path" => "version id primary op path_hex",
            "kind_names" => "version id primary op first count",
            "factory" | "codec" => "version id primary op scenario",
            _ => return Err("unsupported component operation".into()),
        },
    )?;
    if request["version"].as_i64() != Some(1)
        || request["id"].as_str().is_none_or(str::is_empty)
        || !request["primary"].is_null()
    {
        return Err("invalid version, id or primary row".into());
    }
    match op {
        "codec" => {
            if !request["scenario"].as_str().is_some_and(codec::scenario) {
                return Err("invalid codec scenario".into());
            }
        }
        "decode" => {
            unhex(&request["wire_hex"])?;
            if !matches!(
                request["entrypoint"].as_str(),
                Some("nodes" | "source_file")
            ) {
                return Err("invalid decoder entrypoint".into());
            }
        }
        "path" => {
            unhex(&request["path_hex"])?;
        }
        "kind_names" => {
            let first = request["first"].as_i64().ok_or("invalid first kind")?;
            let count = request["count"].as_i64().ok_or("invalid kind count")?;
            if !(-32768..=32767).contains(&first) || !(1..=32768 - first).contains(&count) {
                return Err("invalid kind range".into());
            }
        }
        "factory" => {
            if !matches!(
                request["scenario"].as_str(),
                Some(
                    "hooks-counts-update-clone"
                        | "raw-slice-same"
                        | "visitor-nil-flatten-disable"
                        | "visitor-lift-contracts"
                        | "visitor-role-hooks"
                        | "deep-clone-locations-and-parents"
                        | "subtree-cache-and-exclusions"
                        | "token-subtree-and-precedence"
                        | "source-file-clone-omissions"
                        | "source-cache-panic-once"
                        | "node-index-nil-after-sort"
                        | "source-file-hooks"
                )
            ) {
                return Err("unknown factory scenario".into());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.view.list(id).expect("decoded list owner").nodes())
    }
    fn visit_node_slice(&mut self, ids: NodeSlice) -> ControlFlow<()> {
        self.nodes.extend(
            self.view
                .node_slice(ids)
                .expect("decoded slice owner")
                .iter()
                .flatten(),
        );
        ControlFlow::Continue(())
    }
}
pub fn emit_tree(session: &Session, view: AstView<'_>, root: Option<NodeId>) {
    let mut stack: Vec<_> = root.into_iter().collect();
    let mut count = 0;
    while let Some(id) = stack.pop() {
        let node = view.node(id).expect("decoded node owner");
        let mut children = Children {
            view,
            nodes: Vec::new(),
        };
        let _ = node.for_each_child(&mut children);
        session.observe("decoded_tree","tree_node",json!({"index":count,"kind":node.kind().raw(),"pos":node.pos(),"end":node.end(),"flags":node.flags(),"children":children.nodes.len()}));
        count += 1;
        if count > 1_000_000 {
            session.failure(format!("adapter decoded walk exceeds {count} nodes"));
            return;
        }
        stack.extend(children.nodes.into_iter().rev());
    }
}
pub fn execute(session: &Session, request: &Value) {
    match request["op"].as_str().expect("validated operation") {
        "codec" => codec::execute(session, request),
        "factory" => {
            let output = session.clone();
            let sink: factory::Sink = Arc::new(move |w| {
                output.observe(
                    "factory",
                    "factory",
                    json!({"label":w.label,"ints":w.ints,"bools":w.bools,"strings":w.strings}),
                );
            });
            session.stage("factory", || {
                factory::run(
                    request["scenario"].as_str().expect("validated scenario"),
                    &sink,
                );
                Ok(())
            });
        }
        "path" => {
            session.stage("path",||{
            let path=unhex(&request["path_hex"])?;
            session.observe("path","path",json!({"encoded_root_length":ts_core::path::encoded_root_length(&path),"normalized_hex":hex(&ts_core::path::normalize(&path)),"declaration_file":ts_core::path::is_declaration_file_name(&path)}));
            Ok(())
        });
        }
        "kind_names" => {
            session.stage("kind_names", || {
                let first = request["first"].as_i64().expect("validated first");
                let count = request["count"].as_i64().expect("validated count");
                for raw in first..first + count {
                    session.observe(
                        "kind_names",
                        "kind",
                        json!({"raw":raw,"name":NodeKind::from_raw(raw as i16).to_string()}),
                    );
                }
                Ok(())
            });
        }
        "decode" => {
            let mut tree = None;
            if !session.stage("decode",||{
                let raw=unhex(&request["wire_hex"])?;
                let result=if request["entrypoint"]=="nodes" {ts_encoder::decode_nodes(&raw,&Counters::new())} else {ts_encoder::decode_source_file(&raw,&Counters::new())};
                let decoded=result.map_err(|error|error.to_string())?;
                let root=decoded.root.map(|id|{
                    let node=decoded.builder.view().node(id).expect("decoded root owner");
                    json!({"kind":node.kind().raw(),"pos":node.pos(),"end":node.end(),"flags":node.flags()})
                });
                session.observe("decode","root",json!({"node":root}));
                tree=Some(decoded);Ok(())
            }) {return}
            let tree = tree.expect("successful decoder");
            session.stage("decoded_tree", || {
                emit_tree(session, tree.builder.view(), tree.root);
                Ok(())
            });
        }
        _ => unreachable!(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_boundary_rejects_schema_and_number_coercions() {
        let base = json!({"version":1,"id":"x","primary":null,"op":"decode","wire_hex":"","entrypoint":"nodes"});
        validate(&base).unwrap();
        for (field, bad) in [
            ("version", json!(true)),
            ("entrypoint", json!("other")),
            ("wire_hex", json!("FF")),
            ("primary", json!("case")),
        ] {
            let mut value = base.clone();
            value[field] = bad;
            assert!(validate(&value).is_err());
        }
        for raw in [r#"{"a":1,"a":1}"#, r#"{"a":1.0}"#, r#"{"a":"\ud800"}"#] {
            assert!(serde_json::from_str::<protocol::Strict>(raw).is_err());
        }
    }
}
