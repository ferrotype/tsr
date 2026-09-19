//! Current Rust config outputs; no pass flags or reference output are consumed.
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};
use ts_jsstring::{JsString, SourceText};
use ts_tsoptions::{ConfigValue as C, ParsedCommandLine, TsConfigSourceFile};
use ts_vfs::MemoryBuilder;
#[path = "host.rs"]
mod host;
use host::Host;
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        result.push(char::from(DIGITS[usize::from(b >> 4)]));
        result.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    result
}
fn unhex(value: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let value = value.as_str().ok_or("hex string expected")?;
    if value.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair)?;
            if pair
                .iter()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(b))
            {
                return Err("noncanonical hex".into());
            }
            Ok(u8::from_str_radix(s, 16)?)
        })
        .collect()
}
fn observed(value: &C) -> Value {
    match value {
        C::Null => json!({"kind":"null"}),
        C::EmptyStruct => json!({"kind":"empty_struct"}),
        C::Boolean(value) => json!({"kind":"boolean","value":value}),
        C::Number(value) => json!({"bits":format!("{:016x}",value.to_bits()),"kind":"number"}),
        C::String(value) => json!({"hex":hex(value.as_bytes()),"kind":"string"}),
        C::Array(values) => {
            json!({"kind":"array","nil":values.is_none(),"values":values.iter().flatten().map(observed).collect::<Vec<_>>()})
        }
        C::Object(values) => {
            // The tag's structural keys follow Go encoding/json's map order.
            // Source object property order is preserved by the entries array.
            json!({"entries":values.iter().map(|(key,value)|json!({"key":hex(key.as_bytes()),"value":observed(value)})).collect::<Vec<_>>(),"kind":"object"})
        }
        C::Integer(value) => json!({"kind":"integer","value":value}),
        C::Enum(value) => json!({"kind":"integer","value":value}),
    }
}
fn diagnostic(value: &ts_ast::Diagnostic, parsed: &ParsedCommandLine) -> Value {
    let file = value.file.map(|id| {
        let source = parsed
            .config_file
            .iter()
            .chain(&parsed.config_dependencies)
            .find(|file| file.root == id)
            .expect("diagnostic source owner retained");
        hex(source
            .file
            .view()
            .source_file(source.root)
            .expect("source file")
            .file_name())
    });
    json!({"file":file,"start":value.loc.pos(),"end":value.loc.end(),"code":value.code,"category":value.category,"message_key":hex(value.message_key.as_bytes()),"message_text":hex(value.message_text.as_bytes()),"source":hex(value.source.as_bytes()),"args":value.message_args.iter().map(|s|hex(s.as_bytes())).collect::<Vec<_>>(),"chain":value.message_chain.iter().map(|d|diagnostic(d,parsed)).collect::<Vec<_>>(),"related":value.related_information.iter().map(|d|diagnostic(d,parsed)).collect::<Vec<_>>(),"unnecessary":value.reports_unnecessary,"deprecated":value.reports_deprecated,"skipped_on_no_emit":value.skipped_on_no_emit})
}
pub fn observe_all(requests: &[Value]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    let mut ids = BTreeSet::new();
    for request in requests {
        let id = request["id"].as_str().ok_or("request ID")?;
        if id.is_empty() || !ids.insert(id.to_owned()) {
            return Err("duplicate/empty request ID".into());
        }
        if request.get("units").is_some() {
            rows.push(fixture(request)?);
            continue;
        }
        let cwd = request["cwd"].as_str().ok_or("cwd")?;
        let case_sensitive = request["case_sensitive"]
            .as_bool()
            .ok_or("case sensitive")?;
        let mut fs = MemoryBuilder::new(cwd.as_bytes(), case_sensitive);
        for (name, encoded) in request["files"].as_object().ok_or("files object")? {
            fs.insert_loaded(name.as_bytes(), unhex(encoded)?);
        }
        for (name, target) in request["symlinks"].as_object().ok_or("symlinks object")? {
            fs.insert_symlink(
                name.as_bytes(),
                target.as_str().ok_or("symlink target")?.as_bytes(),
            );
        }
        let host = Host {
            fs: Arc::new(fs.finish()),
            cwd: JsString::from_bytes(cwd.as_bytes()),
        };
        let filename = ts_tspath::absolute(
            request["file_name"]
                .as_str()
                .ok_or("config filename")?
                .as_bytes(),
            cwd.as_bytes(),
        );
        let source = TsConfigSourceFile::parse(
            JsString::from_bytes(filename.as_slice()),
            ts_tspath::to_path(&filename, cwd.as_bytes(), case_sensitive),
            SourceText::from_loaded_bytes(unhex(&request["text_hex"])?),
        );
        let existing = ts_core::CompilerOptions {
            run_external_code: if request["run_external_code"]
                .as_bool()
                .ok_or("run_external_code")?
            {
                ts_core::Tristate::TRUE
            } else {
                ts_core::Tristate::UNKNOWN
            },
            ..Default::default()
        };
        let parsed = ts_tsoptions::parse_json_source_file_config_file_content(
            source,
            &host,
            &ts_tspath::directory(&filename),
            &existing,
            &C::Null,
            &filename,
        )?;
        let options = observed(&ts_tsoptions::compiler_options_value(&parsed.options));
        rows.push(json!({"id":id,"options":options,"root_file_names":parsed.root_file_names.iter().map(|name|hex(name.as_bytes())).collect::<Vec<_>>(),"config_raw":observed(&parsed.raw),"config_diagnostics":parsed.errors.iter().map(|d|diagnostic(d,&parsed)).collect::<Vec<_>>(),"option_diagnostics":[],"compile_on_save":parsed.compile_on_save}));
    }
    Ok(rows)
}

