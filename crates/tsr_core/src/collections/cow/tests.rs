use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct Counted {
    clones: Arc<AtomicUsize>,
    value: usize,
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::Relaxed);
        Self {
            clones: self.clones.clone(),
            value: self.value,
        }
    }
}

#[test]
fn reads_share_and_only_the_first_child_write_clones_entries() {
    let clones = Arc::new(AtomicUsize::new(0));
    let value = |value| Counted {
        clones: clones.clone(),
        value,
    };
    let mut map = CopyOnWriteMap::<_, _>::default();
    map.insert("a", value(1));
    map.insert("b", value(2));
    let parent_backing = Arc::as_ptr(map.backing.as_ref().unwrap());
    {
        let mut scope = map.enter_scope();
        assert_eq!(scope.get("a").unwrap().value, 1);
        assert_eq!(Arc::as_ptr(scope.backing.as_ref().unwrap()), parent_backing);
        assert_eq!(clones.load(Ordering::Relaxed), 0);
        scope.insert("a", value(9));
        assert_eq!(clones.load(Ordering::Relaxed), 2);
        scope.insert("c", value(3));
        assert_eq!(clones.load(Ordering::Relaxed), 2);
        let child_backing = Arc::as_ptr(scope.backing.as_ref().unwrap());
        {
            let nested = scope.enter_scope();
            assert_eq!(nested.get("a").unwrap().value, 9);
            assert_eq!(Arc::as_ptr(nested.backing.as_ref().unwrap()), child_backing);
        }
        scope.insert("d", value(4));
        assert_eq!(
            clones.load(Ordering::Relaxed),
            2,
            "read-only child released its snapshot"
        );
    }
    assert_eq!(Arc::as_ptr(map.backing.as_ref().unwrap()), parent_backing);
    assert_eq!(map.get("a").unwrap().value, 1);
    assert!(!map.contains_key("c"));
    map.insert("e", value(5));
    assert_eq!(clones.load(Ordering::Relaxed), 2, "parent is unique again");
}

#[test]
fn empty_scopes_stay_unallocated_and_discard_child_storage() {
    let mut map = CopyOnWriteMap::<String, String>::default();
    assert!(map.backing.is_none());
    {
        let mut scope = map.enter_scope();
        assert!(scope.backing.is_none());
        assert_eq!(scope.get("a"), None);
        scope.insert("a".into(), "child".into());
        assert!(scope.backing.is_some());
    }
    assert!(map.backing.is_none());
    map.insert("a".into(), "parent".into());
    assert_eq!(map.get("a").map(String::as_str), Some("parent"));
}

#[test]
fn nested_scopes_restore_on_unwind() {
    let mut map = CopyOnWriteMap::<_, _>::default();
    map.insert("a", 1);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut child = map.enter_scope();
        child.insert("a", 2);
        {
            let mut nested = child.enter_scope();
            nested.insert("a", 3);
            assert_eq!(nested.get("a"), Some(&3));
        }
        assert_eq!(child.get("a"), Some(&2));
        panic!("unwind the outer scope");
    }));
    assert!(result.is_err());
    assert_eq!(map.get("a"), Some(&1));
}

#[test]
fn owned_snapshots_isolate_writes_but_keep_value_identity() {
    let value = Arc::new(AtomicUsize::new(1));
    let mut parent = CopyOnWriteMap::<_, _>::default();
    parent.insert("a", value.clone());
    let mut child = parent.clone();
    child.insert("b", Arc::new(AtomicUsize::new(2)));
    child.get("a").unwrap().store(3, Ordering::Relaxed);
    assert!(!parent.contains_key("b"));
    assert_eq!(parent.get("a").unwrap().load(Ordering::Relaxed), 3);
    assert!(Arc::ptr_eq(
        parent.get("a").unwrap(),
        child.get("a").unwrap()
    ));
}

#[test]
fn sets_restore_each_level_and_do_not_require_thread_confinement() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<CopyOnWriteMap<String, String>>();
    send_sync::<CopyOnWriteSet<String>>();
    let mut set = CopyOnWriteSet::<String>::default();
    set.insert("a".into());
    {
        let mut child = set.enter_scope();
        child.insert("b".into());
        {
            let mut nested = child.enter_scope();
            nested.insert("c".into());
            assert!(nested.contains("a") && nested.contains("b") && nested.contains("c"));
        }
        assert!(child.contains("a") && child.contains("b") && !child.contains("c"));
    }
    assert!(set.contains("a") && !set.contains("b") && !set.contains("c"));
}
