//! Phase 1 mutation witnesses: the runtime mutant switch.
//!
//! The repository never calls these functions from production crates. The
//! mutation tooling splices `if ::phase1_mutants::hit(ID) { ... }` guards into
//! a scratch copy of the workspace, builds it once, and selects one mutant at a
//! time. With no active mutant every guard is false, so the spliced build
//! observes exactly what the unspliced build does.
//!
//! Reach is recorded per thread and per row. Production stages (parse, bind,
//! program load) record every site; observation stages (graph dumps, frame
//! encoding) record and activate only parser sites, the `tsr_parser` sites
//! whose every operation is in `internal/parser`, because lazy JSDoc parsing
//! triggered while encoding is production parser work on both sides. A
//! production-only site executed while observing never activates, but it is
//! recorded apart ([`take_row_observe_hits`]), so a home that runs only in
//! observation code is never mistaken for one no oracle executes.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::LocalKey;

/// A mutant's numeric id within one schemata build; 0 selects no mutant.
pub type MutantId = u32;

static ACTIVE: AtomicU32 = AtomicU32::new(0);
static TRACING: AtomicBool = AtomicBool::new(true);

/// Whether the current thread runs production code or observes its results.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stage {
    #[default]
    Production,
    Observe,
}

thread_local! {
    static STAGE: Cell<Stage> = const { Cell::new(Stage::Production) };
    static HITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static OBSERVE_HITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// A per-thread bitset of mutant ids.
type HitSet = LocalKey<RefCell<Vec<u64>>>;

/// Selects the mutant every guard compares against; 0 disables all mutants.
pub fn set_active(id: MutantId) {
    ACTIVE.store(id, Ordering::Relaxed);
}

#[must_use]
pub fn active() -> MutantId {
    ACTIVE.load(Ordering::Relaxed)
}

/// Sets the current thread's stage.
pub fn set_stage(stage: Stage) {
    STAGE.with(|current| current.set(stage));
}

#[must_use]
pub fn stage() -> Stage {
    STAGE.with(Cell::get)
}

/// Turns reach recording on or off for every thread. Kill runs switch it off;
/// the trace pass keeps it on.
pub fn set_tracing(on: bool) {
    TRACING.store(on, Ordering::Relaxed);
}

/// Starts a new row on the current thread by clearing its recorded reach.
pub fn begin_row() {
    for set in [&HITS, &OBSERVE_HITS] {
        set.with(|hits| hits.borrow_mut().iter_mut().for_each(|word| *word = 0));
    }
}

/// The sorted, unique mutant ids reached on this thread since `begin_row` by
/// [`hit`] while producing and by [`hit_parser`] in either stage; taking them
/// clears them.
#[must_use]
pub fn take_row_hits() -> Vec<MutantId> {
    take(&HITS)
}

/// The sorted, unique ids of production-only sites ([`hit`]) executed on this
/// thread while observing since `begin_row`; taking them clears them. Such a
/// site never activates while observing.
#[must_use]
pub fn take_row_observe_hits() -> Vec<MutantId> {
    take(&OBSERVE_HITS)
}

fn take(set: &'static HitSet) -> Vec<MutantId> {
    set.with(|hits| {
        let mut hits = hits.borrow_mut();
        let mut ids = Vec::new();
        for (index, word) in hits.iter_mut().enumerate() {
            let mut bits = std::mem::take(word);
            while bits != 0 {
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                let id = u32::try_from(index * 64).expect("mutant id fits u32") + bit;
                ids.push(id);
            }
        }
        ids
    })
}

#[inline]
fn record(set: &'static HitSet, id: MutantId) {
    if TRACING.load(Ordering::Relaxed) {
        set.with(|hits| {
            let mut hits = hits.borrow_mut();
            let word = (id / 64) as usize;
            if hits.len() <= word {
                hits.resize(word + 1, 0);
            }
            hits[word] |= 1 << (id % 64);
        });
    }
}

/// Records a production-only site executed while observing: observation
/// reach, which never selects the mutant.
#[inline]
fn record_observe(id: MutantId) {
    record(&OBSERVE_HITS, id);
}

/// A guard at a production-only site. While observing it is inactive and its
/// execution is recorded apart as observation reach.
#[inline]
#[must_use]
pub fn hit(id: MutantId) -> bool {
    if stage() == Stage::Observe {
        record_observe(id);
        return false;
    }
    record(&HITS, id);
    active() == id
}

/// A guard at a parser site, a `tsr_parser` site whose every operation is in
/// `internal/parser`: recorded and selectable in both stages.
#[inline]
#[must_use]
pub fn hit_parser(id: MutantId) -> bool {
    record(&HITS, id);
    active() == id
}

#[cfg(test)]
mod tests {
    use super::{
        active, begin_row, hit, hit_parser, set_active, set_stage, set_tracing, take_row_hits,
        take_row_observe_hits, Stage,
    };

    // Globals are shared across test threads, so one test exercises them in order.
    #[test]
    fn guards_record_reach_by_stage_and_select_only_the_active_mutant() {
        set_tracing(true);
        set_active(0);
        begin_row();
        assert!(!hit(3));
        assert!(!hit_parser(130));
        set_stage(Stage::Observe);
        assert!(
            !hit(7),
            "production-only sites are inactive while observing"
        );
        assert!(!hit_parser(65));
        assert!(!hit(200));
        set_stage(Stage::Production);
        assert_eq!(take_row_hits(), [3, 65, 130]);
        assert_eq!(
            take_row_hits(),
            Vec::<u32>::new(),
            "taking the hits clears them"
        );
        assert_eq!(
            take_row_observe_hits(),
            [7, 200],
            "production-only sites executed while observing are observation reach"
        );
        assert!(take_row_observe_hits().is_empty());
        set_stage(Stage::Observe);
        assert!(!hit(8));
        set_stage(Stage::Production);
        begin_row();
        assert!(
            take_row_observe_hits().is_empty(),
            "a new row clears observation reach"
        );

        set_active(3);
        assert_eq!(active(), 3);
        assert!(hit(3));
        assert!(!hit(4));
        set_stage(Stage::Observe);
        assert!(
            !hit(3),
            "an active production-only mutant stays off while observing"
        );
        set_active(65);
        assert!(
            hit_parser(65),
            "parser sites stay selectable while observing"
        );
        set_stage(Stage::Production);

        set_tracing(false);
        begin_row();
        assert!(!hit(9));
        set_stage(Stage::Observe);
        assert!(!hit(10));
        set_stage(Stage::Production);
        assert!(take_row_hits().is_empty(), "kill runs record nothing");
        assert!(take_row_observe_hits().is_empty());
        set_tracing(true);
        set_active(0);
    }
}