fn fixture(request: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let cwd = request["cwd"].as_str().ok_or("fixture cwd")?.as_bytes();
    let config_cwd = request["config_cwd"]
        .as_str()
        .ok_or("config cwd")?
        .as_bytes();
    let units: Vec<_> = request["units"]
        .as_array()
        .ok_or("fixture units")?
        .iter()
        .map(|unit| {
            Ok((
                unit["name"].as_str().ok_or("unit name")?.as_bytes(),
                unhex(&unit["text_hex"])?,
            ))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    let settings: Vec<_> = request["settings"]
        .as_object()
        .ok_or("fixture settings")?
        .iter()
        .map(|(key, value)| {
            Ok((
                JsString::from_bytes(key.as_bytes()),
                JsString::from_bytes(value.as_str().ok_or("setting value")?.as_bytes()),
            ))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    let config_index = units.iter().position(|(name, _)| {
        matches!(
            ts_jsstring::helpers::to_lower_go(ts_tspath::base_name(name)).as_slice(),
            b"tsconfig.json" | b"jsconfig.json"
        )
    });
    let mut parsed = if let Some(index) = config_index {
        let mut fs = MemoryBuilder::new(config_cwd, true);
        for (name, text) in &units {
            fs.insert_loaded(name, text.clone());
        }
        for (name, target) in request["symlinks"].as_object().ok_or("symlinks")? {
            fs.insert_symlink(
                name.as_bytes(),
                target.as_str().ok_or("symlink target")?.as_bytes(),
            );
        }
        let host = Host {
            fs: Arc::new(fs.finish()),
            cwd: JsString::from_bytes(config_cwd),
        };
        let (name, text) = &units[index];
        let filename = ts_tspath::absolute(name, config_cwd);
        let source = TsConfigSourceFile::parse(
            JsString::from_bytes(filename.as_slice()),
            ts_tspath::to_path(&filename, config_cwd, true),
            SourceText::from_loaded_bytes(text.clone()),
        );
        let existing = ts_core::CompilerOptions {
            run_external_code: if request["run_external_code"]
                .as_bool()
                .ok_or("run external code")?
            {
                ts_core::Tristate::TRUE
            } else {
                ts_core::Tristate::UNKNOWN
            },
            ..Default::default()
        };
        ts_tsoptions::parse_json_source_file_config_file_content(
            source,
            &host,
            &ts_tspath::directory(&filename),
            &existing,
            &C::Null,
            &filename,
        )?
    } else {
        ParsedCommandLine::new(ts_core::CompilerOptions::default(), Vec::new())
    };
    let (_, option_diagnostics) = ts_tsoptions::apply_fixture_settings(
        &mut parsed.options,
        &settings,
        cwd,
        config_index.is_some(),
    )?;
    let units: Vec<_> = units
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != config_index)
        .map(|(_, unit)| unit)
        .collect();
    let last_only = config_index.is_none()
        && units.last().is_some_and(|(_, text)| {
            settings.iter().any(|(name, value)| {
                name.as_bytes() == b"noimplicitreferences" && !value.is_empty()
            }) || text.windows(8).any(|w| w == b"require(")
                || text.windows(14).any(|w| {
                    w.starts_with(b"reference")
                        && matches!(w[9], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
                        && w.ends_with(b"path")
                })
        });
    let mut roots = Vec::new();
    for (index, (name, _)) in units.iter().enumerate() {
        let name = ts_tspath::absolute(name, cwd);
        let selected = if config_index.is_some() {
            parsed
                .root_file_names
                .iter()
                .any(|root| root.as_bytes() == name)
        } else {
            !last_only || index + 1 == units.len()
        };
        if selected && !name.ends_with(b".json") && !name.ends_with(b".tsbuildinfo") {
            roots.push(hex(&name));
        }
    }
    Ok(
        json!({"id":request["id"],"options":observed(&ts_tsoptions::compiler_options_value(&parsed.options)),"root_file_names":roots,"config_raw":observed(&parsed.raw),"config_diagnostics":parsed.errors.iter().map(|d|diagnostic(d,&parsed)).collect::<Vec<_>>(),"option_diagnostics":option_diagnostics.iter().map(|d|diagnostic(d,&parsed)).collect::<Vec<_>>(),"compile_on_save":parsed.compile_on_save}),
    )
}
