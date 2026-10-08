//! Counts of node reads and of the accessors called on them, for sizing the
//! node-access levers (`access-stats` feature only). Process-wide, relaxed:
//! the counts order nothing.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub static READS_CORE: AtomicU64 = AtomicU64::new(0);
pub static READS_TRANSACTION: AtomicU64 = AtomicU64::new(0);
pub static READS_OWNED: AtomicU64 = AtomicU64::new(0);
pub static READS_LAZY: AtomicU64 = AtomicU64::new(0);
pub static KIND: AtomicU64 = AtomicU64::new(0);
pub static PARENT: AtomicU64 = AtomicU64::new(0);
pub static FLAGS: AtomicU64 = AtomicU64::new(0);
pub static POS_END: AtomicU64 = AtomicU64::new(0);
pub static DATA: AtomicU64 = AtomicU64::new(0);
pub static DATA_SOURCE: AtomicU64 = AtomicU64::new(0);
/// Reads that served `kind` at least once, anything else at least once, and
/// both: kind-only reads are `WITH_KIND - WITH_BOTH`, unused reads the
/// difference from the read count.
pub static WITH_KIND: AtomicU64 = AtomicU64::new(0);
pub static WITH_OTHER: AtomicU64 = AtomicU64::new(0);
pub static WITH_BOTH: AtomicU64 = AtomicU64::new(0);

/// A read's first use of an accessor class: `bit` 1 the kind, 2 anything
/// else; `before` the classes it served already.
#[inline]
pub fn first_use(bit: u8, before: u8) {
    if bit == 1 {
        bump(&WITH_KIND);
        if before & 2 != 0 {
            bump(&WITH_BOTH);
        }
    } else {
        bump(&WITH_OTHER);
        if before & 1 != 0 {
            bump(&WITH_BOTH);
        }
    }
}

/// Every counter in a fixed order, for per-phase differences.
pub const NAMES: [&str; 13] = [
    "reads_core",
    "reads_transaction",
    "reads_owned",
    "reads_lazy",
    "kind",
    "parent",
    "flags",
    "pos_end",
    "data",
    "data_source",
    "with_kind",
    "with_other",
    "with_both",
];
pub fn snapshot() -> [u64; 13] {
    [
        &READS_CORE,
        &READS_TRANSACTION,
        &READS_OWNED,
        &READS_LAZY,
        &KIND,
        &PARENT,
        &FLAGS,
        &POS_END,
        &DATA,
        &DATA_SOURCE,
        &WITH_KIND,
        &WITH_OTHER,
        &WITH_BOTH,
    ]
    .map(|counter| counter.load(Relaxed))
}

#[inline]
pub fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Relaxed);
}

/// One JSON object of every counter, or of the given values.
pub fn report() -> String {
    report_values(&snapshot())
}
pub fn report_values(values: &[u64; 13]) -> String {
    let fields: Vec<String> = NAMES
        .iter()
        .zip(values)
        .map(|(name, value)| format!("\"{name}\":{value}"))
        .collect();
    format!("{{{}}}", fields.join(","))
}
