//! Test-only construction of files and diagnostics for production display APIs.

use crate::api::{self, Outcome};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_ast::Diagnostic;
use tsr_core::TextRange;
use tsr_jsstring::JsString;

const SUBJECT: &str = "diagnosticWriter";
fn field<'a>(spec: &'a Value, name: &str) -> Option<&'a Value> {
    spec.get(name)
}

fn text_field(spec: &Value, name: &str) -> JsString {
    JsString::from_bytes(
        field(spec, name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
    )
}

fn int_field(spec: &Value, name: &str) -> i64 {
    field(spec, name)
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd-length hex payload {text:?}"));
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).map_err(|_| format!("bad hex {text:?}"))?;
            u8::from_str_radix(digits, 16).map_err(|_| format!("bad hex {text:?}"))
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The request's arguments, as UTF-8 under `args` or as hex under `args_hex`.
/// Exactly one of the two may be present, as on the native side: an argument
/// that can carry invalid bytes must not be allowed to arrive two ways.
fn arguments(spec: &Value) -> Result<Vec<JsString>, String> {
    let plain = field(spec, "args").and_then(Value::as_array);
    let encoded = field(spec, "args_hex").and_then(Value::as_array);
    match (plain, encoded) {
        (Some(_), Some(_)) => Err("a diagnostic spec carries both args and args_hex".into()),
        (Some(items), None) => Ok(items
            .iter()
            .map(|item| JsString::from_bytes(item.as_str().unwrap_or_default().as_bytes().to_vec()))
            .collect()),
        (None, Some(items)) => items
            .iter()
            .map(|item| {
                Ok(JsString::from_bytes(decode_hex(
                    item.as_str().unwrap_or_default(),
                )?))
            })
            .collect(),
        (None, None) => Ok(Vec::new()),
    }
}

fn nested(spec: &Value, name: &str, files: &Files) -> Result<Vec<Arc<Diagnostic>>, String> {
    let Some(items) = field(spec, name).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| Ok(Arc::new(build(item, files)?)))
        .collect()
}

/// Build one diagnostic from the request's spec, through the same two pinned
/// shapes the native probe uses: already-localized external text, or a message
/// key resolved out of the generated table. Nothing here formats a message; it
/// only assembles the input `flattened` is then asked about.
fn build(spec: &Value, files: &Files) -> Result<Diagnostic, String> {
    let name = field(spec, "file")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let file = if name.is_empty() {
        None
    } else {
        Some(files.root(name)?)
    };
    let loc = TextRange::new(int_field(spec, "pos"), int_field(spec, "end"));
    let code =
        i32::try_from(int_field(spec, "code")).map_err(|_| "code out of range".to_owned())?;
    let category = i32::try_from(int_field(spec, "category"))
        .map_err(|_| "category out of range".to_owned())?;
    let source = text_field(spec, "source");
    let message_text = text_field(spec, "message_text");
    let message_key = text_field(spec, "message_key");
    let chain = nested(spec, "chain", files)?;
    let related = nested(spec, "related", files)?;

    let mut diagnostic = if message_text.as_bytes().is_empty() {
        if message_key.as_bytes().is_empty() {
            return Err(
                "a diagnostic spec needs exactly one of message_text and message_key".into(),
            );
        }
        // The counterpart of ast.NewDiagnosticFromSerialized (ast/diagnostic.go:166):
        // `message` stays None so Localize resolves through the key, which is
        // what makes the message table -- not a pointer the harness chose --
        // the thing under comparison.
        Diagnostic {
            file,
            loc,
            code,
            category,
            source,
            message: None,
            message_text: JsString::default(),
            message_key,
            message_args: arguments(spec)?,
            message_chain: Vec::new(),
            related_information: Vec::new(),
            reports_unnecessary: false,
            reports_deprecated: false,
            skipped_on_no_emit: false,
            ad_hoc_message: None,
            repopulate_info: None,
        }
    } else {
        // The counterpart of ast.NewExternalDiagnostic (ast/diagnostic.go:247).
        Diagnostic::external(file, loc, source, category, code, message_text)
    };
    diagnostic.message_chain = chain;
    diagnostic.related_information = related;
    Ok(diagnostic)
}

