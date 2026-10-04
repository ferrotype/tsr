use super::*;
fn js(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}
// source: tsc/internal/project/watch_test.go:TestGetPathComponentsForWatching
#[test]
fn perceived_roots_keep_home_directories_and_unc_shares_together() {
    for (input, expected) in [
        ("/project", vec!["/", "project"]),
        ("C:\\project", vec!["C:/", "project"]),
        (
            "//server/share/project/tsconfig.json",
            vec!["//server/share", "project", "tsconfig.json"],
        ),
        (
            r"\server\share\project\tsconfig.json",
            vec!["/", "server", "share", "project", "tsconfig.json"],
        ),
        (
            r"\\server\share\project\tsconfig.json",
            vec!["//server/share", "project", "tsconfig.json"],
        ),
        ("C:\\Users", vec!["C:/Users"]),
        (
            "C:\\Users\\andrew\\project",
            vec!["C:/Users/andrew", "project"],
        ),
        ("/home", vec!["/home"]),
        ("/home/andrew/project", vec!["/home/andrew", "project"]),
    ] {
        assert_eq!(
            components_for_watching(input.as_bytes(), b""),
            expected
                .into_iter()
                .map(|s| s.as_bytes().to_vec())
                .collect::<Vec<_>>(),
            "{input}"
        );
    }
}
#[test]
fn cloned_watches_preserve_glob_storage_and_distinguish_changed_inputs() {
    let original =
        WatchedFiles::new(js("program files"), ALL_CHANGES, true).with_input(PatternsAndIgnored {
            directories_outside_workspace: vec![js("/home/other/lib")],
            patterns_inside_workspace: vec![js("/src/**/*"), js("/src/**/*")],
            ..Default::default()
        });
    let old = original.watchers();
    assert_eq!(old.workspace.len(), 1);
    assert_eq!(
        old.outside_workspace[0].glob_string(),
        js("file:///home/other/lib/**/*")
    );
    let same = original.with_input(original.input.clone());
    assert!(Arc::ptr_eq(&same.watchers().workspace, &old.workspace));
    assert!(Arc::ptr_eq(
        &same.watchers().outside_workspace,
        &old.outside_workspace
    ));
    assert_eq!(
        same.id(),
        &js("program files watcher 0"),
        "the pin's Clone leaves id at zero when no glob changed"
    );
    let changed = original.with_input(PatternsAndIgnored {
        patterns_inside_workspace: vec![js("/other/**/*")],
        ..Default::default()
    });
    assert_ne!(changed.id(), original.id());
    assert_eq!(old.workspace[0].pattern, js("/src/**/*"));
    assert!(old.outside_workspace[0]
        .to_protocol()
        .unwrap()
        .glob_pattern
        .relative_pattern
        .is_some());
}
#[test]
fn registry_retains_the_original_registration_until_its_last_user_leaves() {
    let registry = WatchRegistry::default();
    let watch = recursive_directory_watcher(&js("/src"), ALL_CHANGES, false);
    assert!(registry.acquire(&watch, js("first")));
    assert!(!registry.acquire(&watch, js("second")));
    registry.mark_pending(&js("group"));
    assert!(registry.is_pending(&js("group")));
    registry.clear_pending(&js("group"));
    assert!(!registry.is_pending(&js("group")));
    assert_eq!(registry.release(&watch), None);
    assert_eq!(registry.release(&watch), Some(js("first")));
    assert_eq!(registry.release(&watch), None);
}
#[test]
fn lookups_watch_misses_and_group_external_parents_without_watching_a_home_root() {
    let files = [
        "/workspace/a.ts",
        "/workspace/missing.ts",
        "/lib/lib.d.ts",
        "/work/b.ts",
        "/outside/node_modules/pkg/a.ts",
        "/home/me",
        "/home/other/project/a.ts",
        "^untitled-1",
    ]
    .into_iter()
    .map(js)
    .collect();
    let result = resolution_patterns(&files, b"/workspace", b"/lib", b"/work", true);
    assert_eq!(
        result.patterns_inside_workspace,
        [
            "/lib/**/*",
            "/outside/node_modules/**/*",
            "/work/**/*",
            "/workspace/**/*"
        ]
        .into_iter()
        .map(js)
        .collect::<Vec<_>>()
    );
    assert_eq!(
        result.directories_outside_workspace,
        vec![js("/home/other/project")]
    );
    assert!(result.ignored.contains(&js("/home")));
}
#[test]
fn invalid_unicode_paths_are_kept_until_the_protocol_boundary() {
    let watch = Watcher {
        pattern: JsString::from_bytes(b"/x\xff/**/*".as_slice()),
        kind: ALL_CHANGES,
        base_uri: None,
    };
    assert_eq!(watch.glob_string().as_bytes(), b"/x\xff/**/*");
    assert!(watch.to_protocol().is_err());
}
