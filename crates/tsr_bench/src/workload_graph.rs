//! Untimed graph reporting over the exact retained outputs of the benchmark pool.
use super::{binder_graph, Loaded};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use ts_ast::CompletedFile;

fn normalize(value: &mut Value, path: &str, names: &mut Vec<Value>) {
    match value {
        Value::Object(fields) => {
            if fields.len() == 2
                && fields.contains_key("raw_hex")
                && fields.get("identity").is_some_and(|id| !id.is_null())
            {
                names.push(
                    json!({"path":path,"raw_hex":fields["raw_hex"],"identity":fields["identity"]}),
                );
                fields.remove("raw_hex");
            } else {
                for (key, value) in fields {
                    normalize(value, &format!("{path}.{key}"), names);
                }
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter_mut().enumerate() {
                normalize(value, &format!("{path}[{index}]"), names);
            }
        }
        _ => {}
    }
}
pub fn write_report(
    out: &mut impl Write,
    file: &CompletedFile,
    input: &Loaded,
    index: usize,
    workers: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let view = file.view();
    let records = binder_graph::collect(view.ast(), view.result().source(), Some(view.result()));
    let mut raw = Sha256::new();
    let mut canonical = Sha256::new();
    let mut names = Vec::new();
    let diagnostics = records[0].1["diagnostics"].clone();
    let source = records[0].1.clone();
    let counts = records.last().expect("graph has counts").1.clone();
    let record_count = records.len();
    for (index, (kind, value)) in records.into_iter().enumerate() {
        let mut record = json!([kind, value]);
        raw.update(serde_json::to_vec(&record)?);
        raw.update(b"\n");
        normalize(&mut record, &format!("$[{index}]"), &mut names);
        canonical.update(serde_json::to_vec(&record)?);
        canonical.update(b"\n");
    }
    let report = json!({"version":1,"kind":"bind_graph","workers":workers,"index":index,
        "filename_hex":binder_graph::hex(input.options.file_name.as_bytes()),"path_hex":binder_graph::hex(input.options.path.as_bytes()),
        "script_kind":input.script_kind.0,"jsx":input.options.external_module_indicator_options.jsx,"force":input.options.external_module_indicator_options.force,
        "source_bytes":input.source.len(),"source_sha256":binder_graph::hex(&Sha256::digest(input.source.as_bytes())),
        "canonical_sha256":binder_graph::hex(&canonical.finalize()),"raw_sha256":binder_graph::hex(&raw.finalize()),
        "records":record_count,"counts":counts,"qualified_names":names,"diagnostics":diagnostics,
        "node_count":source["node_count"],"symbol_count":source["symbol_count"],"parse_diagnostics":view.source_file()?.diagnostics.len(),"bind_diagnostics":view.result().diagnostics().len()});
    serde_json::to_writer(&mut *out, &report)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// A selected retained graph, from the same full native worker run, for a raw
/// field-level mismatch witness. Collection stays after the measured endpoint.
pub fn write_records(
    out: &mut impl Write,
    file: &CompletedFile,
    index: usize,
    workers: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let view = file.view();
    let records = binder_graph::collect(view.ast(), view.result().source(), Some(view.result()));
    for (ordinal, (kind, value)) in records.into_iter().enumerate() {
        serde_json::to_writer(
            &mut *out,
            &json!({"version":1,"kind":"graph_record","workers":workers,"index":index,"ordinal":ordinal,"record_kind":kind,"value":value}),
        )?;
        out.write_all(b"\n")?;
    }
    Ok(())
}
