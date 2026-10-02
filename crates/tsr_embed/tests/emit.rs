//! `Session::emit` against `CheckedProgram::emit` over the same program, with
//! a reference callback that records what each named output received.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tsr_arena::{Counters, Counts};
use tsr_checker::{CheckerRequest, Error};
use tsr_compiler::{CheckedProgram, WriteFileData};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_embed::{EmitOnly, EmitOptions, EmitOutput, FileCache, ProgramOptions, Session};
use tsr_jsstring::JsString;

/// `noLib` keeps the program self-contained; these are the global types the
/// checker otherwise reports missing.
const GLOBALS: &[u8] = b"interface Array<T> { length: number; [n: number]: T; }
interface Boolean {}
interface CallableFunction {}
interface Function {}
interface IArguments {}
interface NewableFunction {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
";
const UTIL: &[u8] = b"export function helper(n: number) {\n    return n + 1;\n}\n";
const MAIN: &[u8] = b"import { helper } from \"./util\";\n/** The answer. */\nexport const value = helper(41);\nexport interface Shape { kind: \"circle\" | \"square\"; size: number }\n";

fn options(edit: impl FnOnce(&mut CompilerOptions)) -> CompilerOptions {
    let mut options = CompilerOptions {
        no_lib: Tristate::TRUE,
        strict: Tristate::TRUE,
        target: ScriptTarget::ES2022,
        module: ModuleKind::ESNEXT,
        ..Default::default()
    };
    edit(&mut options);
    options
}

fn session(
    counters: &Counters,
    files: &[(&[u8], &[u8])],
    options: CompilerOptions,
    single_threaded: Tristate,
) -> Session {
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/src/globals.d.ts", GLOBALS);
    let mut roots = vec![JsString::from_bytes(b"/src/globals.d.ts".as_slice())];
    for &(name, text) in files {
        host.insert_loaded(name, text);
        roots.push(JsString::from_bytes(name));
    }
    Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(options, roots),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
            single_threaded,
        },
        &mut FileCache::new(),
        counters,
    )
    .unwrap()
}

fn program_session(counters: &Counters, options: CompilerOptions) -> Session {
    session(
        counters,
        &[(b"/src/main.ts", MAIN), (b"/src/util.ts", UTIL)],
        options,
        Tristate::UNKNOWN,
    )
}

/// What `CheckedProgram::emit` produces for `session`'s program on a fresh
/// pool: the result's skip flag and diagnostics, and its `EmittedFiles` with
/// the bytes the callback received for each (every name is written once).
type Reference = (bool, Vec<tsr_ast::Diagnostic>, Vec<(Vec<u8>, Vec<u8>)>);

fn reference(session: &Session, options: &EmitOptions<'_>) -> Reference {
    let program = session.program();
    let targets: Option<Vec<_>> = options.target_source_files.map(|files| {
        files
            .iter()
            .map(|file| {
                program
                    .files()
                    .iter()
                    .find(|candidate| std::ptr::eq(candidate.as_ref(), *file))
                    .expect("a file of the program")
                    .clone()
            })
            .collect()
    });
    let written = Mutex::new(HashMap::new());
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        let previous = written.lock().unwrap().insert(name.to_vec(), text.to_vec());
        assert!(previous.is_none(), "one write per name");
        Ok(())
    };
    let checked = CheckedProgram::new(program.clone(), &Counters::new(), None);
    let result = checked
        .emit(
            &CheckerRequest::default(),
            &tsr_compiler::EmitOptions {
                target_source_files: targets.as_deref(),
                emit_only: options.emit_only,
                force_emit: options.force_emit,
                write_file: Some(&write_file),
            },
        )
        .unwrap()
        .expect("an uncanceled emit");
    let mut written = written.into_inner().unwrap();
    let files: Vec<_> = result
        .emitted_files
        .iter()
        .map(|name| {
            let text = written.remove(name.as_bytes()).expect("a written file");
            (name.as_bytes().to_vec(), text)
        })
        .collect();
    assert!(written.is_empty(), "every written file is listed");
    (result.emit_skipped, result.diagnostics, files)
}

fn observed(output: &EmitOutput) -> Reference {
    (
        output.emit_skipped,
        output.diagnostics.clone(),
        output
            .files
            .iter()
            .map(|file| (file.name.as_bytes().to_vec(), file.text.clone()))
            .collect(),
    )
}

