use super::*;
use crate::{
    extended_config::ExtendedConfigCache,
    overlay::FileHandle,
    snapshot_fs::{SnapshotFs, SnapshotFsBuilder},
};
use tsr_core::ScriptKind;
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile, TestFs},
};

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}
fn setup(files: &[(&str, &str)]) -> (Arc<TestFs>, Arc<SnapshotFs>, Overlays) {
    let fs = Arc::new(vfstest::from_map(
        &files
            .iter()
            .map(|(name, text)| {
                (
                    name.as_bytes().to_vec(),
                    InputFile::Text(text.as_bytes().to_vec()),
                )
            })
            .collect(),
        false,
    ));
    let snapshot = SnapshotFs::empty(Arc::new(iovfs::from(fs.clone(), false)), js("/"));
    let overlays = Arc::new(BTreeMap::from([(
        js("/project/src/main.ts"),
        Arc::new(FileHandle::overlay(
            js("/project/src/main.ts"),
            js("export const x = 1;"),
            1,
            ScriptKind::TS,
        )),
    )]));
    (fs, snapshot, overlays)
}
fn builder(
    base: Arc<ConfigFileRegistry>,
    fs: Arc<SnapshotFsBuilder>,
    overlays: Overlays,
    custom: &str,
    owner: Arc<ConfigOwnership>,
) -> ConfigRegistryBuilder {
    ConfigRegistryBuilder::new(base, fs, js("/"), overlays, js(custom), false, owner)
}

// Pinned TestCustomConfigFileName: a custom file anywhere in the ancestor
// chain wins before the nearest tsconfig/jsconfig fallback is considered.
#[test]
fn custom_precedence_same_directory_ancestors_and_search_boundary() {
    let (_, empty, overlays) = setup(&[
        ("/project/custom.json", "{}"),
        ("/project/src/tsconfig.json", "{}"),
        ("/project/src/jsconfig.json", "{}"),
        ("/tsconfig.json", "{}"),
    ]);
    let fs = Arc::new(SnapshotFsBuilder::new(empty, overlays.clone()));
    let owner = Arc::new(ConfigOwnership::new(Arc::default(), 1));
    let mut b = builder(
        Arc::default(),
        fs.clone(),
        overlays.clone(),
        "custom.json",
        owner.clone(),
    );
    assert_eq!(
        b.config_file_name(b"/project/src/main.ts").unwrap(),
        js("/project/custom.json")
    );
    let old = b.finalize();
    let mut b = builder(old.clone(), fs, overlays, "", owner);
    assert_eq!(
        b.config_file_name(b"/project/src/main.ts").unwrap(),
        js("/project/src/tsconfig.json")
    );
    assert_eq!(
        b.ancestor_config_file_name(b"/project/src/main.ts", &js("/project/src/tsconfig.json"))
            .unwrap(),
        js("/project/src/jsconfig.json")
    );
    assert_eq!(
        b.ancestor_config_file_name(b"/project/src/main.ts", &js("/project/src/jsconfig.json"))
            .unwrap(),
        js("/tsconfig.json")
    );
    assert!(b
        .config_file_name(b"/project/node_modules/pkg/index.ts")
        .unwrap()
        .is_empty());
    assert!(b
        .config_file_name(b"^/untitled/ts-nul-authority/Untitled-1")
        .unwrap()
        .is_empty());
    assert_eq!(
        old.file_names[&js("/project/src/main.ts")].nearest,
        js("/project/custom.json")
    );
}

#[test]
fn searched_configs_follow_the_nearest_chain_not_other_cached_ancestors() {
    let (_, empty, overlays) = setup(&[
        ("/project/src/tsconfig.json", "{}"),
        ("/project/tsconfig.json", "{}"),
        ("/else/tsconfig.json", "{}"),
        ("/tsconfig.json", "{}"),
    ]);
    let fs = Arc::new(SnapshotFsBuilder::new(empty, overlays.clone()));
    let owner = Arc::new(ConfigOwnership::new(Arc::default(), 1));
    let mut b = builder(Arc::default(), fs, overlays, "", owner);
    let file = js("/project/src/main.ts");
    assert!(b.searched_config_names(&file).is_empty());
    assert_eq!(
        b.config_file_name(file.as_bytes()).unwrap(),
        js("/project/src/tsconfig.json")
    );
    assert_eq!(
        b.ancestor_config_file_name(file.as_bytes(), &js("/project/src/tsconfig.json"))
            .unwrap(),
        js("/project/tsconfig.json")
    );
    assert_eq!(
        b.ancestor_config_file_name(file.as_bytes(), &js("/project/tsconfig.json"))
            .unwrap(),
        js("/tsconfig.json")
    );
    // A referenced default project may cache an ancestor outside the chain
    // rooted at the file's nearest config. Cleanup must not retain it.
    assert_eq!(
        b.ancestor_config_file_name(file.as_bytes(), &js("/else/src/tsconfig.json"))
            .unwrap(),
        js("/else/tsconfig.json")
    );
    assert!(b
        .ancestor_config_file_name(file.as_bytes(), &js("/tsconfig.json"))
        .unwrap()
        .is_empty());
    assert_eq!(
        b.searched_config_names(&file),
        vec![
            js("/project/src/tsconfig.json"),
            js("/project/tsconfig.json"),
            js("/tsconfig.json"),
        ]
    );
}

