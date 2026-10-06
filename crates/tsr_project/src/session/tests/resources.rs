use super::*;
use crate::api::{ApiSnapshotRequest, ProjectTreeRequest, ResourceRequest};

#[test]
fn api_opens_are_counted_and_do_not_replace_editor_overlays() {
    let (_fs, session) = setup(
        &[
            (
                "/app/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            ("/app/main.ts", "export const disk = 1;"),
        ],
        &Counters::new(),
    );
    let request = ApiSnapshotRequest {
        open_projects: Some([js("/app/tsconfig.json")].into()),
        ..Default::default()
    };
    let retained = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            request.clone(),
        )
        .unwrap()
        .snapshot;
    session
        .api_update(crate::file_change::FileChangeSummary::default(), request)
        .unwrap();
    let close = ApiSnapshotRequest {
        close_projects: Some([js("/app/tsconfig.json")].into()),
        ..Default::default()
    };
    let first = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            close.clone(),
        )
        .unwrap();
    assert!(first.error.is_none());
    assert!(first
        .snapshot
        .project_by_path(b"/app/tsconfig.json")
        .is_some());
    let second = session
        .api_update(crate::file_change::FileChangeSummary::default(), close)
        .unwrap();
    assert!(second
        .snapshot
        .project_by_path(b"/app/tsconfig.json")
        .is_none());
    assert_eq!(text(&retained, "/app/main.ts"), b"export const disk = 1;");

    open(&session, "/app/main.ts", "export const overlay = 2;");
    session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            ApiSnapshotRequest {
                open_files: Some([uri("/app/main.ts")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    let closed = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            ApiSnapshotRequest {
                close_files: Some([js("/app/main.ts")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        text(&closed.snapshot, "/app/main.ts"),
        b"export const overlay = 2;"
    );
}

#[test]
fn api_open_file_searches_configs_and_failed_open_returns_a_retained_snapshot() {
    let (_fs, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["src/a.ts"]}"#,
            ),
            ("/src/a.ts", "export const a = 1;"),
        ],
        &Counters::new(),
    );
    let opened = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            ApiSnapshotRequest {
                open_files: Some([uri("/src/a.ts")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(opened.error.is_none());
    assert_eq!(
        opened
            .snapshot
            .project_for_file(b"/src/a.ts")
            .unwrap()
            .data()
            .unwrap()
            .path,
        js("/tsconfig.json")
    );
    let failed = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            ApiSnapshotRequest {
                open_projects: Some([js("/missing.json")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        failed.error.unwrap(),
        js("project not found for open: /missing.json")
    );
    assert_eq!(text(&failed.snapshot, "/src/a.ts"), b"export const a = 1;");
    let closed = session
        .api_update(
            crate::file_change::FileChangeSummary::default(),
            ApiSnapshotRequest {
                close_files: Some([js("/src/a.ts")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(closed.snapshot.projects().is_empty());
}

#[test]
fn project_tree_loads_unopened_sibling_and_respects_disabled_child_load() {
    for disabled in [false, true] {
        let root = format!(
            r#"{{"compilerOptions":{{"noLib":true,"disableReferencedProjectLoad":{disabled}}},"files":[],"references":[{{"path":"./a"}},{{"path":"./b"}}]}}"#
        );
        let (_fs, session) = setup(
            &[
                ("/tsconfig.json", &root),
                (
                    "/a/tsconfig.json",
                    r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
                ),
                ("/a/a.ts", "export const a = 1;"),
                (
                    "/b/tsconfig.json",
                    r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#,
                ),
                ("/b/b.ts", "import {a} from '../a/a'; a;"),
            ],
            &Counters::new(),
        );
        open(&session, "/a/a.ts", "export const a = 1;");
        let loaded = session
            .flush_resources(
                &ResourceRequest {
                    project_tree: Some(ProjectTreeRequest::All),
                    ..Default::default()
                },
                session.fs.clone(),
            )
            .unwrap();
        assert_eq!(
            loaded.project_by_path(b"/b/tsconfig.json").is_some(),
            !disabled
        );
        assert!(loaded.project_by_path(b"/a/tsconfig.json").is_some());
    }
}

#[test]
fn targeted_tree_loads_only_referencing_branches_and_does_not_create_extends_projects() {
    let (_fs, session) = setup(
        &[
            ("/base.json", r#"{"compilerOptions":{"noLib":true}}"#),
            (
                "/tsconfig.json",
                r#"{"extends":"./base.json","files":[],"references":[{"path":"./a"},{"path":"./b"},{"path":"./c"}]}"#,
            ),
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#,
            ),
            ("/b/b.ts", "import { a } from '../a/a'; a;"),
            (
                "/c/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["c.ts"]}"#,
            ),
            ("/c/c.ts", "export const c = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    let loaded = session
        .flush_resources(
            &ResourceRequest {
                project_tree: Some(ProjectTreeRequest::Referencing(
                    [js("/a/tsconfig.json")].into(),
                )),
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    assert!(loaded.project_by_path(b"/b/tsconfig.json").is_some());
    assert!(loaded.project_by_path(b"/c/tsconfig.json").is_none());
    assert!(loaded.project_by_path(b"/base.json").is_none());
}

#[test]
fn disabled_solution_searching_keeps_ancestor_tree_undiscovered() {
    let (_fs, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"files":[],"references":[{"path":"./a"},{"path":"./b"}]}"#,
            ),
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true,"disableSolutionSearching":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"]}"#,
            ),
            ("/b/b.ts", "export const b = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    let loaded = session
        .flush_resources(
            &ResourceRequest {
                project_tree: Some(ProjectTreeRequest::All),
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    assert!(loaded.project_by_path(b"/a/tsconfig.json").is_some());
    assert!(loaded.project_by_path(b"/b/tsconfig.json").is_none());
    assert!(loaded.project_by_path(b"/tsconfig.json").is_none());
}

#[test]
fn ancestor_projects_are_delayed_without_parsing_their_configs() {
    let (_fs, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"files":[],"references":[{"path":"./a"}]}"#,
            ),
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    let delayed = session.flush(None).unwrap();
    assert_eq!(delayed.delayed_projects().len(), 1);
    let ancestor = delayed.delayed_projects()[0];
    assert_eq!(ancestor.name, js("/tsconfig.json"));
    assert_eq!(
        ancestor.potential_project_references,
        [js("/a/tsconfig.json")].into()
    );
    assert!(delayed.project_by_path(b"/tsconfig.json").is_none());
    assert!(!delayed
        .configs()
        .unwrap()
        .configs
        .contains_key(&js("/tsconfig.json")));
    let loaded = session
        .flush_resources(
            &ResourceRequest {
                projects: [js("/tsconfig.json")].into(),
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    assert!(loaded.delayed_projects().is_empty());
    assert!(loaded.project_by_path(b"/tsconfig.json").is_some());
    assert_eq!(delayed.delayed_projects().len(), 1);
}

#[test]
fn delayed_root_seen_before_its_parent_is_loaded_when_its_child_references_the_target() {
    let (_fs, session) = setup(
        &[
            (
                "/sol/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":[],"references":[{"path":"./d"},{"path":"./b"}]}"#,
            ),
            (
                "/sol/d/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":[],"references":[{"path":"./a"},{"path":"../b"}]}"#,
            ),
            (
                "/sol/d/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
            ),
            ("/sol/d/a/a.ts", "export const a = 1;"),
            (
                "/sol/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"]}"#,
            ),
            ("/sol/b/b.ts", "export const b = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/sol/d/a/a.ts", "export const a = 1;");
    let opened = session
        .api_update(
            FileChangeSummary::default(),
            ApiSnapshotRequest {
                open_projects: Some([js("/sol/tsconfig.json")].into()),
                ..Default::default()
            },
        )
        .unwrap()
        .snapshot;
    assert!(opened
        .delayed_projects()
        .iter()
        .any(|project| project.path == js("/sol/d/tsconfig.json")));
    let loaded = session
        .flush_resources(
            &ResourceRequest {
                project_tree: Some(ProjectTreeRequest::Referencing(
                    [js("/sol/b/tsconfig.json")].into(),
                )),
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    assert!(loaded.project_by_path(b"/sol/d/tsconfig.json").is_some());
}

#[test]
fn closing_an_api_project_does_not_retain_it_for_another_projects_overlay() {
    let (_fs, session) = setup(
        &[
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["../a/a.ts"]}"#,
            ),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    session
        .api_update(
            FileChangeSummary::default(),
            ApiSnapshotRequest {
                open_projects: Some([js("/b/tsconfig.json")].into()),
                ..Default::default()
            },
        )
        .unwrap();
    let closed = session
        .api_update(
            FileChangeSummary::default(),
            ApiSnapshotRequest {
                close_projects: Some([js("/b/tsconfig.json")].into()),
                ..Default::default()
            },
        )
        .unwrap()
        .snapshot;
    assert!(closed.project_by_path(b"/b/tsconfig.json").is_none());
    assert!(closed.project_by_path(b"/a/tsconfig.json").is_some());
}

#[test]
fn loaded_ancestor_references_survive_an_unrelated_open_and_retain_configs() {
    let (_, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"files":[],"references":[{"path":"./a"},{"path":"./b"}]}"#,
            ),
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#,
            ),
            ("/b/b.ts", "import { a } from '../a/a'; a;"),
            ("/other/main.ts", "export const other = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    let loaded = session
        .flush_resources(
            &ResourceRequest {
                project_tree: Some(ProjectTreeRequest::All),
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    let config = &loaded.configs().unwrap().configs[&js("/a/tsconfig.json")];
    assert_eq!(
        config.retaining_projects,
        std::collections::BTreeSet::from([
            js("/a/tsconfig.json"),
            js("/b/tsconfig.json"),
            js("/tsconfig.json")
        ])
    );
    let later = open(&session, "/other/main.ts", "export const other = 1;");
    assert!(later.project_by_path(b"/b/tsconfig.json").is_some());
    assert_eq!(
        later.configs().unwrap().configs[&js("/a/tsconfig.json")].retaining_projects,
        config.retaining_projects
    );
}

#[test]
fn rebuilt_program_releases_only_dropped_reference_config_ownership() {
    let (fs, session) = setup(
        &[
            (
                "/a/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#,
            ),
            ("/a/a.ts", "export const a = 1;"),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#,
            ),
            ("/b/b.ts", "export const b = 1;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/a.ts", "export const a = 1;");
    let before = open(&session, "/b/b.ts", "export const b = 1;");
    assert!(before.configs().unwrap().configs[&js("/a/tsconfig.json")]
        .retaining_projects
        .contains(&js("/b/tsconfig.json")));
    fs.write_file(
        b"b/tsconfig.json",
        br#"{"compilerOptions":{"noLib":true,"composite":true},"files":["b.ts"]}"#,
        0o644,
    )
    .unwrap();
    session
        .enqueue(FileChange::new(
            FileChangeKind::WatchChange,
            uri("/b/tsconfig.json"),
        ))
        .unwrap();
    let after = session.flush(Some(&uri("/b/b.ts"))).unwrap();
    assert_eq!(
        after.configs().unwrap().configs[&js("/a/tsconfig.json")].retaining_projects,
        std::collections::BTreeSet::from([js("/a/tsconfig.json")])
    );
    assert!(
        before.configs().unwrap().configs[&js("/a/tsconfig.json")]
            .retaining_projects
            .contains(&js("/b/tsconfig.json")),
        "retained snapshots must keep their previous edges"
    );
}

#[test]
fn inferred_resource_roots_survive_close_until_the_next_open_cleanup() {
    let (_, session) = setup(
        &[
            ("/user.ts", "const user = 1;"),
            ("/generated/a.d.ts", "export const a: number;"),
        ],
        &Counters::new(),
    );
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    open(&session, "/user.ts", "const user = 1;");
    let retained = session
        .flush_resources(
            &ResourceRequest {
                documents: vec![uri("/generated/a.d.ts")],
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    session.did_close_file(uri("/user.ts")).unwrap();
    let closed = session.flush(None).unwrap();
    let roots = &closed
        .project_by_path(INFERRED_PROJECT_NAME)
        .unwrap()
        .data()
        .unwrap()
        .command_line
        .root_file_names;
    assert_eq!(roots, &[js("/generated/a.d.ts")]);
    let reopened = open(&session, "/other.ts", "const other = 1;");
    assert_eq!(
        reopened
            .project_by_path(INFERRED_PROJECT_NAME)
            .unwrap()
            .data()
            .unwrap()
            .command_line
            .root_file_names,
        [js("/other.ts")]
    );
    assert_eq!(
        retained
            .project_by_path(INFERRED_PROJECT_NAME)
            .unwrap()
            .data()
            .unwrap()
            .command_line
            .root_file_names,
        [js("/generated/a.d.ts"), js("/user.ts")]
    );
}

#[test]
fn requested_unowned_solution_configs_survive_until_open_file_cleanup() {
    let (_, session) = setup(
        &[
            (
                "/solution/tsconfig.json",
                r#"{"files":[],"references":[{"path":"./child"}],"compilerOptions":{"disableReferencedProjectLoad":true}}"#,
            ),
            (
                "/solution/child/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"composite":true},"files":["main.ts"]}"#,
            ),
            ("/solution/child/main.ts", "export const value = 1;"),
        ],
        &Counters::new(),
    );
    let looked_up = session
        .flush_resources(
            &ResourceRequest {
                configured_documents: vec![uri("/solution/other.ts")],
                ..Default::default()
            },
            session.fs.clone(),
        )
        .unwrap();
    let key = js("/solution/tsconfig.json");
    let config = looked_up
        .configs()
        .unwrap()
        .configs
        .get(&key)
        .expect("request lookup must publish the parsed solution config");
    assert!(config.retaining_projects.is_empty());
    assert!(config.retaining_open_files.is_empty());
    let clean = open(&session, "/unrelated.ts", "const unrelated = 1;");
    assert!(!clean.configs().unwrap().configs.contains_key(&key));
    assert!(
        looked_up.configs().unwrap().configs.contains_key(&key),
        "retained snapshot keeps the original registry"
    );
}
