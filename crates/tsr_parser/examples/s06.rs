//! Persistent S06 direct parser adapter; all language behavior lives in crates.
#[path = "../../ts_encoder/examples/support/component.rs"]
mod component;
#[path = "../../ts_encoder/examples/support/protocol.rs"]
mod protocol;
use protocol::{fields, hex, unhex, Session};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use ts_ast::{
    AstBuilder, AstView, Diagnostic, ExternalModuleIndicatorOptions, FactoryMethods, JsString,
    NodeId, NodeIndexCache, ParsedFile, SourceFileParseOptions, SyntaxKind,
};
use ts_jsstring::SourceText;
fn validate(request: &Value) -> Result<(), String> {
    if request["op"] != "parse" {
        return component::validate(request);
    }
    fields(
        request,
        "version id primary op source_hex filename path script_kind jsx force operations",
    )?;
    if request["version"].as_i64() != Some(1) || request["id"].as_str().is_none_or(str::is_empty) {
        return Err("invalid parser version/identity".into());
    }
    if !request["primary"].is_null() && request["primary"].as_str().is_none_or(str::is_empty) {
        return Err("invalid parser primary".into());
    }
    unhex(&request["source_hex"])?;
    for field in ["filename", "path"] {
        request[field].as_str().ok_or("invalid parser path")?;
    }
    for field in ["jsx", "force"] {
        request[field].as_bool().ok_or("invalid parser option")?;
    }
    let kind = request["script_kind"]
        .as_i64()
        .ok_or("invalid script kind")?;
    if i32::try_from(kind).is_err() {
        return Err("script kind outside signed32".into());
    }
    if request["operations"]
        != json!([
            "parse",
            "node_index_before",
            "encode_source_file",
            "node_index_after"
        ])
    {
        return Err("invalid parser operations".into());
    }
    Ok(())
}
fn diagnostic(s: &Session, collection: &str, index: usize, d: &Diagnostic) {
    s.observe("parse","diagnostic",json!({"collection":collection,"index":index,"code":d.code,"category":d.category,"pos":d.loc.pos(),"end":d.loc.end(),"key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),"args_hex":d.message_args.iter().map(|s|hex(s.as_bytes())).collect::<Vec<_>>()}));
}
fn table(s: &Session, stage: &str, view: AstView<'_>, table: &NodeIndexCache) {
    let indices: BTreeMap<_, _> = table
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(index, node)| node.map(|node| (node, index)))
        .collect();
    s.observe(stage, "table", json!({"length":table.nodes().len()}));
    for (index, node) in table.nodes().iter().enumerate() {
        let value=node.map(|id|{
   let node=view.node(id).expect("indexed node owner");
   json!({"kind":node.kind().raw(),"pos":node.pos(),"end":node.end(),"flags":node.flags(),"parent":node.parent().and_then(|id|indices.get(&id)).copied().unwrap_or(0),"lookup":table.get_index(view,Some(&node)).expect("indexed identity")})
  });
        s.observe(stage, "node", json!({"index":index,"node":value}));
    }
    let mut absent = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
    let id = absent.new_token(SyntaxKind::Unknown.into());
    s.observe(
        stage,
        "absent_lookup",
        json!({"index":table.get_index(view,Some(&absent.view().node(id).expect("absent node owner"))).expect("absent lookup")}),
    );
}
fn parse(s: &Session, r: &Value) {
    let mut parsed: Option<ParsedFile> = None;
    if !s.stage("parse",||{
  let source=SourceText::from_loaded_bytes(unhex(&r["source_hex"])?);
  let options=SourceFileParseOptions{file_name:JsString::from_bytes(r["filename"].as_str().expect("validated file").as_bytes()),path:JsString::from_bytes(r["path"].as_str().expect("validated path").as_bytes()),external_module_indicator_options:ExternalModuleIndicatorOptions{jsx:r["jsx"].as_bool().expect("validated jsx"),force:r["force"].as_bool().expect("validated force")}};
  let file=ts_parser::parse_source_file(source,ts_core::ScriptKind(r["script_kind"].as_i64().expect("validated kind") as i32),options);
  let root=file.root();let view=file.view();let node=view.node(root).expect("parsed root owner");let sf=view.source_file(root).expect("parsed source metadata");
  s.observe("parse","source_file",json!({"kind":node.kind().raw(),"pos":node.pos(),"end":node.end(),"flags":node.flags(),"node_count":sf.node_count,"text_count":sf.text_count,"identifier_count":sf.identifier_count,"script_kind":sf.script_kind.0,"language_variant":sf.language_variant.0,"declaration_file":sf.is_declaration_file,"hash":ts_encoder::source_file_hash(&sf)}));
  for (name,diagnostics) in [("parse",&sf.diagnostics),("js",&sf.js_diagnostics),("jsdoc",&sf.jsdoc_diagnostics)] {
   for (index,d) in diagnostics.iter().enumerate(){diagnostic(s,name,index,d);}
  }
  drop(sf);drop(node);parsed=Some(file);Ok(())
 }) {return}
    let parsed = parsed.expect("successful parse");
    let root: NodeId = parsed.root();
    let view = parsed.view();
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut before = None;
    if !s.stage("node_index_before",||{
  let current=ts_encoder::get_node_index_table(view,root,&mut provider).map_err(|e|e.to_string())?.expect("initial node index");
  let independent=ts_encoder::build_node_index_table(view,root,&mut provider).map_err(|e|e.to_string())?;
  let again=ts_encoder::get_node_index_table(view,root,&mut provider).map_err(|e|e.to_string())?.expect("cached node index");
  s.observe("node_index_before","identity",json!({"independent_order_equal":current.nodes()==independent.nodes(),"cache_reused":Arc::ptr_eq(&current,&again)}));
  table(s,"node_index_before",view,&current);before=Some(current);Ok(())
 }) {return}
    let before = before.expect("successful initial index");
    let mut encoded_table = None;
    if !s.stage("encode_source_file", || {
        let encoded =
            ts_encoder::encode_source_file(view, root, &mut provider).map_err(|e| e.to_string())?;
        s.observe(
            "encode_source_file",
            "bytes",
            json!({"length":encoded.bytes.len()}),
        );
        for (index, chunk) in encoded.bytes.chunks(65536).enumerate() {
            s.observe(
                "encode_source_file",
                "chunk",
                json!({"offset":index*65536,"hex":hex(chunk)}),
            );
        }
        encoded_table = encoded.index;
        Ok(())
    }) {
        return;
    }
    s.stage("node_index_after",||{
  let after=ts_encoder::get_node_index_table(view,root,&mut provider).map_err(|e|e.to_string())?.expect("cached node index");
  s.observe("node_index_after","identity",json!({"before_reused":Arc::ptr_eq(&before,&after),"encoded_reused":encoded_table.as_ref().is_some_and(|table|Arc::ptr_eq(table,&after))}));
  table(s,"node_index_after",view,&after);Ok(())
 });
}
fn execute(s: &Session, r: &Value) {
    ts_parser::on_parser_worker(|| {
        if r["op"] == "parse" {
            parse(s, r);
        } else {
            component::execute(s, r);
        }
    });
}
fn main() {
    if let Err(error) = protocol::run(validate, execute) {
        eprintln!("S06 Rust protocol: {error}");
        std::process::exit(2);
    }
}
