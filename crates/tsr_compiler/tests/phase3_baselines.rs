//! The Phase 3 baseline writers (`tools/phase3/harness/baselines.rs`) over
//! real rows of the pinned compiler: each fixture carries a row's inputs and
//! outputs, the pin's three composed baselines, and what the writers read
//! from the program and the emit result
//! (`scripts/tests/fixtures/phase3/baseline-writers.json`, written by
//! `scripts/phase3_baselines.py fixtures --write`). The writers must compose
//! the pin's texts byte for byte; the branches no corpus row reaches (the
//! `noCheck` repeat's blocks, the writers' assertions) are pinned on the same
//! rows with altered inputs, and the repeat's diff against the Go patience
//! diff (`scripts/tests/fixtures/phase3/patience-diff.json`).
#[path = "../../../tools/phase3/harness/baselines.rs"]
#[allow(dead_code)]
mod baselines;

use baselines::{
    CompilationResult, DeclarationCompilationResult, Failure, JsEmitInput, OrderedFiles,
    ProgramView, RepeatOutputs, SourceFileView, SourceMapEmitResult, SourcemapInput,
    SourcemapRecordInput, TestFile, NO_CONTENT,
};
use serde_json::Value;
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;

fn unhex(value: &Value) -> Vec<u8> {
    let text = value.as_str().expect("a hex string");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn fixture_document(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/tests/fixtures/phase3")
        .join(name);
    serde_json::from_slice(&std::fs::read(path).expect("fixture file")).expect("fixture JSON")
}

struct FileFacts {
    file_name: Vec<u8>,
    path: Vec<u8>,
    text: Vec<u8>,
    original_text: Option<Vec<u8>>,
    content_mapper: Vec<u8>,
}

/// The program view a fixture recorded. Library texts are not recorded; the
/// fixture rows read none of them.
struct FactsView {
    files: Vec<FileFacts>,
    options: CompilerOptions,
    current_directory: Vec<u8>,
    case_sensitive: bool,
    common_source_directory: Result<Vec<u8>, String>,
    content_mapper_extensions: Vec<JsString>,
}

impl FactsView {
    fn new(facts: &Value, options: CompilerOptions) -> Self {
        let files = facts["source_files"]
            .as_array()
            .expect("source files")
            .iter()
            .map(|file| FileFacts {
                file_name: unhex(&file["file_name_hex"]),
                path: unhex(&file["path_hex"]),
                text: file.get("text_hex").map(unhex).unwrap_or_default(),
                original_text: file.get("original_text_hex").map(unhex),
                content_mapper: unhex(&file["content_mapper_hex"]),
            })
            .collect();
        let common = &facts["common_source_directory"];
        Self {
            files,
            options,
            current_directory: unhex(&facts["current_directory_hex"]),
            case_sensitive: facts["use_case_sensitive_file_names"] == true,
            common_source_directory: common
                .get("ok_hex")
                .map(unhex)
                .ok_or_else(|| common["error"].as_str().unwrap_or_default().to_owned()),
            content_mapper_extensions: facts["content_mapper_extensions_hex"]
                .as_array()
                .expect("extensions")
                .iter()
                .map(|extension| JsString::from_bytes(unhex(extension)))
                .collect(),
        }
    }

    fn view(file: &FileFacts) -> SourceFileView<'_> {
        SourceFileView {
            file_name: &file.file_name,
            path: &file.path,
            text: &file.text,
            original_text: file.original_text.as_deref().unwrap_or(&file.text),
            content_mapper: &file.content_mapper,
        }
    }
}

impl ProgramView for FactsView {
    fn source_files(&self) -> Vec<SourceFileView<'_>> {
        self.files.iter().map(Self::view).collect()
    }
    fn source_file(&self, file_name: &[u8]) -> Option<SourceFileView<'_>> {
        let path = tsr_tspath::to_path(file_name, &self.current_directory, self.case_sensitive);
        self.files
            .iter()
            .find(|file| file.path == path.as_bytes())
            .map(Self::view)
    }
    fn options(&self) -> &CompilerOptions {
        &self.options
    }
    fn common_source_directory(&self) -> Result<Vec<u8>, Failure> {
        self.common_source_directory.clone().map_err(Failure::Input)
    }
    fn current_directory(&self) -> &[u8] {
        &self.current_directory
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.content_mapper_extensions.clone()
    }
}

