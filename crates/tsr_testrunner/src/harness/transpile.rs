//! Harness port of the pinned transpile runner
//! (`testrunner/transpile_runner.go`): for one configuration of a transpile
//! test, the runs it makes (`TranspileModule` unless the options emit
//! declarations only, then `TranspileDeclaration` when they declare), each
//! unit transpiled through `tsr_transpile`, and the baseline text `runKind`
//! composes from the units and their outputs, with the reported diagnostics
//! rendered by the ported error writer (`tools/s08/p5/errors.rs`).
//!
//! A configuration's inputs are the runner's: the test file's path, the
//! configuration's name, the units `makeUnitsFromTest` splits it into, the
//! compiler options `SetOptionsFromTestConfig` derives and the harness's
//! `ReportDiagnostics`. The naming of the configuration and its baselines is
//! computed here.
use super::errors;
use serde_json::{json, Value};
use tsr_ast::Diagnostic;
use tsr_checker::CheckerRequest;
use tsr_compiler::Program;
use tsr_core::CompilerOptions;
use tsr_transpile::{transpile_declaration, transpile_module, Options, Output};

/// One unit of a transpile test: its name and content.
pub struct Unit {
    pub name: Vec<u8>,
    pub content: Vec<u8>,
}

/// One configuration of a transpile test file, as the runner derives it.
pub struct Configuration<'a> {
    /// The test file's path under `tests/cases`, e.g.
    /// `transpile/declarationBasicSyntax.ts`.
    pub file: &'a [u8],
    /// The vary-by configuration's name (`declarationmap=true`), empty for
    /// the single configuration of a file without one.
    pub name: &'a [u8],
    pub units: &'a [Unit],
    pub options: &'a CompilerOptions,
    pub report_diagnostics: bool,
}

/// One unit's transpilation within a run.
pub struct UnitResult {
    pub unit: Vec<u8>,
    pub output_name: Vec<u8>,
    pub output: Output,
}

/// One `runKind`: the baseline it composes and the outputs behind it.
pub struct Run {
    pub declaration: bool,
    /// The baseline's path under the reference directory
    /// (`transpile/<configured name><extension>`).
    pub baseline_name: Vec<u8>,
    pub baseline: Vec<u8>,
    pub units: Vec<UnitResult>,
}

/// `baseline.NoContent`.
const NO_CONTENT: &[u8] = b"<no content>";

/// Harness port: tsc/internal/testrunner/transpile_runner.go:formatTranspileConfigurationName.
pub fn format_configuration_name(name: &[u8]) -> Vec<u8> {
    let name = replace_all(name, b"declarationmap=", b"declarationMap=");
    let name = replace_all(&name, b"inlinesourcemap=", b"inlineSourceMap=");
    replace_all(&name, b"sourcemap=", b"sourceMap=")
}

/// The configured name and the test file's extension, as `runTest` names
/// them.
pub fn configured_name(file: &[u8], configuration_name: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let extension = tsr_tspath::any_extension_from_path::<&[u8]>(file, &[], false).to_vec();
    let base_name = tsr_tspath::base_name(file);
    let just_name = base_name
        .strip_suffix(extension.as_slice())
        .unwrap_or(base_name);
    let mut configured_name = just_name.to_vec();
    if !configuration_name.is_empty() {
        configured_name.push(b'(');
        configured_name.extend_from_slice(&format_configuration_name(configuration_name));
        configured_name.push(b')');
    }
    (configured_name, extension)
}

/// The runs of one configuration, in the runner's order.
/// Harness port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runTest.
pub fn run_test(configuration: &Configuration<'_>) -> Result<Vec<Run>, String> {
    let (configured_name, extension) = configured_name(configuration.file, configuration.name);
    kinds(configuration.options)
        .into_iter()
        .map(|declaration| run_kind(&configured_name, &extension, configuration, declaration))
        .collect()
}

/// The kinds `runTest` runs for these options, in its order: the module
/// kind (`false`) unless `emitDeclarationOnly`, then the declaration kind
/// (`true`) when `declaration`.
pub fn kinds(options: &CompilerOptions) -> Vec<bool> {
    let mut kinds = Vec::new();
    if !options.emit_declaration_only.is_true() {
        kinds.push(false);
    }
    if options.declaration.is_true() {
        kinds.push(true);
    }
    kinds
}

/// One kind of the configuration's runs: the declaration kind when
/// `declaration`, else the module kind.
pub fn run_one(configuration: &Configuration<'_>, declaration: bool) -> Result<Run, String> {
    let (configured_name, extension) = configured_name(configuration.file, configuration.name);
    run_kind(&configured_name, &extension, configuration, declaration)
}

