//! Nested exclusive phase clocks for the checkerbench phase-timer executable
//! (`data/s08/checker-workload.json`, `phase_timer`). One thread runs the
//! serial workload; the clocks are thread-local. The normal timing executable
//! does not compile this module (`s08-phase-timer` feature only).
#![allow(dead_code)]
use std::cell::{Cell, RefCell};
use std::time::Instant;

pub const INIT: usize = 0;
pub const CHECK: usize = 1;
pub const DISPLAY: usize = 2;
pub const NAMES: [&str; 3] = ["init", "check", "display"];

struct Clocks {
    /// Exclusive nanoseconds per phase.
    totals: [u64; 3],
    /// Phase stack; the top phase owns the time since `since`.
    stack: Vec<usize>,
    since: Option<Instant>,
}

thread_local! {
    static CLOCKS: RefCell<Clocks> = const { RefCell::new(Clocks { totals: [0; 3], stack: Vec::new(), since: None }) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

fn settle(clocks: &mut Clocks, now: Instant) {
    if let (Some(since), Some(&phase)) = (clocks.since, clocks.stack.last()) {
        clocks.totals[phase] += u64::try_from((now - since).as_nanos()).unwrap_or(u64::MAX);
    }
    clocks.since = Some(now);
}

pub fn reset() {
    CLOCKS.with(|c| {
        *c.borrow_mut() = Clocks {
            totals: [0; 3],
            stack: Vec::new(),
            since: None,
        }
    });
}

/// Begin charging `phase`; the enclosing phase stops accumulating until `pop`.
pub fn push(phase: usize) {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        let now = Instant::now();
        settle(&mut clocks, now);
        clocks.stack.push(phase);
    });
}

pub fn pop() {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        let now = Instant::now();
        settle(&mut clocks, now);
        clocks.stack.pop();
        if clocks.stack.is_empty() {
            clocks.since = None;
        }
    });
}

/// Stop every clock (interval pause); `push` resumes charging.
pub fn suspend() {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        let now = Instant::now();
        settle(&mut clocks, now);
        clocks.since = None;
    });
}

/// Resume charging the current top phase after `suspend`.
pub fn resume() {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        if !clocks.stack.is_empty() {
            clocks.since = Some(Instant::now());
        }
    });
}

pub fn totals() -> [u64; 3] {
    CLOCKS.with(|c| c.borrow().totals)
}

pub fn depth() -> usize {
    CLOCKS.with(|c| c.borrow().stack.len())
}

/// A display call: nested inside the check clock, subtracted from it.
pub struct Display(());
impl Display {
    pub fn begin() -> Self {
        // Display calls nest (a symbol display can print types); charge only
        // the outermost one so nested display work is not counted twice.
        if DEPTH.get() == 0 {
            push(DISPLAY);
        }
        DEPTH.set(DEPTH.get() + 1);
        Display(())
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
        if DEPTH.get() == 0 {
            pop();
        }
    }
}
