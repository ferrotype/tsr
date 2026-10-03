use std::collections::HashMap;
use tsr_json::{marshal, unmarshal, Options};

#[test]
fn boxed_map_merge_keeps_prior_entries_and_partial_member_writes() {
    let mut value = Some(Box::new(HashMap::from([
        ("kept".to_string(), vec![1i32]),
        ("replaced".to_string(), vec![9]),
    ])));
    let result = unmarshal(
        br#"{"replaced":[2,3],"failed":[4,"bad"]}"#,
        &mut value,
        Options::default(),
    );
    assert!(result.is_err());
    let map = value.as_ref().unwrap();
    assert_eq!(map["kept"], [1]);
    assert_eq!(map["replaced"], [2, 3]);
    assert_eq!(map["failed"], [4, 0]);
    assert_eq!(
        marshal(
            &value,
            Options {
                deterministic: Some(true),
                ..Options::default()
            }
        )
        .unwrap(),
        br#"{"failed":[4,0],"kept":[1],"replaced":[2,3]}"#
    );
    unmarshal(b"null", &mut value, Options::default()).unwrap();
    assert!(value.is_none());
}

#[test]
fn map_duplicate_policy_checks_decoded_keys_and_reuses_destinations() {
    let mut value: HashMap<i64, i32> = HashMap::from([(1, 5)]);
    unmarshal(br#"{"1":2}"#, &mut value, Options::default()).unwrap();
    assert_eq!(value[&1], 2); // Existing entries are not duplicate input names.
    let error = unmarshal(br#"{"0":1,"-0":2}"#, &mut value, Options::default()).unwrap_err();
    assert!(error.is_duplicate_name());
    assert!(error.to_string().contains("\"-0\""));
    assert_eq!(value[&0], 1);
    unmarshal(
        br#"{"0":3,"-0":4}"#,
        &mut value,
        Options {
            allow_duplicate_names: Some(true),
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(value[&0], 4);
    unmarshal(b"null", &mut value, Options::default()).unwrap();
    assert!(value.is_empty());
}
