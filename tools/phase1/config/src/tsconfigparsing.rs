//! Calls the production config APIs and writer; the shared Go bridge only
//! serializes their complete typed observations and assembles test headings.
use crate::{
    api::Outcome,
    configparse::Host,
    options_wire::{self, Wire},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_ast::Diagnostic;
use tsr_compiler::diagnostic_writer::{DiagnosticWriter, FormattingOptions};
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{
    self as o, ConfigValue, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile,
};
use tsr_vfs::MemoryBuilder;
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing input {key}"))
}
fn js(v: &[u8]) -> JsString {
    JsString::from_bytes(v)
}
fn diagnostics(writer: &DiagnosticWriter<'_>, errors: &[Diagnostic]) -> Result<Value, String> {
    fn one(writer: &DiagnosticWriter<'_>, d: &Diagnostic) -> Result<Value, String> {
        let file = d
            .file
            .map(|id| {
                writer
                    .source(id)
                    .map(|s| options_wire::hex(s.file_name()))
                    .map_err(|e| format!("diagnostic source: {e:?}"))
            })
            .transpose()?;
        Ok(
            json!({"code":d.code,"pos":d.loc.pos(),"end":d.loc.end(),"category":d.category,"file":file,"args":d.message_args.wire(),
            "chain":d.message_chain.iter().map(|d|one(writer,d)).collect::<Result<Vec<_>,_>>()?,
            "related":d.related_information.iter().map(|d|one(writer,d)).collect::<Result<Vec<_>,_>>()?}),
        )
    }
    Ok(json!(errors
        .iter()
        .map(|d| one(writer, d))
        .collect::<Result<Vec<_>, _>>()?))
}
fn acquisition(value: &o::TypeAcquisition) -> Value {
    json!(["struct",{
        "Enable":["int",value.enable.0],
        "Include":value.include.as_ref().map_or_else(||json!(["slice",null]),Wire::wire),
        "Exclude":value.exclude.as_ref().map_or_else(||json!(["slice",null]),Wire::wire),
        "DisableFilenameBasedTypeAcquisition":["int",value.disable_filename_based_type_acquisition.0],
    }])
}
fn row(input: &Value, api: &str) -> Result<Value, String> {
    let content = text(input, "json_text")?;
    let locale = match input.get("locale") {
        None => "",
        Some(Value::String(locale)) => locale,
        Some(_) => return Err("locale must be a string".into()),
    };
    if api == "jsonParse" && !locale.is_empty() {
        return Err("jsonParse baseline uses the default locale".into());
    }
    let existing = CompilerOptions {
        locale: js(locale.as_bytes()),
        ..Default::default()
    };
    let (parsed, errors, format) = if api == "jsonParse" {
        let result = o::parse_config_file_text_to_json(
            js(b"/apath/tsconfig.json"),
            js(b"/apath"),
            SourceText::from_bytes(content.as_bytes()),
        );
        let mut parsed = ParsedCommandLine::new(existing.clone(), vec![]);
        parsed.config_file = Some(result.source);
        parsed.raw = result.value;
        (
            parsed,
            result.diagnostics,
            FormattingOptions {
                new_line: b"\n".to_vec(),
                current_directory: b"/".to_vec(),
                case_sensitive: true,
                ..Default::default()
            },
        )
    } else {
        let name = text(input, "config_file_name")?.as_bytes();
        let cwd = text(input, "base_path")?.as_bytes();
        let base = if cwd.is_empty() {
            tsr_tspath::absolute(&tsr_tspath::directory(name), b"")
        } else {
            cwd.to_vec()
        };
        let absolute = tsr_tspath::absolute(name, &base);
        let mut builder = MemoryBuilder::new(if cwd.is_empty() { b"/" } else { cwd }, true);
        if let Some(files) = input["all_file_list"].as_object() {
            for (name, content) in files {
                builder.insert_physical(
                    name.as_bytes(),
                    content
                        .as_str()
                        .ok_or("non-string file content")?
                        .as_bytes()
                        .to_vec(),
                );
            }
        }
        builder.insert_physical(
            &tsr_tspath::combine(&base, &[name]),
            content.as_bytes().to_vec(),
        );
        let host = Host {
            fs: Arc::new(builder.finish()),
            cwd: js(cwd),
        };
        let path = tsr_tspath::to_path(name, &base, true);
        let parsed = match api {
            "json" => {
                let raw = o::parse_config_file_text_to_json(
                    js(&absolute),
                    path,
                    SourceText::from_bytes(content.as_bytes()),
                );
                o::parse_json_config_file_content(
                    raw.value,
                    &host,
                    &base,
                    &existing,
                    &absolute,
                    &[],
                )
            }
            "jsonSourceFile" => o::parse_json_source_file_config_file_content(
                TsConfigSourceFile::parse(
                    js(&absolute),
                    path,
                    SourceText::from_bytes(content.as_bytes()),
                ),
                &host,
                host.current_directory(),
                &existing,
                &ConfigValue::Null,
                &absolute,
            ),
            _ => return Err(format!("unknown config API {api}")),
        }
        .map_err(|e| format!("config parse: {e:?}"))?;
        let errors = parsed.errors.clone();
        (
            parsed,
            errors,
            FormattingOptions {
                new_line: b"\r\n".to_vec(),
                current_directory: base,
                case_sensitive: true,
                ..Default::default()
            },
        )
    };
    let mut writer = DiagnosticWriter::from_sources(
        &parsed,
        FormattingOptions {
            // The original 87 native envelopes use the default locale. The
            // integration requests explicitly pass a locale to the writer.
            locale: if locale.is_empty() {
                format.locale
            } else {
                parsed.locale().clone()
            },
            ..format
        },
    );
    let diagnostic_rows = diagnostics(&writer, &errors)?;
    let error_text = writer
        .format(&errors.iter().collect::<Vec<_>>(), true)
        .map_err(|e| format!("config diagnostics: {e:?}"))?;
    let mut row = json!({"raw":parsed.raw.wire(),"errors":options_wire::hex(&error_text),"diagnostics":diagnostic_rows});
    if api != "jsonParse" {
        row["compiler"] = options_wire::compiler(&parsed.options);
        row["acquisition"] = parsed
            .type_acquisition
            .as_ref()
            .map_or_else(|| json!(["nil"]), acquisition);
        row["files"] = parsed.root_file_names.wire();
    }
    Ok(row)
}
fn run(request: &Value) -> Result<Value, String> {
    let api = text(request, "api")?;
    let inputs = request["inputs"]
        .as_array()
        .ok_or("missing frozen config inputs")?;
    Ok(json!({"rows":inputs.iter().map(|input|row(input,api)).collect::<Result<Vec<_>,_>>()?}))
}
pub fn observe(request: &Value) -> Option<Outcome> {
    (crate::api::subject(request) == "tsconfigParsingBaseline").then(|| match run(request) {
        Ok(v) => Outcome::Observed(v),
        Err(e) => Outcome::Failed(e),
    })
}
