use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tsr_core::Tristate;
use tsr_vfs::{Entries, FileContent, FileInfo, MemoryBuilder, MemorySnapshot, SnapshotId};

fn strings(values: &[&str]) -> Vec<JsString> {
    values.iter().map(js).collect()
}
fn info(imports: &[&str]) -> TypingsInfo {
    TypingsInfo {
        type_acquisition: TypeAcquisition {
            enable: Tristate::TRUE,
            ..Default::default()
        },
        unresolved_imports: strings(imports).into_iter().collect(),
        ..Default::default()
    }
}
fn memory(files: &[(&str, &str)]) -> MemorySnapshot {
    let mut builder = MemoryBuilder::new(b"/", false);
    for (file, content) in files {
        builder.insert_loaded(file.as_bytes(), content.as_bytes());
    }
    builder.finish()
}
fn discovery(
    files: &[(&str, &str)],
    names: &[&str],
    info: &TypingsInfo,
    cache: &BTreeMap<JsString, CachedTyping>,
    registry: &TypesRegistry,
) -> DiscoveredTypings {
    discover_typings(
        &memory(files),
        &Logger::nop(),
        info,
        &strings(names),
        b"/project",
        cache,
        registry,
    )
    .unwrap()
}
fn version_tags(version: &str) -> BTreeMap<JsString, JsString> {
    BTreeMap::from([(js("latest"), js(version))])
}
fn cached(name: &str, version: &str) -> (JsString, CachedTyping) {
    (
        js(name),
        CachedTyping {
            typings_location: js(format!("/cache/node_modules/@types/{name}/index.d.ts")),
            version: Some(Version::must_parse(version.as_bytes())),
        },
    )
}

