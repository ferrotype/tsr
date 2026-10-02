//! The transpile API's contracts, read from the pinned `transpile.go`: the
//! option overrides, the default file name, the barebones library of a
//! declaration transpilation, `ReportDiagnostics`, the source-map output and
//! cancellation. The pin has no Go test of these beyond `fs_test.go`; the
//! expected texts and codes here are the pinned package's own results for
//! these inputs, observed with an overlay probe of `internal/transpile`. The
//! runner's 41 baselines are the native witness
//! (`tests/phase3_transpile.rs`).
use super::*;
use tsr_core::{CancellationToken, ModuleKind, ScriptTarget};

fn module(input: &str, options: &Options<'_>) -> Output {
    transpile_module(&CheckerRequest::default(), input.as_bytes(), options)
        .expect("transpiles")
        .expect("not canceled")
}

fn declaration(input: &str, options: &Options<'_>) -> Output {
    transpile_declaration(&CheckerRequest::default(), input.as_bytes(), options)
        .expect("transpiles")
        .expect("not canceled")
}

/// The program's files: name, and whether it is the default library.
fn files(output: &Output) -> Vec<(String, bool)> {
    output
        .program
        .files()
        .iter()
        .map(|file| {
            let source = file.bound().view().source_file().expect("a source file");
            let name = String::from_utf8_lossy(source.file_name()).into_owned();
            let path = source.parse_options().path.clone();
            (name, output.program.is_lib(path.as_bytes()))
        })
        .collect()
}