/// Harness port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runKind.
fn run_kind(
    configured_name: &[u8],
    extension: &[u8],
    configuration: &Configuration<'_>,
    declaration: bool,
) -> Result<Run, String> {
    let options = configuration.options;
    let mut result = Vec::new();
    for unit in configuration.units {
        append_section(&mut result, &unit.name, &unit.content);
    }

    let mut units = Vec::new();
    for unit in configuration.units {
        let transpile_options = Options {
            compiler_options: Some(options),
            file_name: &unit.name,
            report_diagnostics: configuration.report_diagnostics,
        };
        let request = CheckerRequest::default();
        let output = if declaration {
            transpile_declaration(&request, &unit.content, &transpile_options)
        } else {
            transpile_module(&request, &unit.content, &transpile_options)
        }
        .map_err(|error| format!("transpilation failed: {error:?}"))?
        .ok_or("transpilation was canceled")?;

        let mut output_extension =
            tsr_tsoptions::output_paths::get_output_extension(&unit.name, options.jsx).to_vec();
        if declaration {
            output_extension = tsr_tspath::declaration_emit_extension_for_path(&unit.name);
        }
        let output_file_name = tsr_tspath::change_extension(&unit.name, &output_extension);
        append_section(
            &mut result,
            &output_file_name,
            output.output_text.as_bytes(),
        );
        if !output.source_map_text.is_empty() {
            let mut map_name = output_file_name.clone();
            map_name.extend_from_slice(b".map");
            append_section(&mut result, &map_name, output.source_map_text.as_bytes());
        }
        if !output.diagnostics.is_empty() {
            result.extend_from_slice(b"\r\n\r\n//// [Diagnostics reported]\r\n");
            let mut diagnostic_file_name = unit.name.clone();
            if let Some(file) = output.diagnostics[0].file {
                diagnostic_file_name = file_name(&output.program, file)?;
            }
            let error_baseline = errors::render(
                &output.program,
                &[errors::InputFile {
                    name: &diagnostic_file_name,
                    content: &unit.content,
                }],
                &output.diagnostics,
                options.pretty.is_true(),
            )
            .map_err(|error| format!("error baseline failed: {error}"))?;
            let error_baseline = unhex(
                error_baseline["text_hex"]
                    .as_str()
                    .ok_or("the error baseline has no content")?,
            )?;
            result.extend_from_slice(&replace_all(
                &error_baseline,
                &diagnostic_file_name,
                &unit.name,
            ));
            if !result.ends_with(b"\n") {
                result.extend_from_slice(b"\r\n");
            }
        }
        units.push(UnitResult {
            unit: unit.name.clone(),
            output_name: output_file_name,
            output,
        });
    }

    let mut configured_file = configured_name.to_vec();
    configured_file.extend_from_slice(extension);
    let mut baseline_extension =
        tsr_tsoptions::output_paths::get_output_extension(&configured_file, options.jsx).to_vec();
    if declaration {
        baseline_extension = tsr_tspath::declaration_emit_extension_for_path(&configured_file);
    }
    let mut baseline_name = b"transpile/".to_vec();
    baseline_name.extend_from_slice(configured_name);
    baseline_name.extend_from_slice(&baseline_extension);
    Ok(Run {
        declaration,
        baseline_name,
        baseline: result,
        units,
    })
}

/// Harness port: tsc/internal/testrunner/transpile_runner.go:appendTranspileSection.
fn append_section(result: &mut Vec<u8>, file_name: &[u8], content: &[u8]) {
    result.extend_from_slice(b"//// [");
    result.extend_from_slice(file_name);
    result.extend_from_slice(b"] ////\r\n");
    result.extend_from_slice(content);
    if !content.ends_with(b"\n") {
        result.extend_from_slice(b"\r\n");
    }
}

/// `strings.ReplaceAll`.
fn replace_all(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return text.to_vec();
    }
    let mut output = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.windows(from.len()).position(|window| window == from) {
        output.extend_from_slice(&rest[..index]);
        output.extend_from_slice(to);
        rest = &rest[index + from.len()..];
    }
    output.extend_from_slice(rest);
    output
}

/// The name of the program file `file`.
fn file_name(program: &Program, file: tsr_arena::NodeId) -> Result<Vec<u8>, String> {
    let source = program
        .files()
        .iter()
        .find(|candidate| candidate.source() == file)
        .ok_or("a diagnostic names a file outside the transpilation's program")?;
    let view = source.bound().view();
    let source_file = view
        .source_file()
        .map_err(|error| format!("unreadable source file: {error:?}"))?;
    Ok(source_file.file_name().to_vec())
}

pub fn hex(raw: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(raw.len() * 2);
    for byte in raw {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 15)]));
    }
    text
}

pub fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|error| error.to_string()))
        .collect()
}

/// A diagnostic in the native capture's shape (`phase3Diagnostics`).
pub fn diagnostic(program: &Program, diagnostic: &Diagnostic) -> Result<Value, String> {
    let file = match diagnostic.file {
        Some(file) => Value::String(hex(&file_name(program, file)?)),
        None => Value::Null,
    };
    let chain = diagnostic
        .message_chain
        .iter()
        .map(|item| self::diagnostic(program, item))
        .collect::<Result<Vec<_>, _>>()?;
    let related = diagnostic
        .related_information
        .iter()
        .map(|item| self::diagnostic(program, item))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "file_hex": file, "pos": diagnostic.loc.pos(), "end": diagnostic.loc.end(),
        "code": diagnostic.code, "category": diagnostic.category,
        "key_hex": hex(diagnostic.message_key.as_bytes()),
        "text_hex": hex(diagnostic.message_text.as_bytes()),
        "args_hex": diagnostic.message_args.iter().map(|arg| hex(arg.as_bytes())).collect::<Vec<_>>(),
        "chain": chain, "related": related,
    }))
}

/// A run in the native capture's shape: the baseline and each unit's
/// output, source map and diagnostics.
pub fn run_value(run: &Run) -> Result<Value, String> {
    let units = run
        .units
        .iter()
        .map(|unit| {
            let diagnostics = unit
                .output
                .diagnostics
                .iter()
                .map(|item| diagnostic(&unit.output.program, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({
                "unit_hex": hex(&unit.unit), "output_name_hex": hex(&unit.output_name),
                "output_hex": hex(unit.output.output_text.as_bytes()),
                "source_map_hex": hex(unit.output.source_map_text.as_bytes()),
                "diagnostics": diagnostics,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let name = String::from_utf8_lossy(&run.baseline_name).into_owned();
    let baseline = if run.baseline == NO_CONTENT {
        json!({"state": "no_content", "name": name})
    } else {
        json!({"state": "content", "name": name, "text_hex": hex(&run.baseline)})
    };
    Ok(json!({"declaration": run.declaration, "baseline": baseline, "units": units}))
}
