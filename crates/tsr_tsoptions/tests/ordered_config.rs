use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{compiler_options_value, parse_string_map, stringify_json, ConfigValue as V};

#[test]
fn config_dir_substitution_of_a_clone_preserves_original_options() {
    let strings = |value: &[u8]| vec![JsString::from_bytes(value)];
    let original = CompilerOptions {
        paths: Some(
            [
                (
                    JsString::from_bytes(b"*".as_slice()),
                    Some(strings(b"${configDir}/src/*")),
                ),
                (
                    JsString::from_bytes(b"plain".as_slice()),
                    Some(strings(b"./unchanged")),
                ),
                (JsString::from_bytes(b"nil".as_slice()), None),
                (JsString::from_bytes(b"empty".as_slice()), Some(vec![])),
            ]
            .into_iter()
            .collect(),
        ),
        root_dirs: Some(strings(b"${configDir}/roots")),
        type_roots: Some(strings(b"${configDir}/types")),
        ..Default::default()
    };
    let before = stringify_json(&compiler_options_value(&original)).unwrap();
    let mut derived = original.clone();
    tsr_tsoptions::substitute_options(&mut derived, b"/project");
    // The pin clones affected Paths entries, RootDirs and TypeRoots before
    // substitution (tsconfigparsing.go:1807-1849). Original bytes stay intact.
    assert_eq!(
        stringify_json(&compiler_options_value(&original)).unwrap(),
        before
    );
    let paths = derived.paths.as_ref().unwrap();
    assert_eq!(
        paths.get(b"*".as_slice()),
        Some(&Some(strings(b"/project/src/*")))
    );
    assert_eq!(
        paths.get(b"plain".as_slice()),
        Some(&Some(strings(b"./unchanged")))
    );
    assert_eq!(paths.get(b"nil".as_slice()), Some(&None));
    assert_eq!(paths.get(b"empty".as_slice()), Some(&Some(vec![])));
    assert_eq!(derived.root_dirs, Some(strings(b"/project/roots")));
    assert_eq!(derived.type_roots, Some(strings(b"/project/types")));
}

#[test]
fn config_overwrite_preserves_order_and_paths_keep_nil_and_empty_values() {
    let mut value = V::Object(tsr_core::collections::OrderedMap::default());
    value.set(JsString::from_bytes(b"b".as_slice()), V::Array(None));
    value.set(
        JsString::from_bytes(b"a".as_slice()),
        V::Array(Some(vec![])),
    );
    value.set(JsString::from_bytes(b"b".as_slice()), V::Array(None));
    assert_eq!(stringify_json(&value).unwrap(), br#"{"b":[],"a":[]}"#);
    let paths = parse_string_map(&value).unwrap();
    assert_eq!(paths.get(b"b".as_slice()), Some(&None));
    assert_eq!(paths.get(b"a".as_slice()), Some(&Some(vec![])));
    let mut options = CompilerOptions {
        paths: Some(paths),
        ..Default::default()
    };
    assert_eq!(
        stringify_json(&compiler_options_value(&options)).unwrap(),
        br#"{"paths":{"b":[],"a":[]}}"#
    );
    // The sharing difference already recorded by F1a stays visible: this
    // migration must not make CompilerOptions::clone start sharing mutation.
    let cloned = options.clone();
    options.paths.as_mut().unwrap().insert(
        JsString::from_bytes(b"b".as_slice()),
        Some(vec![JsString::from_bytes(b"new".as_slice())]),
    );
    assert_eq!(cloned.paths.unwrap().get(b"b".as_slice()), Some(&None));
    options.paths = Some(tsr_core::PathMappings::default());
    assert_eq!(
        stringify_json(&compiler_options_value(&options)).unwrap(),
        br#"{"paths":{}}"#
    );
    options.paths = None;
    assert_eq!(
        stringify_json(&compiler_options_value(&options)).unwrap(),
        b"{}"
    );
}