struct NoJsonErrors;
impl baselines::JsonErrorBaseline for NoJsonErrors {
    fn render(
        &self,
        file: &TestFile<'_>,
        _parsed: &tsr_ast::ParsedFile,
        _diagnostics: &[tsr_ast::Diagnostic],
    ) -> Result<Vec<u8>, Failure> {
        Err(Failure::Input(format!(
            "no fixture output fails to parse ({})",
            String::from_utf8_lossy(file.unit_name)
        )))
    }
}

type Owned = (Vec<u8>, Vec<u8>);

fn files(group: &Value, content: &str) -> Vec<Owned> {
    group
        .as_array()
        .expect("a file group")
        .iter()
        .map(|item| (unhex(&item["name_hex"]), unhex(&item[content])))
        .collect()
}

fn views(files: &[Owned]) -> Vec<TestFile<'_>> {
    files
        .iter()
        .map(|(unit_name, content)| TestFile { unit_name, content })
        .collect()
}

/// One fixture's owned inputs.
struct Row {
    kind: String,
    configured_name: Vec<u8>,
    subfolder: String,
    native: Value,
    declaration: Value,
    header: Vec<u8>,
    ts_config_files: Vec<Owned>,
    to_be_compiled: Vec<Owned>,
    other_files: Vec<Owned>,
    js: Vec<Owned>,
    dts: Vec<Owned>,
    maps: Vec<Owned>,
    options: CompilerOptions,
    facts: FactsView,
    source_maps: Option<Vec<SourceMapEmitResult>>,
}

fn options(native: &Value) -> CompilerOptions {
    tsr_tsoptions::raw::compiler_options(&native["options"]).expect("native options")
}

impl Row {
    fn new(fixture: &Value) -> Self {
        let native = fixture["native"].clone();
        let inputs = &native["baseline_inputs"];
        let outputs = &native["outputs"];
        let source_maps = fixture["facts"]["source_maps"].as_array().map(|maps| {
            maps.iter()
                .map(|map| {
                    let mut raw = tsr_sourcemap::RawSourceMap::default();
                    tsr_json::unmarshal(
                        &unhex(&map["map_text_hex"]),
                        &mut raw,
                        tsr_json::Options::default(),
                    )
                    .expect("map JSON");
                    SourceMapEmitResult {
                        input_source_file_names: map["input_source_file_names_hex"]
                            .as_array()
                            .expect("input names")
                            .iter()
                            .map(|name| JsString::from_bytes(unhex(name)))
                            .collect(),
                        source_map: raw,
                        generated_file: unhex(&map["generated_file_hex"]),
                    }
                })
                .collect()
        });
        Self {
            kind: fixture["kind"].as_str().expect("kind").to_owned(),
            configured_name: fixture["configured_name"]
                .as_str()
                .expect("configured name")
                .as_bytes()
                .to_vec(),
            subfolder: fixture["subfolder"].as_str().expect("subfolder").to_owned(),
            declaration: fixture["declaration"].clone(),
            header: unhex(&inputs["header_hex"]),
            ts_config_files: files(&inputs["ts_config_files"], "content_hex"),
            to_be_compiled: files(&inputs["to_be_compiled"], "content_hex"),
            other_files: files(&inputs["other_files"], "content_hex"),
            js: files(&outputs["js"], "text_hex"),
            dts: files(&outputs["dts"], "text_hex"),
            maps: files(&outputs["maps"], "text_hex"),
            options: options(&native),
            facts: FactsView::new(&fixture["facts"], options(&native)),
            source_maps,
            native,
        }
    }

    fn diagnostics(&self) -> usize {
        usize::try_from(self.native["diagnostics"].as_u64().expect("diagnostics")).expect("count")
    }

