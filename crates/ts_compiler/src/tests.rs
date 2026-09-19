use crate as ts_compiler_error;
use crate::{FileCache, Program, ProgramOptions};
use serde_json::{json, Value};
use std::sync::Arc;
use ts_arena::Counters;
use ts_jsstring::JsString;
use ts_vfs::MemoryBuilder;
#[path = "../../../tools/s07/program/rust_observation.rs"]
mod observation;
use observation::{bytes, observe};
fn load(request: &Value, cache: &mut FileCache, counters: &Counters) -> Program {
    observation::try_load(request, cache, counters, None)
        .unwrap_or_else(|e| panic!("{}: {e:?}", request["id"]))
}
#[test]
fn actual_go_loader_observations() {
    let requests: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/program-requests.json")).unwrap();
    let expected: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/program-observations.json")).unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        let id = request["id"].as_str().unwrap();
        assert_eq!(expected["ID"], id);
        let program = load(request, &mut FileCache::new(), &Counters::new());
        assert_eq!(observe(id, &program), expected, "{id}");
    }
}
#[test]
fn retained_snapshot_edit_reuses_only_equal_parse_inputs() {
    let requests: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/program-requests.json")).unwrap();
    let mut request = requests[0].clone();
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let before = load(&request, &mut cache, &counters);
    let old_main = before.file(b"/src/main.ts").unwrap();
    let old_dep = before.file(b"/src/dep.ts").unwrap();
    let old_source = old_main.source();
    let dep_source = old_dep.source();
    let retained = old_main.bound().clone();
    let locals = retained
        .view()
        .node_binding(old_source)
        .unwrap()
        .unwrap()
        .locals
        .unwrap();
    let old_m = retained
        .view()
        .result()
        .tables()
        .get(locals)
        .unwrap()
        .get(b"m".as_slice())
        .unwrap()
        .unwrap();
    request["files"]["/src/main.ts"] = json!("6578706f727420636f6e7374206d3d34323b");
    request["roots"] = json!(["/src/main.ts", "/src/dep.ts"]);
    let after = load(&request, &mut cache, &counters);
    assert_ne!(after.file(b"/src/main.ts").unwrap().source(), old_source);
    assert_eq!(after.file(b"/src/dep.ts").unwrap().source(), dep_source);
    assert_eq!(
        retained.view().source_file().unwrap().text().as_bytes(),
        bytes(requests[0]["files"]["/src/main.ts"].as_str().unwrap())
    );
    drop(before);
    drop(after);
    cache.prune();
    assert!(retained.view().node(old_source).is_ok());
    assert_eq!(retained.view().symbol(old_m).unwrap().name_bytes(), b"m");
    drop(retained);
    cache.prune();
    assert_eq!(counters.snapshot(), ts_arena::Counts::default());
}

#[test]
fn resolver_scope_routes_retained_files_and_rejects_foreign_generations() {
    use ts_binder::name_resolver::ResolverHost;
    let requests: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/program-requests.json")).unwrap();
    let counters = Counters::new();
    let program = load(&requests[0], &mut FileCache::new(), &counters);
    let foreign = load(&requests[0], &mut FileCache::new(), &counters);
    let before = counters.snapshot();
    {
        let mut host = program.resolver_host(&counters);
        for file in program.files() {
            assert!(host.ast(file.source()).is_ok());
            let table = host
                .binding(file.source())
                .unwrap()
                .unwrap()
                .locals
                .unwrap();
            for symbol in host
                .table(table)
                .unwrap()
                .iter()
                .filter_map(|(_, symbol)| symbol)
            {
                assert!(host.symbol(symbol).is_ok());
                assert!(!host.declarations(symbol).unwrap().is_empty());
            }
        }
        assert!(matches!(
            host.ast(foreign.files()[0].source()),
            Err(ts_arena::Error::WrongOwner)
        ));
        let transient = host
            .new_transient_symbol(
                ts_ast::symbol_flags::PROPERTY | ts_ast::symbol_flags::TRANSIENT,
                JsString::from_bytes(b"arguments".as_slice()),
            )
            .unwrap();
        assert_eq!(host.symbol(transient).unwrap().name_bytes(), b"arguments");
        assert!(host.declarations(transient).unwrap().is_empty());
    }
    assert_eq!(counters.snapshot(), before);
    drop(program);
    drop(foreign);
    assert_eq!(counters.snapshot(), ts_arena::Counts::default());
}

#[test]
fn unimplemented_resolution_does_not_become_unresolved_success() {
    use ts_vfs::FileSystem;
    let mut builder = MemoryBuilder::new(b"/src", true);
    builder.insert_physical(b"/src/main.ts", b"import 'pkg'".as_slice());
    builder.insert_physical(
        b"/src/node_modules/pkg/package.json",
        b"{\"typesVersions\":{\"*\":{\"*\":[\"./index.js\"]}}}".as_slice(),
    );
    let host: Arc<dyn FileSystem> = Arc::new(builder.finish());
    let mut resolver = ts_module::Resolver::new(
        host,
        Arc::new(ts_core::CompilerOptions {
            module_resolution: ts_core::ModuleResolutionKind(-1),
            ..Default::default()
        }),
        b"/src",
    )
    .unwrap();
    assert!(matches!(
        resolver.resolve(b"pkg", b"/src/main.ts", ts_core::ModuleKind::ESNEXT),
        Err(ts_module::Error::Unsupported("module resolution kind"))
    ));
}
