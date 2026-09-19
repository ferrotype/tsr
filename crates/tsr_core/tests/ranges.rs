use ts_core::TextRange;

#[test]
fn range_positions_narrow_before_length_subtraction() {
    let range = TextRange::new(0x1_0000_0007, 0x1_0000_000a);
    assert_eq!((range.pos(), range.end(), range.len()), (7, 10, 3));
    let wrapping = TextRange::new(i64::from(i32::MIN), i64::from(i32::MAX));
    assert_eq!(wrapping.len(), -1);
    assert!(!wrapping.is_empty());
    assert!(TextRange::new(-1, -1).is_empty());
}
