//! Phase 1 mutation witnesses: the runtime mutant switch.
//!
//! The repository never calls these functions from production crates. The
//! mutation tooling splices `if ::phase1_mutants::hit(ID) { ... }` guards into
//! a scratch copy of the workspace, builds it once, and selects one mutant at a
//! time. With no active mutant every guard is false, so the spliced build
//! observes exactly what the unspliced build does.
//!
//! Reach is recorded per row. Production stages (parse, bind, program load, a
//! table column) record every site; observation stages (graph dumps, frame
//! encoding, a table row's setup) record and activate only parser sites, the
//! `tsr_parser` sites whose every operation is in `internal/parser`, because
//! lazy JSDoc parsing triggered while encoding is production parser work on
//! both sides. A production-only site executed while observing never
//! activates, but it is recorded apart ([`take_row_observe_hits`]), so a home
//! that runs only in observation code is never mistaken for one no oracle
//! executes.
//!
//! The row thread is the thread that called [`begin_row`]; it keeps its stage
//! and reach thread-locally. Every other thread is a worker of the running row:
//! a thread a production operation starts (a parallel breadth-first search, a
//! throttle group). A worker runs in the row thread's current stage, and its
//! reach is recorded process-wide and taken with the row's. So one row runs at
//! a time per process, as the drivers run them, and a row's workers finish
//! before the row thread switches stage or takes its reach (scoped threads,
//! joined groups): a worker still running then would record into the next
//! stage or row.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread::LocalKey;

/// A mutant's numeric id within one schemata build; 0 selects no mutant.
pub type MutantId = u32;

static ACTIVE: AtomicU32 = AtomicU32::new(0);
static TRACING: AtomicBool = AtomicBool::new(true);
/// The stage workers run in: the row thread's, as it last set it.
static ROW_STAGE: AtomicU8 = AtomicU8::new(Stage::Production as u8);
/// Workers' reach since the row began, by [`hit`] while producing and by
/// [`hit_parser`] in either stage.
static WORKER_HITS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// Production-only sites workers executed while observing.
static WORKER_OBSERVE_HITS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Whether the current thread runs production code or observes its results.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Stage {
    #[default]
    Production,
    Observe,
}

thread_local! {
    static ROW_THREAD: Cell<bool> = const { Cell::new(false) };
    static STAGE: Cell<Stage> = const { Cell::new(Stage::Production) };
    static HITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static OBSERVE_HITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// A per-thread bitset of mutant ids.
type HitSet = LocalKey<RefCell<Vec<u64>>>;

/// Where one kind of reach is recorded: the row thread's set and the workers'.
struct Reach {
    row: &'static HitSet,
    workers: &'static Mutex<Vec<u64>>,
}

const PRODUCTION: Reach = Reach {
    row: &HITS,
    workers: &WORKER_HITS,
};
const OBSERVATION: Reach = Reach {
    row: &OBSERVE_HITS,
    workers: &WORKER_OBSERVE_HITS,
};

fn row_thread() -> bool {
    ROW_THREAD.with(Cell::get)
}

/// Selects the mutant every guard compares against; 0 disables all mutants.
pub fn set_active(id: MutantId) {
    ACTIVE.store(id, Ordering::Relaxed);
}

#[must_use]
pub fn active() -> MutantId {
    ACTIVE.load(Ordering::Relaxed)
}

/// Sets the current thread's stage, which is also the stage its row's workers
/// run in.
pub fn set_stage(stage: Stage) {
    STAGE.with(|current| current.set(stage));
    ROW_STAGE.store(stage as u8, Ordering::Relaxed);
}

/// The current thread's stage: a worker's is its row's.
#[must_use]
pub fn stage() -> Stage {
    if row_thread() {
        STAGE.with(Cell::get)
    } else if ROW_STAGE.load(Ordering::Relaxed) == Stage::Observe as u8 {
        Stage::Observe
    } else {
        Stage::Production
    }
}

/// Turns reach recording on or off for every thread. Kill runs switch it off;
/// the trace pass keeps it on.
pub fn set_tracing(on: bool) {
    TRACING.store(on, Ordering::Relaxed);
}

/// Starts a new row on the current thread, which becomes the row thread, by
/// clearing its recorded reach and its workers'.
pub fn begin_row() {
    ROW_THREAD.with(|row| row.set(true));
    ROW_STAGE.store(STAGE.with(Cell::get) as u8, Ordering::Relaxed);
    for reach in [PRODUCTION, OBSERVATION] {
        reach
            .row
            .with(|hits| hits.borrow_mut().iter_mut().for_each(|word| *word = 0));
        workers(&reach).clear();
    }
}

fn workers(reach: &Reach) -> std::sync::MutexGuard<'static, Vec<u64>> {
    reach.workers.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The sorted, unique mutant ids reached since `begin_row` by [`hit`] while
/// producing and by [`hit_parser`] in either stage, on this thread and on the
/// row's workers; taking them clears them.
#[must_use]
pub fn take_row_hits() -> Vec<MutantId> {
    take(&PRODUCTION)
}

/// The sorted, unique ids of production-only sites ([`hit`]) executed while
/// observing since `begin_row`, on this thread and on the row's workers;
/// taking them clears them. Such a site never activates while observing.
#[must_use]
pub fn take_row_observe_hits() -> Vec<MutantId> {
    take(&OBSERVATION)
}

fn take(reach: &Reach) -> Vec<MutantId> {
    let mut words: Vec<u64> = reach
        .row
        .with(|hits| hits.borrow_mut().iter_mut().map(std::mem::take).collect());
    {
        let mut shared = workers(reach);
        if words.len() < shared.len() {
            words.resize(shared.len(), 0);
        }
        for (word, shared) in words.iter_mut().zip(shared.iter_mut()) {
            *word |= std::mem::take(shared);
        }
    }
    let mut ids = Vec::new();
    for (index, word) in words.into_iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let bit = bits.trailing_zeros();
            bits &= bits - 1;
            let id = u32::try_from(index * 64).expect("mutant id fits u32") + bit;
            ids.push(id);
        }
    }
    ids
}

