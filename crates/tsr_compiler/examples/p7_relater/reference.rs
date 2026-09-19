//! Independent reference setup and observation. The only production inputs
//! are completed AST/binder owners, options and the loader's module decisions.

use super::{allocator_live, json, measure, mode_of, text, Interval, Program, Result, Value};
use s08_relater_prototype::{self as proto, bound_input};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

fn input(program: &Program) -> Result<bound_input::BoundInput> {
    let mut resolutions = Vec::new();
    for resolution in program.resolutions() {
        if !resolution.result.is_resolved() {
            continue;
        }
        let containing = program
            .file(resolution.file.as_bytes())
            .ok_or("module containing file missing")?;
        let resolved = program
            .file(resolution.result.resolved_file_name.as_bytes())
            .ok_or("resolved module file missing")?;
        resolutions.push(bound_input::ModuleResolution {
            containing_source: containing.source(),
            specifier: resolution.name.as_bytes().to_vec(),
            mode: resolution.mode,
            resolved_source: resolved.source(),
        });
    }
    Ok(bound_input::BoundInput::new(
        program
            .files()
            .iter()
            .map(|file| file.bound().clone())
            .collect(),
        bound_input::BoundInputOptions::from(program.options()),
        resolutions,
    )?)
}

fn diagnostic_payload(diagnostic: &proto::Diagnostic) -> Value {
    json!({
        "file": diagnostic.location.file,
        "pos": diagnostic.location.pos,
        "end": diagnostic.location.end,
        "code": diagnostic.message.code,
        "category": diagnostic.message.category as i32,
        "key_hex": super::hex(diagnostic.message.key.as_bytes()),
        "text_hex": "",
        "args": if diagnostic.args.is_empty() { Value::Null } else { json!(diagnostic.args) },
        "chain": diagnostic.chain.iter().map(diagnostic_payload).collect::<Vec<_>>(),
        "related": diagnostic.related.iter().map(diagnostic_payload).collect::<Vec<_>>(),
    })
}

fn state(owner: &proto::BoundChecker) -> Value {
    let checker = owner.checker();
    let mut caches = serde_json::Map::new();
    for (mode, name) in proto::MODES.iter().zip([
        "identity",
        "assignable",
        "subtype",
        "strict_subtype",
        "comparable",
    ]) {
        let relation = checker.relation(*mode);
        caches.insert(
            name.to_owned(),
            json!({"entries":relation.entries(),"result_flags":relation.result_flags()}),
        );
    }
    json!({
        "caches": caches,
        "types_created": checker.graph.len(),
        "signatures_created": owner.signatures_created(),
        "instantiations": owner.instantiations(),
    })
}

struct Roots {
    declarations: BTreeMap<String, ts_arena::NodeId>,
    types: BTreeMap<String, Rc<proto::TypeCell>>,
}

fn lookup(
    owner: &proto::BoundChecker,
    source: ts_arena::NodeId,
    actions: &[Value],
) -> Result<Roots> {
    let mut roots = Roots {
        declarations: BTreeMap::new(),
        types: BTreeMap::new(),
    };
    for action in actions {
        for key in ["source", "target"] {
            let name = text(&action[key])?;
            if !roots.types.contains_key(name) {
                let declaration = owner
                    .input()
                    .declaration_by_name(source, name.as_bytes())?
                    .ok_or("reference declaration missing")?;
                let ty = owner.declared_type(declaration)?;
                roots.declarations.insert(name.to_owned(), declaration);
                roots.types.insert(name.to_owned(), ty);
            }
        }
    }
    Ok(roots)
}

