//! Phase 3 T8 contracts (docs/PHASE3-plan.md, sections 4 and 5): what
//! `CheckedProgram::emit` (`Program.Emit`) guarantees around its work group:
//! panic retirement, cancellation before emit, determinism across runs and
//! modes, results that own no arena, and bounded workers.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;
use support::{files, Mode, Recorder, LIB};
use tsr_arena::{Counters, Counts, NodeId};
use tsr_checker::CheckerRequest;
use tsr_compiler::{
    emitter, CheckedProgram, EmitOptions, EmitResult, Program, ProgramFile, WriteFileData,
};
use tsr_core::{CancellationToken, CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_transformers::{TransformOptions, Transformer};

/// A program of `count` modules, each importing a shared base and the
/// module before it, with enums, namespaces, classes with parameter
/// properties and inferred declaration types, and one file whose
/// declaration emit reports that a type cannot be named (TS4023, which also
/// blocks that file's declaration output).
fn modules(count: usize) -> Vec<(String, String)> {
    let mut program = files(&[
        ("/lib.d.ts", LIB),
        (
            "/base.ts",
            "export class Base { kind = \"base\"; }\nexport function scale(n: number): number { return n * 2; }\nclass Secret { private hidden = 1; }\nexport function secret() { return new Secret(); }\n",
        ),
    ]);
    for i in 0..count {
        let previous = if i == 0 {
            String::new()
        } else {
            format!("import {{ value{} }} from \"./m{}\";\n", i - 1, i - 1)
        };
        let base_value = if i == 0 {
            "0".to_owned()
        } else {
            format!("value{}", i - 1)
        };
        program.push((
            format!("/m{i}.ts"),
            format!(
                "import {{ Base, scale }} from \"./base\";\n{previous}\
                 export enum Kind{i} {{ A = {i}, B = A + 1 }}\n\
                 export namespace Space{i} {{ export const tag = \"m{i}\"; }}\n\
                 export class Item{i} extends Base {{ constructor(public readonly id: number) {{ super(); }} size(): number {{ return scale(this.id) + Kind{i}.B; }} }}\n\
                 export const value{i}: number = {base_value} + {i};\n\
                 export function make{i}(n: number) {{ return new Item{i}(n); }}\n"
            ),
        ));
    }
    program.push((
        "/unnamed.ts".to_owned(),
        "import { secret } from \"./base\";\nexport const leaked = secret();\n".to_owned(),
    ));
    program
}

fn options() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2017,
        module: ModuleKind::COMMON_JS,
        declaration: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        source_map: Tristate::TRUE,
        ..CompilerOptions::default()
    }
}

fn is_retired<T>(result: &Result<T, tsr_compiler::Error>) -> bool {
    matches!(
        result,
        Err(tsr_compiler::Error::Checker(tsr_checker::Error::Arena(
            tsr_arena::Error::Retired
        )))
    )
}

/// `getScriptTransformers`, except that it panics for `/m3.ts`.
fn panicking_transformers<'t>(
    program: &Program,
    opts: &TransformOptions<'t>,
    source_file: NodeId,
) -> Result<Vec<Transformer<'t>>, tsr_arena::Error> {
    let injected = program.file(b"/m3.ts").map(ProgramFile::source) == Some(source_file);
    assert!(!injected, "injected panic while transforming /m3.ts");
    emitter::get_script_transformers(program, opts, source_file)
}

