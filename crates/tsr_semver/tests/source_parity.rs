//! Observations execute the pinned Go functions, including partially parsed
//! values after errors. Requests are frozen independently of Rust outcomes.
use std::cmp::Ordering;

use serde_json::Value;
use ts_semver::{try_parse_version, try_parse_version_range, ParseError, Version, VersionRange};

fn requests() -> Vec<Vec<u8>> {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("../../../data/s07/semver-requests.json")).unwrap();
    rows.iter()
        .map(|row| {
            row["hex"]
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        })
        .collect()
}
fn observations() -> Value {
    serde_json::from_str(include_str!("../../../data/s07/semver-observations.json")).unwrap()
}
fn qualifiers(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|rows| rows.iter().map(|v| v.as_str().unwrap()).collect())
        .unwrap_or_default()
}
fn versions(requests: &[Vec<u8>]) -> Vec<Version> {
    requests
        .iter()
        .map(|request| try_parse_version(request).0)
        .collect()
}

#[test]
fn version_results_and_partial_failures_match_go() {
    let requests = requests();
    let observations = observations();
    let rows = observations["versions"].as_array().unwrap();
    assert_eq!(rows.len(), requests.len());
    for (request, row) in requests.iter().zip(rows) {
        let (version, error) = try_parse_version(request);
        assert_eq!(
            u64::from(version.major()),
            row["major"].as_u64().unwrap(),
            "{request:?}"
        );
        assert_eq!(
            u64::from(version.minor()),
            row["minor"].as_u64().unwrap(),
            "{request:?}"
        );
        assert_eq!(
            u64::from(version.patch()),
            row["patch"].as_u64().unwrap(),
            "{request:?}"
        );
        assert_eq!(
            version.prerelease(),
            qualifiers(&row["prerelease"]),
            "{request:?}"
        );
        assert_eq!(version.build(), qualifiers(&row["build"]), "{request:?}");
        assert_eq!(
            version.to_string(),
            row["display"].as_str().unwrap(),
            "{request:?}"
        );
        let error_type = match error.as_ref() {
            None => "",
            Some(ParseError::InvalidVersion(_)) => "*semver.SemverParseError",
            Some(ParseError::ComponentOverflow(_)) => "*strconv.NumError",
        };
        assert_eq!(
            error_type,
            row["error_type"].as_str().unwrap(),
            "{request:?}"
        );
        assert_eq!(
            error.as_ref().map(ToString::to_string).unwrap_or_default(),
            row["error"].as_str().unwrap(),
            "{request:?}"
        );
        assert_eq!(
            Version::parse(request).is_ok(),
            error.is_none(),
            "{request:?}"
        );
    }
}

#[test]
fn comparison_matrix_including_nil_and_partial_values_matches_go() {
    let requests = requests();
    let versions = versions(&requests);
    let targets: Vec<_> = std::iter::once(None)
        .chain(versions.iter().map(Some))
        .collect();
    let observations = observations();
    let expected = std::iter::once(observations["nil_compare"].as_str().unwrap()).chain(
        observations["versions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["compare"].as_str().unwrap()),
    );
    for (left, (version, expected)) in targets.iter().zip(expected).enumerate() {
        assert_eq!(expected.len(), targets.len());
        for (right, (target, expected)) in targets.iter().zip(expected.bytes()).enumerate() {
            let actual = match Version::compare(*version, *target) {
                Ordering::Less => b'<',
                Ordering::Equal => b'=',
                Ordering::Greater => b'>',
            };
            assert_eq!(
                actual, expected,
                "left index {left}, right index {right}; {version:?} vs {target:?}"
            );
        }
    }
}

#[test]
fn range_parse_format_and_full_test_matrix_match_go() {
    let requests = requests();
    let versions = versions(&requests);
    let targets: Vec<_> = std::iter::once(None)
        .chain(versions.iter().map(Some))
        .collect();
    let observations = observations();
    let rows = observations["ranges"].as_array().unwrap();
    assert_eq!(rows.len(), requests.len());
    for (request, row) in requests.iter().zip(rows) {
        let (range, ok) = try_parse_version_range(request);
        assert_eq!(ok, row["ok"].as_bool().unwrap(), "{request:?}");
        assert_eq!(VersionRange::parse(request).is_some(), ok, "{request:?}");
        assert_eq!(
            range.to_string(),
            row["display"].as_str().unwrap(),
            "{request:?}"
        );
        let expected = row["matches"].as_str().unwrap();
        assert_eq!(expected.len(), targets.len());
        for (target, expected) in targets.iter().zip(expected.bytes()) {
            assert_eq!(
                range.test(*target),
                expected == b'1',
                "range {request:?}, target {target:?}"
            );
        }
    }
}

#[test]
fn must_parse_keeps_the_source_error_message() {
    let panic = std::panic::catch_unwind(|| Version::must_parse(b"1.4294967296.0")).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<String>().unwrap(),
        "strconv.ParseUint: parsing \"4294967296\": value out of range"
    );
}
