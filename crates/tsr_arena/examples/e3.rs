use serde_json::{json, Map, Value};
use ts_arena::scenarios::Scenario;

fn report(scenarios: &[Scenario]) -> Value {
    let mut metrics = Map::new();
    let mut tests = Vec::new();
    let mut measurements = Vec::new();
    for &(criterion, scenario) in scenarios {
        let result = std::panic::catch_unwind(scenario);
        if criterion != "owners_return_to_baseline" && criterion != "allocations_return_to_baseline"
        {
            metrics.insert(criterion.into(), json!(result.is_ok()));
        }
        match result {
            Ok(measurement) => {
                eprintln!("{criterion}: {measurement:?}");
                measurements.push(measurement);
                tests.push(json!({"id": criterion, "result": "pass"}));
            }
            Err(payload) => {
                let error = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload");
                eprintln!("{criterion}: fail: {error}");
                tests.push(json!({"id": criterion, "result": "fail", "error": error}));
            }
        }
    }
    // A missing measurement could hide unreclaimed storage. Do not infer a zero
    // delta from only those scenarios that happened to finish.
    if measurements.len() == scenarios.len() {
        metrics.insert(
            "live_owner_delta".into(),
            json!(measurements
                .iter()
                .map(|result| result.live_owner_delta)
                .max()
                .unwrap_or(0)),
        );
        metrics.insert(
            "live_allocation_delta".into(),
            json!(measurements
                .iter()
                .map(|result| result.live_allocation_delta)
                .max()
                .unwrap_or(0)),
        );
    }
    json!({"metrics": metrics, "tests": tests})
}

fn main() {
    println!("{}", report(&ts_arena::scenarios::ALL));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_arena::scenarios::Measurement;

    fn succeeds() -> Measurement {
        Measurement {
            live_owner_delta: 0,
            live_allocation_delta: 0,
            peak_owners: 1,
            peak_allocations: 1,
        }
    }

    #[test]
    fn report_preserves_failed_criterion_and_continues_remaining_scenarios() {
        let report = report(&[
            ("wrong_owner_rejected", || {
                panic!("injected \"failure\"\nwith newline")
            }),
            ("concurrent_lazy_storage", succeeds),
        ]);
        assert_eq!(report["metrics"]["wrong_owner_rejected"], false);
        assert_eq!(report["metrics"]["concurrent_lazy_storage"], true);
        assert_eq!(report["tests"][0]["result"], "fail");
        assert_eq!(
            report["tests"][0]["error"],
            "injected \"failure\"\nwith newline"
        );
        assert_eq!(report["tests"][1]["result"], "pass");
        assert!(report["metrics"].get("live_owner_delta").is_none());
        assert!(report["metrics"].get("live_allocation_delta").is_none());
        let round_trip: Value = serde_json::from_str(&report.to_string()).unwrap();
        assert_eq!(round_trip, report);
    }

    #[test]
    fn complete_report_includes_measured_deltas() {
        let report = report(&[("owners_return_to_baseline", succeeds)]);
        assert_eq!(report["metrics"]["live_owner_delta"], 0);
        assert_eq!(report["metrics"]["live_allocation_delta"], 0);
        assert!(report["metrics"].get("owners_return_to_baseline").is_none());
        assert_eq!(report["tests"][0]["result"], "pass");
    }
}