    fn full_emit_paths(&self) -> bool {
        self.native["harness_options"]["FullEmitPaths"] == true
    }

    fn harness_current_directory(&self) -> &[u8] {
        self.native["harness_options"]["CurrentDirectory"]
            .as_str()
            .expect("harness directory")
            .as_bytes()
    }

    /// The harness's compilation result in the pin's output order, which
    /// `newCompilationResult` must reproduce.
    fn result(&self) -> CompilationResult<'_> {
        let (js, dts, maps) = (views(&self.js), views(&self.dts), views(&self.maps));
        let recorded: Vec<TestFile<'_>> =
            maps.iter().chain(&dts).chain(&js).rev().copied().collect();
        let result = baselines::new_compilation_result(
            &self.facts,
            &recorded,
            self.diagnostics(),
            self.source_maps.clone(),
        )
        .expect("newCompilationResult");
        assert_eq!(result.js.files(), js.as_slice(), "{}: js order", self.kind);
        assert_eq!(
            result.dts.files(),
            dts.as_slice(),
            "{}: dts order",
            self.kind
        );
        assert_eq!(
            result.maps.files(),
            maps.as_slice(),
            "{}: map order",
            self.kind
        );
        result
    }

    fn baseline_name(&self, path: &[u8]) -> String {
        format!("{}/{}", self.subfolder, String::from_utf8_lossy(path))
    }

    /// The `.js` baseline with the given repeat outputs (`None`: the
    /// original outputs).
    fn js_emit(
        &self,
        result: &CompilationResult<'_>,
        repeat: Option<&RepeatOutputs<'_>>,
    ) -> Result<baselines::Baseline, Failure> {
        let (to_be_compiled, other_files) = (views(&self.to_be_compiled), views(&self.other_files));
        let ts_config_files = views(&self.ts_config_files);
        let declaration = baselines::prepare_declaration_compilation_context(
            &to_be_compiled,
            &other_files,
            result,
            &self.options,
            self.harness_current_directory(),
        )?
        .map(|context| {
            assert_eq!(self.declaration["state"], "compiled", "{}", self.kind);
            assert_eq!(
                context.decl_input_files.len() as u64,
                self.declaration["inputs"].as_u64().unwrap()
            );
            assert_eq!(
                context.decl_other_files.len() as u64,
                self.declaration["others"].as_u64().unwrap()
            );
            DeclarationCompilationResult {
                decl_input_files: context.decl_input_files,
                decl_other_files: context.decl_other_files,
                diagnostics: usize::try_from(self.declaration["diagnostics"].as_u64().unwrap())
                    .unwrap(),
                error_baseline: unhex(&self.declaration["error_baseline_hex"]),
            }
        });
        if let Some(declaration) = &declaration {
            // The rendered files are the configuration files and the
            // declaration program's files, in this order.
            let rendered = baselines::dts_file_error_inputs(&ts_config_files, declaration);
            assert_eq!(
                rendered.len(),
                ts_config_files.len()
                    + declaration.decl_input_files.len()
                    + declaration.decl_other_files.len()
            );
        }
        let original = RepeatOutputs {
            js: result.js.clone(),
            dts: result.dts.clone(),
        };
        baselines::js_emit_baseline(&JsEmitInput {
            configured_name: &self.configured_name,
            header: &self.header,
            options: &self.options,
            full_emit_paths: self.full_emit_paths(),
            harness_current_directory: self.harness_current_directory(),
            to_be_compiled: &to_be_compiled,
            other_files: &other_files,
            result,
            declaration: declaration.as_ref(),
            no_check_repeat: Some(repeat.unwrap_or(&original)),
            json_errors: &NoJsonErrors,
        })
    }

    fn assert_matches(&self, domain: &str, composed: Option<baselines::Baseline>) {
        let native = &self.native[domain];
        let state = native["state"].as_str().expect("state");
        match composed {
            None => assert_eq!(state, "not_baselined", "{}: {domain}", self.kind),
            Some(baseline) => {
                assert_eq!(
                    native["name"].as_str(),
                    Some(self.baseline_name(&baseline.path).as_str()),
                    "{}: {domain} name",
                    self.kind
                );
                if baseline.actual == NO_CONTENT {
                    assert_eq!(state, "no_content", "{}: {domain}", self.kind);
                } else {
                    assert_eq!(state, "content", "{}: {domain}", self.kind);
                    let expected = unhex(&native["text_hex"]);
                    assert!(
                        baseline.actual == expected,
                        "{}: {domain} text differs\n--- native\n{}\n--- composed\n{}",
                        self.kind,
                        String::from_utf8_lossy(&expected),
                        String::from_utf8_lossy(&baseline.actual)
                    );
                }
            }
        }
    }
}

