//! A property symbol's type is resolved separately from its containing object.
//!
//! Resolving an object's member table must not walk the types of all properties:
//! native `getPropertiesOfType` and `getTypeOfSymbol` are different lazy steps.

use crate::{Error, TypeCell};
use std::cell::{OnceCell, RefCell};
use std::rc::{Rc, Weak};

type Resolve = Box<dyn FnOnce() -> Result<Rc<TypeCell>, Error>>;

struct State {
    value: OnceCell<Weak<TypeCell>>,
    resolver: RefCell<Option<Resolve>>,
}

#[derive(Clone)]
pub(crate) struct TypeLink(Rc<State>);

impl std::fmt::Debug for TypeLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeLink")
            .field("resolved", &self.0.value.get().is_some())
            .finish_non_exhaustive()
    }
}

impl From<Weak<TypeCell>> for TypeLink {
    fn from(value: Weak<TypeCell>) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(value);
        Self(Rc::new(State {
            value: cell,
            resolver: RefCell::new(None),
        }))
    }
}

impl TypeLink {
    pub(crate) fn lazy(resolve: impl FnOnce() -> Result<Rc<TypeCell>, Error> + 'static) -> Self {
        Self(Rc::new(State {
            value: OnceCell::new(),
            resolver: RefCell::new(Some(Box::new(resolve))),
        }))
    }

    pub(crate) fn resolve(&self) -> Result<Rc<TypeCell>, Error> {
        if let Some(value) = self.0.value.get() {
            return value.upgrade().ok_or(Error::Released);
        }
        // Consume before invoking: a panic, failed resolution or reentry cannot
        // publish an empty property type or run the initializer twice.
        let resolve = self
            .0
            .resolver
            .borrow_mut()
            .take()
            .ok_or(Error::ResolutionFailed)?;
        let value = resolve()?;
        self.0
            .value
            .set(Rc::downgrade(&value))
            .map_err(|_| Error::ResolutionFailed)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flags, Graph};
    use std::cell::Cell;

    #[test]
    fn property_type_is_lazy_shared_and_does_not_retain_its_graph() {
        let graph = Rc::new(Graph::new());
        let calls = Rc::new(Cell::new(0));
        let weak = Rc::downgrade(&graph);
        let observed = calls.clone();
        let link = TypeLink::lazy(move || {
            observed.set(observed.get() + 1);
            Ok(weak
                .upgrade()
                .ok_or(Error::Released)?
                .primitive(flags::STRING, "string"))
        });
        let clone = link.clone();
        assert_eq!(calls.get(), 0);
        let first = link.resolve().unwrap();
        assert!(Rc::ptr_eq(&first, &clone.resolve().unwrap()));
        assert_eq!(calls.get(), 1);
        drop(first);
        drop(graph);
        assert_eq!(clone.resolve().unwrap_err(), Error::Released);
    }

    #[test]
    fn failed_resolution_is_terminal() {
        let link = TypeLink::lazy(|| Err(Error::Unsupported("test failure".into())));
        assert!(matches!(link.resolve(), Err(Error::Unsupported(_))));
        assert_eq!(link.resolve().unwrap_err(), Error::ResolutionFailed);
    }
}
