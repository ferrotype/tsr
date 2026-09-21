//! The core/collections leaf group.
use crate::api::Outcome;
use serde_json::Value;

mod cow;
mod ordered;

/// Subjects whose production Rust home does not exist yet.
///
/// Preparation records the gap; it never emulates the algorithm here to make a
/// comparison run. Each row names the pinned Go authority, the signature the
/// port is expected to carry and the file that does not exist yet.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("Set",
     "tsc/internal/collections/set.go:Set",
     "pub struct Set<T> with has/add/add_if_absent -> bool/delete/len/keys/clear/clone/union/unioned_with/equals/is_subset_of/intersects, plus from_items and with_size_hint constructors, over a hash set with Go's nil-receiver and empty-versus-absent distinctions",
     "crates/tsr_core/src/collections/set.rs (absent)"),
    ("MultiMap",
     "tsc/internal/collections/multimap.go:MultiMap",
     "pub struct MultiMap<K, V> with has/get -> &[V]/add/remove/remove_all/len/keys/values/clear and a free group_by(items, key_of), where remove drops a key whose last value is removed",
     "crates/tsr_core/src/collections/multimap.rs (absent)"),
    ("SyncMap",
     "tsc/internal/collections/syncmap.go:SyncMap",
     "pub struct SyncMap<K, V> with load/store/load_or_store/delete/clear/range/size/to_map/keys/clone under interior mutability, reproducing Go's present-with-nil contract: load of a stored nil yields (zero, true) while an absent key yields (zero, false)",
     "crates/tsr_core/src/collections/syncmap.rs (absent)"),
    ("SyncSet",
     "tsc/internal/collections/syncset.go:SyncSet",
     "pub struct SyncSet<T> wrapping SyncMap<T, ()> with has/add/add_if_absent -> bool/delete/range/size/is_empty/to_slice/keys, where add_if_absent is the inverse of load_or_store's loaded flag",
     "crates/tsr_core/src/collections/syncset.rs (absent)"),
];

// Reviewed entry points of the absent APIs above; the request must match
// both subject and identity before it can report a gap.
const MISSING_OPERATIONS: &[(&str, &str)] = &[
    ("MultiMap", "tsc/internal/collections/multimap.go:GroupBy"),
    (
        "MultiMap",
        "tsc/internal/collections/multimap.go:MultiMap.Add",
    ),
    (
        "MultiMap",
        "tsc/internal/collections/multimap.go:MultiMap.Remove",
    ),
    ("Set", "tsc/internal/collections/set.go:NewSetFromItems"),
    ("Set", "tsc/internal/collections/set.go:Set.Clone"),
    ("Set", "tsc/internal/collections/set.go:Set.Keys"),
    ("Set", "tsc/internal/collections/set.go:Set.Union"),
    (
        "SyncMap",
        "tsc/internal/collections/syncmap.go:SyncMap.Clone",
    ),
    (
        "SyncMap",
        "tsc/internal/collections/syncmap.go:SyncMap.Load",
    ),
    (
        "SyncMap",
        "tsc/internal/collections/syncmap.go:SyncMap.Range",
    ),
    (
        "SyncSet",
        "tsc/internal/collections/syncset.go:SyncSet.AddIfAbsent",
    ),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let subject = crate::api::subject(request);
    if subject == "OrderedMap" {
        return Some(ordered::map(request));
    }
    if subject == "OrderedSet" {
        return Some(ordered::set(request));
    }
    if matches!(subject, "CopyOnWriteMap" | "CopyOnWriteSet") {
        return Some(cow::observe(request));
    }
    MISSING.iter().find(|(name, _, _, _)| *name == subject).map(
        |(_, authority, signature, home)| {
            crate::api::missing_for_subject(request, MISSING_OPERATIONS, authority, signature, home)
        },
    )
}