use tsr_ast::{AstFile, NodeId, SourceFileRead};
use tsr_compiler::diagnostic_writer::{
    self as dw, AstDiagnostic, DiagnosticSources, DiagnosticWriter, File, FileKind,
    FormattingOptions,
};
#[derive(Default)]
struct Files {
    names: BTreeMap<String, usize>,
    owners: Vec<(AstFile, NodeId)>,
}
impl Files {
    fn root(&self, name: &str) -> Result<NodeId, String> {
        self.names
            .get(name)
            .map(|i| self.owners[*i].1)
            .ok_or_else(|| format!("no source named {name:?}"))
    }
    fn define(&mut self, action: &Value) -> Result<Value, String> {
        let file_name = JsString::from_bytes(bytes(action, "file_name")?);
        let mut parsed = tsr_parser::parse_source_file(
            tsr_jsstring::SourceText::from_loaded_bytes(bytes(action, "text")?),
            tsr_core::ScriptKind::TS,
            tsr_ast::SourceFileParseOptions {
                file_name: file_name.clone(),
                path: file_name.clone(),
                ..Default::default()
            },
        );
        let root = parsed.root();
        if ["content_mapper", "canonical", "original_text", "segments"]
            .iter()
            .any(|key| action.get(key).is_some() || action.get(format!("{key}_hex")).is_some())
        {
            let canonical = action
                .get("canonical")
                .and_then(Value::as_str)
                .map(|name| self.root(name))
                .transpose()?;
            if let Some(id) = canonical {
                let (owner, _) = self
                    .owners
                    .iter()
                    .find(|(_, root)| *root == id)
                    .expect("known source");
                parsed.builder_mut().retain_file(owner.clone());
            }
            let segments = action
                .get("segments")
                .map(|rows| -> Result<_, String> {
                    let mut segments = Vec::new();
                    for row in rows.as_array().ok_or("segments must be an array")? {
                        let row = row
                            .as_array()
                            .filter(|r| r.len() == 5)
                            .ok_or("segment needs five endpoints/kind")?;
                        let n = |i: usize| {
                            i32::try_from(row[i].as_i64().ok_or("segment value")?)
                                .map_err(|_| "segment outside i32")
                        };
                        segments.push(tsr_ast::SpanSegment {
                            virtual_start: n(0)?,
                            virtual_end: n(1)?,
                            original_start: n(2)?,
                            original_end: n(3)?,
                            kind: n(4)?,
                            features: 0,
                        });
                    }
                    Ok(tsr_ast::span_map::new(&segments))
                })
                .transpose()?;
            parsed
                .builder_mut()
                .source_file_mut(root)
                .map_err(|e| format!("source metadata: {e:?}"))?
                .set_content_mapper_info(tsr_ast::ContentMapperSourceFileInfo {
                    content_mapper: JsString::from_bytes(bytes(action, "content_mapper")?),
                    canonical_source_file: canonical,
                    virtual_file_name: file_name,
                    original_text: tsr_jsstring::SourceText::from_loaded_bytes(bytes(
                        action,
                        "original_text",
                    )?),
                    span_map: segments,
                    ..Default::default()
                });
        }
        let owner = parsed.publish_unbound();
        let source = owner
            .view()
            .source_file(root)
            .map_err(|e| format!("source: {e:?}"))?;
        let row = json!({"op":"define_file","file_name_hex":encode_hex(source.file_name()),"text_len":source.text().as_bytes().len(),"original_text_len":source.original_text().len(),"content_mapper_hex":encode_hex(source.content_mapper()),"has_span_map":source.span_map().is_some(),"has_canonical":source.canonical_source_file().is_some()});
        self.names
            .insert(api::action_str(action, "target").into(), self.owners.len());
        self.owners.push((owner, root));
        Ok(row)
    }
}
impl DiagnosticSources for Files {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>, tsr_compiler::Error> {
        let (owner, _) = self.owners.iter().find(|(_, root)| *root == id).ok_or(
            tsr_compiler::Error::Unsupported("foreign diagnostic source"),
        )?;
        Ok(owner.view().source_file(id)?)
    }
}
fn bytes(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    if let Some(value) = action.get(format!("{key}_hex")) {
        decode_hex(value.as_str().ok_or("hex field must be string")?)
    } else {
        Ok(api::action_str(action, key).as_bytes().to_vec())
    }
}
fn locale(action: &Value) -> Result<tsr_locale::Locale, String> {
    match action.get("locale") {
        None => Ok(tsr_locale::Locale::default()),
        Some(value) => {
            let value = value.as_str().ok_or("locale must be text")?;
            let (locale, valid) = tsr_locale::Locale::parse(value);
            if valid {
                Ok(locale)
            } else {
                Err(format!("invalid locale {value:?}"))
            }
        }
    }
}
fn options(action: &Value) -> Result<FormattingOptions, String> {
    Ok(FormattingOptions {
        locale: locale(action)?,
        new_line: api::action_str(action, "new_line").as_bytes().to_vec(),
        current_directory: api::action_str(action, "current_directory")
            .as_bytes()
            .to_vec(),
        case_sensitive: action["case_sensitive"].as_bool().unwrap_or_default(),
    })
}
fn file_kind(file: &File) -> &'static str {
    match file.kind() {
        FileKind::Source => "source_file",
        FileKind::Original => "original_text_file",
        FileKind::Renamed => "renamed_file",
    }
}
fn selected<'a>(
    built: &'a BTreeMap<String, Diagnostic>,
    action: &Value,
    key: &str,
) -> Result<Vec<&'a Diagnostic>, String> {
    action[key]
        .as_array()
        .ok_or_else(|| format!("{key} must be an array"))?
        .iter()
        .map(|n| {
            built
                .get(n.as_str().unwrap_or_default())
                .ok_or_else(|| format!("missing diagnostic {n}"))
        })
        .collect()
}
#[allow(
    clippy::too_many_lines,
    reason = "one arm per declared diagnostic display action"
)]
fn replay(request: &Value) -> Result<Value, String> {
    let mut built = BTreeMap::<String, Diagnostic>::new();
    let mut files = Files::default();
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        if op == "define_file" {
            rows.push(files.define(action)?);
            continue;
        }
        if op == "define_diagnostic" {
            let diagnostic = build(action.get("diagnostic").ok_or("diagnostic spec")?, &files)?;
            rows.push(json!({"op":op,"code":diagnostic.code,"category":diagnostic.category,"has_file":diagnostic.file.is_some(),"chain_len":diagnostic.message_chain.len(),"related_len":diagnostic.related_information.len()}));
            built.insert(api::action_str(action, "target").into(), diagnostic);
            continue;
        }
        let mut writer = DiagnosticWriter::from_sources(&files, options(action)?);
        let mut row = json!({"op":op});
        let target = api::action_str(action, "target");
        let diag = || {
            built
                .get(target)
                .ok_or_else(|| format!("no diagnostic named {target:?}"))
        };
        let error = |e: tsr_compiler::Error| format!("display: {e:?}");
        match op {
            "error_summary" => {
                let diagnostics = selected(&built, action, "targets")?;
                row["output_hex"] = json!(encode_hex(
                    &writer.error_summary(&diagnostics).map_err(error)?
                ));
            }
            "flatten" | "write_flattened_ast" => {
                row["text_hex"] = json!(encode_hex(
                    &writer
                        .flatten(diag()?, &bytes(action, "new_line")?)
                        .map_err(error)?
                ));
            }
            "astdiag_positions" => {
                let loc = writer.resolved_location(diag()?).map_err(error)?.loc;
                row["pos"] = json!(loc.pos());
                row["end"] = json!(loc.end());
                row["len"] = json!(loc.len());
            }
            "astdiag_source" | "astdiag_prefix" => {
                row["source_hex"] = json!(encode_hex(AstDiagnostic::new(diag()?).source()));
                if op == "astdiag_prefix" {
                    row["prefix_hex"] = json!(encode_hex(dw::prefix(diag()?)));
                }
            }
            "astdiag_message_chain" => {
                let chain = writer.message_chain(diag()?).map_err(error)?;
                row["len"] = json!(chain.len());
                row["entries"] = json!(chain
                    .iter()
                    .map(|d| Ok(json!([
                        d.code,
                        encode_hex(&dw::localized_with_locale(d, &locale(action)?).map_err(error)?)
                    ])))
                    .collect::<Result<Vec<Value>, String>>()?);
            }
            "astdiag_related" => {
                let related = AstDiagnostic::new(diag()?).related_information();
                row["len"] = json!(related.len());
                row["entries"] = json!(related
                    .iter()
                    .map(|d| Ok(json!([
                        d.0.code,
                        encode_hex(d.source()),
                        writer.file(d.0).map_err(error)?.is_some()
                    ])))
                    .collect::<Result<Vec<Value>, String>>()?);
            }
            "astdiag_file" => {
                let file = writer.file(diag()?).map_err(error)?;
                row["kind"] = json!(file.as_ref().map_or("nil", |file| file_kind(file)));
                row["file_name_hex"] = json!(file
                    .as_ref()
                    .map_or_else(String::new, |file| encode_hex(file.name())));
                row["text_len"] = json!(file.as_ref().map_or(-1, |file| file.text().len() as i64));
                row["line_map"] = json!(file.as_ref().map_or(&[][..], |file| file.line_map()));
            }
            "new_original_text_file" | "renamed_file" => {
                let source = writer.source(files.root(target)?).map_err(error)?;
                let name = JsString::from_bytes(bytes(action, "file_name")?);
                let file = if op == "new_original_text_file" {
                    File::original(&source, name)
                } else {
                    File::renamed(&source, name)
                };
                row["kind"] = json!(file_kind(&file));
                row["file_name_hex"] = json!(encode_hex(file.name()));
                row["text_hex"] = json!(encode_hex(file.text()));
                row["line_map"] = json!(file.line_map());
            }
            "category_format" => {
                let category = i32::try_from(int_field(action, "category"))
                    .map_err(|_| "category outside i32")?;
                match dw::color(category) {
                    Ok(color) => {
                        row["panicked"] = json!(false);
                        row["format_hex"] = json!(encode_hex(color));
                        row["panic_hex"] = json!("");
                        row["category_name_hex"] =
                            json!(encode_hex(dw::category(category).map_err(error)?));
                    }
                    Err(tsr_compiler::Error::Unsupported(message)) => {
                        row["panicked"] = json!(true);
                        row["format_hex"] = json!("");
                        row["panic_hex"] = json!(encode_hex(message.as_bytes()));
                        row["category_name_hex"] = json!("");
                    }
                    Err(e) => return Err(error(e)),
                }
            }
            "style_and_reset" => {
                let mut out = Vec::new();
                dw::styled(
                    &mut out,
                    &bytes(action, "text")?,
                    &bytes(action, "style")?,
                    true,
                );
                row["output_hex"] = json!(encode_hex(&out));
            }
            "pretty_path_nil_file" => {
                row["path_hex"] = json!(encode_hex(
                    &writer.pretty_path_for_errors(None, &[]).map_err(error)?
                ));
            }
            "pretty_path" => {
                let file = writer.source_file(files.root(target)?).map_err(error)?;
                let errors = selected(&built, action, "errors")?;
                row["path_hex"] = json!(encode_hex(
                    &writer
                        .pretty_path_for_errors(Some(&file), &errors)
                        .map_err(error)?
                ));
            }
            "tabular_errors" => {
                let groups = action["files"]
                    .as_array()
                    .ok_or("files must be rows")?
                    .iter()
                    .map(|entry| {
                        let names = entry
                            .as_array()
                            .filter(|e| e.len() >= 2)
                            .ok_or("tabular group needs file and diagnostic")?;
                        let file = writer
                            .source_file(files.root(names[0].as_str().ok_or("file name")?)?)
                            .map_err(error)?;
                        let errors = names[1..]
                            .iter()
                            .map(|name| {
                                built
                                    .get(name.as_str().unwrap_or_default())
                                    .ok_or_else(|| format!("missing diagnostic {name}"))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok((file, errors))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                row["output_hex"] =
                    json!(encode_hex(&writer.tabular_errors(&groups).map_err(error)?));
            }
            "status_with_color_and_time" | "status_and_time" => {
                row["output_hex"] = json!(encode_hex(
                    &writer
                        .status(
                            diag()?,
                            &bytes(action, "time")?,
                            op == "status_with_color_and_time"
                        )
                        .map_err(error)?
                ));
            }
            "try_clear_screen" => {
                let opts = tsr_core::CompilerOptions {
                    preserve_watch_output: tsr_core::Tristate(int_field(
                        action,
                        "preserve_watch_output",
                    ) as u8),
                    extended_diagnostics: tsr_core::Tristate(int_field(
                        action,
                        "extended_diagnostics",
                    ) as u8),
                    diagnostics: tsr_core::Tristate(int_field(action, "diagnostics") as u8),
                    ..Default::default()
                };
                let mut out = Vec::new();
                let cleared = dw::try_clear_screen(&mut out, diag()?, &opts);
                row["cleared"] = json!(cleared);
                row["output_hex"] = json!(encode_hex(&out));
            }
            "wrap_one" => {
                let input = diag()?;
                let wrapped = AstDiagnostic::new(input);
                row["not_nil"] = json!(true);
                row["same_pointer"] = json!(std::ptr::eq(input, wrapped.0));
            }
            "wrap_many" | "from_ast" | "to_diagnostics" => {
                let input = selected(&built, action, "targets")?;
                let wrapped = dw::wrap_diagnostics(&input);
                row["len"] = json!(wrapped.len());
                row["input_len"] = json!(input.len());
                row["same_pointers"] = if op == "to_diagnostics" {
                    json!(dw::to_diagnostics(&wrapped)
                        .iter()
                        .zip(&wrapped)
                        .map(|(d, original)| json!([true, std::ptr::eq(*d, original)]))
                        .collect::<Vec<_>>())
                } else {
                    json!(wrapped
                        .iter()
                        .zip(&input)
                        .map(|(d, original)| if op == "wrap_many" {
                            json!(std::ptr::eq(d.0, *original))
                        } else {
                            json!([true, std::ptr::eq(d.0, *original)])
                        })
                        .collect::<Vec<_>>())
                };
            }
            "compare" => {
                let get = |key| {
                    built
                        .get(api::action_str(action, key))
                        .map(AstDiagnostic::new)
                        .ok_or_else(|| format!("missing {key} diagnostic"))
                };
                row["forward_sign"] =
                    json!(writer.compare(get("left")?, get("right")?).map_err(error)? as i8);
                row["backward_sign"] =
                    json!(writer.compare(get("right")?, get("left")?).map_err(error)? as i8);
            }
            _ => return Err(format!("unrecognized diagnostic action {op:?}")),
        }
        rows.push(row);
    }
    Ok(api::ordered(rows))
}
pub fn observe(request: &Value) -> Option<Outcome> {
    (api::subject(request) == SUBJECT).then(|| match replay(request) {
        Ok(value) => Outcome::Observed(value),
        Err(e) => Outcome::Failed(e),
    })
}