#[test]
fn pinned_discovery_safe_list_and_core_modules() {
    let found = discovery(
        &[],
        &[
            "/project/app.js",
            "/project/jquery.js",
            "/project/chroma.min.js",
        ],
        &info(&[]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(found.new_typing_names, strings(&["chroma-js", "jquery"]));
    assert!(found.cached_typing_paths.is_empty());
    assert_eq!(
        found.files_to_watch,
        strings(&["/project/bower_components", "/project/node_modules"])
    );
    let found = discovery(
        &[],
        &["/project/app.js"],
        &info(&["assert", "somename"]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(found.new_typing_names, strings(&["node", "somename"]));
}

#[test]
fn pinned_discovery_cached_removed_expired_and_prerelease_versions() {
    let cache = BTreeMap::from([cached("node", "1.3.0"), cached("commander", "1.0.0")]);
    let registry = BTreeMap::from([
        (js("node"), version_tags("1.3.0")),
        (js("commander"), version_tags("1.3.0")),
    ]);
    let found = discovery(
        &[],
        &["/project/app.js"],
        &info(&["fs", "commander"]),
        &cache,
        &registry,
    );
    assert_eq!(
        found.cached_typing_paths,
        strings(&["/cache/node_modules/@types/node/index.d.ts"])
    );
    assert_eq!(found.new_typing_names, strings(&["commander"]));
    let found = discovery(
        &[],
        &["/project/app.js"],
        &info(&["fs", "bar"]),
        &cache,
        &BTreeMap::new(),
    );
    assert!(found.cached_typing_paths.is_empty());
    assert_eq!(found.new_typing_names, strings(&["bar", "node"]));
    let cache = BTreeMap::from([
        cached("node", "1.3.0-next.0"),
        cached("commander", "1.3.0-next.0"),
    ]);
    let mut node = version_tags("1.3.0");
    node.insert(
        js(format!("ts{}", tsr_core::version_major_minor())),
        js("1.3.0-next.1"),
    );
    let registry = BTreeMap::from([(js("node"), node), (js("commander"), version_tags("1.3.0"))]);
    let found = discovery(
        &[],
        &["/project/app.js"],
        &info(&["http", "commander"]),
        &cache,
        &registry,
    );
    assert!(found.cached_typing_paths.is_empty());
    assert_eq!(found.new_typing_names, strings(&["commander", "node"]));
    let cache = BTreeMap::from([cached("node", "1.0.0")]);
    let registry = BTreeMap::from([(js("node"), version_tags("1.3.0"))]);
    assert_eq!(
        discovery(&[], &[], &info(&["http"]), &cache, &registry).new_typing_names,
        strings(&["node"])
    );
}

#[test]
fn pinned_discovery_search_depth_and_scoped_packages() {
    let found = discovery(
        &[
            ("/project/node_modules/a/package.json", r#"{"name":"a"}"#),
            ("/project/node_modules/a/b/package.json", r#"{"name":"b"}"#),
            (
                "/project/node_modules/@a/b/package.json",
                r#"{"name":"@a/b"}"#,
            ),
        ],
        &["/project/app.js"],
        &info(&[]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(found.new_typing_names, strings(&["@a/b", "a"]));
}

#[test]
fn discovery_manifest_dependencies_own_types_and_watch_malformed_manifests() {
    let found = discovery(
        &[
            (
                "/project/package.json",
                r#"{"dependencies":{"jquery":"*","own":"*","missing":"*"},"peerDependencies":{"peer":"*"}}"#,
            ),
            ("/project/bower.json", "{ malformed"),
            (
                "/project/node_modules/jquery/package.json",
                r#"{"name":"jquery"}"#,
            ),
            (
                "/project/node_modules/own/package.json",
                r#"{"name":"own","types":"types/main.d.ts"}"#,
            ),
            ("/project/node_modules/own/types/main.d.ts", ""),
            (
                "/project/node_modules/missing/package.json",
                r#"{"name":"missing","types":"absent.d.ts"}"#,
            ),
            (
                "/project/node_modules/unlisted/package.json",
                r#"{"name":"unlisted"}"#,
            ),
        ],
        &["/project/app.js"],
        &info(&[]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(
        found.cached_typing_paths,
        strings(&["/project/node_modules/own/types/main.d.ts"])
    );
    assert_eq!(
        found.new_typing_names,
        strings(&["jquery", "missing", "peer"])
    );
    assert_eq!(
        found.files_to_watch,
        strings(&[
            "/project/bower.json",
            "/project/bower_components",
            "/project/package.json",
            "/project/node_modules"
        ])
    );
}

#[test]
fn types_option_disables_manifest_inference_but_imports_and_explicit_includes_remain() {
    let files = [(
        "/project/package.json",
        r#"{"dependencies":{"jquery":"*"}}"#,
    )];
    let mut settings = info(&["node:fs", "assert/strict", "foo"]);
    Arc::make_mut(&mut settings.compiler_options).types = Some(Vec::new());
    settings.type_acquisition.include = Some(strings(&["explicit"]));
    settings.type_acquisition.exclude = Some(strings(&["foo"]));
    settings
        .type_acquisition
        .disable_filename_based_type_acquisition = Tristate::TRUE;
    let found = discovery(
        &files,
        &["/project/jquery.js", "/project/index.jsx"],
        &settings,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(found.new_typing_names, strings(&["explicit", "node"]));
    assert!(found.files_to_watch.is_empty());
    settings
        .type_acquisition
        .disable_filename_based_type_acquisition = Tristate::FALSE;
    assert_eq!(
        discovery(
            &files,
            &["/project/index.jsx", "/project/angular.ts"],
            &settings,
            &BTreeMap::new(),
            &BTreeMap::new()
        )
        .new_typing_names,
        strings(&["explicit", "node", "react"])
    );
}

#[test]
fn pinned_cache_adds_absent_and_excluded_entries() {
    let mut settings = info(&[]);
    settings.type_acquisition.exclude = Some(strings(&["node"]));
    let found = discovery(
        &[],
        &[],
        &settings,
        &BTreeMap::from([cached("node", "1.3.0")]),
        &BTreeMap::from([(js("node"), version_tags("1.3.0"))]),
    );
    assert_eq!(
        found.cached_typing_paths,
        strings(&["/cache/node_modules/@types/node/index.d.ts"])
    );
}

#[test]
fn root_manifest_rejects_duplicate_keys_but_dependency_parser_accepts_them() {
    let root = discovery(
        &[(
            "/project/package.json",
            r#"{"dependencies":{"jquery":"*"},"dependencies":{"commander":"*"}}"#,
        )],
        &[],
        &info(&[]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert!(root.new_typing_names.is_empty());
    assert!(root.files_to_watch.contains(&js("/project/package.json")));
    let dependency = discovery(
        &[(
            "/project/node_modules/a/package.json",
            r#"{"name":"a","name":"b"}"#,
        )],
        &[],
        &info(&[]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(dependency.new_typing_names, strings(&["b"]));
}

#[test]
fn suffix_removal_matches_reverse_scan_edges() {
    for (input, expected) in [
        ("jquery-min.4.2.3", "jquery"),
        ("angular-route.1.2.3", "angular-route"),
        ("x.MIN-min.12", "x"),
        (".min", ".min"),
        ("min", "min"),
        ("123", "123"),
        ("x.minified", "x.minified"),
        ("é-min", "é"),
        ("x-12.0-z", "x-12.0-z"),
    ] {
        assert_eq!(
            remove_min_and_version_numbers(input.as_bytes()),
            expected.as_bytes(),
            "{input}"
        );
    }
}

#[test]
fn pinned_package_name_validation() {
    use NameValidationResult::{
        EmptyName, NameContainsNonUriSafeCharacters, NameOk, NameStartsWithDot,
        NameStartsWithUnderscore, NameTooLong,
    };
    for (input, expected, part, scope) in [
        ("", EmptyName, "", false),
        (".foo", NameStartsWithDot, "", false),
        ("_foo", NameStartsWithUnderscore, "", false),
        ("  scope  ", NameContainsNonUriSafeCharacters, "", false),
        (
            "; say ‘Hello from TypeScript!’ #",
            NameContainsNonUriSafeCharacters,
            "",
            false,
        ),
        ("a/b/c", NameContainsNonUriSafeCharacters, "", false),
        ("@scope/bar", NameOk, "", false),
        ("@.scope/bar", NameStartsWithDot, ".scope", true),
        ("@.scope/.bar", NameStartsWithDot, ".scope", true),
        ("@_scope/bar", NameStartsWithUnderscore, "_scope", true),
        ("@_scope/_bar", NameStartsWithUnderscore, "_scope", true),
        (
            "@  scope  /bar",
            NameContainsNonUriSafeCharacters,
            "  scope  ",
            true,
        ),
        (
            "@; say ‘Hello from TypeScript!’ #/bar",
            NameContainsNonUriSafeCharacters,
            "; say ‘Hello from TypeScript!’ #",
            true,
        ),
        (
            "@  scope  /  bar  ",
            NameContainsNonUriSafeCharacters,
            "  scope  ",
            true,
        ),
        ("@scope/.bar", NameStartsWithDot, ".bar", false),
        ("@scope/_bar", NameStartsWithUnderscore, "_bar", false),
        (
            "@scope/  bar  ",
            NameContainsNonUriSafeCharacters,
            "  bar  ",
            false,
        ),
        (
            "@scope/; say ‘Hello from TypeScript!’ #",
            NameContainsNonUriSafeCharacters,
            "; say ‘Hello from TypeScript!’ #",
            false,
        ),
        ("@/bar", NameContainsNonUriSafeCharacters, "", false),
        ("@scope/", NameContainsNonUriSafeCharacters, "", false),
        ("a~B-._", NameOk, "", false),
        ("a+b", NameContainsNonUriSafeCharacters, "", false),
    ] {
        assert_eq!(
            validate_package_name(input.as_bytes()),
            (expected, part.as_bytes(), scope),
            "{input}"
        );
    }
    assert_eq!(validate_package_name(&[b'a'; 256]).0, NameTooLong);
    assert_eq!(validate_package_name(&[b'a'; 214]).0, NameOk);
    assert_eq!(
        validate_package_name(&[0xff]).0,
        NameContainsNonUriSafeCharacters
    );
    assert_eq!(
        render_package_name_validation_failure(b"@scope/.bar", NameStartsWithDot, b".bar", false),
        "'@scope/.bar':: Package name '.bar' cannot start with '.'"
    );
}

#[test]
fn pinned_npm_long_command_runs_both_batches_even_after_failure() {
    let packages: Vec<_> = include_str!("npm_packages.txt").lines().map(js).collect();
    for fail in [false, true] {
        let batches = Mutex::new(Vec::new());
        let result = install_npm_packages(
            &Context::background(),
            &packages,
            &NpmThrottle::new(5),
            &|batch| {
                batches.lock().unwrap().push(batch.to_vec());
                if fail {
                    Err(AtaError::Npm(NpmError {
                        message: "failed to install packages".into(),
                        output: vec![],
                    }))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.is_err(), fail);
        let mut batches = batches.into_inner().unwrap();
        assert_eq!(batches.len(), 2);
        batches.sort_by_key(|batch| packages.iter().position(|name| name == &batch[0]).unwrap());
        assert_eq!(batches.concat(), packages);
        assert!(batches
            .iter()
            .all(|batch| 100 + batch.iter().map(|p| p.as_bytes().len() + 1).sum::<usize>() < 8000));
    }
}

struct LiveFs(Mutex<MemorySnapshot>);
impl LiveFs {
    fn new(files: &[(&str, &str)]) -> Self {
        Self(Mutex::new(memory(files)))
    }
}
impl FileSystem for LiveFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        false
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        self.0.lock().unwrap().read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        self.0.lock().unwrap().stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        self.0.lock().unwrap().entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        self.0.lock().unwrap().realpath(path)
    }
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), tsr_vfs::Error> {
        let mut fs = self.0.lock().unwrap();
        let mut builder = MemoryBuilder::from_snapshot(&fs);
        builder.insert_loaded(path, data);
        *fs = builder.finish();
        Ok(())
    }
}
struct MockNpm {
    fs: Arc<LiveFs>,
    calls: Mutex<Vec<Vec<JsString>>>,
    fail: AtomicBool,
    cancel: AtomicBool,
}
impl NpmExecutor for MockNpm {
    fn npm_install(
        &self,
        context: &Context,
        cwd: &[u8],
        args: &[JsString],
    ) -> Result<Vec<u8>, NpmError> {
        assert_eq!(cwd, b"/cache");
        self.calls.lock().unwrap().push(args.to_vec());
        if self.cancel.swap(false, Ordering::SeqCst) {
            context.cancel();
            return Err(NpmError {
                message: "canceled".into(),
                output: vec![],
            });
        }
        if args[2].as_bytes() == b"types-registry@latest" {
            self.fs.write_file(b"/cache/node_modules/types-registry/index.json", br#"{"entries":{"jquery":{"latest":"1.3.0"},"node":{"latest":"1.3.0"},"scope__bar":{"latest":"1.3.0"}}}"#).unwrap();
        } else {
            if self.fail.load(Ordering::SeqCst) {
                return Err(NpmError {
                    message: "mock npm exit 1".into(),
                    output: b"npm failed".to_vec(),
                });
            }
            assert_eq!(args[0].as_bytes(), b"install");
            assert_eq!(args[1].as_bytes(), b"--ignore-scripts");
            assert_eq!(args[args.len() - 2].as_bytes(), b"--save-dev");
            assert_eq!(
                args.last().unwrap().as_bytes(),
                format!("--user-agent=\"typesInstaller/{}\"", tsr_core::version()).as_bytes()
            );
            for package in &args[2..args.len() - 2] {
                let package = package
                    .as_bytes()
                    .strip_prefix(b"@types/")
                    .unwrap()
                    .strip_suffix(b"@latest")
                    .unwrap();
                let path = [b"/cache/node_modules/@types/", package, b"/index.d.ts"].concat();
                self.fs
                    .write_file(&path, b"declare const value: number;")
                    .unwrap();
            }
        }
        Ok(vec![])
    }
}
fn installer(fs: Arc<LiveFs>) -> (TypingsInstaller, Arc<MockNpm>) {
    let npm = Arc::new(MockNpm {
        fs: fs.clone(),
        calls: Mutex::new(vec![]),
        fail: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
    });
    (
        TypingsInstaller::new(
            TypingsInstallerOptions {
                typings_location: js("/cache"),
                throttle_limit: 2,
            },
            fs,
            npm.clone(),
        ),
        npm,
    )
}
fn install(
    installer: &TypingsInstaller,
    fs: &dyn FileSystem,
    context: &Context,
    imports: &[&str],
) -> Result<TypingsInstallResult, AtaError> {
    installer.install_typings(
        context,
        &TypingsInstallRequest {
            project_id: b"/project/jsconfig.json",
            typings_info: &info(imports),
            file_names: &strings(&["/project/app.js"]),
            project_root_path: b"/project",
            fs,
            logger: &Logger::nop(),
        },
    )
}

#[test]
fn installer_initializes_once_installs_scoped_and_reuses_cache() {
    let fs = Arc::new(LiveFs::new(&[]));
    let snapshot = MemoryBuilder::from_snapshot(&fs.0.lock().unwrap()).finish();
    let (installer, npm) = installer(fs.clone());
    let first = install(
        &installer,
        &snapshot,
        &Context::background(),
        &["jquery", "@scope/bar"],
    )
    .unwrap();
    assert_eq!(
        first.typings_files,
        strings(&[
            "/cache/node_modules/@types/jquery/index.d.ts",
            "/cache/node_modules/@types/scope__bar/index.d.ts"
        ])
    );
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
    assert_eq!(
        install(
            &installer,
            fs.as_ref(),
            &Context::background(),
            &["jquery", "@scope/bar"]
        )
        .unwrap(),
        first
    );
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
    assert_eq!(
        fs.read_file(b"/cache/package.json")
            .unwrap()
            .unwrap()
            .text
            .as_bytes(),
        b"{ \"private\": true }"
    );
}

#[test]
fn installer_reads_legacy_and_v3_lock_cache_and_refreshes_expired_versions() {
    for (lock, version) in [
        ("dependencies", "1.3.0"),
        ("packages", "1.3.0"),
        ("dependencies", "1.0.0"),
        ("packages", "1.0.0"),
    ] {
        let key = if lock == "packages" {
            "node_modules/@types/jquery"
        } else {
            "@types/jquery"
        };
        let contents = format!(r#"{{"{lock}":{{"{key}":{{"version":"{version}"}}}}}}"#);
        let fs = Arc::new(LiveFs::new(&[
            (
                "/cache/package.json",
                r#"{"devDependencies":{"@types/jquery":"*"}}"#,
            ),
            ("/cache/package-lock.json", &contents),
            ("/cache/node_modules/@types/jquery/index.d.ts", ""),
        ]));
        let (installer, npm) = installer(fs.clone());
        let result = install(&installer, fs.as_ref(), &Context::background(), &["jquery"]).unwrap();
        assert_eq!(
            result.typings_files,
            strings(&["/cache/node_modules/@types/jquery/index.d.ts"])
        );
        assert_eq!(
            npm.calls.lock().unwrap().len(),
            if version == "1.3.0" { 1 } else { 2 }
        );
    }
}

#[test]
fn installer_failure_preserves_output_and_suppresses_retries() {
    let fs = Arc::new(LiveFs::new(&[]));
    let (installer, npm) = installer(fs.clone());
    npm.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        install(&installer, fs.as_ref(), &Context::background(), &["jquery"]),
        Err(AtaError::Npm(NpmError {
            message: "mock npm exit 1".into(),
            output: b"npm failed".to_vec()
        }))
    );
    assert!(
        install(&installer, fs.as_ref(), &Context::background(), &["jquery"])
            .unwrap()
            .typings_files
            .is_empty()
    );
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
}

#[test]
fn invalid_known_package_lookup_does_not_initialize_or_execute_npm() {
    let fs = Arc::new(LiveFs::new(&[]));
    let (installer, npm) = installer(fs.clone());
    assert!(!installer
        .is_known_types_package_name(
            &Context::background(),
            b".invalid",
            fs.as_ref(),
            &Logger::nop()
        )
        .unwrap());
    assert!(npm.calls.lock().unwrap().is_empty());
    assert!(!fs.file_exists(b"/cache/package.json").unwrap());
}

#[test]
fn initialization_cancellation_retries_without_poisoning_missing_packages() {
    let fs = Arc::new(LiveFs::new(&[]));
    let (installer, npm) = installer(fs.clone());
    let context = Context::background().with_cancel();
    npm.cancel.store(true, Ordering::SeqCst);
    assert_eq!(
        install(&installer, fs.as_ref(), &context, &["jquery"]),
        Err(AtaError::Canceled(ContextError::Canceled))
    );
    assert_eq!(
        install(&installer, fs.as_ref(), &Context::background(), &["jquery"])
            .unwrap()
            .typings_files
            .len(),
        1
    );
    assert_eq!(npm.calls.lock().unwrap().len(), 3);
}

#[test]
fn canceled_batch_never_runs_and_throttle_releases_after_failure() {
    let context = Context::background().with_cancel();
    context.cancel();
    let calls = AtomicUsize::new(0);
    let throttle = NpmThrottle::new(1);
    let packages = strings(&["@types/jquery@latest"]);
    assert_eq!(
        install_npm_packages(&context, &packages, &throttle, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }),
        Err(AtaError::Canceled(ContextError::Canceled))
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    install_npm_packages(&Context::background(), &packages, &throttle, &|_| Ok(())).unwrap();
}
