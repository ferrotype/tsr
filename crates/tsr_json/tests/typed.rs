use tsr_core::collections::OrderedMap;
use tsr_json::{marshal, marshal_indent, Encode, Encoder, Error, Options};
use tsr_jsstring::JsString;

#[test]
fn byte_keys_are_checked_after_repair_without_modifying_the_input() {
    let map: OrderedMap<JsString, i64> = [
        (JsString::from_bytes([0xff]), 1),
        (JsString::from_bytes([0xfe]), 2),
    ]
    .into_iter()
    .collect();
    assert_eq!(marshal(&map, Options::default()), Err(Error::DuplicateName));
    assert_eq!(
        marshal(
            &map,
            Options {
                allow_duplicate_names: true,
                ..Options::default()
            }
        )
        .unwrap(),
        "{\"�\":1,\"�\":2}".as_bytes()
    );
    assert_eq!(map.entry_at(0).unwrap().0.as_bytes(), [0xff]);
}

#[test]
fn indent_fork_and_nested_empty_containers_keep_their_shape() {
    let map: OrderedMap<String, Vec<i64>> = [("z".into(), vec![1, 2]), ("a".into(), vec![])]
        .into_iter()
        .collect();
    assert_eq!(
        marshal_indent(&map, "", "").unwrap(),
        br#"{"z":[1,2],"a":[]}"#
    );
    assert_eq!(
        marshal_indent(&map, "\t", "  ").unwrap(),
        b"{\n\t  \"z\": [\n\t    1,\n\t    2\n\t  ],\n\t  \"a\": []\n\t}"
    );
    assert_eq!(
        marshal(
            &map,
            Options {
                indent: Some(""),
                ..Options::default()
            }
        )
        .unwrap(),
        b"{\n\"z\": [\n1,\n2\n],\n\"a\": []\n}"
    );
    assert_eq!(marshal_indent(&map, "x", " "), Err(Error::InvalidIndent));
}

#[test]
fn depth_limit_precedes_stack_exhaustion() {
    struct Nested(usize);
    impl Encode for Nested {
        fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
            if self.0 == 0 {
                out.null();
                Ok(())
            } else {
                out.array([&Self(self.0 - 1)])
            }
        }
    }
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            assert_eq!(
                marshal(&Nested(10_000), Options::default()).unwrap().len(),
                20_004
            );
            assert_eq!(
                marshal(&Nested(10_001), Options::default()),
                Err(Error::NestingDepth)
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
