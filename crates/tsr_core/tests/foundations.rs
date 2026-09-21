use tsr_core::{helpers, slices::SharedSlice, CompilerOptions};
use tsr_jsstring::JsString;

#[test]
fn option_clone_retains_pointees_but_replacing_a_field_does_not() {
    let mut source = CompilerOptions {
        checkers: Some(3.into()),
        types: Some(vec![JsString::from_bytes(b"a".as_slice())].into()),
        ..Default::default()
    };
    let copy = source.clone();
    source.checkers.as_ref().unwrap().set(7);
    source
        .types
        .as_mut()
        .unwrap()
        .set(0, JsString::from_bytes(b"b".as_slice()));
    assert_eq!(copy.checkers.as_ref().unwrap().get(), 7);
    assert_eq!(
        copy.types.as_ref().unwrap().first().unwrap().as_bytes(),
        b"b"
    );
    source.checkers = Some(11.into());
    source.types = None;
    assert_eq!(copy.checkers.unwrap().get(), 7);
    assert_eq!(copy.types.unwrap().len(), 1);
}

#[test]
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
fn shared_slice_value_comparison_does_not_turn_nan_into_equal() {
    let a = SharedSlice::from_vec(vec![f64::NAN]);
    assert_ne!(a, a.clone());
    assert!(a.same(&a.clone()));
}
