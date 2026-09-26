//! Diagnostic-only creation and lazy-identity observations (ADR 0010).
//!
//! Tokens identify objects within one trace, never semantic identities. Reading
//! or labeling an object here must not assign its runtime id or query a checker.
//! The driver uses one fresh, single-threaded process per source witness.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

#[derive(Default)]
struct Trace {
    events: Vec<Value>,
    symbols: HashMap<usize, u64>,
    next_token: u64,
}

thread_local! {
    static ACTIVE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

/// A diagnostic type-store identity, independent of type ids and storage moves.
#[derive(Debug)]
pub struct TypeOwner(u64);

impl Default for TypeOwner {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, u64::MAX, "diagnostic owner identity exhausted");
        Self(id)
    }
}

impl TypeOwner {
    pub fn token(&self) -> u64 {
        self.0
    }
}

/// Starts before parsing/binding, so file-symbol births are observed too.
pub fn begin() {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        assert!(active.is_none(), "creation trace already active");
        *active = Some(Trace::default());
    });
}

/// Finishes without performing any compiler query. Unobserved births remain
/// explicit events; the validator, rather than a guessed origin, rejects them.
pub fn finish() -> Vec<Value> {
    ACTIVE.with(|active| {
        active
            .borrow_mut()
            .take()
            .expect("no creation trace")
            .events
    })
}

pub fn active() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

pub fn record(event: Value) {
    ACTIVE.with(|active| {
        if let Some(trace) = active.borrow_mut().as_mut() {
            trace.events.push(event);
        }
    });
}

#[track_caller]
pub fn symbol_birth(symbol: &(impl crate::SymbolAccess + ?Sized)) {
    let origin = Location::caller();
    ACTIVE.with(|active| {
        if let Some(trace) = active.borrow_mut().as_mut() {
            let key = symbol.diagnostic_runtime_key();
            // An arena row is stable, but an allocator may reuse a dropped row's
            // address. A birth always replaces that address's old trace token.
            trace.next_token += 1;
            let token = trace.next_token;
            trace.symbols.insert(key, token);
            trace
                .events
                .push(json!({"event":"birth", "kind":"symbol", "token":token,
                "origin":{"file":origin.file(), "line":origin.line()},
                "flags":symbol.flags(), "name":symbol.name_bytes(),
                "semantic_id":crate::existing_runtime_symbol_id(symbol)}));
        }
    });
}

/// Registers no identity: an unknown object remains explicitly unobserved.
pub fn symbol_token(symbol: &(impl crate::SymbolAccess + ?Sized)) -> Option<u64> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|trace| trace.symbols.get(&symbol.diagnostic_runtime_key()).copied())
    })
}

pub(crate) fn assigned(key: usize, id: u64) {
    ACTIVE.with(|active| {
        if let Some(trace) = active.borrow_mut().as_mut() {
            trace
                .events
                .push(json!({"event":"id_assignment", "kind":"symbol",
                "token":trace.symbols.get(&key), "semantic_id":id}));
        }
    });
}

#[track_caller]
pub fn type_birth(owner: &TypeOwner, id: u32, flags: u32, object_flags: u32) {
    if active() {
        let origin = Location::caller();
        record(
            json!({"event":"birth", "kind":"type", "owner":owner.token(),
            "token":id, "semantic_id":id, "flags":flags, "object_flags":object_flags,
            "origin":{"file":origin.file(), "line":origin.line()}}),
        );
    }
}
