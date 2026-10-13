//! Public single-file entry points: expectations are the pinned transpile
//! contract, including differences from Session's whole-program emit.
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_core::{CancellationToken, CompilerOptions, ScriptTarget, Tristate};
use tsr_embed::{
    transpile_declaration, transpile_module, EmitOptions, FileCache, ProgramOptions, Session,
    TranspileError, TranspileOptions, TranspileOutput, TranspileRequest,
};
use tsr_jsstring::JsString;

fn whole_program(input: &[u8], options: CompilerOptions) -> Session {
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/module.ts", input);
    Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                vec![JsString::from_bytes(b"/module.ts".as_slice())],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap()
}

#[test]
fn module_transpilation_overrides_program_only_options_and_returns_source_map() {
    let input = b"export const x: number = 1;\n";
    let base = CompilerOptions {
        no_emit: Tristate::TRUE,
        declaration: Tristate::TRUE,
        emit_declaration_only: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        source_map: Tristate::TRUE,
        no_lib: Tristate::TRUE,
        ..Default::default()
    };
    let before = base.clone();
    let session = whole_program(input, base.clone());
    assert!(session
        .emit(&EmitOptions::default())
        .unwrap()
        .files
        .is_empty());

    // Public aliases are sufficient to name the complete operation signature.
    let result: Result<Option<TranspileOutput>, TranspileError> = transpile_module(
        &TranspileRequest::default(),
        input,
        &TranspileOptions {
            compiler_options: Some(&base),
            ..Default::default()
        },
    );
    let output = result.unwrap().unwrap();
    assert_eq!(
        output.output_text.as_bytes(),
        b"export const x = 1;\n//# sourceMappingURL=module.js.map"
    );
    assert_eq!(
        output.source_map_text.as_bytes(),
        br#"{"version":3,"file":"module.js","sourceRoot":"","sources":["module.ts"],"names":[],"mappings":"AAAA,MAAM,CAAC,MAAM,CAAC,GAAW,CAAC,CAAC"}"#
    );
    assert!(output.diagnostics.is_empty());
    assert_eq!(base, before);
    assert_eq!(output.program.files().len(), 1);
}

#[test]
fn declaration_transpilation_forces_declarations_and_reports_isolation_errors() {
    let input = b"export const sym = Symbol();\nexport function f() { return 1; }\n";
    let base = CompilerOptions {
        target: ScriptTarget::ES2015,
        no_emit: Tristate::TRUE,
        declaration: Tristate::FALSE,
        no_lib: Tristate::TRUE,
        ..Default::default()
    };
    let before = base.clone();
    let session = whole_program(input, base.clone());
    assert!(session
        .emit(&EmitOptions::default())
        .unwrap()
        .files
        .is_empty());

    let output = transpile_declaration(
        &TranspileRequest::default(),
        input,
        &TranspileOptions {
            compiler_options: Some(&base),
            // Emit diagnostics are included even with this left false.
            report_diagnostics: false,
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        output.output_text.as_bytes(),
        b"export declare const sym: unique symbol;\nexport declare function f(): number;\n"
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        [9010]
    );
    assert!(output.source_map_text.is_empty());
    assert_eq!(base, before);
    assert!(output.program.file(b"/lib/lib.es6.d.ts").is_some());
    assert_eq!(
        output.program.options().isolated_declarations,
        Tristate::TRUE
    );
}

#[test]
fn output_owns_text_and_diagnostic_files_after_input_and_options_are_dropped() {
    let output = {
        let input = b"export const a string = 1;\n".to_vec();
        let name = b"src/../bad.ts".to_vec();
        let base = CompilerOptions::default();
        transpile_module(
            &TranspileRequest::default(),
            &input,
            &TranspileOptions {
                compiler_options: Some(&base),
                file_name: &name,
                report_diagnostics: true,
            },
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(
        output.output_text.as_bytes(),
        b"export const a, string = 1;\n"
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        [1005]
    );
    let weak = Arc::downgrade(&output.program);
    let retained_program = output.program.clone();
    let diagnostics = output.diagnostics;
    drop(output.output_text);
    drop(output.source_map_text);
    drop(output.program);
    {
        let file = retained_program
            .file_of_node(diagnostics[0].file.unwrap())
            .unwrap();
        let source = file.bound().view().source_file().unwrap();
        assert_eq!(source.file_name(), b"/bad.ts");
        assert_eq!(source.text().as_bytes(), b"export const a string = 1;\n");
    }
    drop(retained_program);
    // Diagnostic identities alone keep no source storage alive.
    assert!(weak.upgrade().is_none());
    assert_eq!(diagnostics[0].code, 1005);
}

#[test]
fn cancellation_stops_module_but_not_forced_declaration_emit() {
    let token = CancellationToken::new();
    token.cancel();
    let request = TranspileRequest {
        cancellation: Some(token),
        ..Default::default()
    };
    assert!(transpile_module(
        &request,
        b"export const x = 1;\n",
        &TranspileOptions::default()
    )
    .unwrap()
    .is_none());
    let declaration = transpile_declaration(
        &request,
        b"export const x = 1;\n",
        &TranspileOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        declaration.output_text.as_bytes(),
        b"export declare const x = 1;\n"
    );
    // A subsequent independent request remains usable after cancellation.
    let module = transpile_module(
        &TranspileRequest::default(),
        b"export const x = 2;\n",
        &TranspileOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(module.output_text.as_bytes(), b"export const x = 2;\n");
}
