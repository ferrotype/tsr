//! Phase 1 mutation witnesses: resolves port-marker sites and splices
//! type-directed mutant switches into a scratch workspace copy.
//!
//! ```text
//! phase1_mutation_splicer plan --root <src> --sites <sites.json> --out <plan.json> [--root-tree <sha>]
//! phase1_mutation_splicer splice --root <scratch ws> --plan <plan.json> --switch <switch crate> [--report <file>]
//! ```
//!
//! `scripts/phase1_mutation_plan.py` finds the sites with the scope's marker
//! rules and drives both commands; docs/PHASE1-mutation-witnesses.md is the
//! evidence contract. The committed sources are never edited: `splice` refuses
//! any root that is not a scratch copy carrying the planned span digests.

mod index;
mod json;
mod operators;
mod plan;
mod source;
mod splice;
#[cfg(test)]
mod tests;
mod types;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

fn options(args: &[String], known: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let mut found = BTreeMap::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let name = flag
            .strip_prefix("--")
            .filter(|name| known.contains(name))
            .ok_or_else(|| format!("unknown argument {flag:?}"))?;
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if found.insert(name.to_owned(), value.clone()).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    Ok(found)
}

fn required(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    options
        .get(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("--{name} is required"))
}

fn run(args: &[String]) -> Result<(), String> {
    let Some((command, rest)) = args.split_first() else {
        return Err("usage: phase1_mutation_splicer plan|splice ...".to_owned());
    };
    match command.as_str() {
        "plan" => {
            let options = options(rest, &["root", "sites", "out", "root-tree"])?;
            let root = required(&options, "root")?;
            let sites_path = required(&options, "sites")?;
            let out = required(&options, "out")?;
            let bytes = std::fs::read(&sites_path)
                .map_err(|error| format!("{}: {error}", sites_path.display()))?;
            let document: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", sites_path.display()))?;
            let (sites, unsited) = plan::read_sites(&document)?;
            let planned = plan::plan(
                &root,
                &sites,
                &unsited,
                options.get("root-tree").map(String::as_str),
            )?;
            std::fs::write(&out, json::canonical(&planned) + "\n")
                .map_err(|error| format!("{}: {error}", out.display()))?;
            eprintln!(
                "planned {} mutants; {} unsupported entries",
                planned["counts"]["mutants"],
                planned["unsupported"].as_array().map_or(0, Vec::len)
            );
            Ok(())
        }
        "splice" => {
            let options = options(rest, &["root", "plan", "switch", "report"])?;
            let root = required(&options, "root")?;
            let report_path = options
                .get("report")
                .map_or_else(|| root.join(".phase1-mutation-splice.json"), PathBuf::from);
            let report = splice::splice(
                &root,
                &required(&options, "plan")?,
                &required(&options, "switch")?,
            )?;
            splice::write_report(&report_path, &report)?;
            eprintln!(
                "spliced {} mutants into {} files",
                report["mutants"],
                report["files"].as_array().map_or(0, Vec::len)
            );
            Ok(())
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("phase1_mutation_splicer: {error}");
            ExitCode::FAILURE
        }
    }
}
