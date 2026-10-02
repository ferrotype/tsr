//! A rebuild reuses the semantic diagnostics of an unchanged file, which name
//! files as the previous program held them. Each program parses its files
//! into owners of its own, so the rebuilt program holds an unchanged file as
//! another node: the reused diagnostics must name the rebuilt program's
//! files, or formatting them with it fails with `WrongOwner`.
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_checker::CheckerRequest;
use tsr_compiler::diagnostic_writer::{DiagnosticWriter, FormattingOptions};
use tsr_compiler::{CheckedProgram, FileCache, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_incremental::{create_host, new_program, Program, ProgramCompilerHost};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;

const LIB: &str = "interface Array<T> { length: number; [n: number]: T }\n\
interface Boolean {}\ninterface CallableFunction {}\ninterface Function {}\n\
interface IArguments {}\ninterface NewableFunction {}\ninterface Number {}\n\
interface Object {}\ninterface RegExp {}\ninterface String {}\n";

const FILES: [(&str, &str); 3] = [
    ("/lib.d.ts", LIB),
    (
        "/b.ts",
        "export function f(x: number): number { return x; }\n",
    ),
    (
        "/a.ts",
        "import { f } from \"./b\";\nexport const v: string = f();\n",
    ),
];

/// The files loaded with a file cache of their own, so every file is parsed
/// into a new owner, and the incremental program over them and `old`.
fn build(old: Option<&Program>, counters: &Counters) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in FILES {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let options = CompilerOptions {
        no_lib: Tristate::TRUE,
        incremental: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    let names = FILES
        .iter()
        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
        .collect();
    let loaded = Arc::new(
        tsr_compiler::Program::load(
            ProgramOptions {
                config: ParsedCommandLine::new(options, names),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut FileCache::new(),
            counters,
        )
        .expect("the program loads"),
    );
    let checked = Arc::new(CheckedProgram::new(loaded.clone(), counters, None));
    let host = create_host(Arc::new(ProgramCompilerHost::new(loaded)));
    new_program(checked, old, host, None, true).expect("the incremental program")
}

/// The semantic diagnostics of `/a.ts`, formatted with the program that
/// reports them.
fn formatted(program: &Program) -> String {
    let file = program.get_source_file(b"/a.ts").expect("/a.ts");
    let diagnostics = program
        .get_semantic_diagnostics(&CheckerRequest::default(), Some(file))
        .expect("semantic diagnostics");
    let diagnostics: Vec<_> = diagnostics.iter().collect();
    let mut writer =
        DiagnosticWriter::new(program.program().program(), FormattingOptions::default());
    String::from_utf8(
        writer
            .format(&diagnostics, true)
            .expect("the diagnostics format"),
    )
    .expect("UTF-8")
}

#[test]
fn a_rebuild_names_the_files_of_its_own_program_in_reused_diagnostics() {
    let counters = Counters::new();
    let first = build(None, &counters);
    let expected = formatted(&first);
    assert!(expected.contains("TS2554"), "{expected}");
    assert!(
        expected.contains("An argument for 'x' was not provided."),
        "{expected}"
    );

    let rebuilt = build(Some(&first), &counters);
    for name in [b"/a.ts".as_slice(), b"/b.ts".as_slice()] {
        assert_ne!(
            first.get_source_file(name).expect("first").source(),
            rebuilt.get_source_file(name).expect("rebuilt").source(),
            "each build parses its files into owners of its own"
        );
    }
    assert_eq!(formatted(&rebuilt), expected);
    // The rebuilt program's diagnostics read the same through the first.
    assert_eq!(formatted(&first), expected);
}

#[test]
fn reused_diagnostics_do_not_retain_previous_programs() {
    let counters = Counters::new();
    let mut current = build(None, &counters);
    let expected = formatted(&current);
    for _ in 0..3 {
        // Cache both the errors (including cross-file related information)
        // and the empty diagnostic lists of the other unchanged files.
        current
            .get_semantic_diagnostics(&CheckerRequest::default(), None)
            .expect("all semantic diagnostics");
        let identities: Vec<_> = [b"/a.ts".as_slice(), b"/lib.d.ts"]
            .into_iter()
            .map(|path| {
                let path = JsString::from_bytes(path);
                let id = current
                    .get_testing_data()
                    .unwrap()
                    .semantic_diagnostics_per_file
                    .cached_semantic_diagnostics_identity(&path)
                    .unwrap();
                (path, id)
            })
            .collect();
        let previous = Arc::downgrade(current.program().program());
        current = build(Some(&current), &counters);
        assert!(
            previous.upgrade().is_none(),
            "reused diagnostics must not pin the obsolete loaded program"
        );
        for (path, identity) in identities {
            assert_eq!(
                Some(identity),
                current
                    .get_testing_data()
                    .unwrap()
                    .semantic_diagnostics_per_file
                    .cached_semantic_diagnostics_identity(&path),
                "unchanged nonempty and empty diagnostic caches retain their actual identity"
            );
        }
        assert_eq!(formatted(&current), expected);
    }
}
