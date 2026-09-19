use ts_diagnostics::{by_code, by_key, Category, MESSAGES};

#[test]
fn every_pinned_message_is_reachable_by_code_and_key() {
    assert_eq!(MESSAGES.len(), 2211);
    for message in MESSAGES {
        assert!(std::ptr::eq(by_code(message.code).unwrap(), message));
        assert!(std::ptr::eq(by_key(message.key).unwrap(), message));
    }
    assert_eq!(by_code(i32::MIN), None);
    assert_eq!(by_code(i32::MAX), None);
    assert_eq!(by_key("Identifier_expected"), None);
    assert_eq!(by_key(""), None);
}

#[test]
fn preserves_pinned_names_keys_categories_and_extra_messages() {
    let identifier = ts_diagnostics::Identifier_expected;
    assert_eq!(identifier.code, 1003);
    assert_eq!(identifier.text, "Identifier expected.");
    assert_eq!(identifier.key, "Identifier_expected_1003");
    assert_eq!(identifier.category, Category::Error);

    // Leading digit names get a Go export prefix, but localization keys do not.
    let extra = ts_diagnostics::X_4_unless_singleThreaded_is_passed;
    assert_eq!(extra.code, 100_004);
    assert_eq!(extra.category, Category::Message);
    assert_eq!(extra.text, "4, unless --singleThreaded is passed.");
    assert_eq!(extra.key, "4_unless_singleThreaded_is_passed_100004");

    let placeholder = ts_diagnostics::Type_0_is_not_assignable_to_type_1;
    assert_eq!(
        placeholder.text,
        "Type '{0}' is not assignable to type '{1}'."
    );
}

#[test]
fn preserves_all_three_independent_message_flags() {
    assert!(by_code(6133).unwrap().reports_unnecessary);
    assert!(by_code(6385).unwrap().reports_deprecated);
    assert_eq!(by_code(6385).unwrap().category, Category::Suggestion);
    assert_eq!(
        MESSAGES
            .iter()
            .filter(|message| message.reports_unnecessary)
            .count(),
        9
    );
    assert_eq!(
        MESSAGES
            .iter()
            .filter(|message| message.reports_deprecated)
            .count(),
        2
    );
    assert_eq!(
        MESSAGES
            .iter()
            .filter(|message| message.elided_in_compatibility_pyramid)
            .count(),
        4
    );
}
