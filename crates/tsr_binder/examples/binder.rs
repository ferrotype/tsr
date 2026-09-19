//! Strict source parser/binder adapter. Observation never runs under native panic recovery.
#[path = "support/graph.rs"]
mod graph;
#[path = "support/graph_stream.rs"]
mod graph_stream;
#[path = "../../ts_encoder/examples/support/protocol.rs"]
mod protocol;
use protocol::{fields, unhex, Session};
use serde_json::{json, Value};
use ts_ast::{ExternalModuleIndicatorOptions, JsString, SourceFileParseOptions};
use ts_jsstring::SourceText;
fn validate(request: &Value) -> Result<(), String> {
    fields(
        request,
        "version id primary op source_hex filename path script_kind jsx force operations",
    )?;
    if request["version"] != 1
        || request["op"] != "bind"
        || request["id"].as_str().is_none_or(str::is_empty)
    {
        return Err("invalid binder identity/version".into());
    }
    if !request["primary"].is_null() && request["primary"].as_str().is_none_or(str::is_empty) {
        return Err("invalid primary row".into());
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
            "parsed_graph",
            "bind",
            "bound_graph",
            "repeat_bind",
            "repeated_graph"
        ])
    {
        return Err("invalid binder operation sequence".into());
    }
    Ok(())
}
fn execute(s: &Session, r: &Value) {
    let mut parsed = None;
    if !s.stage("parse", || {
        let source = SourceText::from_loaded_bytes(unhex(&r["source_hex"])?);
        let options = SourceFileParseOptions {
            file_name: JsString::from_bytes(r["filename"].as_str().unwrap().as_bytes()),
            path: JsString::from_bytes(r["path"].as_str().unwrap().as_bytes()),
            external_module_indicator_options: ExternalModuleIndicatorOptions {
                jsx: r["jsx"].as_bool().unwrap(),
                force: r["force"].as_bool().unwrap(),
            },
        };
        parsed = Some(ts_parser::parse_source_file(
            source,
            ts_core::ScriptKind(r["script_kind"].as_i64().unwrap() as i32),
            options,
        ));
        Ok(())
    }) {
        return;
    }
    let parsed = parsed.expect("successful parser stage");
    let source = parsed.root();
    graph_stream::dump(s, "parsed_graph", parsed.view(), source, None);
    s.stage("parsed_graph", || Ok(()));
    let file = parsed.publish_unbound();
    let mut bound = None;
    if !s.stage("bind", || {
        bound =
            Some(ts_binder::bind_source_file(&file, source).map_err(|error| error.to_string())?);
        Ok(())
    }) {
        return;
    }
    let bound = bound.expect("successful binder stage");
    let view = bound.view();
    graph_stream::dump(s, "bound_graph", view.ast(), source, Some(view.result()));
    s.stage("bound_graph", || Ok(()));
    if !s.stage("repeat_bind", || {
        ts_binder::bind_source_file(&file, source).map_err(|error| error.to_string())?;
        Ok(())
    }) {
        return;
    }
    let view = bound.view();
    graph_stream::dump(s, "repeated_graph", view.ast(), source, Some(view.result()));
    s.stage("repeated_graph", || Ok(()));
}
fn main() {
    if let Err(error) = protocol::run(validate, execute) {
        eprintln!("S07 Rust adapter protocol: {error}");
        std::process::exit(2);
    }
}