fn codes(output: &Output) -> Vec<i32> {
    output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn transpile_module_erases_types() {
    let output = module("export const x: number = 1;\n", &Options::default());
    assert_eq!(output.output_text.as_bytes(), b"export const x = 1;\n");
    assert!(output.diagnostics.is_empty());
    assert!(output.source_map_text.is_empty());
}

/// Without a name the input is `/module.ts`, or `/module.tsx` when `jsx` is
/// set; a given name is made absolute under `/`. No library is read.
#[test]
fn the_input_file_name_defaults_by_jsx_and_is_rooted() {
    let output = module("let x = 1;", &Options::default());
    assert_eq!(files(&output), [("/module.ts".to_owned(), false)]);

    let jsx = CompilerOptions {
        jsx: JsxEmit::PRESERVE,
        ..CompilerOptions::default()
    };
    let output = module(
        "export const a = <div />;\n",
        &Options {
            compiler_options: Some(&jsx),
            ..Options::default()
        },
    );
    assert_eq!(files(&output), [("/module.tsx".to_owned(), false)]);
    assert_eq!(
        output.output_text.as_bytes(),
        b"export const a = <div />;\n"
    );

    let output = module(
        "let x = 1;",
        &Options {
            file_name: b"src/../a.ts",
            ..Options::default()
        },
    );
    assert_eq!(files(&output), [("/a.ts".to_owned(), false)]);
}

/// The options that do not apply to one file are cleared and the
/// transpilation's own are set, on a copy: the caller's options are
/// unchanged.
#[test]
fn transpile_module_overrides_the_options_on_a_copy() {
    let base = CompilerOptions {
        declaration: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        emit_declaration_only: Tristate::TRUE,
        isolated_declarations: Tristate::TRUE,
        no_emit: Tristate::TRUE,
        no_emit_on_error: Tristate::TRUE,
        composite: Tristate::TRUE,
        incremental: Tristate::TRUE,
        lib: Some(vec![JsString::from_bytes(b"es2015".as_slice())]),
        out_file: JsString::from_bytes(b"out.js".as_slice()),
        ts_build_info_file: JsString::from_bytes(b"a.tsbuildinfo".as_slice()),
        declaration_dir: JsString::from_bytes(b"types".as_slice()),
        types: Some(Vec::new()),
        root_dirs: Some(Vec::new()),
        allow_importing_ts_extensions: Tristate::TRUE,
        module: ModuleKind::COMMON_JS,
        ..CompilerOptions::default()
    };
    let before = base.clone();
    let output = module(
        "export const x: number = 1;\n",
        &Options {
            compiler_options: Some(&base),
            ..Options::default()
        },
    );
    assert_eq!(base, before);
    let options = output.program.options();
    for (name, value, expected) in [
        ("declaration", options.declaration, Tristate::FALSE),
        ("declarationMap", options.declaration_map, Tristate::FALSE),
        (
            "isolatedDeclarations",
            options.isolated_declarations,
            Tristate::FALSE,
        ),
        (
            "emitDeclarationOnly",
            options.emit_declaration_only,
            Tristate::UNKNOWN,
        ),
        ("noEmit", options.no_emit, Tristate::UNKNOWN),
        ("noEmitOnError", options.no_emit_on_error, Tristate::UNKNOWN),
        ("composite", options.composite, Tristate::UNKNOWN),
        ("incremental", options.incremental, Tristate::UNKNOWN),
        (
            "allowImportingTsExtensions",
            options.allow_importing_ts_extensions,
            Tristate::UNKNOWN,
        ),
        ("isolatedModules", options.isolated_modules, Tristate::TRUE),
        ("noCheck", options.no_check, Tristate::TRUE),
        ("noResolve", options.no_resolve, Tristate::TRUE),
        ("noLib", options.no_lib, Tristate::TRUE),
        (
            "suppressOutputPathCheck",
            options.suppress_output_path_check,
            Tristate::TRUE,
        ),
        (
            "allowNonTsExtensions",
            options.allow_non_ts_extensions,
            Tristate::TRUE,
        ),
    ] {
        assert_eq!(value, expected, "{name}");
    }
    assert!(options.lib.is_none() && options.types.is_none() && options.root_dirs.is_none());
    assert!(options.out_file.is_empty() && options.ts_build_info_file.is_empty());
    assert!(options.declaration_dir.is_empty());
    assert_eq!(options.module, ModuleKind::COMMON_JS);
    assert_eq!(
        output.output_text.as_bytes(),
        b"\"use strict\";\nObject.defineProperty(exports, \"__esModule\", { value: true });\nexports.x = void 0;\nexports.x = 1;\n"
    );
}

/// `verbatimModuleSyntax` makes `isolatedModules` redundant, so it is left
/// as given.
#[test]
fn verbatim_module_syntax_leaves_isolated_modules_unset() {
    let base = CompilerOptions {
        verbatim_module_syntax: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    let output = module(
        "export const x = 1;\n",
        &Options {
            compiler_options: Some(&base),
            ..Options::default()
        },
    );
    assert_eq!(output.program.options().isolated_modules, Tristate::UNKNOWN);
}

/// A declaration transpilation reads the barebones library under the
/// target's default library name in `/lib`, emits the declaration only, and
/// reports the isolated-declaration errors of emit whatever
/// `ReportDiagnostics` says.
#[test]
fn transpile_declaration_uses_the_barebones_library() {
    let base = CompilerOptions {
        target: ScriptTarget::ES2015,
        ..CompilerOptions::default()
    };
    let output = declaration(
        "export const sym = Symbol();\nexport function f() { return 1; }\n",
        &Options {
            compiler_options: Some(&base),
            ..Options::default()
        },
    );
    assert_eq!(
        files(&output),
        [
            ("/lib/lib.es6.d.ts".to_owned(), true),
            ("/module.ts".to_owned(), false)
        ]
    );
    let options = output.program.options();
    assert_eq!(options.declaration, Tristate::TRUE);
    assert_eq!(options.emit_declaration_only, Tristate::TRUE);
    assert_eq!(options.isolated_declarations, Tristate::TRUE);
    assert_eq!(options.no_lib, Tristate::FALSE);
    assert_eq!(
        output.output_text.as_bytes(),
        b"export declare const sym: unique symbol;\nexport declare function f(): number;\n"
    );
    // `Symbol()` needs an annotation; the literal return is inferred.
    assert_eq!(codes(&output), [9010]);
}

/// `ReportDiagnostics` adds the syntactic and option diagnostics; without
/// it a syntax error is not reported and the text is still emitted.
#[test]
fn report_diagnostics_adds_syntactic_diagnostics() {
    let input = "export const a string = 1;\n";
    let quiet = module(input, &Options::default());
    assert!(quiet.diagnostics.is_empty());
    assert_eq!(
        quiet.output_text.as_bytes(),
        b"export const a, string = 1;\n"
    );
    let reported = module(
        input,
        &Options {
            report_diagnostics: true,
            ..Options::default()
        },
    );
    assert_eq!(codes(&reported), [1005]);
    assert_eq!(reported.output_text, quiet.output_text);
}

/// A source map is returned beside the script; an inline map stays in the
/// script.
#[test]
fn source_maps_are_returned_beside_the_output() {
    let mapped = CompilerOptions {
        source_map: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    let output = module(
        "export const x: number = 1;\n",
        &Options {
            compiler_options: Some(&mapped),
            ..Options::default()
        },
    );
    assert_eq!(
        output.output_text.as_bytes(),
        b"export const x = 1;\n//# sourceMappingURL=module.js.map"
    );
    assert_eq!(
        output.source_map_text.as_bytes(),
        br#"{"version":3,"file":"module.js","sourceRoot":"","sources":["module.ts"],"names":[],"mappings":"AAAA,MAAM,CAAC,MAAM,CAAC,GAAW,CAAC,CAAC"}"#
    );

    let inline = CompilerOptions {
        inline_source_map: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    let output = module(
        "export const x: number = 1;\n",
        &Options {
            compiler_options: Some(&inline),
            ..Options::default()
        },
    );
    assert!(output.source_map_text.is_empty());
    assert_eq!(
        output.output_text.as_bytes(),
        b"export const x = 1;\n//# sourceMappingURL=data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoibW9kdWxlLmpzIiwic291cmNlUm9vdCI6IiIsInNvdXJjZXMiOlsibW9kdWxlLnRzIl0sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiJBQUFBLE1BQU0sQ0FBQyxNQUFNLENBQUMsR0FBVyxDQUFDLENBQUMifQ=="
    );
}

/// A module transpilation whose request is canceled before the emit
/// returns the pin's nil output. A declaration transpilation forces its emit,
/// which skips `Program.Emit`'s cancellation check, so it still emits.
#[test]
fn a_canceled_request_stops_only_the_module_transpilation() {
    let token = CancellationToken::new();
    token.cancel();
    let request = CheckerRequest {
        cancellation: Some(token),
        ..CheckerRequest::default()
    };
    let output = transpile_module(&request, b"export const x = 1;\n", &Options::default())
        .expect("transpiles");
    assert!(output.is_none());
    let output = transpile_declaration(&request, b"export const x = 1;\n", &Options::default())
        .expect("transpiles")
        .expect("a forced emit is not canceled");
    assert_eq!(
        output.output_text.as_bytes(),
        b"export declare const x = 1;\n"
    );
    assert!(output.diagnostics.is_empty());
}
