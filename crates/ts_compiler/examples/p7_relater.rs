//! S08 relater comparison child (`data/s08/relater-fixtures.json`).
//!
//! `p7_relater REQUESTS OUTPUT --implementation id|reference` runs one ordered
//! pass of every fixture and mode group in a fresh process. Each group has a
//! setup interval (checker creation and A/B declared-type lookup) and a relation interval
//! (the four frozen actions with their lazy resolution). The `s08-allocation`
//! feature adds the S07 allocator wrapper for requested and live bytes.
//!
//! The ID implementation is the production checker's relater through the
//! relation probe. The reference implementation is
//! `tools/s08/relater-prototype`, which constructs its own types from retained
//! bound files and loader facts. Its setup never creates a production checker.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_checker::{CheckerOwner, Operation, RelationKind, TypeRef};
use ts_compiler::{Program, ProgramCheckerHost};

use s08_relater_prototype as proto;

mod p3;
#[path = "p7_relater/reference.rs"]
mod reference;
use p3::{array, load, text, Result};

#[cfg(feature = "s08-allocation")]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(not(feature = "s08-allocation"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const MODE: &str = if cfg!(feature = "s08-allocation") {
    "alloc"
} else {
    "normal"
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Implementation {
    Id,
    Reference,
}

/// Wall time and allocator endpoints of one interval.
#[derive(Default)]
struct Interval {
    ns: u64,
    requested_bytes: u64,
}

fn allocator_total() -> u64 {
    #[cfg(feature = "s08-allocation")]
    {
        ALLOCATOR.total_allocated() as u64
    }
    #[cfg(not(feature = "s08-allocation"))]
    {
        0
    }
}

fn allocator_live() -> u64 {
    #[cfg(feature = "s08-allocation")]
    {
        ALLOCATOR.allocated() as u64
    }
    #[cfg(not(feature = "s08-allocation"))]
    {
        0
    }
}

fn measure<T>(work: impl FnOnce() -> T) -> (T, Interval) {
    let total = allocator_total();
    let started = Instant::now();
    let value = work();
    let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    (
        value,
        Interval {
            ns,
            requested_bytes: allocator_total() - total,
        },
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("formatting into String is infallible");
    }
    text
}

fn mode_of(value: &Value) -> Result<(RelationKind, proto::Mode)> {
    Ok(match text(value)? {
        "identity" => (RelationKind::Identity, proto::Mode::Identity),
        "assignable" => (RelationKind::Assignable, proto::Mode::Assignable),
        "subtype" => (RelationKind::Subtype, proto::Mode::Subtype),
        "strict_subtype" => (RelationKind::StrictSubtype, proto::Mode::StrictSubtype),
        "comparable" => (RelationKind::Comparable, proto::Mode::Comparable),
        _ => return Err("unknown relation".into()),
    })
}

fn diagnostic_payload(program: &Program, d: &ts_ast::Diagnostic) -> Result<Value> {
    let file = if let Some(file) = d.file {
        let file = program
            .files()
            .iter()
            .find(|entry| entry.source() == file)
            .ok_or("diagnostic file missing")?;
        Some(String::from_utf8(
            file.bound().view().source_file()?.file_name().to_vec(),
        )?)
    } else {
        None
    };
    Ok(
        json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,"key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),
        "args":if d.message_args.is_empty() { Value::Null } else { json!(d.message_args.iter().map(|v| String::from_utf8(v.as_bytes().to_vec())).collect::<std::result::Result<Vec<_>,_>>()?) },
        "chain":d.message_chain.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?,"related":d.related_information.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?}),
    )
}

/// A/B declaration nodes and declared types of one fixture on one checker.
struct Lookup {
    declarations: BTreeMap<Vec<u8>, ts_arena::NodeId>,
    types: BTreeMap<String, TypeRef>,
}

fn lookup(program: &Arc<Program>, op: &mut Operation<'_>, actions: &[Value]) -> Result<Lookup> {
    let file = program.file(b"/fixture.ts").ok_or("fixture missing")?;
    let ast = file.bound().view().ast();
    let statements = ast
        .node(file.source())?
        .statement_list()
        .ok_or("statements missing")?;
    let mut declarations = BTreeMap::new();
    for node in ast
        .node_slice(ast.list(statements)?.nodes())?
        .iter()
        .flatten()
    {
        if let Some(name) = ast.node(node)?.name() {
            declarations.insert(ast.node_text(name)?.as_bytes().to_vec(), node);
        }
    }
    let mut types = BTreeMap::new();
    for action in actions {
        for key in ["source", "target"] {
            let name = text(&action[key])?;
            if !types.contains_key(name) {
                let node = *declarations
                    .get(name.as_bytes())
                    .ok_or("declaration missing")?;
                let symbol = op
                    .get_symbol_at_location(
                        ast.node(node)?.name().ok_or("declaration name missing")?,
                    )?
                    .ok_or("symbol missing")?;
                types.insert(name.to_owned(), op.get_declared_type_of_symbol(symbol)?);
            }
        }
    }
    Ok(Lookup {
        declarations,
        types,
    })
}

