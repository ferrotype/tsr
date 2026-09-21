use std::sync::Arc;
use tsr_core::{CompilerOptions, ModuleKind, Tristate};
use tsr_jsstring::JsString;
use tsr_module::Resolver;
use tsr_vfs::MemoryBuilder;

#[test]
fn paths_mappings_do_not_intercept_relative_backslash_imports() {
    let mut host = MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/shim.ts", b"export {};".as_slice());
    host.insert_loaded(b"/mapped.ts", b"export {};".as_slice());
    let mut resolver = Resolver::new(
        Arc::new(host.finish()),
        Arc::new(CompilerOptions {
            trace_resolution: Tristate::TRUE,
            paths: Some(
                [(
                    JsString::from_bytes(b"*".as_slice()),
                    Some(vec![JsString::from_bytes(b"/mapped.ts".as_slice())]),
                )]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        }),
        b"/",
    )
    .unwrap();
    for (name, file) in [
        (b".\\shim".as_slice(), b"/main.ts".as_slice()),
        (b"..\\shim".as_slice(), b"/nested/main.ts".as_slice()),
    ] {
        let result = resolver.resolve(name, file, ModuleKind::NONE).unwrap();
        assert_eq!(result.resolved_file_name.as_bytes(), b"/shim.ts");
        assert!(!resolver.take_trace().iter().any(|entry| entry.message.code == tsr_diagnostics::X_paths_option_is_specified_looking_for_a_pattern_to_match_module_name_0.code));
    }
    assert_eq!(
        resolver
            .resolve(b"shim", b"/main.ts", ModuleKind::NONE)
            .unwrap()
            .resolved_file_name
            .as_bytes(),
        b"/mapped.ts"
    );
    assert!(resolver.take_trace().iter().any(|entry| entry.message.code == tsr_diagnostics::X_paths_option_is_specified_looking_for_a_pattern_to_match_module_name_0.code));
}

#[test]
fn paths_keep_source_order_on_equal_prefixes_but_exact_matches_win() {
    let mut host = MemoryBuilder::new(b"/", true);
    for name in [b"/first.ts".as_slice(), b"/second.ts", b"/exact.ts"] {
        host.insert_loaded(name, b"export {};".as_slice());
    }
    // Both wildcard patterns have a one-byte prefix. The suffix is not a
    // tie-breaker in the pin's FindBestPatternMatch; first insertion wins.
    let paths = [
        ("x*", "/first.ts"),
        ("x*z", "/second.ts"),
        ("xy", "/exact.ts"),
    ]
    .into_iter()
    .map(|(key, path)| {
        (
            JsString::from_bytes(key.as_bytes()),
            Some(vec![JsString::from_bytes(path.as_bytes())]),
        )
    })
    .collect();
    let mut resolver = Resolver::new(
        Arc::new(host.finish()),
        Arc::new(CompilerOptions {
            paths: Some(paths),
            ..Default::default()
        }),
        b"/",
    )
    .unwrap();
    assert_eq!(
        resolver
            .resolve(b"xyz", b"/main.ts", ModuleKind::NONE)
            .unwrap()
            .resolved_file_name
            .as_bytes(),
        b"/first.ts"
    );
    assert_eq!(
        resolver
            .resolve(b"xy", b"/main.ts", ModuleKind::NONE)
            .unwrap()
            .resolved_file_name
            .as_bytes(),
        b"/exact.ts"
    );
}