// Pinned TestExtendedConfigCacheOwnership: multi-extends shares one ancestor;
// retained snapshots retain their old syntax after the ancestor changes.
#[test]
fn extended_config_ownership_and_transitive_hash_invalidation() {
    let (live, empty, overlays) = setup(&[
        (
            "/project/tsconfig.json",
            r#"{"extends":["./base1.json","./base2.json"]}"#,
        ),
        (
            "/project/base1.json",
            r#"{"extends":"./root.json","compilerOptions":{"strict":true}}"#,
        ),
        ("/project/base2.json", r#"{"extends":"./root.json"}"#),
        (
            "/project/root.json",
            r#"{"compilerOptions":{"target":"ES2020"}}"#,
        ),
        ("/project/src/main.ts", "export const x = 1;"),
    ]);
    let cache = Arc::new(ExtendedConfigCache::default());
    let first_owner = Arc::new(ConfigOwnership::new(cache.clone(), 1));
    let fs = Arc::new(SnapshotFsBuilder::new(empty, overlays.clone()));
    let mut b = builder(
        Arc::default(),
        fs.clone(),
        overlays.clone(),
        "",
        first_owner.clone(),
    );
    let name = js("/project/tsconfig.json");
    let before = b.acquire_for_project(&name, &name).unwrap().unwrap();
    assert_eq!(before.extended_source_files().len(), 3);
    assert_eq!(cache.len(), 3);
    let registry = b.finalize();
    let snapshot = fs.finalize();
    // An unchanged registry is reused by identity, without re-parsing configs.
    let next_owner = Arc::new(ConfigOwnership::new(cache.clone(), 2));
    next_owner.inherit(&first_owner);
    let next_fs = Arc::new(SnapshotFsBuilder::new(snapshot.clone(), overlays.clone()));
    let mut b = builder(
        registry.clone(),
        next_fs,
        overlays.clone(),
        "",
        next_owner.clone(),
    );
    assert!(Arc::ptr_eq(
        &before,
        &b.acquire_for_project(&name, &name).unwrap().unwrap()
    ));
    assert!(Arc::ptr_eq(&registry, &b.finalize()));
    live.write_file(
        b"project/root.json",
        br#"{"compilerOptions":{"target":"ES2022"}}"#,
        0o644,
    )
    .unwrap();
    let next_fs = Arc::new(SnapshotFsBuilder::new(snapshot, overlays.clone()));
    let mut changes = FileChangeSummary {
        changed: BTreeSet::from([tsr_lsproto::DocumentUri::from_file_name(
            b"/project/root.json",
        )]),
        ..Default::default()
    };
    next_fs.mark_dirty_files(&mut changes).unwrap();
    let mut b = builder(registry, next_fs, overlays, "", next_owner.clone());
    assert!(b
        .did_change_files(&changes)
        .unwrap()
        .projects
        .contains(&name));
    let after = b.acquire_for_project(&name, &name).unwrap().unwrap();
    assert_ne!(before.options.target, after.options.target);
    assert!(!Arc::ptr_eq(
        &before.config_dependencies[0],
        &after.config_dependencies[0]
    ));
    drop(b);
    drop(next_owner);
    assert_eq!(cache.len(), 3);
    drop(first_owner);
    assert!(cache.is_empty());
}

// Pinned TestConfigFileChanges: wildcard additions/deletions reload file names,
// while explicitly listed missing files remain roots and produce diagnostics.
#[test]
fn wildcard_reload_keeps_config_syntax_and_explicit_roots() {
    let (live, empty, overlays) = setup(&[
        (
            "/project/tsconfig.json",
            r#"{"files":["missing.ts"],"include":["src/**/*.ts"]}"#,
        ),
        ("/project/src/main.ts", "export const x = 1;"),
    ]);
    let cache = Arc::new(ExtendedConfigCache::default());
    let owner = Arc::new(ConfigOwnership::new(cache, 1));
    let fs = Arc::new(SnapshotFsBuilder::new(empty, overlays.clone()));
    let mut b = builder(
        Arc::default(),
        fs.clone(),
        overlays.clone(),
        "",
        owner.clone(),
    );
    let name = js("/project/tsconfig.json");
    let first = b.acquire_for_project(&name, &name).unwrap().unwrap();
    let old = b.finalize();
    let old_fs = fs.finalize();
    live.write_file(b"project/src/extra.ts", b"export const y = 2;", 0o644)
        .unwrap();
    let fs = Arc::new(SnapshotFsBuilder::new(old_fs, overlays.clone()));
    let mut b = builder(old, fs, overlays, "", owner);
    let changes = FileChangeSummary {
        created: BTreeSet::from([tsr_lsproto::DocumentUri::from_file_name(
            b"/project/src/extra.ts",
        )]),
        ..Default::default()
    };
    assert!(b
        .did_change_files(&changes)
        .unwrap()
        .projects
        .contains(&name));
    let second = b.acquire_for_project(&name, &name).unwrap().unwrap();
    assert!(second.root_file_names.contains(&js("/project/missing.ts")));
    assert!(second
        .root_file_names
        .contains(&js("/project/src/extra.ts")));
    assert!(!first.root_file_names.contains(&js("/project/src/extra.ts")));
    assert!(Arc::ptr_eq(
        first.config_file.as_ref().unwrap(),
        second.config_file.as_ref().unwrap()
    ));
}
