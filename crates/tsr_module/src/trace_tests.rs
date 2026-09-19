use super::*;
use crate::Resolver;
use serde_json::{json, Value};
use std::sync::Arc;
use ts_vfs::MemoryBuilder;
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}
#[test]
fn independent_go_trace_callbacks_and_typed_arguments() {
    let requests: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/module-trace-requests.json")).unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/module-trace-observations.json"
    ))
    .unwrap();
    let mut output = Vec::new();
    for request in requests {
        let cwd = request["cwd"].as_str().unwrap().as_bytes();
        let mut files = MemoryBuilder::new(cwd, request["case_sensitive"].as_bool().unwrap());
        for (path, content) in request["files"].as_object().unwrap() {
            files.insert_loaded(
                path.as_bytes(),
                content.as_str().unwrap().as_bytes().to_vec(),
            );
        }
        let options = ts_tsoptions::raw::compiler_options(&request["options"]).unwrap();
        let mut resolver = Resolver::new(Arc::new(files.finish()), Arc::new(options), cwd).unwrap();
        let mut previous: Option<Arc<crate::PackageJson>> = None;
        for (index, operation) in request["operations"].as_array().unwrap().iter().enumerate() {
            let name = operation["name"].as_str().unwrap().as_bytes();
            let file = operation["file"].as_str().unwrap().as_bytes();
            let mode =
                ts_core::ModuleKind(i32::try_from(operation["mode"].as_i64().unwrap()).unwrap());
            let mut observed_package = Value::Null;
            match operation["kind"].as_str().unwrap() {
                "automatic" => {
                    resolver.automatic_type_directive_names().unwrap();
                }
                "metadata" => {
                    if let Some(entry) = resolver.package_scope(name).unwrap() {
                        observed_package = json!({"directory_hex":hex(entry.directory.as_bytes()),"same_entry":previous.as_ref().is_some_and(|previous|Arc::ptr_eq(previous,&entry)),"shared_contents":previous.as_ref().is_some_and(|previous|Arc::ptr_eq(&previous.shared,&entry.shared))});
                        previous = Some(entry);
                    }
                }
                "module" => {
                    resolver.resolve(name, file, mode).unwrap();
                }
                "type" => {
                    resolver.resolve_type_reference(name, file, mode).unwrap();
                }
                kind => panic!("unknown trace fixture operation {kind}"),
            }
            let traces:Vec<_>=resolver.take_trace().into_iter().map(|trace|json!({"code":trace.message.code,"args":trace.args.into_iter().map(|arg|match arg {TraceArg::Text(text)=>json!({"text_hex":hex(text.as_bytes())}),TraceArg::Bool(value)=>json!({"bool":value})}).collect::<Vec<_>>()})).collect();
            let actual = json!({"id":request["id"],"operation":index,"traces":traces,"package":observed_package});
            let wanted = &expected[output.len()];
            let a = actual["traces"].as_array().unwrap();
            let b = wanted["traces"].as_array().unwrap();
            for (position, (actual, expected)) in a.iter().zip(b).enumerate() {
                assert_eq!(
                    actual, expected,
                    "{} operation {index}, callback {position}",
                    request["id"]
                );
            }
            assert_eq!(
                actual,
                wanted.clone(),
                "{} operation {index}",
                request["id"]
            );
            output.push(actual);
        }
    }
    assert_eq!(output.len(), expected.len());
}
#[test]
fn disabled_trace_does_not_build_arguments_and_metadata_does_not_start_trace() {
    let files = MemoryBuilder::new(b"/repo", true).finish();
    let mut resolver = Resolver::new(
        Arc::new(files),
        Arc::new(ts_core::CompilerOptions::default()),
        b"/repo",
    )
    .unwrap();
    let called = std::cell::Cell::new(false);
    trace!(resolver, ts_diagnostics::File_0_does_not_exist, {
        called.set(true);
        b"/unused"
    });
    assert!(!called.get());
    assert!(resolver.take_trace().is_empty());
    resolver.package_scope(b"/repo").unwrap();
    assert!(resolver.take_trace().is_empty());
    assert!(resolver
        .resolve(
            b"./absent",
            b"/repo/main.ts",
            ts_core::ModuleKind::COMMON_JS
        )
        .is_ok());
    assert!(resolver.take_trace().is_empty());
    assert!(!resolver.tracer.active);
}

#[test]
fn trace_scope_retires_after_error_and_native_unwind() {
    let files = MemoryBuilder::new(b"/repo", true).finish();
    let options = ts_core::CompilerOptions {
        trace_resolution: ts_core::Tristate::TRUE,
        module_resolution: ts_core::ModuleResolutionKind(999),
        ..Default::default()
    };
    let mut resolver = Resolver::new(Arc::new(files), Arc::new(options), b"/repo").unwrap();
    assert!(resolver
        .resolve(b"missing", b"/repo/main.ts", ts_core::ModuleKind::COMMON_JS)
        .is_err());
    assert!(!resolver.tracer.active);
    resolver.take_trace();
    resolver.package_scope(b"/repo").unwrap();
    assert!(resolver.take_trace().is_empty());
    resolver.tracer.begin(true);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), crate::Error> =
            resolver.trace_operation(|_| panic!("native source failure"));
    }));
    assert_eq!(
        outcome.unwrap_err().downcast_ref::<&str>(),
        Some(&"native source failure")
    );
    assert!(!resolver.tracer.active);
}

#[test]
fn directory_alias_preserves_uncomputed_and_initialized_version_cache_identity() {
    let mut files = MemoryBuilder::new(b"/repo", false);
    files.insert_loaded(
        b"/repo/node_modules/pkg/package.json",
        b"{\"typesVersions\":{\"*\":{\"*\":[\"types/*\"]}}}".to_vec(),
    );
    let mut resolver = Resolver::new(
        Arc::new(files.finish()),
        Arc::new(ts_core::CompilerOptions::default()),
        b"/repo",
    )
    .unwrap();
    let first = resolver
        .package_json(b"/repo/node_modules/pkg")
        .unwrap()
        .unwrap();
    assert!(first.version_paths.get().is_none());
    let alias = resolver
        .package_json(b"/REPO/node_modules/pkg/")
        .unwrap()
        .unwrap();
    assert_eq!(alias.directory.as_bytes(), b"/REPO/node_modules/pkg/");
    assert!(!Arc::ptr_eq(&first, &alias));
    assert!(Arc::ptr_eq(&first.shared, &alias.shared));
    assert!(alias.version_paths.get().is_none());
    let (_, paths) = resolver.version_paths(&first).unwrap();
    let original = std::ptr::from_ref(paths);
    let (_, paths) = resolver.version_paths(&alias).unwrap();
    assert!(std::ptr::eq(original, paths));
    let repeated = resolver
        .package_json(b"/repo/node_modules/pkg")
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&first, &repeated));
}