pub(super) fn group(program: &Arc<Program>, actions: &[Value]) -> Result<Value> {
    let live_before = allocator_live();
    let (_, mode) = mode_of(&actions[0]["mode"])?;
    let (setup, setup_interval) =
        measure(|| -> Result<_> { Ok(proto::BoundChecker::new(input(program)?)?) });
    let owner = match setup {
        Ok(owner) => owner,
        Err(error) => {
            // No checker reached its publication point. Keep this group's
            // actual setup work and absence of state, then let the other modes
            // run; an unsupported constructor must not erase the capture.
            let live_after = allocator_live();
            return Ok(json!({
                "mode": actions[0]["mode"],
                "state": "unsupported",
                "reason": format!("bound construction: {error}"),
                "before_lookup": Value::Null, "after_lookup": Value::Null,
                "actions": [],
                "setup_ns": setup_interval.ns, "relation_ns": 0,
                "setup_parts_ns": { "bound_construction": setup_interval.ns, "lookup": 0 },
                "allocation": {
                    "setup_requested_bytes": setup_interval.requested_bytes,
                    "relation_requested_bytes": 0,
                    "live_before": live_before, "live_after": live_after,
                    "live_after_release": live_after,
                },
            }));
        }
    };
    let before_lookup = state(&owner);
    let (roots, lookup_interval) = measure(|| {
        let source = program
            .file(b"/fixture.ts")
            .ok_or("fixture missing")?
            .source();
        lookup(&owner, source, actions)
    });
    let after_lookup = state(&owner);
    // The observer accumulates every checkTypeRelatedTo outcome. Relations run
    // while resolving the looked-up declarations belong to setup; the
    // production probe scopes its calls to one action, so drop them here
    // instead of attributing them to the first action.
    owner.checker().take_observed();
    let mut observations = Vec::new();
    let mut unsupported = None;
    let mut relation_interval = Interval::default();
    let live_after;
    match roots {
        Ok(roots) => {
            let (result, interval) = measure(|| -> Result<()> {
                for action in actions {
                    let before = state(&owner);
                    let source_name = text(&action["source"])?;
                    let target_name = text(&action["target"])?;
                    let location = if action["report_errors"]
                        .as_bool()
                        .ok_or("report_errors must be boolean")?
                    {
                        Some(owner.declaration_location(roots.declarations[source_name])?)
                    } else {
                        None
                    };
                    let checker = owner.checker();
                    let diagnostics_before = checker.structured_diagnostics().len();
                    let outcome = checker.check_type_related_to_at(
                        &roots.types[source_name],
                        &roots.types[target_name],
                        mode,
                        location,
                    );
                    let calls = checker.take_observed();
                    let (ternary, related) = outcome?;
                    let diagnostics = checker.structured_diagnostics()[diagnostics_before..]
                        .iter()
                        .map(diagnostic_payload)
                        .collect::<Vec<_>>();
                    observations.push(json!({
                        "action": action, "result": related, "top_ternary": ternary,
                        "ternary_calls": calls, "before": before, "after": state(&owner),
                        "diagnostics": diagnostics,
                    }));
                }
                Ok(())
            });
            relation_interval = interval;
            if let Err(error) = result {
                unsupported = Some(error.to_string());
            }
            live_after = allocator_live();
            drop(roots);
        }
        Err(error) => {
            unsupported = Some(error.to_string());
            live_after = allocator_live();
        }
    }
    drop(owner);
    let live_after_release = allocator_live();
    Ok(json!({
        "mode": actions[0]["mode"],
        "source_mode": "bound_program",
        "state": if unsupported.is_some() { "unsupported" } else { "executed" },
        "reason": unsupported,
        "before_lookup": before_lookup, "after_lookup": after_lookup,
        "actions": observations,
        "setup_ns": setup_interval.ns + lookup_interval.ns,
        "relation_ns": relation_interval.ns,
        "setup_parts_ns": { "bound_construction": setup_interval.ns, "lookup": lookup_interval.ns },
        "allocation": {
            "setup_requested_bytes": setup_interval.requested_bytes + lookup_interval.requested_bytes,
            "relation_requested_bytes": relation_interval.requested_bytes,
            "live_before": live_before, "live_after": live_after, "live_after_release": live_after_release,
        },
    }))
}
