use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Output(Mutex<Vec<u8>>);
impl crate::Writer for Output {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
}
fn report(statistics: &Statistics) -> String {
    let output = Arc::new(Output::default());
    let writer: SharedWriter = output.clone();
    statistics.report(&writer, None);
    let bytes = output.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn table_keeps_zero_parse_and_total_hides_optional_zero_and_uses_kib() {
    let stat = Statistics {
        files: 3,
        lines: 10,
        identifiers: 21,
        symbols: 31,
        types: 41,
        instantiations: 51,
        memory_used: Some(2049),
        memory_allocs: Some(9),
        ..Default::default()
    };
    // Observed from the pinned Statistics.Report via a Go 1.27.1 overlay
    // test with the same fixed fields, not inferred from the Rust formatter.
    assert_eq!(
        report(&stat),
        concat!(
            "Files:               3\n",
            "Lines:              10\n",
            "Identifiers:        21\n",
            "Symbols:            31\n",
            "Types:              41\n",
            "Instantiations:     51\n",
            "Memory used:        2K\n",
            "Memory allocs:       9\n",
            "Parse time:     0.000s\n",
            "Total time:     0.000s\n",
        )
    );
}

#[test]
fn mapper_rows_sort_by_identity_and_gate_on_operation_count() {
    use tsr_contentmapper::{MapperTimings, OperationTiming};
    let mut stat = Statistics::default();
    stat.compile_times.content_mapper_times.mappers.insert(
        "z".into(),
        MapperTimings {
            spawn: OperationTiming {
                count: 1,
                duration: Duration::from_millis(20),
            },
            initialize: OperationTiming {
                count: 0,
                duration: Duration::from_millis(30),
            },
            transform: OperationTiming {
                count: 0,
                duration: Duration::from_secs(99),
            },
            ..Default::default()
        },
    );
    stat.compile_times.content_mapper_times.mappers.insert(
        "a".into(),
        MapperTimings {
            transform: OperationTiming {
                count: 1,
                duration: Duration::ZERO,
            },
            ..Default::default()
        },
    );
    let text = report(&stat);
    assert!(text.find("a transform time:").unwrap() < text.find("z initialization time:").unwrap());
    assert!(text.contains("0.050s"));
    assert!(!text.contains("z transform"));
    assert!(text.contains("unavailable"));
}

#[test]
fn aggregate_sums_program_counters_without_adding_mapper_or_total_times() {
    let mut stat = Statistics {
        files: 2,
        symbols: i64::from(u32::MAX),
        memory_used: Some(1024),
        memory_allocs: Some(5),
        ..Default::default()
    };
    stat.compile_times.parse_time = Duration::from_millis(250);
    stat.compile_times.total_time = Duration::from_secs(9);
    stat.compile_times.content_mapper_times.request_wait = Duration::from_secs(2);
    let mut aggregate = Statistics::default();
    aggregate.aggregate(&stat);
    aggregate.aggregate(&stat);
    assert_eq!(aggregate.files, 4);
    assert_eq!(aggregate.symbols, 2 * i64::from(u32::MAX));
    assert_eq!(aggregate.memory_used, Some(2048));
    assert_eq!(
        aggregate.compile_times.parse_time,
        Duration::from_millis(500)
    );
    assert_eq!(aggregate.compile_times.total_time, Duration::ZERO);
    assert_eq!(
        aggregate.compile_times.content_mapper_times.request_wait,
        Duration::ZERO
    );
    aggregate.set_total_time(Duration::from_millis(600));
    let text = report(&aggregate);
    assert!(text.starts_with("Projects in scope:"));
    assert!(text.contains("Aggregate Total time:"));
    assert!(text.contains("0.600s"));
}