fn names(output: &EmitOutput) -> Vec<String> {
    output
        .files
        .iter()
        .map(|file| String::from_utf8_lossy(file.name.as_bytes()).into_owned())
        .collect()
}

fn text<'o>(output: &'o EmitOutput, name: &str) -> &'o str {
    let file = output
        .files
        .iter()
        .find(|file| file.name.as_bytes() == name.as_bytes())
        .unwrap_or_else(|| panic!("no {name} in {:?}", names(output)));
    std::str::from_utf8(&file.text).expect("UTF-8 output")
}

/// JavaScript, declarations and both maps for every file, in `EmittedFiles`
/// order (program order: the import before its importer), with the bytes `CheckedProgram::emit` writes, in both program
/// modes; a second emit and an emit after queries on the session's own
/// checker give the same bytes.
#[test]
fn emit_returns_the_bytes_checked_program_emit_writes() {
    for single_threaded in [Tristate::TRUE, Tristate::FALSE] {
        let counters = Counters::new();
        let session = session(
            &counters,
            &[(b"/src/main.ts", MAIN), (b"/src/util.ts", UTIL)],
            options(|options| {
                options.declaration = Tristate::TRUE;
                options.declaration_map = Tristate::TRUE;
                options.source_map = Tristate::TRUE;
            }),
            single_threaded,
        );
        let all = EmitOptions::default();
        let expected = reference(&session, &all);
        let output = session.emit(&all).unwrap();
        assert_eq!(observed(&output), expected);
        assert_eq!(
            names(&output),
            [
                "/src/util.js.map",
                "/src/util.js",
                "/src/util.d.ts.map",
                "/src/util.d.ts",
                "/src/main.js.map",
                "/src/main.js",
                "/src/main.d.ts.map",
                "/src/main.d.ts",
            ]
        );
        assert!(!output.emit_skipped);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert!(text(&output, "/src/main.js").contains("export const value = helper(41);"));
        assert!(text(&output, "/src/main.js").ends_with("//# sourceMappingURL=main.js.map"));
        assert!(text(&output, "/src/util.d.ts")
            .starts_with("export declare function helper(n: number): number;"));
        assert!(text(&output, "/src/main.d.ts").contains("export declare const value: number;"));

        assert_eq!(observed(&session.emit(&all).unwrap()), expected);
        {
            let mut operation = session.operation().unwrap();
            let file = session.program().file(b"/src/main.ts").unwrap();
            session
                .program()
                .semantic_diagnostics_with_checker(&mut operation, file)
                .unwrap();
        }
        assert_eq!(observed(&session.emit(&all).unwrap()), expected);
        drop(session);
        assert_eq!(counters.snapshot(), Counts::default());
    }
}