/// A panic during one file's emit retires the program's checker generation
/// and fails the group (ADR 0012; the pin has no recovery, a Go panic ends
/// the process): the file's emit holds its checker's operation (`newEmitHost`
/// takes the file's checker until the emit returns), so unwinding through it
/// retires the pool's generation. The panic reaches `emit`'s caller with no
/// emit result (concurrently once the group's other functions finish,
/// single-threaded at once, before the functions still queued run); from
/// then on the program's checkers refuse every operation, so a second emit
/// and the program's diagnostics fail with `Retired` instead of serving a
/// checker that may hold half-done state; and a fresh program pool over the
/// same program emits what a pool that never panicked emits. Both injection
/// points (the caller's write callback, which runs while the file is
/// printed, and a script transformer) and both modes.
#[test]
fn a_panic_during_one_files_emit_retires_the_generation_and_fails_the_group() {
    let program_files = modules(6);
    for mode in Mode::BOTH {
        let (reference, _) = support::checked(&program_files, &options(), mode);
        let expected = support::emit_all(&reference);
        let program = reference.program().clone();
        for injection in ["write", "transform"] {
            let counters = Counters::new();
            let checked = CheckedProgram::new(program.clone(), &counters, None);
            let recorder = Recorder::default();
            let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
                let injected = injection == "write" && name == b"/m3.js";
                assert!(!injected, "injected panic while writing /m3.js");
                recorder.write(name, text);
                Ok(())
            };
            let emit_options = EmitOptions {
                write_file: Some(&write_file),
                ..EmitOptions::default()
            };
            let request = CheckerRequest::default();
            let panic = catch_unwind(AssertUnwindSafe(|| {
                if injection == "write" {
                    checked.emit(&request, &emit_options)
                } else {
                    checked.emit_with_script_transformers(
                        &request,
                        &emit_options,
                        &panicking_transformers,
                    )
                }
            }))
            .expect_err("the injected panic reaches the caller");
            assert!(
                support::panic_text(&*panic).starts_with("injected panic while "),
                "{mode:?} {injection}: {}",
                support::panic_text(&*panic)
            );
            let pool = checked
                .compiler_checker_pool()
                .expect("the compiler's pool");
            assert_eq!(
                pool.generation().validate(),
                Err(tsr_arena::Error::Retired),
                "{mode:?} {injection}"
            );
            for owner in pool.checkers().expect("the pool's checkers") {
                assert!(matches!(
                    owner.operation(),
                    Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
                ));
            }
            assert!(
                is_retired(&checked.emit(&request, &emit_options)),
                "{mode:?} {injection}: a second emit"
            );
            assert!(is_retired(&checked.semantic_diagnostics(&request, None)));
            let fresh = CheckedProgram::new(program.clone(), &counters, None);
            assert_eq!(support::emit_all(&fresh), expected, "{mode:?} {injection}");
        }
    }
}

/// A program whose one source file has a semantic error.
fn erroneous() -> Vec<(String, String)> {
    files(&[
        ("/lib.d.ts", LIB),
        ("/a.ts", "export const x: number = \"text\";\n"),
    ])
}

/// What `CheckedProgram::emit` returned, and the files it wrote.
type Emitted = (
    Result<Option<EmitResult>, tsr_compiler::Error>,
    Vec<(String, Vec<u8>)>,
);

/// What `CheckedProgram::emit` returned and wrote under `request`.
fn emit_with(checked: &CheckedProgram, request: &CheckerRequest, force_emit: bool) -> Emitted {
    let recorder = Recorder::default();
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        recorder.write(name, text);
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        force_emit,
        ..EmitOptions::default()
    };
    let result = checked.emit(request, &options);
    (result, recorder.take())
}

/// Cancellation before emit (`Program.Emit`: after `HandleNoEmitOptions`,
/// `if result != nil || ctx.Err() != nil { return result }`): a canceled
/// request emits nothing and returns no result; under `noEmit` it returns
/// `HandleNoEmitOptions`' empty result; `forceEmit` skips that step, so the
/// pin never consults the context there and the canceled request emits.
/// Under `noEmitOnError` the canceled request's checks are canceled and the
/// collection then asks the canceled checkers for their global diagnostics
/// again (`GetDiagnosticsOfAnyProgram`), which they refuse: the pin panics
/// there (`checkNotCanceled`, "Checker was previously cancelled") and the
/// port returns that refusal as an error, so nothing is written. Uncanceled,
/// the same programs emit, skip with the error, and skip silently. Both
/// modes.
#[test]
fn a_canceled_request_emits_nothing() {
    let canceled = CancellationToken::new();
    canceled.cancel();
    let canceled = CheckerRequest {
        cancellation: Some(canceled),
        ..CheckerRequest::default()
    };
    let live = CheckerRequest::default();
    for mode in Mode::BOTH {
        let load = |options: CompilerOptions| support::checked(&erroneous(), &options, mode).0;
        let emit = |options: CompilerOptions, request: &CheckerRequest, force: bool| {
            let (result, written) = emit_with(&load(options), request, force);
            (result.expect("the emit succeeds"), written)
        };

        let (result, written) = emit(CompilerOptions::default(), &canceled, false);
        assert!(result.is_none() && written.is_empty(), "{mode:?}");
        let (result, written) = emit(CompilerOptions::default(), &live, false);
        assert!(!result.expect("a result").emit_skipped, "{mode:?}");
        assert_eq!(written.len(), 1, "{mode:?}");

        let no_emit = || CompilerOptions {
            no_emit: Tristate::TRUE,
            ..CompilerOptions::default()
        };
        for request in [&canceled, &live] {
            let (result, written) = emit(no_emit(), request, false);
            let result = result.expect("HandleNoEmitOptions' result");
            assert!(!result.emit_skipped && result.emitted_files.is_empty() && written.is_empty());
        }

        let (result, written) = emit(CompilerOptions::default(), &canceled, true);
        assert!(!result.expect("a forced result").emit_skipped, "{mode:?}");
        assert_eq!(written.len(), 1, "{mode:?}");

        let no_emit_on_error = || CompilerOptions {
            no_emit_on_error: Tristate::TRUE,
            ..CompilerOptions::default()
        };
        let (result, written) = emit(no_emit_on_error(), &live, false);
        let result = result.expect("a result");
        assert!(result.emit_skipped && written.is_empty(), "{mode:?}");
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            vec![2322],
            "{mode:?}"
        );
        let (result, written) = emit_with(&load(no_emit_on_error()), &canceled, false);
        assert!(
            matches!(
                result,
                Err(tsr_compiler::Error::Checker(
                    tsr_checker::Error::PreviouslyCanceled
                ))
            ),
            "{mode:?}: {result:?}"
        );
        assert!(written.is_empty(), "{mode:?}");
    }
}

