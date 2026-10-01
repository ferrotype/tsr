use tsr_ast::{SourceFileParseOptions, SourceFileState};
use tsr_core::{CompilerOptions, ScriptKind, Tristate};
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::output_paths::{
    for_each_emitted_file, get_common_source_directory, get_output_js_file_name,
    get_output_paths_for, get_source_file_path_in_new_dir, ForceEmitPaths, OutputPaths,
    OutputPathsHost,
};

fn text(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}

// Ported from outputpaths_test.go:TestGetSourceFilePathInNewDirSourceMatchesCommonDirectory.
#[test]
fn get_source_file_path_in_new_dir_source_matches_common_directory() {
    let actual = get_source_file_path_in_new_dir(
        b"/project/src",
        b"/project/out",
        b"/project",
        b"/project/src/",
        true,
    );
    assert_eq!(actual, b"/project/src");
}

// Ported from outputpaths_test.go:TestGetSourceFilePathInNewDirCanonicalizationShrinksCommonDirectory.
// Each Kelvin sign U+212A case-folds to the single-byte 'k', so the raw common
// directory is longer in bytes than the prefix it matches; the trim is per rune.
#[test]
fn get_source_file_path_in_new_dir_canonicalization_shrinks_common_directory() {
    let actual = get_source_file_path_in_new_dir(
        b"/kkkk/a.ts",
        b"/out",
        b"/",
        "/\u{212A}\u{212A}\u{212A}\u{212A}/".as_bytes(),
        false,
    );
    assert_eq!(actual, b"/out/a.ts");
}

// The tests below have no Go counterpart; every expectation was derived by
// reading the pinned outputpaths.go and commonsourcedirectory.go.

struct TestHost {
    common: JsString,
    extensions: Vec<JsString>,
    common_calls: usize,
}

impl TestHost {
    fn new(common: &str) -> Self {
        Self {
            common: text(common),
            extensions: Vec::new(),
            common_calls: 0,
        }
    }
}