/// `EmitOnly` and target files select outputs as `CheckedProgram::emit` does.
#[test]
fn emit_only_and_target_files_select_outputs() {
    let counters = Counters::new();
    let session = program_session(
        &counters,
        options(|options| {
            options.declaration = Tristate::TRUE;
            options.source_map = Tristate::TRUE;
        }),
    );
    let util = session.program().file(b"/src/util.ts").unwrap();
    let main = session.program().file(b"/src/main.ts").unwrap();
    let cases: [(EmitOptions<'_>, &[&str]); 4] = [
        (
            EmitOptions {
                target_source_files: Some(&[util]),
                ..EmitOptions::default()
            },
            &["/src/util.js.map", "/src/util.js", "/src/util.d.ts"],
        ),
        (
            EmitOptions {
                emit_only: EmitOnly::Js,
                ..EmitOptions::default()
            },
            &[
                "/src/util.js.map",
                "/src/util.js",
                "/src/main.js.map",
                "/src/main.js",
            ],
        ),
        (
            EmitOptions {
                target_source_files: Some(&[util, main]),
                emit_only: EmitOnly::Dts,
                ..EmitOptions::default()
            },
            &["/src/util.d.ts", "/src/main.d.ts"],
        ),
        (
            EmitOptions {
                target_source_files: Some(&[]),
                ..EmitOptions::default()
            },
            &[],
        ),
    ];
    for (options, expected_names) in cases {
        let output = session.emit(&options).unwrap();
        assert_eq!(observed(&output), reference(&session, &options));
        assert_eq!(names(&output), expected_names);
    }
}

/// The pin's `HandleNoEmitOptions`: `noEmit` without target files gives an
/// empty result that is not skipped, with target files a skipped one, and
/// `ForceEmit` writes anyway.
#[test]
fn no_emit_writes_nothing_unless_forced() {
    let counters = Counters::new();
    let session = program_session(
        &counters,
        options(|options| options.no_emit = Tristate::TRUE),
    );
    let util = session.program().file(b"/src/util.ts").unwrap();

    let all = EmitOptions::default();
    let output = session.emit(&all).unwrap();
    assert_eq!(observed(&output), reference(&session, &all));
    assert!(!output.emit_skipped);
    assert!(output.files.is_empty());

    let targeted = EmitOptions {
        target_source_files: Some(&[util]),
        ..EmitOptions::default()
    };
    let output = session.emit(&targeted).unwrap();
    assert_eq!(observed(&output), reference(&session, &targeted));
    assert!(output.emit_skipped);
    assert!(output.files.is_empty());

    let forced = EmitOptions {
        target_source_files: Some(&[util]),
        force_emit: true,
        ..EmitOptions::default()
    };
    let output = session.emit(&forced).unwrap();
    assert_eq!(observed(&output), reference(&session, &forced));
    assert!(!output.emit_skipped);
    assert_eq!(names(&output), ["/src/util.js"]);
}

/// A declaration emit error is an emit diagnostic: it skips the file's
/// declaration output and leaves its JavaScript written.
#[test]
fn declaration_emit_diagnostics_are_returned() {
    let counters = Counters::new();
    let session = session(
        &counters,
        &[(
            b"/src/hidden.ts",
            b"export const Hidden = class {\n    private secret = 1;\n};\n",
        )],
        options(|options| options.declaration = Tristate::TRUE),
        Tristate::UNKNOWN,
    );
    let all = EmitOptions::default();
    let output = session.emit(&all).unwrap();
    assert_eq!(observed(&output), reference(&session, &all));
    assert!(output.emit_skipped);
    assert_eq!(names(&output), ["/src/hidden.js"]);
    let diagnostics: Vec<_> = output
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic
                    .message_args
                    .iter()
                    .map(|arg| String::from_utf8_lossy(arg.as_bytes()).into_owned())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(diagnostics, [(4094, vec!["secret".to_owned()])]);
}

/// `noEmitOnError` returns the program's diagnostics that skipped the emit.
#[test]
fn no_emit_on_error_returns_the_blocking_diagnostics() {
    let counters = Counters::new();
    let session = session(
        &counters,
        &[(b"/src/bad.ts", b"export const value: string = 1;\n")],
        options(|options| options.no_emit_on_error = Tristate::TRUE),
        Tristate::UNKNOWN,
    );
    let all = EmitOptions::default();
    let output = session.emit(&all).unwrap();
    assert_eq!(observed(&output), reference(&session, &all));
    assert!(output.emit_skipped);
    assert!(output.files.is_empty());
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        [2322]
    );
}

/// A file of another program is refused before anything is emitted.
#[test]
fn target_files_of_another_program_are_refused() {
    let counters = Counters::new();
    let session = program_session(&counters, options(|_| {}));
    let other = program_session(&counters, options(|_| {}));
    let foreign = other.program().file(b"/src/util.ts").unwrap();
    let result = session.emit(&EmitOptions {
        target_source_files: Some(&[foreign]),
        ..EmitOptions::default()
    });
    assert!(
        matches!(
            result,
            Err(tsr_compiler::Error::Ast(tsr_arena::Error::WrongOwner))
        ),
        "{result:?}"
    );
}

/// Retirement refuses later emits, before and after the first one.
#[test]
fn retirement_refuses_emit() {
    let counters = Counters::new();
    for emit_first in [false, true] {
        let session = program_session(&counters, options(|_| {}));
        if emit_first {
            session.emit(&EmitOptions::default()).unwrap();
        }
        session.retire();
        let result = session.emit(&EmitOptions::default());
        assert!(
            matches!(
                result,
                Err(tsr_compiler::Error::Checker(Error::Arena(
                    tsr_arena::Error::Retired
                )))
            ),
            "{result:?}"
        );
        drop(session);
    }
    assert_eq!(counters.snapshot(), Counts::default());
}
