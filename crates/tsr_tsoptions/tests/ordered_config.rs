use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{compiler_options_value, parse_string_map, stringify_json, ConfigValue as V};

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