/// The emit is deterministic (plan section 5: "two runs byte-identical; the
/// parallel and single-threaded emits identical"): over a program of twelve
/// modules with cross-file types, source maps, declaration maps and a
/// declaration diagnostic, two emits of one program, and emits of fresh
/// loads, in the single-threaded and the concurrent mode (four checkers, the
/// files' emits on the work group's threads), all produce the same written
/// bytes, `EmittedFiles` in the same order, the same diagnostics, the same
/// `EmitSkipped` and the same source maps. `EmittedFiles` and `SourceMaps`
/// follow the program's file order (`CombineEmitResults` in input order),
/// though the single-threaded group emits the last file first.
#[test]
fn two_runs_and_both_modes_emit_the_same_bytes() {
    let program_files = modules(12);
    let (single, _) = support::checked(&program_files, &options(), Mode::Single);
    let first = support::emit_all(&single);
    assert_eq!(
        support::emit_all(&single),
        first,
        "a second single-threaded run"
    );
    let (fresh, _) = support::checked(&program_files, &options(), Mode::Single);
    assert_eq!(
        support::emit_all(&fresh),
        first,
        "a fresh single-threaded load"
    );
    for run in 0..2 {
        let (concurrent, _) = support::checked(&program_files, &options(), Mode::Concurrent);
        assert_eq!(
            concurrent
                .compiler_checker_pool()
                .expect("the compiler's pool")
                .checker_count(),
            4
        );
        let observed = support::emit_all(&concurrent);
        assert_eq!(observed, first, "concurrent load {run}");
        assert_eq!(
            support::emit_all(&concurrent),
            first,
            "concurrent load {run}, again"
        );
    }

    assert!(
        first.emit_skipped,
        "the unnamed type blocks one declaration file"
    );
    assert_eq!(first.diagnostics.len(), 1, "{:?}", first.diagnostics);
    assert!(
        first.diagnostics[0].starts_with("/unnamed.ts:"),
        "{:?}",
        first.diagnostics
    );
    let stems: Vec<&str> = program_files
        .iter()
        .filter(|(name, _)| !name.ends_with(".d.ts"))
        .map(|(name, _)| name.trim_end_matches(".ts"))
        .collect();
    let mut expected = Vec::new();
    for stem in &stems {
        expected.push(format!("{stem}.js.map"));
        expected.push(format!("{stem}.js"));
        if *stem != "/unnamed" {
            expected.push(format!("{stem}.d.ts.map"));
            expected.push(format!("{stem}.d.ts"));
        }
    }
    assert_eq!(first.emitted_files, expected);
    let maps: Vec<&str> = first
        .source_maps
        .iter()
        .map(|(generated, _, _)| generated.as_str())
        .collect();
    let expected_maps: Vec<String> = expected
        .iter()
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_none_or(|e| e != "map")
        })
        .cloned()
        .collect();
    assert_eq!(maps, expected_maps);
}