/// One fixture/mode group on a fresh checker.
fn group(
    program: &Arc<Program>,
    actions: &[Value],
    generation: &Generation,
    counters: &Counters,
    implementation: Implementation,
) -> Result<Value> {
    if implementation == Implementation::Reference {
        return reference::group(program, actions);
    }
    let live_before = allocator_live();
    let (relation_kind, _) = mode_of(&actions[0]["mode"])?;
    let (setup, setup_interval) = measure(|| -> Result<_> {
        let owner = Arc::new(CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), counters),
            counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )?);
        Ok(owner)
    });
    let owner = setup?;
    let mut op = owner.operation()?;
    let before_lookup = op.relation_state();
    let (looked_up, lookup_interval) = measure(|| lookup(program, &mut op, actions));
    let looked_up = looked_up?;
    let after_lookup = op.relation_state();
    let checker_ns = setup_interval.ns;
    let lookup_ns = lookup_interval.ns;
    let setup_ns = setup_interval.ns + lookup_interval.ns;
    let setup_bytes = setup_interval.requested_bytes + lookup_interval.requested_bytes;
    let (result, interval) = measure(|| -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for action in actions {
            let before = op.relation_state();
            let node = if action["report_errors"] == true {
                Some(
                    *looked_up
                        .declarations
                        .get(text(&action["source"])?.as_bytes())
                        .ok_or("declaration missing")?,
                )
            } else {
                None
            };
            let (result, calls, diagnostic) = op.observe_type_relation(
                looked_up.types[text(&action["source"])?],
                looked_up.types[text(&action["target"])?],
                relation_kind,
                node,
            )?;
            let diagnostics = diagnostic
                .iter()
                .map(|d| diagnostic_payload(program, d))
                .collect::<Result<Vec<_>>>()?;
            rows.push(json!({"action":action,"result":result,"ternary_calls":calls,"before":before,"after":op.relation_state(),"diagnostics":diagnostics}));
        }
        Ok(rows)
    });
    let observations = result?;
    let relation_interval = interval;
    let live_after = allocator_live();
    let live_after_release = {
        drop(op);
        drop(owner);
        allocator_live()
    };
    Ok(json!({
        "mode": actions[0]["mode"],
        "state": "executed",
        "reason": Value::Null,
        "before_lookup": before_lookup,
        "after_lookup": after_lookup,
        "actions": observations,
        "setup_ns": setup_ns, "relation_ns": relation_interval.ns,
        "setup_parts_ns": {"checker": checker_ns, "lookup": lookup_ns, "translate": 0},
        "allocation": {"setup_requested_bytes": setup_bytes, "relation_requested_bytes": relation_interval.requested_bytes,
            "live_before": live_before, "live_after": live_after, "live_after_release": live_after_release},
    }))
}

fn observe(request_bytes: &[u8], implementation: Implementation) -> Result<Value> {
    let request_sha256 = format!("{:x}", Sha256::digest(request_bytes));
    let requests: Value = serde_json::from_slice(request_bytes)?;
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut rows = Vec::new();
    let mut totals = json!({"setup_ns": 0u64, "relation_ns": 0u64, "setup_requested_bytes": 0u64, "relation_requested_bytes": 0u64,
        "retained_bytes": 0u64, "groups_executed": 0u64, "groups_unsupported": 0u64});
    let started = Instant::now();
    for request in array(&requests)? {
        let (program, load_interval) = measure(|| load(request, &counters));
        let program = program?;
        let mut groups = Vec::new();
        let actions = array(&request["actions"])?;
        let mut start = 0;
        while start < actions.len() {
            let mut end = start + 1;
            while end < actions.len() && actions[end]["mode"] == actions[start]["mode"] {
                end += 1;
            }
            let value = group(
                &program,
                &actions[start..end],
                &generation,
                &counters,
                implementation,
            )?;
            let add = |totals: &mut Value, key: &str, delta: u64| {
                totals[key] = json!(totals[key].as_u64().unwrap_or(0) + delta);
            };
            add(
                &mut totals,
                "setup_ns",
                value["setup_ns"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                "relation_ns",
                value["relation_ns"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                "setup_requested_bytes",
                value["allocation"]["setup_requested_bytes"]
                    .as_u64()
                    .unwrap_or(0),
            );
            add(
                &mut totals,
                "relation_requested_bytes",
                value["allocation"]["relation_requested_bytes"]
                    .as_u64()
                    .unwrap_or(0),
            );
            let live_before = i128::from(value["allocation"]["live_before"].as_u64().unwrap_or(0));
            let live_after = i128::from(value["allocation"]["live_after"].as_u64().unwrap_or(0));
            let retained = i128::from(totals["retained_bytes"].as_i64().unwrap_or(0));
            totals["retained_bytes"] = json!(i64::try_from(retained + live_after - live_before)?);
            if value["state"] == "executed" {
                add(&mut totals, "groups_executed", 1);
            } else {
                add(&mut totals, "groups_unsupported", 1);
            }
            groups.push(value);
            start = end;
        }
        rows.push(json!({"id":request["id"],"groups":groups,"state":"executed","load_ns":load_interval.ns}));
    }
    totals["process_ns"] = json!(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    Ok(
        json!({"version":1,"implementation":match implementation { Implementation::Id => "id", Implementation::Reference => "reference" },
        "source_mode":"bound_program",
        "mode":MODE,"request_sha256":request_sha256,"stack_bytes":256usize << 20,"rows":rows,"totals":totals}),
    )
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 || args[3] != "--implementation" {
        return Err(
            "usage: p7_relater requests.json output.json --implementation id|reference".into(),
        );
    }
    let implementation = match args[4].as_str() {
        "id" => Implementation::Id,
        "reference" => Implementation::Reference,
        _ => return Err("implementation must be id or reference".into()),
    };
    let request_bytes = std::fs::read(&args[1])?;
    // The frozen deep fixtures nest 4,096 levels; both implementations recurse
    // on one generously sized worker stack.
    let observed = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || observe(&request_bytes, implementation).map_err(|e| e.to_string()))?
        .join()
        .map_err(|_| "relater worker panicked")??;
    std::fs::write(&args[2], serde_json::to_vec(&observed)?)?;
    println!("{}", serde_json::to_string(&observed["totals"])?);
    Ok(())
}