fn rows() -> Vec<Row> {
    let document = fixture_document("baseline-writers.json");
    document["fixtures"]
        .as_array()
        .expect("fixtures")
        .iter()
        .map(Row::new)
        .collect()
}

fn row(kind: &str) -> Row {
    rows()
        .into_iter()
        .find(|row| row.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} fixture"))
}

#[test]
fn the_writers_compose_the_pinned_baselines_of_every_fixture() {
    let rows = rows();
    assert_eq!(rows.len(), 11);
    for row in &rows {
        let result = row.result();
        row.assert_matches(
            "output",
            Some(row.js_emit(&result, None).expect("the .js baseline")),
        );
        let sourcemap = baselines::sourcemap_baseline(&SourcemapInput {
            configured_name: &row.configured_name,
            options: &row.options,
            full_emit_paths: row.full_emit_paths(),
            result: &result,
        })
        .expect("the .js.map baseline");
        row.assert_matches("sourcemap", sourcemap);
        let record = baselines::sourcemap_record_baseline(&SourcemapRecordInput {
            configured_name: &row.configured_name,
            options: &row.options,
            result: &result,
        })
        .expect("the .sourcemap.txt baseline");
        row.assert_matches("sourcemap_record", Some(record));
    }
}

#[test]
fn the_fixtures_cover_each_writer_path() {
    let rows = rows();
    let by_kind = |kind: &str| rows.iter().find(|row| row.kind == kind).expect("fixture");
    let text = |row: &Row| unhex(&row.native["output"]["text_hex"]);
    assert_eq!(
        by_kind("no_content").native["output"]["state"],
        "no_content"
    );
    assert!(!by_kind("declarations").dts.is_empty());
    assert!(by_kind("full_emit_paths").full_emit_paths());
    assert!(by_kind("json_output")
        .js
        .iter()
        .any(|(name, _)| name.ends_with(b".json")));
    assert!(by_kind("bom")
        .dts
        .iter()
        .any(|(_, text)| text.starts_with(b"\xef\xbb\xbf")));
    assert!(text(by_kind("dts_file_errors"))
        .windows(20)
        .any(|w| w == b"//// [DtsFileErrors]"));
    for kind in [
        "sourcemap_and_record",
        "inline_sources_map_root",
        "declaration_map_content_mapper",
    ] {
        assert_eq!(
            by_kind(kind).native["sourcemap"]["state"],
            "content",
            "{kind}"
        );
        assert_eq!(
            by_kind(kind).native["sourcemap_record"]["state"],
            "content",
            "{kind}"
        );
    }
    assert_eq!(
        by_kind("inline_source_map").native["sourcemap"]["state"],
        "not_baselined"
    );
    assert_eq!(
        by_kind("inline_source_map").native["sourcemap_record"]["state"],
        "content"
    );
    assert!(by_kind("declaration_map_content_mapper")
        .facts
        .files
        .iter()
        .any(|file| !file.content_mapper.is_empty()));
}

