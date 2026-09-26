//! Declaration-emit module specifiers and the checker's project-reference
//! callers on a program that includes a referenced project's built `.d.ts` in
//! place of its source. Every expected display is a pinned Go observation:
//! `Checker.TypeToStringEx(type, declaration, 0)` on the same workspace, built
//! by `NewProgram` from the same tsconfig files. Every expected diagnostic is
//! the pinned `tsgo -p tsconfig.json --noEmit --pretty false` output on the
//! same workspace (C3.5).

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
    let fs: Arc<dyn tsr_vfs::FileSystem> =
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish())));
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
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
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

/// `(code, line, column, message)` of the semantic diagnostics of `file`,
/// positioned one-based as the native non-pretty output prints them.
fn semantic(
    owner: &Arc<CheckerOwner>,
    program: &Program,
    file: &[u8],
    text: &[u8],
) -> Vec<(i32, usize, usize, String)> {
    let file = program.file(file).unwrap();
    let mut op = owner.operation().unwrap();
    op.semantic_diagnostics(file.source())
        .unwrap()
        .iter()
        .map(|diagnostic| {
            let prefix = &text[..usize::try_from(diagnostic.loc.pos()).unwrap()];
            let lines: Vec<&[u8]> = prefix.split(|&byte| byte == b'\n').collect();
            let line = lines.len();
            let column = lines.last().unwrap().len() + 1;
            let message = tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap();
            (
                diagnostic.code,
                line,
                column,
                String::from_utf8(message).unwrap(),
            )
        })
        .collect()
}

const DEFAULT_IMPORTER: &[u8] = b"import def from \"./ref/src/a\";\nexport const v = def;\n";
const SYNTHETIC_ROOT: &[u8] = br#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler","allowSyntheticDefaultImports":true,"types":[],"target":"esnext"},"files":["main.mts"],"references":[{"path":"./ref"}]}"#;
const ESM_REF: &[u8] = br#"{"compilerOptions":{"composite":true,"outDir":"out","rootDir":"src","module":"esnext","types":[],"target":"esnext"}}"#;
const CJS_REF: &[u8] = br#"{"compilerOptions":{"composite":true,"outDir":"out","rootDir":"src","module":"commonjs","types":[],"target":"esnext"}}"#;

#[test]
fn a_synthetic_default_follows_the_referenced_projects_module_format() {
    // canHaveSyntheticDefault asks the program for the referenced output's
    // emit module format, which the referenced project's own options decide.
    // Native: `main.mts(1,8): error TS1192: Module '".../ref/out/a"' has no
    // default export.` when that project emits ES modules; nothing when it
    // emits CommonJS.
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", SYNTHETIC_ROOT),
        (b"/p/ref/tsconfig.json", ESM_REF),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (b"/p/main.mts", DEFAULT_IMPORTER),
    ]);
    assert_eq!(
        semantic(&owner, &program, b"/p/main.mts", DEFAULT_IMPORTER),
        vec![(
            1192,
            1,
            8,
            "Module '\"/p/ref/out/a\"' has no default export.".to_string()
        )]
    );

    let (owner, program) = load(&[
        (b"/p/tsconfig.json", SYNTHETIC_ROOT),
        (b"/p/ref/tsconfig.json", CJS_REF),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (b"/p/main.mts", DEFAULT_IMPORTER),
    ]);
    assert_eq!(
        semantic(&owner, &program, b"/p/main.mts", DEFAULT_IMPORTER),
        vec![]
    );
}

#[test]
fn a_missing_reference_output_is_reported_as_not_built() {
    // The source resolves but the declaration output that stands in for it is
    // not in the program. Native: `main.mts(1,17): error TS6305: Output file
    // '.../ref/out/a.d.ts' has not been built from source file
    // '.../ref/src/a.ts'.`
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", SYNTHETIC_ROOT),
        (b"/p/ref/tsconfig.json", CJS_REF),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/main.mts", DEFAULT_IMPORTER),
    ]);
    assert!(program.file(b"/p/ref/out/a.d.ts").is_none());
    assert_eq!(
        semantic(&owner, &program, b"/p/main.mts", DEFAULT_IMPORTER),
        vec![(
            6305,
            1,
            17,
            "Output file '/p/ref/out/a.d.ts' has not been built from source file '/p/ref/src/a.ts'."
                .to_string()
        )]
    );
}

const REWRITE_IMPORTER: &[u8] =
    b"import { make } from \"./ref/src/a.ts\";\nexport const v = make();\n";

#[test]
fn a_rewritten_import_across_projects_compares_the_output_layout() {
    // resolveExternalModule compares the relative path between the two
    // projects' root directories with the one between their output
    // directories. Native: `main.ts(1,22): error TS2878` when the referenced
    // project emits to ref/out while its sources are under ref/src; nothing
    // when both projects keep the same layout under dist.
    let root = br#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler","rewriteRelativeImportExtensions":true,"allowImportingTsExtensions":true,"types":[],"target":"esnext"},"files":["main.ts"],"references":[{"path":"./ref"}]}"#;
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", root),
        (b"/p/ref/tsconfig.json", ESM_REF),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/ref/out/a.d.ts", A_OUTPUT),
        (b"/p/main.ts", REWRITE_IMPORTER),
    ]);
    assert_eq!(
        semantic(&owner, &program, b"/p/main.ts", REWRITE_IMPORTER),
        vec![(
            2878,
            1,
            22,
            "This import path is unsafe to rewrite because it resolves to another project, and the relative path between the projects' output files is not the same as the relative path between its input files."
                .to_string()
        )]
    );

    let root = br#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler","rewriteRelativeImportExtensions":true,"allowImportingTsExtensions":true,"outDir":"dist","types":[],"target":"esnext"},"files":["main.ts"],"references":[{"path":"./ref"}]}"#;
    let reference = br#"{"compilerOptions":{"composite":true,"outDir":"../dist/ref/src","rootDir":"src","module":"esnext","types":[],"target":"esnext"}}"#;
    let (owner, program) = load(&[
        (b"/p/tsconfig.json", root),
        (b"/p/ref/tsconfig.json", reference),
        (b"/p/ref/src/a.ts", A_SOURCE),
        (b"/p/dist/ref/src/a.d.ts", A_OUTPUT),
        (b"/p/main.ts", REWRITE_IMPORTER),
    ]);
    assert!(program.file(b"/p/dist/ref/src/a.d.ts").is_some());
    assert_eq!(
        semantic(&owner, &program, b"/p/main.ts", REWRITE_IMPORTER),
        vec![]
    );
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
