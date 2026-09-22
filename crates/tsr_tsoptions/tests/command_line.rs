use tsr_core::{Tristate, WatchFileKind};
use tsr_jsstring::JsString;
use tsr_tsoptions::{
    parse_build_command_line, parse_command_line, ConfigValue as V, ParseConfigHost,
};
use tsr_vfs::{Error, FileSystem, MemoryBuilder, MemorySnapshot};

struct Host(MemorySnapshot);
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        &self.0
    }
    fn current_directory(&self) -> &[u8] {
        b"/project"
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, Error> {
        panic!("argv parsing must not resolve configs")
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, Error> {
        panic!("argv parsing must not load plugins")
    }
}
fn host(files: &[(&[u8], &[u8])]) -> Host {
    let mut builder = MemoryBuilder::new(b"/project", false);
    for (name, bytes) in files {
        builder.insert_physical(name, bytes.to_vec());
    }
    Host(builder.finish())
}
fn argv(args: &[&str]) -> Vec<JsString> {
    args.iter()
        .map(|s| JsString::from_bytes(s.as_bytes()))
        .collect()
}
fn keys(value: &V) -> Vec<&[u8]> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, _)| key.as_bytes())
        .collect()
}

#[test]
fn raw_options_preserve_order_and_paths_while_converted_values_are_owned() {
    let result = parse_command_line(
        &argv(&[
            "--outDir",
            "out",
            "--strict",
            "false",
            "--outDir",
            "next",
            "--typeRoots",
            "types,more",
            "--excludeDirectories",
            "cache",
        ]),
        &host(&[]),
    );
    assert!(result.errors.is_empty());
    assert_eq!(
        keys(&result.raw),
        [
            b"outDir".as_slice(),
            b"strict",
            b"typeRoots",
            b"excludeDirectories"
        ]
    );
    assert_eq!(
        result.raw.get(b"outDir"),
        Some(&V::String(JsString::from_bytes(b"next".as_slice())))
    );
    assert_eq!(result.options.out_dir.as_bytes(), b"/project/next");
    assert_eq!(
        result.options.type_roots.as_ref().unwrap(),
        &argv(&["/project/types", "/project/more"])
    );
    assert_eq!(
        result.watch_options.as_ref().unwrap().exclude_dir,
        Some(argv(&["cache"]))
    );
    assert_eq!(result.options.strict, Tristate::FALSE);
    let mut cloned = result.clone();
    cloned
        .watch_options
        .as_mut()
        .unwrap()
        .exclude_dir
        .as_mut()
        .unwrap()
        .push(JsString::default());
    assert_eq!(result.watch_options.unwrap().exclude_dir.unwrap().len(), 1);
}

#[test]
fn values_consume_arguments_in_the_native_order() {
    let result = parse_command_line(
        &argv(&[
            "--lib",
            "--strict",
            "source.ts",
            "--composite",
            "true",
            "--paths",
            "null",
            "--target",
        ]),
        &host(&[]),
    );
    assert_eq!(result.root_file_names, argv(&["source.ts"]));
    assert_eq!(result.options.strict, Tristate::TRUE);
    assert!(result.raw.get(b"composite").is_none());
    assert_eq!(result.raw.get(b"paths"), Some(&V::Null));
    assert_eq!(result.raw.get(b"lib"), Some(&V::Array(Some(vec![]))));
    assert!(result.raw.get(b"target").is_none());
    assert_eq!(
        result.errors.iter().map(|d| d.code).collect::<Vec<_>>(),
        [6230, 6044, 6046]
    );
}

#[test]
fn response_files_keep_diagnostic_order_and_only_guard_active_paths() {
    let host = host(&[
        (b"/project/a.rsp", b"a.ts @sub/b.rsp @A.rsp --strict false"),
        (
            b"/project/sub/b.rsp",
            b"\"b file.ts\" @missing.rsp \"unterminated",
        ),
    ]);
    let result = parse_command_line(&argv(&["@a.rsp", "@a.rsp", "last.ts"]), &host);
    assert_eq!(
        result.root_file_names,
        argv(&["a.ts", "b file.ts", "a.ts", "b file.ts", "last.ts"])
    );
    assert_eq!(result.options.strict, Tristate::FALSE);
    // Tokenization reports the unterminated quote before recursively parsing
    // the earlier @missing token. Sibling references are each parsed afresh.
    assert_eq!(
        result.errors.iter().map(|d| d.code).collect::<Vec<_>>(),
        [6045, 5083, 6045, 5083]
    );
    assert_eq!(
        result.errors[1].message_args[0].as_bytes(),
        b"/project/missing.rsp"
    );
}

