//! Per-request server processing time, answered on the connection itself.
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tsr_json::{Encode, Encoder, Error};
use tsr_jsonrpc::{field, object};

/// The meta-requests a connection answers itself when it collects timing.
pub const METHOD_GET_SERVER_TIMING: &str = "getServerTiming";
pub const METHOD_RESET_SERVER_TIMING: &str = "resetServerTiming";

/// The number of most recent requests the ring buffer keeps.
const SERVER_RECENT_REQUEST_CAPACITY: usize = 5;

#[derive(Clone, Debug, PartialEq)]
struct ServerRequestTiming {
    method: String,
    processing_time_ms: f64,
    timestamp: i64,
}

impl Encode for ServerRequestTiming {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"method", &self.method),
                field(b"processingTimeMs", &self.processing_time_ms),
                field(b"timestamp", &self.timestamp),
            ],
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ServerTimingTotals {
    request_count: u64,
    total_processing_time_ms: f64,
}

impl Encode for ServerTimingTotals {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"requestCount", &self.request_count),
                field(b"totalProcessingTimeMs", &self.total_processing_time_ms),
            ],
        )
    }
}

/// A snapshot of the collected timing, the answer to `getServerTiming`.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerTimingInfo {
    enabled: bool,
    totals: ServerTimingTotals,
    recent_requests: Vec<ServerRequestTiming>,
}

impl Encode for ServerTimingInfo {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        struct Recent<'a>(&'a [ServerRequestTiming]);
        impl Encode for Recent<'_> {
            fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
                out.array(self.0.iter())
            }
        }
        object(
            out,
            &[
                field(b"enabled", &self.enabled),
                field(b"totals", &self.totals),
                field(b"recentRequests", &Recent(&self.recent_requests)),
            ],
        )
    }
}

#[derive(Default)]
struct Collected {
    totals: ServerTimingTotals,
    ring: Vec<ServerRequestTiming>,
    head: usize,
}

/// Running totals and a ring buffer of the most recent requests.
#[derive(Default)]
pub struct TimingCollector(Mutex<Collected>);

impl TimingCollector {
    /// port: tsc/internal/ipc/timing.go:newTimingCollector
    pub fn new() -> Self {
        Self::default()
    }

    /// port: tsc/internal/ipc/timing.go:timingCollector.record
    pub fn record(&self, method: &str, duration: Duration) {
        let processing_time_ms = duration_to_millis(duration);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            });
        let mut collected = self.0.lock().expect("timing lock");
        collected.totals.request_count += 1;
        collected.totals.total_processing_time_ms += processing_time_ms;
        let entry = ServerRequestTiming {
            method: method.to_owned(),
            processing_time_ms,
            timestamp,
        };
        if collected.ring.len() < SERVER_RECENT_REQUEST_CAPACITY {
            collected.ring.push(entry);
        } else {
            let head = collected.head;
            collected.ring[head] = entry;
            collected.head = (head + 1) % SERVER_RECENT_REQUEST_CAPACITY;
        }
    }

    /// The collected timing, recent requests oldest first.
    /// port: tsc/internal/ipc/timing.go:timingCollector.snapshot
    pub fn snapshot(&self) -> ServerTimingInfo {
        let collected = self.0.lock().expect("timing lock");
        let length = collected.ring.len();
        ServerTimingInfo {
            enabled: true,
            totals: collected.totals.clone(),
            recent_requests: (0..length)
                .map(|index| collected.ring[(collected.head + index) % length].clone())
                .collect(),
        }
    }

    /// port: tsc/internal/ipc/timing.go:timingCollector.reset
    pub fn reset(&self) {
        *self.0.lock().expect("timing lock") = Collected::default();
    }
}

/// The collector's snapshot, or the disabled snapshot without a collector.
/// port: tsc/internal/ipc/timing.go:serverTimingSnapshot
pub fn server_timing_snapshot(collector: Option<&TimingCollector>) -> ServerTimingInfo {
    collector.map_or_else(disabled_server_timing_info, TimingCollector::snapshot)
}

/// port: tsc/internal/ipc/timing.go:disabledServerTimingInfo
fn disabled_server_timing_info() -> ServerTimingInfo {
    ServerTimingInfo {
        enabled: false,
        totals: ServerTimingTotals::default(),
        recent_requests: Vec::new(),
    }
}

