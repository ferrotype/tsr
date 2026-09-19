use super::*;
use crate::{AstBuilder, FactoryMethods, JsString};
use std::collections::BTreeMap;
use ts_arena::Counters;
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn base() -> Diagnostic {
    let mut diagnostic = Diagnostic::external(
        None,
        TextRange::new(1, 2),
        JsString::default(),
        1,
        100,
        JsString::default(),
    );
    diagnostic.message_key = JsString::from_bytes(b"key".as_slice());
    diagnostic
}
fn pair(
    name: &'static str,
    edit: impl FnOnce(&mut Diagnostic, &mut Diagnostic),
) -> (&'static str, Diagnostic, Diagnostic) {
    let (mut left, mut right) = (base(), base());
    edit(&mut left, &mut right);
    (name, left, right)
}

#[test]
fn source_diagnostic_order_and_equality_discriminators() {
    let expected: BTreeMap<_, _> =
        include_str!("../../../data/s07/diagnostic-order-observations.tsv")
            .lines()
            .map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                (
                    fields[0],
                    (
                        fields[1].parse::<i8>().unwrap(),
                        fields[2].parse::<bool>().unwrap(),
                        fields[3].parse::<bool>().unwrap(),
                    ),
                )
            })
            .collect();
    let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
    let file_a = factory.new_identifier(JsString::from_bytes(b"file a".as_slice()));
    let file_a_other_path =
        factory.new_identifier(JsString::from_bytes(b"file a, other path".as_slice()));
    let file_z = factory.new_identifier(JsString::from_bytes(b"file z".as_slice()));
    let file_ff = factory.new_identifier(JsString::from_bytes(b"file ff".as_slice()));
    let file_c0 = factory.new_identifier(JsString::from_bytes(b"file c0".as_slice()));
    let names: BTreeMap<_, &[u8]> = [
        (file_a, b"/a.ts".as_slice()),
        (file_a_other_path, b"/a.ts"),
        (file_z, b"/z.ts"),
        (file_ff, b"/\xff.ts"),
        (file_c0, b"/\xc0.ts"),
    ]
    .into();
    let file_name = |id| names.get(&id).copied().ok_or("missing source file");
    static ADHOC_A: ts_diagnostics::Message = ts_diagnostics::Message {
        code: -1,
        category: ts_diagnostics::Category::Error,
        key: "-1",
        text: "a",
        reports_unnecessary: false,
        reports_deprecated: false,
        elided_in_compatibility_pyramid: false,
    };
    static ADHOC_Z: ts_diagnostics::Message = ts_diagnostics::Message {
        text: "z",
        ..ADHOC_A
    };
    let cases = vec![
        pair("same", |_, _| {}),
        pair("file_nil", |_, r| r.file = Some(file_a)),
        pair("file_name_bytes", |l, r| {
            l.file = Some(file_ff);
            r.file = Some(file_c0);
        }),
        pair("same_name_other_path", |l, r| {
            l.file = Some(file_a);
            r.file = Some(file_a_other_path);
        }),
        pair("position_extremes", |l, r| {
            l.loc = TextRange::new(i64::from(i32::MIN), 2);
            r.loc = TextRange::new(i64::from(i32::MAX), 2);
        }),
        pair("end_extremes", |l, r| {
            l.loc = TextRange::new(1, i64::from(i32::MAX));
            r.loc = TextRange::new(1, i64::from(i32::MIN));
        }),
        pair("code_extremes", |l, r| {
            l.code = i32::MIN;
            r.code = i32::MAX;
        }),
        pair("category_extremes", |l, r| {
            l.category = i32::MAX;
            r.category = i32::MIN;
        }),
        pair("source_bytes", |l, r| {
            l.source = JsString::from_bytes(b"\xff".as_slice());
            r.source = JsString::from_bytes(b"\xc0".as_slice());
        }),
        pair("localized_identity", |l, r| {
            l.message_text = JsString::from_bytes(b"a".as_slice());
            r.message_text = JsString::from_bytes(b"z".as_slice());
            l.message_key = JsString::from_bytes(b"z".as_slice());
            r.message_key = JsString::from_bytes(b"a".as_slice());
        }),
        pair("adhoc_identity", |l, r| {
            l.code = -1;
            r.code = -1;
            l.message = Some(&ADHOC_Z);
            r.message = Some(&ADHOC_A);
        }),
        pair("arguments", |l, r| {
            l.message_args = vec![
                JsString::from_bytes(b"a".as_slice()),
                JsString::from_bytes(b"\xff".as_slice()),
            ];
            r.message_args = vec![
                JsString::from_bytes(b"a".as_slice()),
                JsString::from_bytes(b"\xc0".as_slice()),
            ];
        }),
        pair("chain_code_ignored_by_sort", |l, r| {
            let (mut a, mut b) = (base(), base());
            a.code = 1;
            b.code = 2;
            l.message_chain = vec![Arc::new(a)];
            r.message_chain = vec![Arc::new(b)];
        }),
        pair("chain_text_ignored_by_both", |l, r| {
            let (mut a, mut b) = (base(), base());
            a.message_text = JsString::from_bytes(b"a".as_slice());
            b.message_text = JsString::from_bytes(b"z".as_slice());
            l.message_chain = vec![Arc::new(a)];
            r.message_chain = vec![Arc::new(b)];
        }),
        pair("larger_chain_first", |l, r| {
            l.message_chain = vec![Arc::new(base()), Arc::new(base())];
            r.message_chain = vec![Arc::new(base())];
        }),
        pair("recursive_size_before_arguments", |l, r| {
            let (mut a, mut b) = (base(), base());
            a.message_args = vec![JsString::from_bytes(b"z".as_slice())];
            b.message_args = vec![JsString::from_bytes(b"a".as_slice())];
            a.message_chain = vec![Arc::new(base())];
            l.message_chain = vec![Arc::new(a)];
            r.message_chain = vec![Arc::new(b)];
        }),
        pair("recursive_arguments", |l, r| {
            let (mut a, mut b) = (base(), base());
            a.message_args = vec![JsString::from_bytes(b"a".as_slice())];
            b.message_args = vec![JsString::from_bytes(b"z".as_slice())];
            l.message_chain = vec![Arc::new(a)];
            r.message_chain = vec![Arc::new(b)];
        }),
        pair("larger_related_first", |l, _| {
            l.related_information = vec![Arc::new(base())];
        }),
        pair("related_file_order", |l, r| {
            let (mut a, mut b) = (base(), base());
            a.file = Some(file_z);
            b.file = Some(file_a);
            l.related_information = vec![Arc::new(a)];
            r.related_information = vec![Arc::new(b)];
        }),
        pair("flags_ignored", |l, _| {
            l.reports_unnecessary = true;
            l.reports_deprecated = true;
            l.skipped_on_no_emit = true;
        }),
        pair("nil_empty_arguments", |_, r| {
            r.message_args = Vec::with_capacity(1);
        }),
    ];
    assert_eq!(cases.len(), expected.len());
    for (name, left, right) in cases {
        let order = match compare_diagnostics(&left, &right, &file_name).unwrap() {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        };
        let actual = (
            order,
            equal_diagnostics(&left, &right, &file_name).unwrap(),
            equal_diagnostics_no_related_info(&left, &right, &file_name).unwrap(),
        );
        assert_eq!(actual, expected[name], "{name}");
    }
}

#[test]
fn identity_shortcut_and_fallible_borrowed_lookup() {
    let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
    let mut left = base();
    left.file = Some(factory.new_identifier(JsString::from_bytes(b"file".as_slice())));
    let fail = |_| Err::<&[u8], _>("retired owner");
    assert_eq!(
        compare_diagnostics(&left, &left, &fail),
        Ok(Ordering::Equal)
    );
    assert_eq!(equal_diagnostics(&left, &left, &fail), Ok(true));
    assert_eq!(
        equal_diagnostics_no_related_info(&left, &left, &fail),
        Ok(true)
    );
    let right = left.clone();
    assert_eq!(
        compare_diagnostics(&left, &right, &fail),
        Err("retired owner")
    );
    assert_eq!(
        equal_diagnostics(&left, &right, &fail),
        Err("retired owner")
    );
}
