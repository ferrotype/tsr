use serde_json::{json, Value};
use std::sync::Arc;
use ts_module::resolve_config;
use ts_vfs::MemoryBuilder;

#[test]
fn original_go_config_resolution() {
    let requests: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/config-resolver-requests.json"
    ))
    .unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/config-resolver-observations.json"
    ))
    .unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(&expected) {
        let text = |key| request[key].as_str().unwrap().as_bytes();
        let mut host =
            MemoryBuilder::new(text("cwd"), request["case_sensitive"].as_bool().unwrap());
        for (path, value) in request["files"].as_object().unwrap() {
            host.insert_physical(path.as_bytes(), value.as_str().unwrap().as_bytes());
        }
        let result = resolve_config(
            text("name"),
            text("file"),
            Arc::new(host.finish()),
            text("cwd"),
        )
        .unwrap();
        assert!(
            result.resolution_diagnostics.is_empty(),
            "{}: unexpected diagnostics",
            request["id"]
        );
        let p = &result.package_id;
        let observed = json!({"id":request["id"],"result":{"ResolutionDiagnostics":null,"ResolvedFileName":result.resolved_file_name.as_str().unwrap(),"OriginalPath":result.original_path.as_str().unwrap(),"Extension":result.extension.as_str().unwrap(),"ResolvedUsingTsExtension":result.resolved_using_ts_extension,"ResolvedUsingExtraExtensions":result.resolved_using_extra_extensions,"PackageId":{"Name":p.name.as_str().unwrap(),"SubModuleName":p.sub_module_name.as_str().unwrap(),"Version":p.version.as_str().unwrap(),"PeerDependencies":p.peer_dependencies.as_str().unwrap()},"IsExternalLibraryImport":result.is_external_library_import,"AlternateResult":result.alternate_result.as_str().unwrap()}});
        assert_eq!(&observed, expected, "{}", request["id"]);
    }
}