fn insert(hits: &mut Vec<u64>, id: MutantId) {
    let word = (id / 64) as usize;
    if hits.len() <= word {
        hits.resize(word + 1, 0);
    }
    hits[word] |= 1 << (id % 64);
}

#[inline]
fn record(reach: &Reach, id: MutantId) {
    if TRACING.load(Ordering::Relaxed) {
        if row_thread() {
            reach.row.with(|hits| insert(&mut hits.borrow_mut(), id));
        } else {
            insert(&mut workers(reach), id);
        }
    }
}

/// A guard at a production-only site. While observing it is inactive and its
/// execution is recorded apart as observation reach.
#[inline]
#[must_use]
pub fn hit(id: MutantId) -> bool {
    if stage() == Stage::Observe {
        record(&OBSERVATION, id);
        return false;
    }
    record(&PRODUCTION, id);
    active() == id
}

/// A guard at a parser site, a `tsr_parser` site whose every operation is in
/// `internal/parser`: recorded and selectable in both stages.
#[inline]
#[must_use]
pub fn hit_parser(id: MutantId) -> bool {
    record(&PRODUCTION, id);
    active() == id
}

#[cfg(test)]
mod tests {
    use super::{
        active, begin_row, hit, hit_parser, set_active, set_stage, set_tracing, stage,
        take_row_hits, take_row_observe_hits, Stage,
    };
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::thread;

    // The switch is process-global, so the tests take turns.
    fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `guard` on a worker thread the row does not own, as a production
    /// operation that starts scoped threads does.
    fn on_worker<T: Send>(guard: impl FnOnce() -> T + Send) -> T {
        thread::scope(|scope| scope.spawn(guard).join().unwrap())
    }

    #[test]
    fn workers_run_in_the_rows_stage_and_their_reach_is_the_rows() {
        let _serial = serial();
        set_tracing(true);
        set_active(0);
        set_stage(Stage::Production);
        begin_row();
        on_worker(|| {
            assert_eq!(stage(), Stage::Production);
            assert!(!hit(5));
            assert!(!hit_parser(70));
        });
        set_stage(Stage::Observe);
        on_worker(|| {
            assert_eq!(
                stage(),
                Stage::Observe,
                "a worker observes while its row observes"
            );
            assert!(!hit(9));
            assert!(!hit_parser(71));
        });
        set_stage(Stage::Production);
        assert_eq!(
            take_row_hits(),
            [5, 70, 71],
            "worker reach is the row's reach"
        );
        assert_eq!(
            take_row_observe_hits(),
            [9],
            "a production-only site a worker executes while observing is observation reach"
        );
        assert!(take_row_hits().is_empty() && take_row_observe_hits().is_empty());

        // The row thread's own reach and its workers' merge.
        assert!(!hit(130));
        on_worker(|| hit(2));
        assert_eq!(take_row_hits(), [2, 130]);

        // A worker's guard selects the active mutant in production only.
        set_active(5);
        assert!(on_worker(|| hit(5)));
        assert!(!on_worker(|| hit(6)));
        set_stage(Stage::Observe);
        assert!(!on_worker(|| hit(5)), "inactive while its row observes");
        set_stage(Stage::Production);
        set_active(0);

        // A new row starts from a clean slate, workers included.
        on_worker(|| hit(8));
        begin_row();
        assert!(take_row_hits().is_empty());

        // Kill runs record nothing on workers either.
        set_tracing(false);
        on_worker(|| hit(3));
        assert!(take_row_hits().is_empty());
        set_tracing(true);
    }

    #[test]
    fn guards_record_reach_by_stage_and_select_only_the_active_mutant() {
        let _serial = serial();
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