#[test]
fn response_runes_replace_each_invalid_byte_and_use_ascii_control_whitespace() {
    let result = parse_command_line(
        &argv(&["@bytes.rsp"]),
        &host(&[(b"/project/bytes.rsp", b"\xe2\x82.ts\tfoo\xc2\xa0bar.ts")]),
    );
    assert!(result.errors.is_empty());
    assert_eq!(
        result.root_file_names,
        argv(&["\u{fffd}\u{fffd}.ts", "foo\u{a0}bar.ts"])
    );
}

#[test]
fn deep_response_nesting_does_not_use_the_call_stack() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut builder = MemoryBuilder::new(b"/project", true);
            for i in 0..2048 {
                builder.insert_physical(
                    format!("/project/{i}.rsp").as_bytes(),
                    format!("@{}.rsp", i + 1).into_bytes(),
                );
            }
            builder.insert_physical(b"/project/2048.rsp", b"end.ts".to_vec());
            let result = parse_command_line(&argv(&["@0.rsp"]), &Host(builder.finish()));
            assert!(result.errors.is_empty());
            assert_eq!(result.root_file_names, argv(&["end.ts"]));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn build_aliases_and_conflicts_are_distinct_from_compiler_mode() {
    let parsed = parse_build_command_line(
        &argv(&[
            "-d",
            "-v",
            "--clean",
            "--force",
            "-w",
            "--watchFile",
            "usefsevents",
        ]),
        &host(&[]),
    );
    assert_eq!(parsed.projects, argv(&["."]));
    assert_eq!(parsed.build_options.dry, Tristate::TRUE);
    assert_eq!(parsed.build_options.verbose, Tristate::TRUE);
    assert_eq!(parsed.compiler_options.declaration, Tristate::UNKNOWN);
    assert_eq!(parsed.compiler_options.version, Tristate::UNKNOWN);
    assert_eq!(parsed.watch_options.file_kind, WatchFileKind::USE_FS_EVENTS);
    assert_eq!(
        parsed
            .errors
            .iter()
            .map(|d| d.message_args.clone())
            .collect::<Vec<_>>(),
        [
            argv(&["clean", "force"]),
            argv(&["clean", "verbose"]),
            argv(&["clean", "watch"]),
            argv(&["watch", "dry"])
        ]
    );
    let ordinary = parse_command_line(&argv(&["-d", "-v"]), &host(&[]));
    assert_eq!(ordinary.options.declaration, Tristate::TRUE);
    assert_eq!(ordinary.options.version, Tristate::TRUE);
}

#[test]
fn watch_nulls_preserve_enums_but_clear_lists_and_numbers() {
    let mut options = tsr_core::WatchOptions::default();
    for (key, value) in [
        (b"watchFile".as_slice(), V::Enum(5)),
        (b"watchInterval", V::Integer(50)),
        (b"excludeFiles", V::Array(Some(vec![]))),
    ] {
        tsr_tsoptions::parse_watch_options(key, &value, &mut options);
        tsr_tsoptions::parse_watch_options(key, &V::Null, &mut options);
    }
    assert_eq!(options.file_kind, WatchFileKind::USE_FS_EVENTS);
    assert_eq!(options.interval, None);
    assert_eq!(options.exclude_files, None);
}

#[test]
fn build_locale_is_lazy_and_stays_at_the_first_observed_value() {
    let mut result = parse_build_command_line(&argv(&[]), &host(&[]));
    result.compiler_options.locale = JsString::from_bytes(b"ja".as_slice());
    assert_eq!(result.locale().tag_string(), "ja");
    result.compiler_options.locale = JsString::from_bytes(b"en".as_slice());
    assert_eq!(result.locale().tag_string(), "ja");
}

#[test]
fn resolved_build_projects_normalize_in_order_and_cache_the_first_result() {
    let mut parsed = parse_build_command_line(
        &argv(&["../a", "sub/../tsconfig.dev.json", "/x/custom.JSON", "../a"]),
        &host(&[]),
    );
    let expected = argv(&[
        "/a/tsconfig.json",
        "/project/tsconfig.dev.json",
        "/x/custom.JSON/tsconfig.json",
        "/a/tsconfig.json",
    ]);
    // A pre-observation clone has its own lazy cache, following the owned
    // parse-result policy. Initializing it cannot populate the original.
    let mut clone = parsed.clone();
    clone.projects = argv(&["other"]);
    assert_eq!(
        clone.resolved_project_paths(),
        argv(&["/project/other/tsconfig.json"])
    );
    assert_eq!(parsed.resolved_project_paths(), expected);
    parsed.projects.clear();
    parsed.current_directory = JsString::from_bytes(b"/changed".as_slice());
    assert_eq!(parsed.resolved_project_paths(), expected);
    assert_eq!(parsed.clone().resolved_project_paths(), expected);
}
