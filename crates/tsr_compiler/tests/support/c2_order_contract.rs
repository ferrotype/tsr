//! C2.11(8): source-program witnesses for the actual comparator fallback.
//! Native: checker.CompareTypes / Checker.compareSymbolsWorker at the pin.
#[path = "c2_order_program.rs"]
mod program;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (String, u64, u64);
fn identity(kind: &str, value: &Value) -> Identity {
    let token = value["token"].as_u64().expect("birth must be observed");
    assert_ne!(token, 0);
    (kind.into(), value["owner"].as_u64().unwrap_or(0), token)
}
fn snapshot(kind: &str, value: &Value, assigned: &BTreeMap<Identity, u64>) -> Identity {
    assert!(value.get("unobserved").is_none());
    let id = identity(kind, value);
    assert_eq!(assigned.get(&id), value["semantic_id"].as_u64().as_ref());
    if kind == "type" && !value["symbol"].is_null() {
        snapshot("symbol", &value["symbol"], assigned);
    }
    if kind == "symbol" {
        assert!(value["declarations"].is_array());
    }
    id
}
#[derive(Debug, PartialEq, Eq)]
struct TraceContract {
    ties: BTreeSet<(usize, usize, i64)>,
    families: BTreeSet<(String, String, u64, u64, u64, u64)>,
}
fn ties(witness: &Value, observed: &Value) -> TraceContract {
    let kind = if witness["request"].get("property").is_some() {
        "symbol"
    } else {
        "type"
    };
    let operands = if kind == "symbol" {
        &observed["order"]["before"]
    } else {
        &observed["types"]
    };
    let labels: BTreeMap<_, _> = operands
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, v)| (identity(kind, v), i))
        .collect();
    assert_eq!(
        labels.len(),
        witness["request"]["queries"].as_array().unwrap().len()
    );
    let mut assigned = BTreeMap::new();
    let mut sorts = Vec::new();
    let mut result = BTreeSet::new();
    let mut families = BTreeSet::new();
    for event in observed["trace"].as_array().unwrap() {
        let current = event["kind"].as_str().unwrap();
        assert!(matches!(current, "type" | "symbol"));
        match event["event"].as_str().unwrap() {
            "birth" => {
                let sites = event["origin"]["stack"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![event["origin"].clone()]);
                assert!(!sites.is_empty());
                for site in sites {
                    assert!(site["file"].as_str().is_some_and(|s| !s.is_empty()));
                    assert!(site["line"].as_u64().is_some_and(|n| n > 0));
                }
                let id = identity(current, event);
                assert!(assigned
                    .insert(id, event["semantic_id"].as_u64().unwrap())
                    .is_none());
                if current == "symbol" {
                    assert_eq!(event["semantic_id"], 0);
                } else {
                    assert_eq!(event["semantic_id"], event["token"]);
                }
            }
            "id_assignment" => {
                assert_eq!(current, "symbol");
                let id = identity(current, event);
                assert_eq!(assigned.get(&id), Some(&0));
                let semantic = event["semantic_id"].as_u64().unwrap();
                assert_ne!(semantic, 0);
                assigned.insert(id, semantic);
            }
            "sort_begin" | "sort_end" => {
                let mut ids: Vec<_> = event["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| snapshot(current, v, &assigned))
                    .collect();
                ids.sort();
                if event["event"] == "sort_begin" {
                    sorts.push((current, event["operation"].as_str().unwrap(), ids));
                } else {
                    assert_eq!(
                        sorts.pop(),
                        Some((current, event["operation"].as_str().unwrap(), ids))
                    );
                }
            }
            "fallback" => {
                let left = snapshot(current, &event["left"], &assigned);
                let right = snapshot(current, &event["right"], &assigned);
                let delta = event["left"]["semantic_id"].as_i64().unwrap()
                    - event["right"]["semantic_id"].as_i64().unwrap();
                assert_eq!(event["sign"].as_i64(), Some(delta.signum()));
                assert!(matches!(
                    event["branch"].as_str().unwrap(),
                    "final_type_id" | "declarationless" | "equal_first_declaration"
                ));
                let branch = event["branch"].as_str().unwrap();
                if current == "symbol" {
                    let a = &event["left"];
                    let b = &event["right"];
                    assert_eq!(a["name"], b["name"]);
                    match branch {
                        "declarationless" => {
                            assert!(a["declarations"].as_array().unwrap().is_empty());
                            assert!(b["declarations"].as_array().unwrap().is_empty());
                        }
                        "equal_first_declaration" => {
                            assert!(!a["declarations"].as_array().unwrap().is_empty());
                            assert!(!b["declarations"].as_array().unwrap().is_empty());
                            assert_eq!(a["declarations"][0], b["declarations"][0]);
                        }
                        other => panic!("invalid symbol fallback {other}"),
                    }
                } else {
                    assert_eq!(branch, "final_type_id");
                }
                families.insert((
                    current.into(),
                    branch.into(),
                    event["left"]["flags"].as_u64().unwrap(),
                    event["right"]["flags"].as_u64().unwrap(),
                    event["left"]["object_flags"].as_u64().unwrap_or(0),
                    event["right"]["object_flags"].as_u64().unwrap_or(0),
                ));
                if current == kind && event["branch"] == witness["required_branch"] {
                    if let (Some(&a), Some(&b)) = (labels.get(&left), labels.get(&right)) {
                        if a != b {
                            result.insert((
                                a.min(b),
                                a.max(b),
                                if a < b {
                                    delta.signum()
                                } else {
                                    -delta.signum()
                                },
                            ));
                        }
                    }
                }
            }
            other => panic!("unknown trace event {other}"),
        }
    }
    assert!(sorts.is_empty());
    assert!(
        !result.is_empty(),
        "actual queried operands must reach the required fallback"
    );
    if witness["id"] == "reverse-mapped-order" {
        for operand in operands.as_array().unwrap() {
            assert_ne!(
                operand["object_flags"].as_u64().unwrap()
                    & u64::from(tsr_checker::object_flags::REVERSE_MAPPED),
                0
            );
        }
    }
    TraceContract {
        ties: result,
        families,
    }
}