impl OutputPathsHost for TestHost {
    fn common_source_directory(&mut self) -> JsString {
        self.common_calls += 1;
        self.common.clone()
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.extensions.clone()
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/"
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
}

fn source_file(name: &str, script_kind: ScriptKind) -> SourceFileState {
    let mut file = SourceFileState::new(
        SourceFileParseOptions {
            file_name: text(name),
            path: text(name),
            ..Default::default()
        },
        SourceText::default(),
    );
    file.script_kind = script_kind;
    file
}

#[test]
fn output_paths_for_a_source_under_out_dir() {
    let options = CompilerOptions {
        out_dir: text("/out"),
        declaration: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        source_map: Tristate::TRUE,
        ..Default::default()
    };
    let mut host = TestHost::new("/src/");
    let file = source_file("/src/lib/a.mts", ScriptKind::TS);
    let paths = get_output_paths_for(&file, &options, &mut host, ForceEmitPaths::default());
    assert_eq!(paths.js_file_path(), b"/out/lib/a.mjs");
    assert_eq!(paths.source_map_file_path(), b"/out/lib/a.mjs.map");
    assert_eq!(paths.declaration_file_path(), b"/out/lib/a.d.mts");
    assert_eq!(paths.declaration_map_path(), b"/out/lib/a.d.mts.map");
}

#[test]
fn json_emitted_to_its_own_location_has_no_outputs() {
    let options = CompilerOptions {
        declaration: Tristate::TRUE,
        source_map: Tristate::TRUE,
        ..Default::default()
    };
    let mut host = TestHost::new("/src/");
    let file = source_file("/src/data.json", ScriptKind::JSON);
    let paths = get_output_paths_for(&file, &options, &mut host, ForceEmitPaths::default());
    assert_eq!(paths.js_file_path(), b"");
    assert_eq!(paths.source_map_file_path(), b"");
    assert_eq!(paths.declaration_file_path(), b"");
    assert_eq!(host.common_calls, 0);
    let options = CompilerOptions {
        out_dir: text("/out"),
        source_map: Tristate::TRUE,
        ..Default::default()
    };
    let paths = get_output_paths_for(&file, &options, &mut host, ForceEmitPaths::default());
    assert_eq!(paths.js_file_path(), b"/out/data.json");
    // A JSON output never gets a source map.
    assert_eq!(paths.source_map_file_path(), b"");
}

#[test]
fn forced_paths_override_emit_declaration_only_and_declaration_flags() {
    let options = CompilerOptions {
        emit_declaration_only: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        ..Default::default()
    };
    let mut host = TestHost::new("/src/");
    let file = source_file("/src/a.ts", ScriptKind::TS);
    let paths = get_output_paths_for(&file, &options, &mut host, ForceEmitPaths::default());
    assert_eq!(paths, OutputPaths::default());
    let paths = get_output_paths_for(
        &file,
        &options,
        &mut host,
        ForceEmitPaths {
            dts: true,
            js: true,
            declaration_map: true,
        },
    );
    assert_eq!(paths.js_file_path(), b"/src/a.js");
    assert_eq!(paths.declaration_file_path(), b"/src/a.d.ts");
    assert_eq!(paths.declaration_map_path(), b"/src/a.d.ts.map");
}

#[test]
fn for_each_emitted_file_stops_at_the_first_true_action() {
    let options = CompilerOptions {
        declaration: Tristate::TRUE,
        ..Default::default()
    };
    let mut host = TestHost::new("/");
    let a = source_file("/a.ts", ScriptKind::TS);
    let b = source_file("/b.ts", ScriptKind::TS);
    let c = source_file("/c.ts", ScriptKind::TS);
    let mut seen = Vec::new();
    let stopped = for_each_emitted_file(
        &mut host,
        &options,
        |paths, file| {
            seen.push((
                paths.declaration_file_path().to_vec(),
                file.file_name().to_vec(),
            ));
            file.file_name() == b"/b.ts"
        },
        &[&a, &b, &c],
        false,
    );
    assert!(stopped);
    assert_eq!(
        seen,
        [
            (b"/a.d.ts".to_vec(), b"/a.ts".to_vec()),
            (b"/b.d.ts".to_vec(), b"/b.ts".to_vec()),
        ]
    );
}

#[test]
fn output_js_file_name_asks_for_the_common_directory_only_with_out_dir() {
    let mut host = TestHost::new("/src/");
    let options = CompilerOptions::default();
    assert_eq!(
        get_output_js_file_name(b"/src/a.tsx", &options, &mut host),
        b"/src/a.js"
    );
    // The JSON output would overwrite its input.
    assert_eq!(
        get_output_js_file_name(b"/src/a.json", &options, &mut host),
        b""
    );
    assert_eq!(host.common_calls, 0);
    let options = CompilerOptions {
        out_dir: text("/out"),
        jsx: tsr_core::JsxEmit::PRESERVE,
        ..Default::default()
    };
    assert_eq!(
        get_output_js_file_name(b"/src/a.tsx", &options, &mut host),
        b"/out/a.jsx"
    );
    assert_eq!(host.common_calls, 1);
    host.extensions = vec![text(".vue")];
    assert_eq!(
        get_output_js_file_name(b"/src/a.vue", &options, &mut host),
        b""
    );
    assert_eq!(host.common_calls, 1);
    let options = CompilerOptions {
        emit_declaration_only: Tristate::TRUE,
        ..Default::default()
    };
    assert_eq!(
        get_output_js_file_name(b"/src/a.ts", &options, &mut host),
        b""
    );
}

#[test]
fn common_source_directory_prefers_root_dir_then_config_directory() {
    let files = || vec![text("/a/b/x.ts"), text("/a/c/y.ts")];
    let mut checked = Vec::new();
    let options = CompilerOptions {
        root_dir: text("/a/b"),
        config_file_path: text("/cfg/tsconfig.json"),
        ..Default::default()
    };
    let directory = get_common_source_directory(
        &options,
        files,
        b"/",
        true,
        Some(&mut |files: &[JsString], root: &[u8]| {
            checked.push((files.len(), root.to_vec()));
            false
        }),
    );
    assert_eq!(directory, b"/a/b/");
    let options = CompilerOptions {
        config_file_path: text("/cfg/tsconfig.json"),
        ..Default::default()
    };
    let directory = get_common_source_directory(
        &options,
        files,
        b"/",
        true,
        Some(&mut |files: &[JsString], root: &[u8]| {
            checked.push((files.len(), root.to_vec()));
            true
        }),
    );
    assert_eq!(directory, b"/cfg/");
    assert_eq!(checked, [(2, b"/a/b".to_vec()), (2, b"/cfg".to_vec())]);
    // Without a check the file list is not read at all.
    let options = CompilerOptions {
        root_dir: text("/r/"),
        ..Default::default()
    };
    let directory = get_common_source_directory(
        &options,
        || panic!("files read without a check"),
        b"/",
        true,
        None,
    );
    assert_eq!(directory, b"/r/");
    let directory =
        get_common_source_directory(&CompilerOptions::default(), files, b"/", true, None);
    assert_eq!(directory, b"/a/");
    let directory =
        get_common_source_directory(&CompilerOptions::default(), Vec::new, b"/cwd", true, None);
    assert_eq!(directory, b"/cwd/");
}

// Derived from computeCommonSourceDirectoryOfFilenames: a file with no
// directory component is not a mismatch at index 0; it truncates the common
// path to nothing and the current directory is returned.
#[test]
fn computed_common_directory_of_a_bare_root_is_the_current_directory() {
    use tsr_tsoptions::output_paths::computed_common;
    assert_eq!(
        computed_common(&[text("/"), text("/a/b.ts")], b"/w", true),
        b"/w"
    );
    assert_eq!(
        computed_common(&[text("/a/b.ts"), text("/")], b"/w", true),
        b"/w"
    );
    assert_eq!(
        computed_common(&[text("/a/b.ts"), text("/c/d.ts")], b"/w", true),
        b"/"
    );
    assert_eq!(
        computed_common(&[text("/a/x/b.ts"), text("/a/d.ts")], b"/w", true),
        b"/a"
    );
}
