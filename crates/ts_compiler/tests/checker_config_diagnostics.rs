#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;

#[test]
fn original_config_diagnostics_reach_the_baseline_executor() {
    let requests: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/requests.json"
    ))
    .unwrap();
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/observations.json"
    ))
    .unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        let actual = executor::observe(request, |_, _, _, _| {
            panic!("diagnostic-only request must not run a baseline walker")
        });
        assert_eq!(
            actual["load"]["state"], "executed",
            "{}: {actual}",
            request["id"]
        );
        assert_eq!(actual["phases"]["config"]["state"], "executed");
        assert_eq!(
            actual["phases"]["config"]["diagnostics"], expected["diagnostics"],
            "{}",
            request["id"]
        );
    }
}

#[test]
fn inherited_config_diagnostics_keep_their_source_during_formatting() {
    let requests: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/requests.json"
    ))
    .unwrap();
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/observations.json"
    ))
    .unwrap();
    let mut request = requests[0].clone();
    let inputs = request["error_inputs"].as_array_mut().unwrap();
    let original = inputs[0].clone();
    let base_name = executor::diagnostics::hex(b"/foo/base.json");
    inputs[0]["content_hex"] =
        serde_json::json!(executor::diagnostics::hex(br#"{"extends":"./base.json"}"#));
    inputs.push(serde_json::json!({"name_hex":base_name,"content_hex":original["content_hex"]}));
    request["error_baseline_requested"] = true.into();
    let result = executor::observe(&request, |program, _, _, diagnostics| {
        let sorted = program
            .sort_and_deduplicate_diagnostics(diagnostics.unwrap())
            .unwrap();
        let mut writer = ts_compiler::diagnostic_writer::DiagnosticWriter::new(
            program,
            ts_compiler::diagnostic_writer::FormattingOptions::default(),
        );
        assert_eq!(sorted.len(), 8);
        for diagnostic in &sorted {
            assert_eq!(
                writer.file(diagnostic).unwrap().unwrap().name(),
                b"/foo/base.json"
            );
        }
        executor::BaselineResults {
            type_symbols: serde_json::json!({"state":"not_requested"}),
            errors: serde_json::json!({"state":"not_requested"}),
        }
    });
    let mut diagnostics = expected[0]["diagnostics"].clone();
    for diagnostic in diagnostics.as_array_mut().unwrap() {
        diagnostic["file_hex"] = base_name.clone().into();
    }
    assert_eq!(result["phases"]["config"]["diagnostics"], diagnostics);
}

#[test]
fn config_include_specs_reach_program_diagnostics() {
    let request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/rootdir-request.json"
    ))
    .unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/rootdir-observations.json"
    ))
    .unwrap();
    let result = executor::observe(&request, |program, _, _, diagnostics| {
        let sorted = program
            .sort_and_deduplicate_diagnostics(diagnostics.unwrap())
            .unwrap();
        executor::BaselineResults {
            type_symbols: serde_json::json!({"state":"not_requested"}),
            errors: executor::diagnostics::phase(program, &sorted),
        }
    });
    assert_eq!(result["error_baseline"]["diagnostics"], expected);
}

#[test]
fn original_config_text_is_not_decoded_as_a_filesystem_read() {
    let mut request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/rootdir-request.json"
    ))
    .unwrap();
    // parseTestCaseContentWithSettings passes the unit's bytes directly to
    // ParseSourceFile. A BOM is part of this source, not a file-load prefix.
    let text = b"\xef\xbb\xbf{\"compilerOptions\":{}}";
    request["error_inputs"][0]["content_hex"] = executor::diagnostics::hex(text).into();
    let result = executor::observe(&request, |program, _, _, _| {
        let config = program.config().config_file.as_ref().unwrap();
        assert_eq!(
            config
                .file
                .view()
                .source_file(config.root)
                .unwrap()
                .text()
                .as_bytes(),
            text
        );
        executor::BaselineResults {
            type_symbols: serde_json::json!({"state":"not_requested"}),
            errors: serde_json::json!({"state":"not_requested"}),
        }
    });
    assert_eq!(result["load"]["state"], "executed");
}

#[test]
fn missing_original_config_is_an_explicit_capture_failure() {
    let requests: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/config-diagnostics/requests.json"
    ))
    .unwrap();
    let mut request = requests[0].clone();
    request["error_inputs"].as_array_mut().unwrap().remove(0);
    let result = executor::observe(&request, |_, _, _, _| {
        panic!("config failure must stop loading")
    });
    assert_eq!(result["load"]["state"], "failed");
    assert_eq!(result["load"]["class"], "config_parse");
    assert_eq!(
        result["load"]["reason"],
        "config source missing from original baseline inputs"
    );
}
