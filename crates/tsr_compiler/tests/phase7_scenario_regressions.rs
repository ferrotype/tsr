//! Differences the Phase 7 benchmarking scenarios found against the pinned Go
//! compiler, reduced to the programs that show them and its diagnostics.
use std::sync::{Arc, Mutex};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::{CheckerOwner, CheckerRequest};
use tsr_compiler::{
    CheckedProgram, EmitOptions, FileCache, Program, ProgramCheckerHost, ProgramOptions,
    WriteFileData,
};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(files: &[(&str, &str)]) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let roots = files
        .iter()
        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
        .collect();
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        ..Default::default()
    };
    Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, roots),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

fn checker(program: &Arc<Program>) -> Arc<CheckerOwner> {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    )
}

/// vscode: `key in data` with a unique symbol key narrows a type whose only
/// index signature is a string one to `Data & Record<typeof key, unknown>`:
/// a string index signature never applies to a symbol name
/// (getApplicableIndexInfoForName), so the key is not a known property.
#[test]
fn in_narrowing_by_a_unique_symbol_ignores_string_index_signatures() {
    let text = "interface Data { from?: string; [key: string]: string | unknown | undefined; }\n\
                function narrowed(data?: Data) {\n\
                \tif (data && key in data) {\n\
                \t\treturn Boolean(data[key]);\n\
                \t}\n\
                \treturn false;\n\
                }\n\
                function plain(data: Data) {\n\
                \treturn data[key];\n\
                }\n\
                const key = Symbol('key');\n";
    let program = program(&[("/a.ts", text)]);
    let owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let diagnostics = op
        .semantic_diagnostics(program.file(b"/a.ts").unwrap().source())
        .unwrap();
    // Go: only the access that no `in` check narrowed (a.ts(9,14)).
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, 2538);
    let plain = text.find("return data[key]").unwrap() + "return data[".len();
    assert_eq!(diagnostics[0].loc.pos(), i64::try_from(plain).unwrap());
}

/// bluesky: an import used only by the heritage clause of a `declare class` is
/// elided. checkIdentifier marks aliases through markLinkedReferences, which
/// ignores references in ambient contexts.
#[test]
fn an_import_used_only_in_an_ambient_class_heritage_clause_is_elided() {
    let program = program(&[
        (
            "/a.ts",
            "import { NativeModule, requireNativeModule } from './expo';\n\
             declare class EmojiPickerModule extends NativeModule {}\n\
             export default requireNativeModule<EmojiPickerModule>('EmojiPicker');\n",
        ),
        (
            "/expo.d.ts",
            "export declare class NativeModule {}\n\
             export declare function requireNativeModule<T>(name: string): T;\n",
        ),
    ]);
    let checked = CheckedProgram::new(program, &Counters::new(), None);
    // As the command line does: check every file, then emit. Checking the
    // heritage clause is what marked the alias.
    let request = CheckerRequest::default();
    assert!(checked
        .semantic_diagnostics(&request, None)
        .unwrap()
        .is_empty());
    let written = Mutex::new(Vec::new());
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        written.lock().unwrap().push((name.to_vec(), text.to_vec()));
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        ..EmitOptions::default()
    };
    checked.emit(&request, &options).unwrap();
    let written = written.into_inner().unwrap();
    let (_, text) = written
        .iter()
        .find(|(name, _)| name.as_slice() == b"/a.js")
        .expect("a.js is written");
    // Go: the import keeps only the value requireNativeModule calls.
    assert_eq!(
        String::from_utf8_lossy(text).lines().next(),
        Some("import { requireNativeModule } from './expo';")
    );
}
