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
    /// The node-read counters when the top phase last started charging, and
    /// their exclusive growth per phase (`access-stats` feature only).
    #[cfg(feature = "access-stats")]
    counts_since: [u64; 13],
    #[cfg(feature = "access-stats")]
    count_totals: [[u64; 13]; 3],
}

thread_local! {
    static CLOCKS: RefCell<Clocks> = const { RefCell::new(Clocks {
        totals: [0; 3],
        stack: Vec::new(),
        since: None,
        #[cfg(feature = "access-stats")]
        counts_since: [0; 13],
        #[cfg(feature = "access-stats")]
        count_totals: [[0; 13]; 3],
    }) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// The per-phase counter growth of every variant so far (`reset` folds a
/// finished variant in).
#[cfg(feature = "access-stats")]
static PHASE_COUNTS: std::sync::Mutex<[[u64; 13]; 3]> = std::sync::Mutex::new([[0; 13]; 3]);

#[cfg(feature = "access-stats")]
pub fn phase_counts() -> [[u64; 13]; 3] {
    let mut counts = *PHASE_COUNTS.lock().expect("phase counts");
    let pending = CLOCKS.with(|c| c.borrow().count_totals);
    for (total, phase) in counts.iter_mut().zip(pending) {
        for (total, value) in total.iter_mut().zip(phase) {
            *total += value;
        }
    }
    counts
}

fn settle(clocks: &mut Clocks, now: Instant) {
    if let (Some(since), Some(&phase)) = (clocks.since, clocks.stack.last()) {
        clocks.totals[phase] += u64::try_from((now - since).as_nanos()).unwrap_or(u64::MAX);
        #[cfg(feature = "access-stats")]
        {
            let counts = tsr_ast::access_stats::snapshot();
            for (total, (now, before)) in clocks.count_totals[phase]
                .iter_mut()
                .zip(counts.iter().zip(clocks.counts_since.iter()))
            {
                *total += now - before;
            }
        }
    }
    clocks.since = Some(now);
    #[cfg(feature = "access-stats")]
    {
        clocks.counts_since = tsr_ast::access_stats::snapshot();
    }
}

pub fn reset() {
    CLOCKS.with(|c| {
        #[cfg(feature = "access-stats")]
        {
            let finished = c.borrow().count_totals;
            let mut counts = PHASE_COUNTS.lock().expect("phase counts");
            for (total, phase) in counts.iter_mut().zip(finished) {
                for (total, value) in total.iter_mut().zip(phase) {
                    *total += value;
                }
            }
        }
        *c.borrow_mut() = Clocks {
            totals: [0; 3],
            stack: Vec::new(),
            since: None,
            #[cfg(feature = "access-stats")]
            counts_since: [0; 13],
            #[cfg(feature = "access-stats")]
            count_totals: [[0; 13]; 3],
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

/// Stop every clock (interval pause) at `now`, the timestamp the interval
/// clock stops at, so a pause leaks no clock-read gap into the phases.
pub fn suspend_at(now: Instant) {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        settle(&mut clocks, now);
        clocks.since = None;
    });
}

/// Resume charging the current top phase at `now`, the timestamp the
/// interval clock resumes at.
pub fn resume_at(now: Instant) {
    CLOCKS.with(|c| {
        let mut clocks = c.borrow_mut();
        if !clocks.stack.is_empty() {
            clocks.since = Some(now);
            #[cfg(feature = "access-stats")]
            {
                clocks.counts_since = tsr_ast::access_stats::snapshot();
            }
        }
    });
}

/// `suspend_at` now.
pub fn suspend() {
    suspend_at(Instant::now());
}

/// `resume_at` now.
pub fn resume() {
    resume_at(Instant::now());
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
