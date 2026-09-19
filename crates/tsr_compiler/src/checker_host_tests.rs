use super::*;
use crate::{FileCache, ProgramOptions};
use tsr_arena::Counters;
use tsr_core::{ModuleResolutionKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;
use tsr_vfs::MemoryBuilder;

fn load(
    options: CompilerOptions,
    roots: &[&[u8]],
    files: &[(&[u8], &[u8])],
    case_sensitive: bool,
) -> Arc<Program> {
    let mut fs = MemoryBuilder::new(b"/src", case_sensitive);
    for &(name, text) in files {
        fs.insert_loaded(name, text);
    }
    fs.insert_symlink(b"/src/loop", b"/src/loop");
    Arc::new(
        Program::load(
            ProgramOptions {
                config: ParsedCommandLine::new(
                    options,
                    roots
                        .iter()
                        .map(|&name| JsString::from_bytes(name))
                        .collect(),
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/src".as_slice()),
                default_library_path: JsString::from_bytes(b"/lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

fn usage(file: &CompletedFile, specifier: &[u8]) -> NodeId {
    let view = file.view().ast();
    file.view()
        .source_file()
        .unwrap()
        .imports()
        .unwrap()
        .iter()
        .flatten()
        .copied()
        .find(|&id| view.node_text(id).unwrap().as_bytes() == specifier)
        .unwrap()
}

#[test]
fn checker_host_borrows_program_order_metadata_and_resolution_without_owner_clones() {
    let program = load(
        CompilerOptions {
            target: ScriptTarget::ES5,
            module: ModuleKind::NODE16,
            module_resolution: ModuleResolutionKind::NODE16,
            ..Default::default()
        },
        &[b"/src/Main.ts"],
        &[
            (b"/src/package.json", br#"{"type":"module"}"#),
            (
                b"/src/Main.ts",
                b"import './dep.js'; export const value = 1;",
            ),
            (b"/src/dep.ts", b"export const dependency = 1;"),
            (b"/lib/lib.d.ts", b"interface Object {}"),
        ],
        false,
    );
    let host = ProgramCheckerHost::new(program.clone());
    let program_count = Arc::strong_count(&program);
    let file_counts: Vec<_> = program.files().iter().map(Arc::strong_count).collect();
    assert_eq!(host.source_file_count(), 3);
    for (index, file) in program.files().iter().enumerate() {
        assert!(std::ptr::eq(host.source_file(index), file.bound()));
    }
    assert!(std::ptr::eq(host.options(), program.options()));
    let main = host.get_source_file(b"MAIN.TS").unwrap();
    assert_eq!(
        main.source(),
        program.file(b"/src/main.ts").unwrap().source()
    );
    assert_eq!(host.file_exists(b"/SRC/MAIN.TS"), Ok(true));
    assert_eq!(host.file_exists(b"missing.ts"), Ok(false));
    assert_eq!(host.get_current_directory(), b"/src");
    assert!(!host.use_case_sensitive_file_names());
    assert!(host.is_source_file_default_library(b"/lib/lib.d.ts"));
    assert!(!host.is_source_file_default_library(b"/src/main.ts"));
    let meta = host.get_source_file_meta_data(b"MAIN.TS").unwrap();
    assert_eq!(meta.package_json_type.as_bytes(), b"module");
    assert_eq!(meta.package_json_directory.as_bytes(), b"/src");
    assert_eq!(meta.implied_node_format, ModuleKind::ESNEXT);
    let resolution = host
        .get_resolved_module(b"MAIN.TS", b"./dep.js", ModuleKind::ESNEXT)
        .unwrap()
        .unwrap();
    assert_eq!(resolution.resolved_file_name.as_bytes(), b"/src/dep.ts");
    assert!(host
        .get_resolved_module(b"MAIN.TS", b"./dep.js", ModuleKind::COMMON_JS)
        .unwrap()
        .is_none());
    assert_eq!(
        host.get_source_file_for_resolved_module(resolution.resolved_file_name.as_bytes())
            .unwrap()
            .source(),
        program.file(b"/src/dep.ts").unwrap().source()
    );
    assert_eq!(host.common_source_directory().unwrap(), b"/src/");
    let directory = host.common_source_directory().unwrap().as_ptr();
    assert_eq!(directory, host.common_source_directory().unwrap().as_ptr());
    assert_eq!(Arc::strong_count(&program), program_count);
    assert_eq!(
        program
            .files()
            .iter()
            .map(Arc::strong_count)
            .collect::<Vec<_>>(),
        file_counts
    );
}

#[test]
fn checker_host_emit_syntax_is_independent_of_resolution_attributes_and_resolver_mode() {
    let source = br"
        import './static.js';
        import type { X } from './typed.js' with { 'resolution-mode': 'require' };
        import equals = require('./equals.js');
        const lazy = import('./dynamic.js');
    ";
    for (module, resolution, static_mode, dynamic_mode) in [
        (
            ModuleKind::ESNEXT,
            ModuleResolutionKind::NODE10,
            ModuleKind::ESNEXT,
            ModuleKind::ESNEXT,
        ),
        (
            ModuleKind::COMMON_JS,
            ModuleResolutionKind::NODE10,
            ModuleKind::COMMON_JS,
            ModuleKind::COMMON_JS,
        ),
        (
            ModuleKind::NODE16,
            ModuleResolutionKind::NODE16,
            ModuleKind::COMMON_JS,
            ModuleKind::ESNEXT,
        ),
        (
            ModuleKind::PRESERVE,
            ModuleResolutionKind::BUNDLER,
            ModuleKind::ESNEXT,
            ModuleKind::ESNEXT,
        ),
    ] {
        let host = ProgramCheckerHost::new(load(
            CompilerOptions {
                no_lib: Tristate::TRUE,
                module,
                module_resolution: resolution,
                ..Default::default()
            },
            &[b"/src/main.ts"],
            &[(b"/src/main.ts", source)],
            true,
        ));
        let file = host.get_source_file(b"main.ts").unwrap();
        for (specifier, expected) in [
            (b"./static.js".as_slice(), static_mode),
            (b"./typed.js", static_mode),
            (b"./equals.js", ModuleKind::COMMON_JS),
            (b"./dynamic.js", dynamic_mode),
        ] {
            assert_eq!(
                host.get_emit_syntax_for_usage_location(b"main.ts", usage(file, specifier)),
                Ok(expected),
                "module {module:?}, specifier {specifier:?}"
            );
        }
    }
}

#[test]
fn checker_host_uses_extension_and_package_context_for_emit_formats() {
    let host = ProgramCheckerHost::new(load(
        CompilerOptions {
            no_lib: Tristate::TRUE,
            module: ModuleKind::ESNEXT,
            module_resolution: ModuleResolutionKind::NODE16,
            ..Default::default()
        },
        &[
            b"/src/plain.ts",
            b"/src/esm.mts",
            b"/src/cjs.cts",
            b"/src/pkg/main.ts",
        ],
        &[
            (b"/src/plain.ts", b"export {};"),
            (b"/src/esm.mts", b"export {};"),
            (b"/src/cjs.cts", b"export {};"),
            (b"/src/pkg/main.ts", b"export {};"),
            (b"/src/pkg/package.json", br#"{"type":"commonjs"}"#),
        ],
        true,
    ));
    for (name, implied, emit) in [
        (b"plain.ts".as_slice(), ModuleKind::NONE, ModuleKind::ESNEXT),
        (b"esm.mts", ModuleKind::ESNEXT, ModuleKind::ESNEXT),
        (b"cjs.cts", ModuleKind::COMMON_JS, ModuleKind::COMMON_JS),
        (b"pkg/main.ts", ModuleKind::COMMON_JS, ModuleKind::COMMON_JS),
    ] {
        assert_eq!(host.get_implied_node_format_for_emit(name), Ok(implied));
        assert_eq!(host.get_emit_module_format_of_file(name), Ok(emit));
    }
}

#[test]
fn checker_host_resolved_module_files_follow_published_package_redirects() {
    let host = ProgramCheckerHost::new(load(
        CompilerOptions {
            no_lib: Tristate::TRUE,
            module_resolution: ModuleResolutionKind::NODE10,
            ..Default::default()
        },
        &[b"/src/one/main.ts", b"/src/two/main.ts"],
        &[
            (b"/src/one/main.ts", b"import 'pkg';"),
            (b"/src/two/main.ts", b"import 'pkg';"),
            (
                b"/src/one/node_modules/pkg/package.json",
                br#"{"name":"pkg","version":"1.0.0","types":"index.d.ts"}"#,
            ),
            (
                b"/src/two/node_modules/pkg/package.json",
                br#"{"name":"pkg","version":"1.0.0","types":"index.d.ts"}"#,
            ),
            (
                b"/src/one/node_modules/pkg/index.d.ts",
                b"export const x: string;",
            ),
            (
                b"/src/two/node_modules/pkg/index.d.ts",
                b"export const x: string;",
            ),
        ],
        true,
    ));
    let first = host
        .get_source_file_for_resolved_module(b"/src/one/node_modules/pkg/index.d.ts")
        .unwrap();
    let redirected = host
        .get_source_file_for_resolved_module(b"/src/two/node_modules/pkg/index.d.ts")
        .unwrap();
    assert!(std::ptr::eq(first, redirected));
    assert_eq!(host.source_file_count(), 3);
}

#[test]
fn checker_host_emit_eligibility_observes_force_dts_and_source_classification() {
    let host = ProgramCheckerHost::new(load(
        CompilerOptions {
            no_lib: Tristate::TRUE,
            allow_js: Tristate::TRUE,
            no_emit_for_js_files: Tristate::TRUE,
            resolve_json_module: Tristate::TRUE,
            module: ModuleKind::COMMON_JS,
            module_resolution: ModuleResolutionKind::NODE10,
            ..Default::default()
        },
        &[
            b"/src/code/main.ts",
            b"/src/data.json",
            b"/src/types.d.ts",
            b"/src/plain.js",
        ],
        &[
            (b"/src/code/main.ts", b"import 'pkg'; export {};"),
            (b"/src/data.json", b"{}"),
            (b"/src/types.d.ts", b"declare const x: string;"),
            (b"/src/plain.js", b"var x = 1;"),
            (
                b"/src/node_modules/pkg/package.json",
                br#"{"name":"pkg","version":"1.0.0","types":"index.ts"}"#,
            ),
            (b"/src/node_modules/pkg/index.ts", b"export const x = 1;"),
        ],
        true,
    ));
    for (name, normal, forced) in [
        (b"code/main.ts".as_slice(), true, true),
        (b"data.json", false, true),
        (b"types.d.ts", false, false),
        (b"plain.js", false, false),
        (b"node_modules/pkg/index.ts", false, false),
    ] {
        let file = host.get_source_file(name).unwrap();
        assert_eq!(host.source_file_may_be_emitted(file, false), Ok(normal));
        assert_eq!(host.source_file_may_be_emitted(file, true), Ok(forced));
    }
    assert_eq!(host.common_source_directory().unwrap(), b"/src/code/");
}

#[test]
fn checker_host_retains_program_and_preserves_foreign_source_and_vfs_errors() {
    let options = CompilerOptions {
        no_lib: Tristate::TRUE,
        ..Default::default()
    };
    let program = load(
        options.clone(),
        &[b"/src/main.ts"],
        &[(b"/src/main.ts", b"import './x';")],
        true,
    );
    let foreign = load(
        options,
        &[b"/src/main.ts"],
        &[(b"/src/main.ts", b"import './x';")],
        true,
    );
    let weak = Arc::downgrade(&program);
    let host = ProgramCheckerHost::new(program);
    assert!(weak.upgrade().is_some());
    let file = foreign.files()[0].bound();
    assert_eq!(
        host.source_file_may_be_emitted(file, false),
        Err(Error::Arena(tsr_arena::Error::WrongOwner))
    );
    assert_eq!(
        host.get_emit_syntax_for_usage_location(b"main.ts", usage(file, b"./x")),
        Err(Error::Arena(tsr_arena::Error::WrongOwner))
    );
    assert_eq!(
        host.get_emit_module_format_of_file(b"foreign.ts"),
        Err(Error::Arena(tsr_arena::Error::WrongOwner))
    );
    assert_eq!(host.file_exists(b"loop"), Err(tsr_vfs::Error::SymlinkCycle));
    // Project references are rejected before files load, so the native lookup
    // is an empty one rather than an unported boundary.
    assert!(host
        .get_redirect_for_resolution(b"main.ts")
        .unwrap()
        .is_none());
    assert!(host
        .get_project_reference_from_output_dts(b"main.d.ts")
        .unwrap()
        .is_none());
    drop(host);
    assert!(weak.upgrade().is_none());
}
