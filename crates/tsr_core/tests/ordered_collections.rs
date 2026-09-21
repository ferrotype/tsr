use std::sync::Arc;
use tsr_core::collections::{MapChange, OrderedMap, OrderedSet};

#[test]
fn live_iteration_observes_overwrites_appends_and_index_shifts() {
    let mut map: OrderedMap<_, _> = [("a", 1), ("b", 2), ("c", 3)].into_iter().collect();
    let mut seen = Vec::new();
    map.visit_entries_mut(|map, key, value| {
        seen.push((key, value));
        if key == "a" {
            map.insert("b", 20);
            map.insert("d", 4);
        }
        true
    });
    assert_eq!(seen, [("a", 1), ("b", 20), ("c", 3), ("d", 4)]);
    seen.clear();
    map.visit_entries_mut(|map, key, value| {
        seen.push((key, value));
        if key == "a" {
            map.remove("a");
        }
        true
    });
    // Go increments its key-slice index after the callback; b has shifted to
    // the index already visited, so it is deliberately skipped.
    assert_eq!(seen, [("a", 1), ("c", 3), ("d", 4)]);
}

#[test]
fn diff_orders_additions_before_old_map_changes_and_uses_the_comparator() {
    let old: OrderedMap<_, _> = [("b", "one"), ("a", "two"), ("gone", "old")]
        .into_iter()
        .collect();
    let new: OrderedMap<_, _> = [
        ("added2", "2"),
        ("a", "changed"),
        ("added1", "1"),
        ("b", "six"),
    ]
    .into_iter()
    .collect();
    let mut calls = Vec::new();
    old.diff_by(&new, |a, b| a.len() == b.len(), |event| calls.push(event));
    assert_eq!(
        calls,
        [
            MapChange::Added(&"added2", &"2"),
            MapChange::Added(&"added1", &"1"),
            MapChange::Modified(&"a", &"two", &"changed"),
            MapChange::Removed(&"gone", &"old"),
        ]
    );
}

#[test]
fn clone_separates_entries_and_preserves_pointer_value_identity() {
    let value = Arc::new(17);
    let mut map: OrderedMap<_, _> = [("a", value.clone()), ("b", value.clone())]
        .into_iter()
        .collect();
    let clone = map.clone();
    map.remove("a");
    map.insert("a", Arc::new(9));
    assert_eq!(map.keys().copied().collect::<Vec<_>>(), ["b", "a"]);
    assert_eq!(clone.keys().copied().collect::<Vec<_>>(), ["a", "b"]);
    assert!(Arc::ptr_eq(clone.get("a").unwrap(), &value));
    assert!(!Arc::ptr_eq(map.get("a").unwrap(), &value));
}

#[test]
fn repeated_delete_reinsert_and_clear_leave_no_stale_key_slots() {
    let mut map = OrderedMap::<i32, i32>::default();
    // Move the deque's head repeatedly, then exercise middle removal, entry
    // lookup and reuse after clear across the resulting wrapped allocation.
    for key in 0..8 {
        map.insert(key, key);
    }
    for key in 0..64 {
        assert_eq!(map.remove(&key), Some(key));
        map.insert(key + 8, key + 8);
    }
    assert_eq!(
        map.keys().copied().collect::<Vec<_>>(),
        (64..72).collect::<Vec<_>>()
    );
    assert_eq!(map.remove(&67), Some(67));
    assert_eq!(map.entry_at(3), Some((&68, &68)));
    assert_eq!(map.entry_at(-1), None);
    assert_eq!(map.entry_at(7), None);
    map.clear();
    assert!(map.is_empty());
    map.insert(1, 2);
    assert_eq!(map.entries().collect::<Vec<_>>(), [(&1, &2)]);
}

#[test]
fn ordered_set_live_iteration_and_early_stop_share_map_semantics() {
    let mut set = OrderedSet::<&str>::default();
    set.insert("a");
    set.insert("b");
    let mut seen = Vec::new();
    set.visit_values_mut(|set, key| {
        seen.push(key);
        set.insert("c");
        true
    });
    assert_eq!(seen, ["a", "b", "c"]);
    seen.clear();
    set.visit_values_mut(|_, key| {
        seen.push(key);
        false
    });
    assert_eq!(seen, ["a"]);
}