/// The emit's results own no arena (plan section 5: "emitted text owns no
/// arena"): the bytes handed to the write callback are borrowed for the call
/// and copied by the caller, and the `EmitResult` (emitted file names,
/// diagnostics with their arguments and source maps with inlined sources)
/// keeps no storage of the program alive: with the result and the copies
/// held, dropping the program releases every owner and allocation its
/// counter domain recorded, the checkers' included, and the result reads the
/// same afterwards.
#[test]
fn emitted_text_and_results_own_no_arena() {
    let program_files = modules(4);
    let options = CompilerOptions {
        inline_sources: Tristate::TRUE,
        ..options()
    };
    for mode in Mode::BOTH {
        let counters = Counters::new();
        let program = support::load(&program_files, &options, mode, &counters);
        let checked = CheckedProgram::new(program.clone(), &counters, None);
        let recorder = Recorder::default();
        let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
            recorder.write(name, text);
            Ok(())
        };
        let result = checked
            .emit(
                &CheckerRequest::default(),
                &EmitOptions {
                    write_file: Some(&write_file),
                    ..EmitOptions::default()
                },
            )
            .expect("the emit succeeds")
            .expect("a result");
        let written = recorder.take();
        let before = support::observe(&program, &result, written.clone());
        assert!(
            !before.diagnostics.is_empty(),
            "the result carries a diagnostic"
        );
        assert!(
            before
                .source_maps
                .iter()
                .any(|(_, _, map)| map.sources_content.is_some()),
            "the result carries inlined sources"
        );
        let arguments: Vec<Vec<Vec<u8>>> = result
            .diagnostics
            .iter()
            .map(|d| {
                d.message_args
                    .iter()
                    .map(|a| a.as_bytes().to_vec())
                    .collect()
            })
            .collect();
        drop(checked);
        drop(program);
        assert_eq!(counters.snapshot(), Counts::default(), "{mode:?}");
        let after: Vec<Vec<Vec<u8>>> = result
            .diagnostics
            .iter()
            .map(|d| {
                d.message_args
                    .iter()
                    .map(|a| a.as_bytes().to_vec())
                    .collect()
            })
            .collect();
        assert_eq!(after, arguments, "{mode:?}");
        let names: Vec<String> = result
            .emitted_files
            .iter()
            .map(|name| String::from_utf8_lossy(name.as_bytes()).into_owned())
            .collect();
        assert_eq!(names, before.emitted_files, "{mode:?}");
        let maps: Vec<_> = result
            .source_maps
            .iter()
            .map(|map| map.source_map.clone())
            .collect();
        let maps_before: Vec<_> = before
            .source_maps
            .iter()
            .map(|(_, _, map)| map.clone())
            .collect();
        assert_eq!(maps, maps_before, "{mode:?}");
        let mut sorted = written;
        sorted.sort();
        assert_eq!(sorted, before.written, "{mode:?}");
    }
}

/// A whole-program emit runs on bounded workers (plan sections 3 and 6: the
/// pin queues one task per emitted file; a parallel group runs them on at
/// most `worker_bound()` threads, each with the reserved stack, never one
/// thread per file): sixty-four files emitted concurrently are written from
/// at least two and at most `worker_bound()` threads, none of them the
/// caller's; single-threaded, every file is written from the caller's
/// thread.
#[test]
fn a_whole_program_emit_runs_on_bounded_workers() {
    let mut program_files = files(&[("/lib.d.ts", LIB)]);
    for i in 0..64 {
        program_files.push((
            format!("/f{i}.ts"),
            format!("export const v{i}: number = {i};\n"),
        ));
    }
    let caller = std::thread::current().id();
    for mode in Mode::BOTH {
        let (checked, _) = support::checked(&program_files, &CompilerOptions::default(), mode);
        let threads = Mutex::new(BTreeSet::new());
        let written = Mutex::new(0usize);
        let write_file = |_: &[u8], _: &[u8], _: &mut WriteFileData| {
            threads
                .lock()
                .expect("threads")
                .insert(format!("{:?}", std::thread::current().id()));
            *written.lock().expect("count") += 1;
            Ok(())
        };
        checked
            .emit(
                &CheckerRequest::default(),
                &EmitOptions {
                    write_file: Some(&write_file),
                    ..EmitOptions::default()
                },
            )
            .expect("the emit succeeds")
            .expect("a result");
        assert_eq!(*written.lock().expect("count"), 64);
        let threads = threads.into_inner().expect("threads");
        let caller = format!("{caller:?}");
        match mode {
            Mode::Single => assert_eq!(threads, BTreeSet::from([caller])),
            Mode::Concurrent => {
                let bound = tsr_core::workgroup::worker_bound();
                assert!(
                    (2..=bound).contains(&threads.len()) && !threads.contains(&caller),
                    "{} threads, bound {bound}",
                    threads.len()
                );
            }
        }
    }
}