/// Fractional milliseconds from the whole nanosecond duration.
/// port: tsc/internal/ipc/timing.go:durationToMillis
fn duration_to_millis(duration: Duration) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let nanoseconds = duration.as_nanos() as f64;
    nanoseconds / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_five_most_recent_requests_oldest_first() {
        let collector = TimingCollector::new();
        for index in 0..7u64 {
            collector.record(&format!("m{index}"), Duration::from_micros(1500));
        }
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.totals.request_count, 7);
        assert!((snapshot.totals.total_processing_time_ms - 10.5).abs() < 1e-9);
        let methods: Vec<_> = snapshot
            .recent_requests
            .iter()
            .map(|r| r.method.as_str())
            .collect();
        assert_eq!(methods, ["m2", "m3", "m4", "m5", "m6"]);
        collector.reset();
        assert_eq!(collector.snapshot().recent_requests, Vec::new());
        let disabled =
            tsr_json::marshal(&server_timing_snapshot(None), tsr_json::Options::default()).unwrap();
        assert_eq!(
            disabled,
            br#"{"enabled":false,"totals":{"requestCount":0,"totalProcessingTimeMs":0},"recentRequests":[]}"#
        );
    }

    /// Ports `TestTimingCollector` of tsc/internal/ipc/timing_test.go.
    #[test]
    fn accumulates_totals_and_records_recent_requests() {
        let collector = TimingCollector::new();
        collector.record("getSourceFile", Duration::from_millis(2));
        collector.record("getSymbolAtPosition", Duration::from_micros(500));
        let snapshot = collector.snapshot();
        assert!(snapshot.enabled);
        assert_eq!(snapshot.totals.request_count, 2);
        assert_eq!(snapshot.totals.total_processing_time_ms, 2.5);
        assert_eq!(snapshot.recent_requests.len(), 2);
        assert_eq!(snapshot.recent_requests[0].method, "getSourceFile");
        assert_eq!(snapshot.recent_requests[0].processing_time_ms, 2.0);
        assert_eq!(snapshot.recent_requests[1].method, "getSymbolAtPosition");
        assert_eq!(snapshot.recent_requests[1].processing_time_ms, 0.5);
        // The pin clamps a negative duration to zero; `Duration` cannot be
        // negative, so the zero duration is the boundary that remains.
        let collector = TimingCollector::new();
        collector.record("x", Duration::ZERO);
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.totals.total_processing_time_ms, 0.0);
        assert_eq!(snapshot.recent_requests[0].processing_time_ms, 0.0);
    }

    /// Ports `TestServerTimingSnapshotDisabled` of tsc/internal/ipc/timing_test.go.
    #[test]
    fn the_disabled_snapshot_is_empty() {
        let snapshot = server_timing_snapshot(None);
        assert!(!snapshot.enabled);
        assert_eq!(snapshot.totals.request_count, 0);
        assert!(snapshot.recent_requests.is_empty());
    }

    /// Ports `TestTimingCollectorReset` of tsc/internal/ipc/timing_test.go.
    #[test]
    fn reset_empties_the_collector_and_keeps_it_usable() {
        let collector = TimingCollector::new();
        collector.record("a", Duration::from_millis(1));
        collector.record("b", Duration::from_millis(1));
        collector.reset();
        let snapshot = collector.snapshot();
        assert!(snapshot.enabled);
        assert_eq!(snapshot.totals.request_count, 0);
        assert_eq!(snapshot.totals.total_processing_time_ms, 0.0);
        assert!(snapshot.recent_requests.is_empty());
        collector.record("c", Duration::from_millis(2));
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.totals.request_count, 1);
        assert_eq!(snapshot.recent_requests[0].method, "c");
    }

    /// Ports `TestDurationToMillis` of tsc/internal/ipc/timing_test.go.
    #[test]
    fn duration_to_millis_keeps_sub_microsecond_precision() {
        assert_eq!(duration_to_millis(Duration::from_micros(1500)), 1.5);
        assert_eq!(duration_to_millis(Duration::ZERO), 0.0);
        assert_eq!(duration_to_millis(Duration::from_nanos(500)), 0.0005);
        assert_eq!(duration_to_millis(Duration::from_nanos(1234)), 0.001_234);
    }
}
