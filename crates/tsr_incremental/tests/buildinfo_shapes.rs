//! Go `BuildInfo` uses `omitzero` for these nullable collections. Expectations
//! were checked against json.Unmarshal/json.Marshal in the pinned package;
//! unlike `omitempty`, `omitzero` preserves non-nil empty slices and maps.
use tsr_incremental::BuildInfo;

#[test]
fn build_info_preserves_null_and_empty_collection_shapes() {
    for field in [
        "root",
        "packageJsons",
        "missingPackageJsons",
        "contentMapperIdentities",
        "fileNames",
        "fileInfos",
        "fileIdsList",
        "referencedMap",
        "semanticDiagnosticsPerFile",
        "emitDiagnosticsPerFile",
        "changeFileSet",
        "affectedFilesPendingEmit",
        "emitSignatures",
        "resolvedRoot",
        "options",
    ] {
        let empty = if field == "options" { "{}" } else { "[]" };
        let empty_text = format!("{{\"{field}\":{empty}}}");
        let null_text = format!("{{\"{field}\":null}}");
        for (text, expected) in [
            (empty_text.as_str(), empty_text.as_str()),
            (&null_text, "{}"),
        ] {
            let mut info = BuildInfo::default();
            tsr_json::unmarshal(text.as_bytes(), &mut info, tsr_json::Options::default()).unwrap();
            assert_eq!(
                tsr_json::marshal(&info, tsr_json::Options::default()).unwrap(),
                expected.as_bytes(),
                "{text}"
            );
            // A present but empty fileNames is still non-incremental (the pin
            // checks len, not nil). Shape must not change cache eligibility.
            assert!(!BuildInfo::is_incremental(Some(&info)), "{text}");
        }
    }
}

#[test]
fn build_info_collection_decoding_clears_null_and_retains_absent_fields() {
    let mut info = BuildInfo::default();
    for (text, expected) in [
        (
            r#"{"fileNames":["./a.ts"],"referencedMap":[],"affectedFilesPendingEmit":[]}"#,
            r#"{"fileNames":["./a.ts"],"referencedMap":[],"affectedFilesPendingEmit":[]}"#,
        ),
        (
            "{}",
            r#"{"fileNames":["./a.ts"],"referencedMap":[],"affectedFilesPendingEmit":[]}"#,
        ),
        (
            r#"{"fileNames":[],"referencedMap":null,"affectedFilesPendingEmit":null}"#,
            r#"{"fileNames":[]}"#,
        ),
        (r#"{"fileNames":null}"#, "{}"),
    ] {
        tsr_json::unmarshal(text.as_bytes(), &mut info, tsr_json::Options::default()).unwrap();
        assert_eq!(
            tsr_json::marshal(&info, tsr_json::Options::default()).unwrap(),
            expected.as_bytes(),
            "after {text}"
        );
        assert_eq!(
            BuildInfo::is_incremental(Some(&info)),
            expected.contains("./a.ts")
        );
    }
}

#[test]
fn build_info_preserves_nested_diagnostic_collection_shapes() {
    for field in ["messageArgs", "messageChain", "relatedInformation"] {
        for value in ["null", "[]"] {
            let diagnostic = format!("{{\"{field}\":{value}}}");
            let text = format!(
                "{{\"fileNames\":[\"./a.ts\"],\"semanticDiagnosticsPerFile\":[[1,[{diagnostic}]]]}}"
            );
            let expected_diagnostic = if value == "null" { "{}" } else { &diagnostic };
            let expected = format!(
                "{{\"fileNames\":[\"./a.ts\"],\"semanticDiagnosticsPerFile\":[[1,[{expected_diagnostic}]]]}}"
            );
            let mut info = BuildInfo::default();
            tsr_json::unmarshal(text.as_bytes(), &mut info, tsr_json::Options::default()).unwrap();
            assert_eq!(
                tsr_json::marshal(&info, tsr_json::Options::default()).unwrap(),
                expected.as_bytes(),
                "{text}"
            );
        }
    }
}
