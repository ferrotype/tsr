//! Set membership with explicit retention of Go's live `Keys` map. Ordinary
//! lookups borrow the backing; only a retained key view clones the `Rc`.
//! These unsynchronized, aliasable containers are confined to one thread.
use std::{
    cell::{Ref, RefCell, RefMut},
    collections::HashSet,
    hash::Hash,
    rc::Rc,
};

/// Source type: tsc/internal/collections/set.go:Set
#[derive(Debug)]
pub struct Set<T> {
    keys: Option<Rc<RefCell<HashSet<T>>>>,
}
impl<T> Default for Set<T> {
    fn default() -> Self {
        Self { keys: None }
    }
}
/// An explicitly retained, live key map. Mutations remain visible after the
/// source set is dropped; cloning a `Set` itself instead copies membership.
#[derive(Debug)]
pub struct SetKeys<T>(Rc<RefCell<HashSet<T>>>);
impl<T> Clone for SetKeys<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> SetKeys<T> {
    pub fn read(&self) -> Ref<'_, HashSet<T>> {
        self.0.borrow()
    }
    pub fn write(&self) -> RefMut<'_, HashSet<T>> {
        self.0.borrow_mut()
    }
}
impl<T> Set<T> {
    /// port: tsc/internal/collections/set.go:NewSetWithSizeHint
    pub fn with_capacity(size: usize) -> Self {
        Self {
            keys: Some(Rc::new(RefCell::new(HashSet::with_capacity(size)))),
        }
    }
    /// port: tsc/internal/collections/set.go:Set.Len
    pub fn len(&self) -> usize {
        self.keys.as_ref().map_or(0, |keys| keys.borrow().len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Borrow the backing map without retaining its allocation.
    /// port: tsc/internal/collections/set.go:Set.Keys
    pub fn keys(&self) -> Option<Ref<'_, HashSet<T>>> {
        self.keys.as_ref().map(|m| m.borrow())
    }
    pub fn retain_keys(&self) -> Option<SetKeys<T>> {
        self.keys.as_ref().map(|m| SetKeys(m.clone()))
    }
    /// port: tsc/internal/collections/set.go:Set.Clear
    pub fn clear(&mut self) {
        if let Some(keys) = &self.keys {
            keys.borrow_mut().clear();
        }
    }
}
impl<T: Eq + Hash> Set<T> {
    /// port: tsc/internal/collections/set.go:Set.Has
    pub fn contains<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
    {
        self.keys.as_ref().is_some_and(|m| m.borrow().contains(key))
    }
    /// port: tsc/internal/collections/set.go:Set.Add
    pub fn insert(&mut self, key: T) {
        self.insert_if_absent(key);
    }
    /// port: tsc/internal/collections/set.go:Set.AddIfAbsent
    pub fn insert_if_absent(&mut self, key: T) -> bool {
        self.keys
            .get_or_insert_with(|| Rc::new(RefCell::new(HashSet::new())))
            .borrow_mut()
            .insert(key)
    }
    /// port: tsc/internal/collections/set.go:Set.Delete
    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q)
    where
        T: std::borrow::Borrow<Q>,
    {
        if let Some(m) = &self.keys {
            m.borrow_mut().remove(key);
        }
    }
    /// port: tsc/internal/collections/set.go:Set.Equals
    pub fn equals(left: Option<&Self>, right: Option<&Self>) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(a), Some(b)) => a.len() == b.len() && Self::is_subset_of(Some(a), Some(b)),
            _ => false,
        }
    }
    /// port: tsc/internal/collections/set.go:Set.IsSubsetOf
    pub fn is_subset_of(left: Option<&Self>, right: Option<&Self>) -> bool {
        left.and_then(Self::keys).is_none_or(|keys| {
            keys.iter()
                .all(|key| right.is_some_and(|s| s.contains(key)))
        })
    }
    /// port: tsc/internal/collections/set.go:Set.Intersects
    pub fn intersects(left: Option<&Self>, right: Option<&Self>) -> bool {
        left.and_then(Self::keys).is_some_and(|keys| {
            keys.iter()
                .any(|key| right.is_some_and(|s| s.contains(key)))
        })
    }
}
impl<T: Eq + Hash + Clone> Clone for Set<T> {
    /// port: tsc/internal/collections/set.go:Set.Clone
    fn clone(&self) -> Self {
        Self {
            keys: self
                .keys
                .as_ref()
                .map(|m| Rc::new(RefCell::new(m.borrow().clone()))),
        }
    }
}
impl<T: Eq + Hash + Clone> Set<T> {
    /// Nil receivers are allowed only when both sets are empty. The nonempty
    /// target plus nil source path has Go's nil-receiver panic, not a no-op.
    /// port: tsc/internal/collections/set.go:Set.Union
    pub fn union_into(target: Option<&mut Self>, other: Option<&Self>) {
        if target.as_ref().is_none_or(|s| s.is_empty()) && other.is_none_or(Self::is_empty) {
            return;
        }
        let target = target.expect("cannot modify nil Set");
        let other = other.expect("nil pointer dereference");
        if target.keys.is_none() {
            *target = other.clone();
        } else if let Some(keys) = other.keys() {
            for key in &*keys {
                target.insert(key.clone());
            }
        }
    }
    /// port: tsc/internal/collections/set.go:Set.UnionedWith
    pub fn unioned_with(left: Option<&Self>, other: Option<&Self>) -> Option<Self> {
        if left.is_none() && other.is_none() {
            return None;
        }
        let mut result = left.cloned().unwrap_or_default();
        if let Some(other) = other {
            if result.keys.is_none() {
                result = Self::with_capacity(other.len());
            }
            if let Some(keys) = other.keys() {
                for key in &*keys {
                    result.insert(key.clone());
                }
            }
        }
        Some(result)
    }
}
impl<T: Eq + Hash> FromIterator<T> for Set<T> {
    /// port: tsc/internal/collections/set.go:NewSetFromItems
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        let mut set = Self::default();
        for item in items {
            set.insert(item);
        }
        set
    }
}
