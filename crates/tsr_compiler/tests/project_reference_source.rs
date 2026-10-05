//! Editor source redirection through the real module resolver and parser.
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_compiler::{FileCache, Program, ProgramHostServices, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleResolutionKind, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::{ParsedCommandLine, ProjectReference};
use tsr_vfs::{FileSystem, MemoryBuilder};

fn options(fs: Arc<dyn FileSystem>, preserve_symlinks: bool, disabled: bool) -> ProgramOptions {
    let mut config = ParsedCommandLine::new(
        CompilerOptions {
            no_lib: Tristate::TRUE,
            types: Some(vec![JsString::from_bytes("*".as_bytes())]),
            module_resolution: ModuleResolutionKind::NODE10,
            preserve_symlinks: Tristate::from(preserve_symlinks),
            disable_source_of_project_reference_redirect: Tristate::from(disabled),
            ..Default::default()
        },
        vec![JsString::from_bytes("/src/app/main.ts".as_bytes())],
    );
    config.project_references = Some(vec![ProjectReference {
        path: JsString::from_bytes("/src/lib".as_bytes()),
        original_path: JsString::from_bytes("../lib".as_bytes()),
        circular: false,
    }]);
    ProgramOptions {
        config,
        host: fs,
        current_directory: JsString::from_bytes("/src/app".as_bytes()),
        default_library_path: JsString::from_bytes("/lib".as_bytes()),
        skip_module_resolution: false,
        single_threaded: Tristate::TRUE,
    }
}

fn fixture(main: &[u8], case_sensitive: bool) -> MemoryBuilder {
    let mut fs = MemoryBuilder::new(b"/src/app", case_sensitive);
    fs.insert_loaded(b"/src/app/main.ts", main);
    fs.insert_loaded(b"/src/lib/tsconfig.json", br#"{"compilerOptions":{"composite":true,"noLib":true,"rootDir":"src","outDir":"dist","declarationDir":"types"},"files":["src/index.ts","src/dep.ts","src/esm.mts","src/cjs.cts"]}"#.as_slice());
    fs.insert_loaded(
        b"/src/lib/src/index.ts",
        b"export { value } from './dep';".as_slice(),
    );
    fs.insert_loaded(
        b"/src/lib/src/dep.ts",
        b"export const value = 'source';".as_slice(),
    );
    fs.insert_loaded(b"/src/lib/src/esm.mts", b"export const esm = 1;".as_slice());
    fs.insert_loaded(b"/src/lib/src/cjs.cts", b"export const cjs = 2;".as_slice());
    fs.insert_loaded(
        b"/src/lib/package.json",
        br#"{"name":"lib","version":"1.0.0","types":"types/index.d.ts"}"#.as_slice(),
    );
    // Automatic type discovery must use the source host, whose entries can be
    // listed, while resolving its declarations uses the declaration-faking host.
    fs.insert_loaded(
        b"/src/app/node_modules/@types/ambient/index.d.ts",
        b"declare const ambient: number;".as_slice(),
    );
    fs
}

fn load(fs: MemoryBuilder, preserve_symlinks: bool, disabled: bool) -> Program {
    Program::load_live_for_project_with_host_services(
        options(Arc::new(fs.finish()), preserve_symlinks, disabled),
        ProgramHostServices::default(),
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap()
}

#[test]
fn missing_declarations_redirect_to_source_and_use_referenced_options() {
    let program = load(
        fixture(
            b"import { value } from '../lib/types/index'; export { value };",
            true,
        ),
        false,
        false,
    );
    let source = program
        .source_file(b"/src/lib/src/index.ts")
        .expect("source redirect");
    assert!(program.source_file(b"/src/lib/src/dep.ts").is_some());
    assert!(program.source_file(b"/src/lib/types/index.d.ts").is_none());
    assert!(program
        .source_file(b"/src/app/node_modules/@types/ambient/index.d.ts")
        .is_some());
    assert!(program.skip_type_checking(source, false).unwrap());
    assert!(program.is_source_from_project_reference(b"/src/lib/src/index.ts"));
    assert!(program.missing_files().is_empty());
    assert_eq!(
        program.resolutions()[0]
            .result
            .resolved_file_name
            .as_bytes(),
        b"/src/lib/types/index.d.ts"
    );
    assert!(!program
        .host()
        .file_exists(b"/src/lib/types/index.d.ts")
        .unwrap());
}

#[test]
fn package_symlink_redirects_with_and_without_preserve_symlinks() {
    for preserve_symlinks in [false, true] {
        for case_sensitive in [false, true] {
            let mut fs = fixture(
                b"import { value } from 'lib'; export { value };",
                case_sensitive,
            );
            fs.insert_symlink(b"/src/app/node_modules/lib", b"/src/lib");
            let program = load(fs, preserve_symlinks, false);
            assert!(
                program.source_file(b"/src/lib/src/index.ts").is_some(),
                "preserve={preserve_symlinks}, case={case_sensitive}"
            );
            let resolution = &program.resolutions()[0].result;
            assert_eq!(
                resolution.resolved_file_name.as_bytes(),
                if preserve_symlinks {
                    b"/src/app/node_modules/lib/types/index.d.ts".as_slice()
                } else {
                    b"/src/lib/types/index.d.ts"
                }
            );
            assert!(program.missing_files().is_empty());
        }
    }
}

#[test]
fn built_declarations_also_redirect_unless_option_disables_it() {
    for disabled in [false, true] {
        let mut fs = fixture(
            b"import { value } from '../lib/types/index'; export { value };",
            true,
        );
        fs.insert_loaded(
            b"/src/lib/types/index.d.ts",
            b"export declare const value: number;".as_slice(),
        );
        let program = load(fs, false, disabled);
        assert_eq!(
            program.source_file(b"/src/lib/src/index.ts").is_some(),
            !disabled
        );
        assert_eq!(
            program.source_file(b"/src/lib/types/index.d.ts").is_some(),
            disabled
        );
    }
}

#[test]
fn absent_source_cannot_make_an_unbuilt_declaration_resolve() {
    let mut fs = fixture(b"import { missing } from '../lib/types/missing';", true);
    fs.insert_loaded(b"/src/lib/tsconfig.json", br#"{"compilerOptions":{"composite":true,"rootDir":"src","declarationDir":"types"},"files":["src/missing.ts"]}"#.as_slice());
    let program = load(fs, false, false);
    assert!(!program.resolutions()[0].result.is_resolved());
    assert!(program.source_file(b"/src/lib/src/missing.ts").is_none());
}

#[test]
fn module_specific_declaration_extensions_redirect() {
    let program = load(fixture(b"import { esm } from '../lib/types/esm.mjs'; import { cjs } from '../lib/types/cjs.cjs'; export { esm, cjs };", true), false, false);
    assert!(program.source_file(b"/src/lib/src/esm.mts").is_some());
    assert!(program.source_file(b"/src/lib/src/cjs.cts").is_some());
}
