//! Package deduplication keeps metadata aligned with the published program.
//! Pinned `filesParser.parse` records metadata after the duplicate-package exit;
//! `Program.GetSymlinkCache` then resolves every metadata key to a source file.
//! This reduces the topology of `packageDeduplicationDuplicateGlobals.ts`.

use std::sync::Arc;
use tsr_arena::Counters;
use tsr_checker::CheckerHost;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, Tristate};
use tsr_jsstring::JsString;

#[test]
fn discarded_package_dependencies_do_not_poison_module_specifier_paths() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", b"import 'a'; import 'b';".as_slice());
    for (package, global_version, global_type) in
        [("a", "1.0.0", "string"), ("b", "2.0.0", "number")]
    {
        let directory = format!("/node_modules/{package}");
        fs.insert_loaded(
            format!("{directory}/index.d.ts").as_bytes(),
            b"export { useFoo } from 'foo';".as_slice(),
        );
        fs.insert_loaded(
            format!("{directory}/package.json").as_bytes(),
            format!(r#"{{"name":"{package}","version":"1.0.0"}}"#).into_bytes(),
        );
        let store = format!("/store/{package}/node_modules");
        fs.insert_symlink(
            format!("{directory}/node_modules/foo").as_bytes(),
            format!("{store}/foo").as_bytes(),
        );
        fs.insert_loaded(
            format!("{store}/foo/package.json").as_bytes(),
            br#"{"name":"foo","version":"1.0.0"}"#.as_slice(),
        );
        fs.insert_loaded(
            format!("{store}/foo/index.d.ts").as_bytes(),
            b"import 'globals'; export declare function useFoo(): typeof myGlobal;".as_slice(),
        );
        fs.insert_loaded(
            format!("{store}/globals/package.json").as_bytes(),
            format!(r#"{{"name":"globals","version":"{global_version}"}}"#).into_bytes(),
        );
        fs.insert_loaded(
            format!("{store}/globals/index.d.ts").as_bytes(),
            format!("declare var myGlobal: {global_type};").into_bytes(),
        );
    }
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        module: ModuleKind::COMMON_JS,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/main.ts".as_slice())],
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    );
    let retained = b"/store/a/node_modules/foo/index.d.ts";
    let redirected = b"/store/b/node_modules/foo/index.d.ts";
    let discarded = b"/store/b/node_modules/globals/index.d.ts";
    assert!(program.file(discarded).is_none());
    assert_eq!(
        program.file(retained).unwrap().source(),
        program.file(redirected).unwrap().source(),
    );
    let host = ProgramCheckerHost::new(program);
    assert_eq!(
        host.get_source_file_meta_data(redirected)
            .unwrap()
            .package_json_directory
            .as_bytes(),
        b"/store/a/node_modules/foo",
    );
    // This production call initializes GetSymlinkCache, which previously failed
    // on the discarded globals metadata before it could return any paths.
    let paths = host
        .get_module_specifier_paths(b"/main.ts", retained)
        .unwrap();
    assert!(host.program().metadata(discarded).is_none());
    for expected in [
        retained.as_slice(),
        redirected.as_slice(),
        b"/node_modules/a/node_modules/foo/index.d.ts",
    ] {
        assert!(paths
            .iter()
            .any(|path| path.file_name.as_bytes() == expected));
    }
}
