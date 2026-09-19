//! Focused renderer protocol; diagnostics are inputs, not expected output text.
use crate::errors;
use crate::errors::hex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_ast::Diagnostic;
use tsr_compiler::{
    diagnostic_writer::{DiagnosticWriter, FormattingOptions},
    FileCache, Program, ProgramOptions,
};
use tsr_jsstring::JsString;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    scope: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    files: Vec<File>,
    inputs: Vec<usize>,
    diagnostics: Vec<Spec>,
    #[serde(default)]
    cwd: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name_hex: String,
    content_hex: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    file: Option<usize>,
    pos: i64,
    end: i64,
    code: i32,
    category: i32,
    #[serde(default)]
    text_hex: Option<String>,
    #[serde(default)]
    key: String,
    #[serde(default)]
    args_hex: Vec<String>,
    #[serde(default)]
    source_hex: String,
    #[serde(default)]
    chain: Vec<Spec>,
    #[serde(default)]
    related: Vec<Spec>,
}
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err("odd hex bytes".into());
    }
    fn digit(b: u8) -> Result<u8> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("noncanonical hex".into()),
        }
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| Ok((digit(p[0])? << 4) | digit(p[1])?))
        .collect()
}

fn diagnostic(spec: &Spec, ids: &[tsr_ast::NodeId]) -> Result<Diagnostic> {
    let mut d = Diagnostic::external(
        spec.file
            .map(|i| ids.get(i).copied().ok_or("diagnostic file index"))
            .transpose()?,
        tsr_core::TextRange::new(spec.pos, spec.end),
        JsString::from_bytes(unhex(&spec.source_hex)?),
        spec.category,
        spec.code,
        JsString::from_bytes(unhex(spec.text_hex.as_deref().unwrap_or(""))?),
    );
    d.message_key = JsString::from_bytes(spec.key.as_bytes());
    d.message_args = spec
        .args_hex
        .iter()
        .map(|s| Ok(JsString::from_bytes(unhex(s)?)))
        .collect::<Result<_>>()?;
    d.message_chain = spec
        .chain
        .iter()
        .map(|s| Ok(Arc::new(diagnostic(s, ids)?)))
        .collect::<Result<_>>()?;
    d.related_information = spec
        .related
        .iter()
        .map(|s| Ok(Arc::new(diagnostic(s, ids)?)))
        .collect::<Result<_>>()?;
    Ok(d)
}
fn run(case: &Case) -> Result<Value> {
    let files: Vec<_> = case
        .files
        .iter()
        .map(|f| Ok((unhex(&f.name_hex)?, unhex(&f.content_hex)?)))
        .collect::<Result<_>>()?;
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in &files {
        fs.insert_loaded(name, text.as_slice());
    }
    let counters = tsr_arena::Counters::new();
    let program = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    allow_non_ts_extensions: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                files
                    .iter()
                    .map(|(n, _)| JsString::from_bytes(n.as_slice()))
                    .collect(),
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/missing".as_slice()),
            skip_module_resolution: true,
        },
        &mut FileCache::new(),
        &counters,
    )?;
    let ids: Vec<_> = files
        .iter()
        .map(|(name, _)| {
            program
                .files()
                .iter()
                .find(|f| f.bound().view().source_file().unwrap().file_name() == name)
                .map(|file| file.source())
                .ok_or("unloaded fixture source")
        })
        .collect::<std::result::Result<_, _>>()?;
    let diagnostics: Vec<_> = case
        .diagnostics
        .iter()
        .map(|s| diagnostic(s, &ids))
        .collect::<Result<_>>()?;
    let mut writer = DiagnosticWriter::new(
        &program,
        FormattingOptions {
            new_line: b"\r\n".to_vec(),
            current_directory: case.cwd.as_bytes().to_vec(),
            ..Default::default()
        },
    );
    let refs: Vec<_> = diagnostics.iter().collect();
    let plain = writer.format(&refs, false)?;
    let pretty = writer.format(&refs, true)?;
    let summary = writer.error_summary(&refs)?;
    let inputs = case
        .inputs
        .iter()
        .map(|&i| {
            files
                .get(i)
                .map(|(name, content)| errors::InputFile { name, content })
                .ok_or("input file index")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(
        json!({"id":case.id,"state":"executed","plain_hex":hex(&plain),"pretty_hex":hex(&pretty),"summary_hex":hex(&summary),"errors_plain":errors::render(&program,&inputs,&diagnostics,false)?,"errors_pretty":errors::render(&program,&inputs,&diagnostics,true)?}),
    )
}
pub fn observe(request: &Value) -> Result<Value> {
    let request: Request = serde_json::from_value(request.clone())?;
    if request.version != 1 || request.scope != "diagnostic-writer-focused" {
        return Err("unknown diagnostic writer request".into());
    }
    let cases: Vec<_> = request
        .cases
        .iter()
        .map(
            |case| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(case))) {
                Ok(Ok(row)) => row,
                Ok(Err(e)) => json!({"id":case.id,"state":"failed","reason":e.to_string()}),
                Err(_) => json!({"id":case.id,"state":"failed","reason":"panic"}),
            },
        )
        .collect();
    Ok(json!({"version":1,"cases":cases}))
}
