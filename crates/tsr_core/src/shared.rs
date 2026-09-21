//! Shared pointer fields of compiler options. Cloning retains the pointee;
//! replacing the handle is distinct from mutating it, as with Go's *T fields.
use std::sync::{Arc, RwLock, RwLockReadGuard};
#[derive(Debug)]
pub struct SharedValue<T>(Arc<RwLock<T>>);
impl<T> Clone for SharedValue<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> From<T> for SharedValue<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(RwLock::new(value)))
    }
}
impl<T: Default> Default for SharedValue<T> {
    fn default() -> Self {
        T::default().into()
    }
}
impl<T> SharedValue<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn set(&self, value: T) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
    }
    pub fn update<R>(&self, update: impl FnOnce(&mut T) -> R) -> R {
        update(
            &mut self
                .0
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}
impl<T: Copy> SharedValue<T> {
    pub fn get(&self) -> T {
        *self.read()
    }
}
impl<T: PartialEq> PartialEq for SharedValue<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.read() == *other.read()
    }
}
impl<T: Eq> Eq for SharedValue<T> {}

impl<A, T: FromIterator<A>> FromIterator<A> for SharedValue<T> {
    fn from_iter<I: IntoIterator<Item = A>>(items: I) -> Self {
        T::from_iter(items).into()
    }
}
