//! Supplied checker-host resolution state, independently executed by
//! `tsr_checker/src/alternate_resolution_native_test.go` against the pin.
//! This does not prove ordinary source/resolver reachability of this branch.
use super::*;
use tsr_arena::{CheckerIdentity, Generation};
use tsr_checker::CheckerOwner;

fn freeze(d: &Diagnostic) -> serde_json::Value {
    serde_json::json!({
        "code": d.code,
        "pos": d.loc.pos(),
        "end": d.loc.end(),
        "args": d.message_args.iter().map(|arg| std::str::from_utf8(arg.as_bytes()).unwrap()).collect::<Vec<_>>(),
        "chain": d.message_chain.iter().map(|child| freeze(child)).collect::<Vec<_>>(),
        "repopulate": d.repopulate_info.as_ref().map(|info| serde_json::json!({
            "Kind": info.kind,
            "ModuleReference": std::str::from_utf8(info.module_reference.as_bytes()).unwrap(),
            "Mode": info.mode,
            "PackageName": std::str::from_utf8(info.package_name.as_bytes()).unwrap(),
        })),
    })
}

#[test]
fn supplied_alternate_resolution_matches_native_chain_and_repopulate() {
    let native: serde_json::Value =
        serde_json::from_str(include_str!("alternate_resolution_native.json")).unwrap();
    let rows = native.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .map(|row| row["chain"][0]["args"][0].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "/p/node_modules/@scope/pkg/index.d.ts",
            "/p/node_modules/@types/scope__pkg/index.d.ts"
        ]
    );
    for row in rows {
        let counters = Counters::new();
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/p", true);
        fs.insert_loaded(
            b"/p/main.ts",
            b"import {value} from \"@scope/pkg\";\n".as_slice(),
        );
        let options = CompilerOptions {
            no_lib: Tristate::TRUE,
            module: ModuleKind::ESNEXT,
            module_resolution: tsr_core::ModuleResolutionKind::BUNDLER,
            ..Default::default()
        };
        let mut program = Program::load(
            ProgramOptions {
                config: ParsedCommandLine::new(
                    options,
                    vec![JsString::from_bytes(b"/p/main.ts".as_slice())],
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/p".as_slice()),
                default_library_path: JsString::from_bytes(b"/lib".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap();
        assert_eq!(program.resolutions.len(), 1);
        // Keep the real parsed/bound program and mode. Supply the same host
        // result as the Go Program override, whose empty filename is unresolved.
        let resolution = &mut program.resolutions[0].result;
        assert!(!resolution.is_resolved());
        resolution.alternate_result =
            JsString::from_bytes(row["chain"][0]["args"][0].as_str().unwrap().as_bytes());
        let program = Arc::new(program);
        let source = program.file(b"/p/main.ts").unwrap().source();
        let owner = Arc::new(
            CheckerOwner::for_program(
                CheckerIdentity::new(Generation::new(&counters), &counters),
                &counters,
                Arc::new(crate::ProgramCheckerHost::new(program)),
            )
            .unwrap(),
        );
        let mut operation = owner.operation().unwrap();
        let diagnostics = operation.semantic_diagnostics(source).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].file, Some(source));
        assert_eq!(freeze(&diagnostics[0]), *row);
    }
}