/// The BOM is stripped from a declaration file the re-compilation reads, and
/// kept in the `.js` baseline.
#[test]
fn declaration_inputs_lose_their_byte_order_mark() {
    let row = row("bom");
    let result = row.result();
    let (to_be_compiled, other_files) = (views(&row.to_be_compiled), views(&row.other_files));
    let context = baselines::prepare_declaration_compilation_context(
        &to_be_compiled,
        &other_files,
        &result,
        &row.options,
        row.harness_current_directory(),
    )
    .expect("context")
    .expect("a declaration re-compilation");
    assert_eq!(context.decl_input_files.len(), 1);
    let emitted = &row.dts[0].1;
    assert!(emitted.starts_with(b"\xef\xbb\xbf"));
    assert_eq!(context.decl_input_files[0].content, &emitted[3..]);
}

/// A `noCheck` emit with a file the original emit lacks, and one whose text
/// differs: the blocks `compareResultFileSets` appends, declarations first.
#[test]
fn the_no_check_repeat_appends_missing_and_differing_files() {
    let row = row("declarations");
    let result = row.result();
    let original = row.js_emit(&result, None).expect("baseline").actual;

    let extra_js = (b"/.src/extra.js".to_vec(), b"extra();\n".to_vec());
    let changed_dts: Vec<Owned> = row
        .dts
        .iter()
        .map(|(name, text)| (name.clone(), [text.as_slice(), b"export {};\r\n"].concat()))
        .collect();
    let mut js = result.js.files().to_vec();
    js.push(TestFile {
        unit_name: &extra_js.0,
        content: &extra_js.1,
    });
    let repeat = RepeatOutputs {
        js: OrderedFiles::from_ordered(js).unwrap(),
        dts: OrderedFiles::from_ordered(views(&changed_dts)).unwrap(),
    };
    let composed = row
        .js_emit(&result, Some(&repeat))
        .expect("baseline")
        .actual;

    let (dts_name, dts_text) = &row.dts[0];
    let mut expected = original.clone();
    expected.extend_from_slice(b"\r\n\r\n!!!! File ");
    expected.extend(baselines::remove_test_path_prefixes(dts_name, false));
    expected.extend_from_slice(b" differs from original emit in noCheck emit\r\n//// [");
    expected.extend_from_slice(tsr_tspath::base_name(dts_name));
    expected.extend_from_slice(b"]\r\n");
    expected.extend(baselines::patience::diff_text(
        b"Expected\tThe full check baseline",
        b"Actual\twith noCheck set",
        dts_text,
        &changed_dts[0].1,
    ));
    expected.extend_from_slice(b"\r\n\r\n!!!! File extra.js missing from original emit, but present in noCheck emit\r\n//// [extra.js]\r\nextra();\n");
    assert_eq!(
        String::from_utf8_lossy(&composed),
        String::from_utf8_lossy(&expected)
    );
    // The diff is the patience diff of the two declaration texts.
    assert!(expected
        .windows(b"+export {};".len())
        .any(|w| w == b"+export {};"));
}

/// With no original output and a noCheck output, the blocks alone make the
/// baseline (as under `noEmitOnError` with errors).
#[test]
fn a_repeat_block_alone_is_content() {
    let row = row("plain");
    let mut result = row.result();
    result.js = OrderedFiles::default();
    result.diagnostics = 1;
    assert_eq!(row.js_emit(&result, None).unwrap().actual, NO_CONTENT);
    let file = (b"/.src/a.js".to_vec(), b"a;\n".to_vec());
    let repeat = RepeatOutputs {
        js: OrderedFiles::from_ordered(views(std::slice::from_ref(&file))).unwrap(),
        dts: OrderedFiles::default(),
    };
    let composed = row.js_emit(&result, Some(&repeat)).unwrap().actual;
    let ts_code = baselines::ts_code(
        &row.header,
        &views(&row.other_files),
        &views(&row.to_be_compiled),
    );
    let expected = [
        ts_code.as_slice(),
        b"\r\n\r\n\r\n\r\n!!!! File a.js missing from original emit, but present in noCheck emit\r\n//// [a.js]\r\na;\n",
    ]
    .concat();
    assert_eq!(
        String::from_utf8_lossy(&composed),
        String::from_utf8_lossy(&expected)
    );
}

