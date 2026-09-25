//! Group `concurrency`: core concurrency, request context, BFS.
//! Go: `tools/phase1/tables/go/concurrency_columns.go`; spec:
//! `data/phase1/tables/concurrency.json`.
use super::{decode, Column};
use crate::protocol::hex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tsr_core::bfs::{self, BreadthFirstSearchLevel, BreadthFirstSearchOptions, VisitedSet};
use tsr_core::context::{CheckerLifetime, RequestContext};

pub const COLUMNS: &[&str] = &[
    "core.RequestContext",
    "core.BreadthFirstSearchParallel",
    "core.BreadthFirstSearchParallelEx",
    "core.ThrottleGroup",
];

/// Go's `typedValuesColumn`.
fn typed<I: serde::de::DeserializeOwned + 'static>(
    input: &Value,
    f: fn(&I) -> Result<Value, String>,
) -> Result<Column, String> {
    let input: I = decode(input)?;
    Ok(Box::new(move || f(&input)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextOp {
    op: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    lifetime: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextOps {
    ops: Vec<ContextOp>,
}

/// Go's `levelPrune`.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LevelPrune {
    #[serde(default)]
    delete: Vec<String>,
    #[serde(default)]
    has: String,
    #[serde(default)]
    range: usize,
}

/// Go's `bfsGraph`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Graph {
    #[serde(default)]
    edges: HashMap<String, Vec<String>>,
    start: String,
    #[serde(default)]
    results: Vec<String>,
    #[serde(default)]
    stops: Vec<String>,
    #[serde(default)]
    visited: Vec<String>,
    #[serde(default)]
    key_len: usize,
    #[serde(default)]
    levels: Vec<LevelPrune>,
}

impl Graph {
    fn visit(&self, node: &String) -> (bool, bool) {
        let stop = self.stops.contains(node);
        (stop || self.results.contains(node), stop)
    }

    fn neighbors(&self, node: &String) -> Vec<String> {
        self.edges.get(node).cloned().unwrap_or_default()
    }

    fn key(&self, node: &str) -> String {
        if self.key_len > 0 && node.len() > self.key_len {
            node[..self.key_len].to_owned()
        } else {
            node.to_owned()
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Graphs {
    graphs: Vec<Graph>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThrottleJob {
    value: i64,
    #[serde(default)]
    fail: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThrottleCase {
    limit: usize,
    jobs: Vec<ThrottleJob>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThrottleCases {
    cases: Vec<ThrottleCase>,
}

/// Go's `bfsValue`.
fn bfs_value((stopped, path): (bool, Vec<String>)) -> Value {
    json!([
        stopped,
        path.iter()
            .map(|node| hex(node.as_bytes()))
            .collect::<Vec<_>>()
    ])
}

fn lifetime(value: i64) -> CheckerLifetime {
    match value {
        1 => CheckerLifetime::Diagnostics,
        2 => CheckerLifetime::Api,
        _ => CheckerLifetime::Temporary,
    }
}

fn lifetime_value(lifetime: CheckerLifetime) -> i64 {
    match lifetime {
        CheckerLifetime::Temporary => 0,
        CheckerLifetime::Diagnostics => 1,
        CheckerLifetime::Api => 2,
    }
}

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "core.RequestContext" => typed::<ContextOps>(input, |input| {
            let mut context = RequestContext::default();
            let mut out = Vec::new();
            for op in &input.ops {
                match op.op.as_str() {
                    "with_request_id" => {
                        let mut derived = context.clone();
                        derived.with_request_id(op.id.as_bytes());
                        context = derived;
                    }
                    "with_checker_lifetime" => {
                        let mut derived = context.clone();
                        derived.with_checker_lifetime(lifetime(op.lifetime));
                        context = derived;
                    }
                    "get_request_id" => out.push(json!(hex(context.request_id()))),
                    _ => out.push(json!(lifetime_value(context.checker_lifetime()))),
                }
            }
            Ok(Value::Array(out))
        }),
        "core.BreadthFirstSearchParallel" => typed::<Graphs>(input, |input| {
            Ok(Value::Array(
                input
                    .graphs
                    .iter()
                    .map(|graph| {
                        bfs_value(bfs::breadth_first_search_parallel(
                            graph.start.clone(),
                            &|node| graph.neighbors(node),
                            &|node| graph.visit(node),
                        ))
                    })
                    .collect(),
            ))
        }),
        "core.BreadthFirstSearchParallelEx" => typed::<Graphs>(input, |input| {
            let mut out = Vec::new();
            for graph in &input.graphs {
                let visited = Arc::new(VisitedSet::default());
                for key in &graph.visited {
                    visited.add_if_absent(key.clone());
                }
                let log = Mutex::new(Vec::new());
                let level = Mutex::new(0usize);
                let preprocess = |l: &mut BreadthFirstSearchLevel<'_, String, String>| {
                    let prune = {
                        let mut index = level
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let prune = graph.levels.get(*index).cloned().unwrap_or_default();
                        *index += 1;
                        prune
                    };
                    let mut read = Vec::new();
                    l.range(&mut |node: &String| {
                        if read.len() >= prune.range {
                            return false;
                        }
                        read.push(hex(node.as_bytes()));
                        true
                    });
                    let has = l.has(&prune.has);
                    for key in &prune.delete {
                        l.delete(key);
                    }
                    log.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(json!([read, has]));
                };
                let options = BreadthFirstSearchOptions {
                    visited: Some(visited),
                    preprocess_level: Some(&preprocess),
                };
                let result = bfs::breadth_first_search_parallel_ex(
                    graph.start.clone(),
                    &|node| graph.neighbors(node),
                    &|node| graph.visit(node),
                    &options,
                    &|node| graph.key(node),
                );
                let mut value = bfs_value(result);
                value.as_array_mut().expect("bfs value").push(Value::Array(
                    log.into_inner()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                ));
                out.push(value);
            }
            Ok(Value::Array(out))
        }),
        "core.ThrottleGroup" => typed::<ThrottleCases>(input, |input| {
            let mut out = Vec::new();
            for case in &input.cases {
                let values = Mutex::new(Vec::new());
                let (running, most) = (AtomicI64::new(0), AtomicI64::new(0));
                let semaphore = Arc::new(tsr_core::semaphore::LimitedSemaphore::new(case.limit));
                let mut group = tsr_core::workgroup::ThrottleGroup::<String>::new(semaphore);
                for job in &case.jobs {
                    let (values, running, most) = (&values, &running, &most);
                    group.go(move || {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        values
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(job.value);
                        running.fetch_sub(1, Ordering::SeqCst);
                        if job.fail {
                            return Err("job failed".to_owned());
                        }
                        Ok(())
                    });
                }
                let failure = match group.wait() {
                    Err(error) => json!(hex(error.as_bytes())),
                    Ok(()) => Value::Null,
                };
                let mut values = values
                    .into_inner()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                values.sort_unstable();
                let limit = i64::try_from(case.limit).map_err(|error| error.to_string())?;
                out.push(json!([
                    values,
                    failure,
                    most.load(Ordering::SeqCst) <= limit
                ]));
            }
            Ok(Value::Array(out))
        }),
        _ => return None,
    })
}
