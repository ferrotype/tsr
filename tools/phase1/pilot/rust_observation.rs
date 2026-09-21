//! Phase 1 F0 pilot: the Rust side of the observation protocol.
//!
//! This driver calls real production APIs and reports what it finds. Where the
//! production entry point does not exist it emits an explicit `not_implemented`
//! row naming the missing operation, its intended signature and its production
//! home. It never emulates the missing algorithm to make a comparison run, and
//! it never reads an expected result.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{json, Map, Value};
use tsr_jsstring::JsString;
use tsr_vfs::MemoryBuilder;

/// A production entry point this phase still has to write. Each variant carries
/// the identity the gap queue needs; none of them is emulated here.
struct Missing {
    operation: &'static str,
    missing_identity: &'static str,
    go_authority: &'static str,
    intended_signature: &'static str,
    production_home: &'static str,
}

const MISSING: &[Missing] = &[
    Missing {
        operation: "tsoptions.parseCommandLine",
        missing_identity: "tsc/internal/tsoptions/commandlineparser.go:ParseCommandLine",
        go_authority: "tsc/internal/tsoptions/commandlineparser.go:ParseCommandLine",
        intended_signature:
            "pub fn parse_command_line(args: &[JsString], host: &dyn ParseConfigHost) -> ParsedCommandLine",
        production_home: "crates/tsr_tsoptions/src/command_line.rs (absent)",
    },
    Missing {
        operation: "tsoptions.parseBuildCommandLine",
        missing_identity: "tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine",
        go_authority: "tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine",
        intended_signature:
            "pub fn parse_build_command_line(args: &[JsString], host: &dyn ParseConfigHost) -> ParsedBuildCommandLine",
        production_home: "crates/tsr_tsoptions/src/command_line.rs (absent)",
    },
    Missing {
        operation: "json.marshalOrdered",
        missing_identity: "tsc/internal/json/json.go:Marshal",
        go_authority: "tsc/internal/json/json.go:Marshal",
        intended_signature: "pub fn marshal(value: &OrderedValue, options: MarshalOptions) -> Vec<u8>",
        production_home: "no dedicated Rust home; order-sensitive readers live in their consumers",
    },
    Missing {
        operation: "locale.selectTranslation",
        missing_identity: "tsc/internal/locale/locale.go:Parse",
        go_authority: "tsc/internal/locale/locale.go",
        intended_signature: "pub fn select(requested: &str) -> Option<Translation>",
        production_home: "no Rust home at this pin",
    },
];

fn strings(value: &Value, key: &str) -> Vec<JsString> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| JsString::from_bytes(s.as_bytes().to_vec()))
                .collect()
        })
        .unwrap_or_default()
}

/// `tsr_tsoptions::glob::read_directory` is the configuration-matching dialect.
/// It is deliberately not the `internal/glob` LSP grammar; the request carries
/// its dialect so the two can never dispatch to each other silently.
fn read_directory(request: &Value) -> Result<Value, Box<dyn Error>> {
    let dialect = request.get("dialect").and_then(Value::as_str).unwrap_or("");
    if dialect != "vfsmatch" {
        return Err(format!("request dialect {dialect:?} is not the configuration matcher").into());
    }
    let cwd = request
        .get("currentDirectory")
        .and_then(Value::as_str)
        .ok_or("request is missing currentDirectory")?;
    let case_sensitive = request
        .get("useCaseSensitiveFileNames")
        .and_then(Value::as_bool)
        .ok_or("request is missing useCaseSensitiveFileNames")?;
    let mut builder = MemoryBuilder::new(cwd.as_bytes(), case_sensitive);
    for entry in request
        .get("files")
        .and_then(Value::as_array)
        .ok_or("request is missing files")?
    {
        let path = entry.as_str().ok_or("file entry must be a string")?;
        builder.insert_physical(path.as_bytes(), Vec::new());
    }
    let snapshot = builder.finish();
    let path = request.get("path").and_then(Value::as_str).unwrap_or(cwd);
    let depth = request
        .get("depth")
        .and_then(Value::as_i64)
        .map_or(-1, |value| value as isize);
    let matched = tsr_tsoptions::glob::read_directory(
        &snapshot,
        cwd.as_bytes(),
        path.as_bytes(),
        &strings(request, "extensions"),
        &strings(request, "excludes"),
        &strings(request, "includes"),
        depth,
    )?;
    // Order is load-bearing: the native authority asserts an ordered list.
    let files: Vec<Value> = matched
        .iter()
        .map(|name| Value::String(String::from_utf8_lossy(name.as_bytes()).into_owned()))
        .collect();
    Ok(json!({ "files": files }))
}

fn observe(request: &Value) -> Map<String, Value> {
    let mut row = Map::new();
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    row.insert("case".into(), Value::String(case.to_owned()));
    row.insert("operation".into(), Value::String(operation.to_owned()));

    if let Some(missing) = MISSING.iter().find(|m| m.operation == operation) {
        row.insert("result".into(), Value::String("not_implemented".into()));
        row.insert(
            "missing_operation".into(),
            json!({
                "operation": missing.missing_identity,
                "go_authority": missing.go_authority,
                "intended_signature": missing.intended_signature,
                "production_home": missing.production_home,
            }),
        );
        return row;
    }

    let outcome = match operation {
        "vfsmatch.readDirectory" => read_directory(request),
        other => Err(format!("driver has no dispatch entry for operation {other:?}").into()),
    };
    match outcome {
        Ok(value) => {
            row.insert("result".into(), Value::String("observed".into()));
            row.insert("observation".into(), value);
        }
        Err(error) => {
            // A driver failure is never a semantic result; the comparison layer
            // treats harness_failed as invalidating, not as a non-match.
            row.insert("result".into(), Value::String("harness_failed".into()));
            row.insert("error".into(), Value::String(error.to_string()));
        }
    }
    row
}

pub fn run(input: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let raw = std::fs::read(input)?;
    let document: Value = serde_json::from_slice(&raw)?;
    let requests = document
        .get("requests")
        .and_then(Value::as_array)
        .ok_or("request document has no `requests` array")?;
    let mut seen = BTreeSet::new();
    let mut rows = Vec::with_capacity(requests.len());
    for request in requests {
        let case = request.get("case").and_then(Value::as_str).unwrap_or("");
        if !seen.insert(case.to_owned()) {
            return Err(format!("duplicate case id {case:?} in the request document").into());
        }
        rows.push(Value::Object(observe(request)));
    }
    let observations = json!({
        "version": 1,
        "family": document.get("family").cloned().unwrap_or(Value::Null),
        "requests_sha256": document.get("requests_sha256").cloned().unwrap_or(Value::Null),
        "observations": rows,
    });
    let mut bytes = serde_json::to_vec_pretty(&observations)?;
    bytes.push(b'\n');
    std::fs::write(output, bytes)?;
    Ok(())
}
