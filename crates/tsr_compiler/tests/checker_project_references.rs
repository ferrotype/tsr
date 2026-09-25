//! Declaration-emit module specifiers on a program that includes a referenced
//! project's built `.d.ts` in place of its source. Every expected display is a
//! pinned Go observation: `Checker.TypeToStringEx(type, declaration, 0)` on the
//! same workspace, built by `NewProgram` from the same tsconfig files.

use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_checker::{CheckerOwner, Error};
use tsr_compiler::{CompilerConfigHost, FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;

const ROOT_CONFIG: &[u8] = br#"{"compilerOptions":{"module":"esnext","declaration":true,"noLib":true},"files":["main.ts"],"references":[{"path":"./ref"}]}"#;
const REF_CONFIG: &[u8] = br#"{"compilerOptions":{"composite":true,"outDir":"out","rootDir":"src","module":"esnext","noLib":true}}"#;
const A_SOURCE: &[u8] =
    b"export interface Foo { x: number }\nexport declare function make(): Foo;\n";
const A_OUTPUT: &[u8] =
    b"export interface Foo {\n    x: number;\n}\nexport declare function make(): Foo;\n";

fn load(files: &[(&[u8], &[u8])]) -> (Arc<CheckerOwner>, Arc<Program>) {
    let counters = Counters::new();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/p", true);
    for &(name, text) in files {
        fs.insert_loaded(name, text);
    }
    let fs: Arc<dyn tsr_vfs::FileSystem> = Arc::new(fs.finish());
    let cwd = JsString::from_bytes(b"/p".as_slice());
    let config = tsr_tsoptions::get_parsed_command_line_of_config_file(
        b"/p/tsconfig.json",
        &CompilerOptions::default(),
        &tsr_tsoptions::ConfigValue::Null,
        &CompilerConfigHost::new(fs.clone(), cwd.clone()),
    )
    .unwrap()
    .command_line
    .unwrap();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config,
                host: fs,
                current_directory: cwd,
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let checker = CheckerOwner::for_program(
        CheckerIdentity::new(Generation::new(&counters), &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )
    .unwrap();
    (Arc::new(checker), program)
}

/// `TypeToStringEx(GetTypeAtLocation(name), declaration, 0)` for the first
/// declaration of the variable statement at `statement` in `file`.
fn display(
    owner: &Arc<CheckerOwner>,
    program: &Program,
    file: &[u8],
    statement: usize,
) -> Result<String, Error> {
    let file = program.file(file).unwrap();
    let view = file.bound().view().ast();
    let statement = view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .at(statement)
        .unwrap();
    let list = view
        .node(statement)
        .unwrap()
        .data_source()
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations = view
        .node(list)
        .unwrap()
        .data_source()
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    let declaration: NodeId = view
        .node_slice(view.list(declarations).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    let name = view.node(declaration).unwrap().name().unwrap();
    let mut op = owner.operation().unwrap();
    let typ = op.get_type_at_location(name)?;
    op.type_to_string_at(typ, Some(declaration), 0)
        .map(|text| String::from_utf8_lossy(text.as_bytes()).into_owned())
}

#[test]
fn a_referenced_output_is_named_by_its_source() {
    // The module's file is ref/out/a.d.ts; the pin names it by its source
    // (GetModuleSpecifiersWithInfo), so the user's import is reused.
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", ROOT_CONFIG),
        (b"/p/ref/tsconfig.json", REF_CONFIG),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (
            b"/p/main.ts",
            b"import { make } from \"./ref/src/a\";\nexport const v = make();\n",
        ),
    ]);
    assert!(program.file(b"/p/ref/out/a.d.ts").is_some());
    assert_eq!(
        display(&owner, &program, b"/p/main.ts", 1).unwrap(),
        "import(\"./ref/src/a\").Foo"
    );

    // Without an import to reuse, the relative specifier is computed to the
    // source as well, not to the output that the program holds.
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", ROOT_CONFIG),
        (b"/p/ref/tsconfig.json", REF_CONFIG),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (
            b"/p/ref/src/b.ts",
            b"import { make } from \"./a\";\nexport declare const get: typeof make;\n",
        ),
        (
            b"/p/ref/out/b.d.ts",
            b"import { make } from \"./a\";\nexport declare const get: typeof make;\n",
        ),
        (
            b"/p/main.ts",
            b"import { get } from \"./ref/src/b\";\nexport const v = get();\n",
        ),
    ]);
    assert_eq!(
        display(&owner, &program, b"/p/main.ts", 1).unwrap(),
        "import(\"./ref/src/a\").Foo"
    );
}

#[test]
fn an_importer_that_is_a_referenced_output_fails_closed() {
    // Native answers `() => import("./a").Foo` for b.d.ts: the pin sorts the
    // module's paths from the importer's source directory (ref/src), so the
    // import "./a" is reused before "../out/a". That directory is chosen inside
    // tsr_checker's proximity sort, which reads the importer's own name, so the
    // host refuses rather than answer "../out/a".
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", ROOT_CONFIG),
        (b"/p/ref/tsconfig.json", REF_CONFIG),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (
            b"/p/ref/src/b.ts",
            b"import { make } from \"./a\";\nimport {} from \"../out/a\";\nexport const w = make;\n",
        ),
        (
            b"/p/ref/out/b.d.ts",
            b"import { make } from \"./a\";\nimport {} from \"../out/a\";\nexport declare const w: typeof make;\n",
        ),
        (
            b"/p/main.ts",
            b"import { w } from \"./ref/src/b\";\nexport const v = w;\n",
        ),
    ]);
    assert_eq!(
        display(&owner, &program, b"/p/main.ts", 1).unwrap(),
        "() => import(\"./ref/src/a\").Foo"
    );
    assert_eq!(
        display(&owner, &program, b"/p/ref/out/b.d.ts", 2),
        Err(Error::Unsupported(
            "module specifiers for an importer that is a project-reference output"
        ))
    );
}
