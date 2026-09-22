//! Production results for the shared, test-only Go baseline renderer. This
//! driver does not read expected output sections or reconstruct Go JSON bytes.
use crate::{
    api::Outcome,
    options_wire::{self, Wire},
};
use serde_json::{json, Value};
use std::sync::OnceLock;
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{
    OptionDeclaration, OptionKind, ParseConfigHost, ParsedCommandLine, COMPILER_OPTIONS,
};
use tsr_vfs::{FileSystem, MemoryBuilder, MemorySnapshot};

struct Host(MemorySnapshot);
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        &self.0
    }
    fn current_directory(&self) -> &[u8] {
        b"/phase1"
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported(
            "argv parser must not resolve modules",
        ))
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported(
            "argv parser must not resolve modules",
        ))
    }
}
fn declarations(kind: &str) -> Result<&'static [OptionDeclaration], String> {
    static STRING: OnceLock<Vec<OptionDeclaration>> = OnceLock::new();
    static NUMBER: OnceLock<Vec<OptionDeclaration>> = OnceLock::new();
    let (cell, kind) = match kind {
        "" => return Ok(COMPILER_OPTIONS),
        "string" => (&STRING, OptionKind::String),
        "number" => (&NUMBER, OptionKind::Number),
        other => return Err(format!("unknown synthetic declaration {other:?}")),
    };
    // createVerifyNullForNonNullIncluded appends this test-only declaration.
    // Descriptive metadata unused by parsing is absent from Rust declarations.
    Ok(cell.get_or_init(|| {
        let mut declarations = COMPILER_OPTIONS.to_vec();
        declarations.push(OptionDeclaration {
            name: "optionName",
            short_name: "",
            kind,
            is_file_path: false,
            is_tsconfig_only: true,
            is_command_line_only: false,
            enum_values: &[],
            deprecated_keys: &[],
            element: None,
            extra_validation: "",
            min_value: 0,
            allow_config_dir_template: false,
            preserve_falsy: false,
        });
        declarations
    }))
}
fn run(request: &Value) -> Result<Value, String> {
    let args = request
        .get("args")
        .and_then(Value::as_array)
        .ok_or("missing args input")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| JsString::from_bytes(s.as_bytes()))
                .ok_or("non-string arg")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let host = Host(MemoryBuilder::new(b"/phase1", true).finish());
    let (compiler, build, files, errors) = match request["operation"].as_str() {
        Some("tsoptions.parseCommandLineBaseline") => {
            let declarations = declarations(
                request["synthetic_option"]
                    .as_str()
                    .ok_or("missing synthetic_option")?,
            )?;
            let (options, files, errors) =
                tsr_tsoptions::parse_command_line_test_worker(&args, &host, declarations);
            (options.wire(), Value::Null, files, errors)
        }
        Some("tsoptions.parseBuildOptionsBaseline") => {
            let parsed = tsr_tsoptions::parse_build_command_line(&args, &host);
            (
                options_wire::compiler(&parsed.compiler_options),
                options_wire::build(&parsed.build_options),
                parsed.projects,
                parsed.errors,
            )
        }
        _ => return Err("unknown command-line baseline operation".into()),
    };
    let source = ParsedCommandLine::new(CompilerOptions::default(), vec![]);
    let mut writer = tsr_compiler::diagnostic_writer::DiagnosticWriter::from_sources(
        &source,
        tsr_compiler::diagnostic_writer::FormattingOptions::default(),
    );
    let error_text = writer
        .format(&errors.iter().collect::<Vec<_>>(), false)
        .map_err(|e| format!("diagnostic writer: {e:?}"))?;
    Ok(
        json!({"compiler":compiler,"build":build,"files":files.wire(),
            "errors":options_wire::hex(&error_text),
            "diagnostics":errors.iter().map(|d|json!([d.code,d.message_args.wire()])).collect::<Vec<_>>()
        }),
    )
}
pub fn observe(request: &Value) -> Option<Outcome> {
    (crate::api::subject(request) == "commandLineBaseline").then(|| match run(request) {
        Ok(v) => Outcome::Observed(v),
        Err(e) => Outcome::Failed(e),
    })
}
