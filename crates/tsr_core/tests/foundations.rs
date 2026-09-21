#[cfg(feature = "go-slice-compat")]
use tsr_core::slices::SharedSlice;
use tsr_core::{helpers, CompilerOptions};
use tsr_jsstring::JsString;

#[test]
fn option_clone_isolates_all_nine_formerly_shared_fields() {
    let text = || vec![JsString::from_bytes(b"original".as_slice())];
    let mut source = CompilerOptions {
        checkers: Some(3),
        max_node_module_js_depth: Some(2),
        types: Some(text()),
        lib: Some(text()),
        custom_conditions: Some(text()),
        module_suffixes: Some(text()),
        root_dirs: Some(text()),
        type_roots: Some(text()),
        paths: Some(
            [(JsString::from_bytes(b"*".as_slice()), Some(text()))]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    let copy = std::sync::Arc::new(source.clone());
    *source.checkers.as_mut().unwrap() = 7;
    *source.max_node_module_js_depth.as_mut().unwrap() = 9;
    for field in [
        &mut source.types,
        &mut source.lib,
        &mut source.custom_conditions,
        &mut source.module_suffixes,
        &mut source.root_dirs,
        &mut source.type_roots,
    ] {
        field.as_mut().unwrap()[0] = JsString::from_bytes(b"changed".as_slice());
        field
            .as_mut()
            .unwrap()
            .push(JsString::from_bytes(b"appended".as_slice()));
    }
    source
        .paths
        .as_mut()
        .unwrap()
        .get_mut(b"*".as_slice())
        .unwrap()
        .as_mut()
        .unwrap()[0] = JsString::from_bytes(b"changed".as_slice());
    source
        .paths
        .as_mut()
        .unwrap()
        .insert(JsString::from_bytes(b"extra".as_slice()), None);
    assert_eq!(copy.checkers, Some(3));
    assert_eq!(copy.max_node_module_js_depth, Some(2));
    for field in [
        &copy.types,
        &copy.lib,
        &copy.custom_conditions,
        &copy.module_suffixes,
        &copy.root_dirs,
        &copy.type_roots,
    ] {
        assert_eq!(field.as_ref().unwrap(), &text());
    }
    assert_eq!(copy.paths.as_ref().unwrap().len(), 1);
    assert_eq!(
        copy.paths.as_ref().unwrap().get(b"*".as_slice()),
        Some(&Some(text()))
    );
}

#[test]
#[cfg(feature = "go-slice-compat")]
fn callback_can_change_a_later_element_through_an_alias() {
    let source = SharedSlice::from_vec(vec![1, 2, 3]);
    let mut alias = source.clone();
    let result = helpers::map(&source, |value| {
        if *value == 1 {
            alias.set(1, 20);
        }
        *value
    });
    assert_eq!(&*result.read(), &[1, 20, 3]);
    let result = helpers::filter(&source, |value| {
        if *value == 1 {
            alias.set(2, 30);
        }
        *value != 20
    });
    assert_eq!(&*result.read(), &[1, 30]);
}

#[test]
fn memoize_retries_a_panicking_initializer_and_retains_success() {
    let mut calls = 0;
    let mut value = helpers::memoize(|| {
        calls += 1;
        assert!(calls != 1, "first attempt");
        calls
    });
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut value)).is_err());
    assert_eq!(value(), 2);
    assert_eq!(value(), 2);
}

#[test]
#[cfg(feature = "go-slice-compat")]
fn shared_slice_value_comparison_does_not_turn_nan_into_equal() {
    let a = SharedSlice::from_vec(vec![f64::NAN]);
    assert_ne!(a, a.clone());
    assert!(a.same(&a.clone()));
}
