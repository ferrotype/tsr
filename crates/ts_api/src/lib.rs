//! Snapshot-local API roots and server-owned response commitment (ADR 0012).
//! Transport framing and the remaining wire endpoints belong to the later API
//! server. Numeric type handles stay checker Type.Id values; generation tokens
//! never appear in serialized response bytes.
#![forbid(unsafe_code)]

mod printing;
pub use printing::{print_node, PrintError, PrintNodeOptions};

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use ts_arena::Generation;
use ts_checker::{CheckerOwner, Operation, RetainedType, TypeRef};
use ts_project::CheckerSlot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Checker(ts_checker::Error),
    UnknownType,
    WrongSnapshot,
    QueueFull,
    Capacity,
    Panicked(String),
}
impl From<ts_checker::Error> for Error {
    fn from(value: ts_checker::Error) -> Self {
        Self::Checker(value)
    }
}
impl From<ts_arena::Error> for Error {
    fn from(value: ts_arena::Error) -> Self {
        Self::Checker(value.into())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checker(error) => error.fmt(f),
            Self::UnknownType => f.write_str("type handle not found in snapshot registry"),
            Self::WrongSnapshot => f.write_str("response belongs to another snapshot"),
            Self::QueueFull => f.write_str("response queue has no capacity"),
            Self::Capacity => f.write_str("registry capacity exhausted"),
            Self::Panicked(message) => write!(
                f,
                "checker request panicked and its generation retired: {message}"
            ),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
struct Registry {
    types: HashMap<u32, RetainedType>,
    latest: Option<Arc<PublishedResult>>,
}

struct SnapshotData {
    project: ts_project::Snapshot,
    checker: Arc<CheckerOwner>,
    registry: Mutex<Registry>,
}

/// Clones retain the same snapshot registry. `new` creates a distinct registry,
/// even when the project snapshot shares the same checker pool.
#[derive(Clone)]
pub struct Snapshot(Arc<SnapshotData>);
impl Snapshot {
    pub fn new(project: ts_project::Snapshot) -> Result<Self, Error> {
        let checker = project.project().pool().acquire(CheckerSlot::Api)?;
        let owner = checker.owner().clone();
        let snapshot = Self(Arc::new(SnapshotData {
            project,
            checker: owner,
            registry: Mutex::new(Registry::default()),
        }));
        // The birth boundary follows private construction. Retirement that
        // already won cannot publish a fresh-looking snapshot over a dead pool.
        snapshot
            .generation()
            .enter()?
            .validate_checker(snapshot.checker().identity())?;
        Ok(snapshot)
    }

    pub fn generation(&self) -> &Generation {
        self.0.project.project().pool().generation()
    }

    pub fn checker(&self) -> &Arc<CheckerOwner> {
        &self.0.checker
    }

    /// Execute computation/serialization or an external callback outside all
    /// publication locks. An operation must be dropped before callback reentry;
    /// reacquiring it validates retirement and exact identity again.
    /// The returned value is private request work, not committed success.
    pub fn request<R>(&self, work: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
        match catch_unwind(AssertUnwindSafe(move || {
            self.generation().validate()?;
            let result = work();
            self.generation().validate()?;
            result
        })) {
            Ok(result) => result,
            Err(payload) => {
                // Covers serialization/host callbacks that panic without an
                // active operation, as well as CheckerLease's earlier retire.
                self.generation().retire();
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload")
                    .to_owned();
                Err(Error::Panicked(message))
            }
        }
    }

    /// Stage owning handles and already serialized bytes. No registry mutation
    /// happens until commit. A sibling slot's equal numeric type id is rejected.
    pub fn prepare(
        &self,
        operation: &Operation<'_>,
        types: &[TypeRef],
        bytes: Vec<u8>,
    ) -> Result<PreparedResponse, Error> {
        if !Arc::ptr_eq(operation.owner(), &self.0.checker) {
            return Err(ts_arena::Error::WrongOwner.into());
        }
        let types = types
            .iter()
            .map(|ty| operation.retain_type(*ty))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreparedResponse {
            snapshot: self.0.clone(),
            result: Arc::new(PublishedResult {
                bytes: bytes.into(),
                types,
            }),
        })
    }

    /// The only shared-result/registry/success publication path. Queue capacity
    /// is preallocated; full queues and failed registry reservation mutate none
    /// of the three surfaces. No callback, permit acquisition or I/O under gate.
    pub fn commit(&self, prepared: PreparedResponse, queue: &ResponseQueue) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.0, &prepared.snapshot) {
            return Err(Error::WrongSnapshot);
        }
        // Hold an extra root outside the locks: replacing `latest` must not
        // dispose caller-owned checker hosts inside the gate.
        let displaced;
        {
            let gate = self.generation().enter()?;
            gate.validate_checker(self.checker().identity())?;
            let mut registry = self
                .0
                .registry
                .lock()
                .map_err(|_| ts_arena::Error::Retired)?;
            let mut output = queue.state.lock().map_err(|_| ts_arena::Error::Retired)?;
            if output.len() == queue.capacity {
                return Err(Error::QueueFull);
            }
            registry
                .types
                .try_reserve(prepared.result.types.len())
                .map_err(|_| Error::Capacity)?;
            for ty in &prepared.result.types {
                if let Some(existing) = registry.types.get(&ty.id()) {
                    assert!(
                        Arc::ptr_eq(existing.owner(), ty.owner()),
                        "duplicate type identity"
                    );
                }
            }
            for ty in &prepared.result.types {
                registry.types.entry(ty.id()).or_insert_with(|| ty.clone());
            }
            displaced = registry.latest.replace(prepared.result.clone());
            #[cfg(test)]
            tests::before_response_queue_publication();
            output.push_back(prepared.result);
        }
        drop(displaced);
        Ok(())
    }

    fn retained_type(&self, id: u32) -> Result<RetainedType, Error> {
        let gate = self.generation().enter()?;
        gate.validate_checker(self.checker().identity())?;
        self.0
            .registry
            .lock()
            .map_err(|_| ts_arena::Error::Retired)?
            .types
            .get(&id)
            .cloned()
            .ok_or(Error::UnknownType)
    }

    /// Lock order: gate → registry; release both → exact checker permit → gate.
    /// Computation runs outside the gate. It must use `prepare` and `commit` to
    /// publish its result; returning from this helper alone publishes nothing.
    pub fn with_type<R>(
        &self,
        id: u32,
        work: impl FnOnce(&mut Operation<'_>, TypeRef) -> Result<R, Error>,
    ) -> Result<R, Error> {
        self.request(|| {
            let retained = self.retained_type(id)?;
            let mut operation = retained.owner().operation()?;
            self.generation()
                .enter()?
                .validate_checker(operation.owner().identity())?;
            let ty = operation.import_type(&retained)?;
            work(&mut operation, ty)
        })
    }

    pub fn latest(&self) -> Result<Option<Arc<PublishedResult>>, Error> {
        let gate = self.generation().enter()?;
        gate.validate_checker(self.checker().identity())?;
        Ok(self
            .0
            .registry
            .lock()
            .map_err(|_| ts_arena::Error::Retired)?
            .latest
            .clone())
    }
}

pub struct PreparedResponse {
    snapshot: Arc<SnapshotData>,
    result: Arc<PublishedResult>,
}

/// An owned, already committed result. Delivery after retirement is allowed:
/// commitment was ordered before retirement. Its type roots still refuse new
/// checker operations after retirement and are freed on final drop.
pub struct PublishedResult {
    bytes: Arc<[u8]>,
    types: Vec<RetainedType>,
}
impl PublishedResult {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn type_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.types.iter().map(RetainedType::id)
    }
}

/// Server-owned bounded queue. Transport drains it after commitment, outside
/// the generation gate. Backpressure is an uncommitted error, never blocking I/O.
pub struct ResponseQueue {
    state: Mutex<VecDeque<Arc<PublishedResult>>>,
    capacity: usize,
}
impl ResponseQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }
    pub fn pop(&self) -> Option<Arc<PublishedResult>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
}

#[cfg(test)]
mod tests;