#[test]
fn the_writers_fail_where_the_pin_asserts() {
    // No JavaScript and no diagnostics.
    let plain = row("plain");
    let mut result = plain.result();
    result.js = OrderedFiles::default();
    assert_eq!(
        plain.js_emit(&result, None).unwrap_err(),
        Failure::Assertion(
            "Expected at least one js file to be emitted or at least one error to be created."
                .into()
        )
    );

    // A missing map.
    let mapped = row_with_maps();
    let mut result = mapped.result();
    let kept = result.maps.files()[1..].to_vec();
    result.maps = OrderedFiles::from_ordered(kept).unwrap();
    let error = baselines::sourcemap_baseline(&SourcemapInput {
        configured_name: &mapped.configured_name,
        options: &mapped.options,
        full_emit_paths: mapped.full_emit_paths(),
        result: &result,
    })
    .unwrap_err();
    assert_eq!(
        error,
        Failure::Assertion("Number of sourcemap files should be same as js files.".into())
    );

    // Declarations that do not match the JavaScript files.
    let declared = row("declarations");
    let mut result = declared.result();
    result.dts = OrderedFiles::default();
    let error = baselines::prepare_declaration_compilation_context(
        &views(&declared.to_be_compiled),
        &views(&declared.other_files),
        &result,
        &declared.options,
        declared.harness_current_directory(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        Failure::Assertion("There were no errors and declFiles generated did not match number of js files generated".into())
    );
}

fn row_with_maps() -> Row {
    let row = row("sourcemap_and_record");
    assert!(!row.maps.is_empty());
    row
}

/// The source-map preview link: base64 of the JavaScript, the map and each
/// source's original text, as the pin appends it.
#[test]
fn the_preview_link_encodes_the_output_the_map_and_the_sources() {
    let row = row_with_maps();
    let text = unhex(&row.native["sourcemap"]["text_hex"]);
    let marker = b"\n//// https://sokra.github.io/source-map-visualization#base64,";
    let at = text
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("a preview link");
    let link = &text[at + marker.len()..];
    let link = &link[..link
        .iter()
        .position(|&b| b == b'\n')
        .expect("the link's newline")];
    let parts: Vec<&[u8]> = link.split(|&b| b == b',').collect();
    assert_eq!(parts.len(), 3, "the output, the map and one source");
    assert!(parts.iter().all(|part| part.len() % 4 == 0));
}

#[test]
fn test_path_prefixes_are_replaced_in_one_pass() {
    let cases: [(&[u8], bool, &[u8]); 6] = [
        (b"/.src/a.ts", false, b"a.ts"),
        (b"/.src/a.ts", true, b"/a.ts"),
        (
            b"file:///./src/a.ts and /.lib/lib.d.ts",
            false,
            b"file:///a.ts and lib.d.ts",
        ),
        (b"bundled:///libs/lib.es5.d.ts", true, b"/lib.es5.d.ts"),
        // The replacement is not re-scanned.
        (b"/.s/.src/rc/", false, b"/.src/"),
        (b"/.ts/x", false, b"x"),
    ];
    for (text, retain, expected) in cases {
        assert_eq!(
            baselines::remove_test_path_prefixes(text, retain),
            expected,
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

/// `baseline.DiffText` against the pinned patience diff, over the cases
/// `tools/phase3/patience_probe` rendered with the pin's module.
#[test]
fn the_patience_diff_matches_the_pinned_module() {
    let document = fixture_document("patience-diff.json");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 40);
    for case in cases {
        let (expected, actual) = (unhex(&case["expected_hex"]), unhex(&case["actual_hex"]));
        assert_eq!(
            String::from_utf8_lossy(&baselines::patience::diff_text(
                b"Expected\tThe full check baseline",
                b"Actual\twith noCheck set",
                &expected,
                &actual,
            )),
            String::from_utf8_lossy(&unhex(&case["diff_hex"])),
            "{:?} -> {:?}",
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&actual)
        );
    }
}