#[test]
fn source_creation_order_matches_native_without_changing_ordinary_outputs() {
    let frozen: Value =
        serde_json::from_str(include_str!("../../../../data/phase2/c2-order-traces.json")).unwrap();
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../../tools/phase2/order-trace/witnesses.json"
    ))
    .unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../../data/upstream.json")).unwrap();
    assert_eq!(frozen["manifest"], manifest);
    assert_eq!(manifest["pin"], pin["pin"]);
    for (path, bytes) in [
        (
            "scripts/phase2_order_trace.py",
            include_bytes!("../../../../scripts/phase2_order_trace.py").as_slice(),
        ),
        (
            "tools/phase2/order-trace/ast_trace.go",
            include_bytes!("../../../../tools/phase2/order-trace/ast_trace.go").as_slice(),
        ),
        (
            "tools/phase2/order-trace/checker_trace.go",
            include_bytes!("../../../../tools/phase2/order-trace/checker_trace.go").as_slice(),
        ),
        (
            "tools/phase2/order-trace/driver_test.go",
            include_bytes!("../../../../tools/phase2/order-trace/driver_test.go").as_slice(),
        ),
    ] {
        assert_eq!(
            frozen["build"]["inputs"][path],
            format!("{:x}", Sha256::digest(bytes))
        );
    }
    assert_eq!(manifest["witnesses"].as_array().unwrap().len(), 4);
    assert_eq!(frozen["rows"].as_array().unwrap().len(), 4);
    for (witness, native) in manifest["witnesses"]
        .as_array()
        .unwrap()
        .iter()
        .zip(frozen["rows"].as_array().unwrap())
    {
        assert_eq!(witness["id"], native["id"]);
        assert_eq!(native["on"]["ordinary"], native["off"]["ordinary"]);
        assert!(native["off"]["trace"].as_array().unwrap().is_empty());
        let enabled = program::run(witness["request"].clone(), true).unwrap();
        let disabled = program::run(witness["request"].clone(), false).unwrap();
        assert_eq!(
            enabled["ordinary"], disabled["ordinary"],
            "{} tracing changes output",
            witness["id"]
        );
        assert!(disabled["trace"].as_array().unwrap().is_empty());
        assert_eq!(
            enabled["ordinary"], native["on"]["ordinary"],
            "{} native output",
            witness["id"]
        );
        assert_eq!(
            ties(witness, &enabled),
            ties(witness, &native["on"]),
            "{} semantic order",
            witness["id"]
        );
    }
}

#[test]
fn binary_fallback_uses_left_type_when_context_comes_from_a_binding_pattern() {
    // checker.go:getContextualTypeForBinaryOperand: a pattern-derived context
    // does not independently contextualize the RHS of || or ??. The controls
    // use an unconstrained call and an explicitly typed tuple on the left.
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/c2/contextual_audit.json")).unwrap();
    let trace: Value =
        serde_json::from_str(include_str!("../../../../data/phase2/c2-order-traces.json")).unwrap();
    assert_eq!(fixture["build"], trace["build"]);
    for row in fixture["rows"].as_array().unwrap() {
        let observed = program::run(row["request"].clone(), false).unwrap();
        assert_eq!(observed["ordinary"], row["native"], "{}", row["id"]);
    }
}
