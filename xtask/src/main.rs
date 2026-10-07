//! Repository tasks.
//!
//! - `cargo xtask gen [--check | --verify]` writes, or checks, the code
//!   generated from the pinned upstream schemas (`gen`).
//! - `cargo xtask validate` checks the port ledger, `PORTS.toml`, against the
//!   upstream manifest (`data/upstream.json`) and the function inventory
//!   (`data/go-functions.tsv`), and every `port:` marker under `crates/` and
//!   `tools/` against that inventory. It exits 1 on any error.
//! - `cargo xtask status [--out DIR]` renders suite parity, performance runs
//!   and port coverage into `DIR`, by default `target/status` (`status`).
//!   Nothing it writes is tracked.
//!
//! docs/EVIDENCE-plan.md, sections 3, 7 and 8, describes the model.

mod gen;
mod status;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

// ---------------------------------------------------------------- inputs

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    #[serde(default)]
    pin: String,
    #[serde(default)]
    file: Vec<LedgerFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    go: String,
    package: String,
    #[serde(rename = "crate")]
    krate: String,
    phase: i64,
    kind: String,
    status: String,
    #[serde(default)]
    rust: Vec<String>,
    /// The retired verification checks. The field is leaving PORTS.toml; it is
    /// accepted and ignored until the last entry has dropped it.
    #[serde(default, rename = "verify")]
    _verify: serde::de::IgnoredAny,
    pin: String,
    source_hash: String,
    loc: i64,
}

fn read_ledger(root: &Path) -> Ledger {
    let p = root.join("PORTS.toml");
    let text = fs::read_to_string(&p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    toml::from_str(&text).unwrap_or_else(|e| die(&format!("PORTS.toml: {e}")))
}

/// Receiver-qualified inventory IDs, generated from one clean upstream commit.
fn read_inventory(root: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let text = fs::read_to_string(root.join("data/go-functions.tsv")).map_err(|e| e.to_string())?;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines().skip(2) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 7 || cols[6].is_empty() {
            return Err("malformed inventory row".into());
        }
        if !seen.insert(cols[6].to_string()) {
            return Err(format!("duplicate inventory ID {}", cols[6]));
        }
        out.entry(cols[1].to_string())
            .or_default()
            .push(cols[6].to_string());
    }
    if seen.is_empty() {
        return Err("empty function inventory".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- markers

/// Comment markers under `crates/` and `tools/`, sorted and distinct: `port:`
/// markers naming the inventory function a Rust item ports, and the Go tests
/// named by `source: <path>_test.go:<Name>` comments.
#[derive(Default)]
struct Markers {
    ports: Vec<String>,
    tests: Vec<String>,
}

/// The value of a `/// <tag>:`, `//! <tag>:` or `// <tag>:` comment that
/// starts its line.
fn comment_value<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let t = line.trim_start();
    ["/// ", "//! ", "// "]
        .into_iter()
        .find_map(|p| t.strip_prefix(p)?.strip_prefix(tag)?.strip_prefix(':'))
        .map(str::trim)
}

/// The `<path>_test.go:<Name>` identity a `source:` comment names, when it
/// names a Go test.
fn ported_test(value: &str) -> Option<&str> {
    let identity = value.split_whitespace().next()?;
    let (file, name) = identity.split_once(':')?;
    (file.ends_with("_test.go") && !name.is_empty()).then_some(identity)
}

fn scan_markers(root: &Path) -> Markers {
    fn walk(dir: &Path, out: &mut Markers) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let Ok(text) = fs::read_to_string(&p) else {
                    continue;
                };
                for line in text.lines() {
                    if let Some(m) = comment_value(line, "port").filter(|m| m.contains(':')) {
                        out.ports.push(m.to_string());
                    } else if let Some(t) = comment_value(line, "source").and_then(ported_test) {
                        out.tests.push(t.to_string());
                    }
                }
            }
        }
    }
    let mut markers = Markers::default();
    for dir in ["crates", "tools"] {
        walk(&root.join(dir), &mut markers);
    }
    for list in [&mut markers.ports, &mut markers.tests] {
        list.sort();
        list.dedup();
    }
    markers
}

/// The inventory package of an upstream path: its directory below `tsc/`.
fn go_package(file: &str) -> String {
    file.trim_start_matches("tsc/")
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- provenance

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
}
fn sha_file(root: &Path, path: &str) -> Result<String, String> {
    if !safe_path(path) {
        return Err(format!("invalid relative input path: {path}"));
    }
    let p = root.join(path);
    let canonical = p.canonicalize().map_err(|e| format!("{path}: {e}"))?;
    if !canonical.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
        return Err(format!("input escapes repository: {path}"));
    }
    Ok(hash(&read(&p)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamManifest {
    schema_version: u32,
    pin: String,
    ledger_generated_sha256: String,
    inventory_sha256: String,
}

/// The digest of the ledger fields `scripts/ledger-init.py` generates, in the
/// canonical form that script hashes.
fn ledger_generated_hash(root: &Path) -> Result<String, String> {
    let ledger: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("PORTS.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    fn text(value: &toml::Value, field: &str) -> Result<String, String> {
        value
            .get(field)
            .and_then(toml::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("missing/invalid generated field {field}"))
    }
    fn hex(value: &str, n: usize) -> bool {
        value.len() == n
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    let pin = text(&ledger, "pin")?;
    if !hex(&pin, 40) {
        return Err("invalid full ledger pin".into());
    }
    let files = ledger
        .get("file")
        .and_then(toml::Value::as_array)
        .ok_or("missing generated file array")?;
    let mut projected = BTreeMap::new();
    for file in files {
        let mut entry = BTreeMap::<String, serde_json::Value>::new();
        for key in ["go", "package", "crate", "kind", "pin", "source_hash"] {
            let value = text(file, key)?;
            if (key == "pin" && !hex(&value, 40)) || (key == "source_hash" && !hex(&value, 64)) {
                return Err(format!("invalid generated {key}"));
            }
            if key == "kind"
                && !matches!(
                    value.as_str(),
                    "source" | "generated" | "harness" | "out-of-scope"
                )
            {
                return Err("invalid generated kind".into());
            }
            entry.insert(key.into(), value.into());
        }
        for key in ["phase", "loc"] {
            let value = file
                .get(key)
                .and_then(toml::Value::as_integer)
                .filter(|n| *n >= 0)
                .ok_or_else(|| format!("invalid nonnegative generated {key}"))?;
            entry.insert(key.into(), value.into());
        }
        let path = text(file, "go")?;
        if projected.insert(path.clone(), entry).is_some() {
            return Err(format!("duplicate generated source path {path}"));
        }
    }
    // Canonical ordering must not depend on another workspace crate enabling
    // serde_json's preserve_order feature through Cargo feature unification.
    let canonical = BTreeMap::from([
        ("pin", serde_json::Value::String(pin)),
        (
            "file",
            serde_json::to_value(projected.into_values().collect::<Vec<_>>())
                .map_err(|e| e.to_string())?,
        ),
    ]);
    Ok(hash(
        &serde_json::to_vec(&canonical).map_err(|e| e.to_string())?,
    ))
}

/// The ledger, the inventory and the upstream manifest agree on the pin, and
/// the generated ledger fields and the inventory are the ones the manifest
/// hashes.
fn provenance(root: &Path, pin: &str) -> Result<(), String> {
    let m: UpstreamManifest = serde_json::from_slice(&read(&root.join("data/upstream.json"))?)
        .map_err(|e| e.to_string())?;
    if m.schema_version != 2
        || m.pin != pin
        || pin.len() != 40
        || !pin.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("upstream manifest pin/version mismatch".into());
    }
    if ledger_generated_hash(root)? != m.ledger_generated_sha256
        || sha_file(root, "data/go-functions.tsv")? != m.inventory_sha256
    {
        return Err(
            "generated ledger fields/inventory changed: regenerate their upstream manifest".into(),
        );
    }
    let inventory = read(&root.join("data/go-functions.tsv"))?;
    if !inventory.starts_with(format!("# upstream {pin}\n").as_bytes()) {
        return Err("inventory pin does not match ledger".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- coverage

/// Ledger states, in the order the status tables show them.
const STATES: [&str; 4] = ["planned", "in-progress", "ported", "out-of-scope"];

#[derive(Default)]
struct PackageCoverage {
    functions: usize,
    mapped: usize,
    tests: usize,
}

struct Coverage {
    pin: String,
    files: Vec<LedgerFile>,
    errors: Vec<String>,
    functions: usize,
    mapped: usize,
    tests: usize,
    packages: BTreeMap<String, PackageCoverage>,
    /// Counted inventory functions no marker names, sorted, by package.
    unmapped: BTreeMap<String, Vec<String>>,
    unknown_markers: Vec<String>,
}

/// Source and generated files in scope: the files the coverage numbers count.
fn counted(f: &LedgerFile) -> bool {
    matches!(f.kind.as_str(), "source" | "generated") && f.status != "out-of-scope"
}

/// Validate the ledger and the markers, and measure what they map. An invalid
/// entry is reported in `errors` and counted at the state it supports: an
/// unknown status as planned, a ported entry without Rust paths as in progress.
fn coverage(root: &Path) -> Coverage {
    let mut ledger = read_ledger(root);
    let mut errors = Vec::new();
    if let Err(e) = provenance(root, &ledger.pin) {
        errors.push(e);
    }
    for f in &mut ledger.file {
        if !STATES.contains(&f.status.as_str()) {
            errors.push(format!(
                "{}: status must be planned, in-progress, ported or out-of-scope, not {}",
                f.go, f.status
            ));
            f.status = "planned".into();
        }
        if f.source_hash.len() != 64
            || !f.source_hash.bytes().all(|c| c.is_ascii_hexdigit())
            || f.package.is_empty()
        {
            errors.push(format!("{}: invalid source provenance", f.go));
        }
        let rust_exists = !f.rust.is_empty()
            && f.rust
                .iter()
                .all(|p| safe_path(p) && root.join(p).is_file());
        if f.status == "ported" && !rust_exists {
            errors.push(format!("{}: ported entry needs existing Rust paths", f.go));
            f.status = "in-progress".into();
        }
    }

    let inventory = read_inventory(root).unwrap_or_else(|e| {
        errors.push(e);
        BTreeMap::new()
    });
    let markers = scan_markers(root);
    let source_files: HashSet<&str> = ledger
        .file
        .iter()
        .filter(|f| counted(f) && f.kind == "source")
        .map(|f| f.go.as_str())
        .collect();
    // Only functions of counted source files are counted.
    let mut packages: BTreeMap<String, PackageCoverage> = BTreeMap::new();
    let mut package_of: HashMap<&str, &str> = HashMap::new();
    for (pkg, keys) in &inventory {
        for k in keys {
            if source_files.contains(k.split(':').next().unwrap_or("")) {
                package_of.insert(k, pkg);
                packages.entry(pkg.clone()).or_default().functions += 1;
            }
        }
    }
    // A marker is unknown only when it names no inventory function; one that
    // names a function in a harness or other uncounted file is valid but not
    // counted.
    let inventory_keys: HashSet<&str> = inventory.values().flatten().map(String::as_str).collect();
    let mut mapped: HashSet<&str> = HashSet::new();
    let mut unknown_markers = Vec::new();
    for m in &markers.ports {
        if let Some(pkg) = package_of.get(m.as_str()) {
            mapped.insert(m);
            packages.entry((*pkg).to_string()).or_default().mapped += 1;
        } else if !inventory_keys.contains(m.as_str()) {
            unknown_markers.push(m.clone());
        }
    }
    for t in &markers.tests {
        let file = t.split(':').next().unwrap_or("");
        packages.entry(go_package(file)).or_default().tests += 1;
    }
    let mut unmapped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (pkg, keys) in &inventory {
        let mut missing: Vec<String> = keys
            .iter()
            .filter(|k| package_of.contains_key(k.as_str()) && !mapped.contains(k.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            missing.sort();
            unmapped.insert(pkg.clone(), missing);
        }
    }
    Coverage {
        functions: package_of.len(),
        mapped: mapped.len(),
        tests: markers.tests.len(),
        pin: ledger.pin,
        files: ledger.file,
        errors,
        packages,
        unmapped,
        unknown_markers,
    }
}

// ---------------------------------------------------------------- main

fn die(msg: &str) -> ! {
    eprintln!("xtask: {msg}");
    std::process::exit(2)
}

fn repo_root() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Print the validation errors and unknown markers; exit 1 when there are any.
fn report(c: &Coverage) -> ExitCode {
    for e in &c.errors {
        eprintln!("xtask: {e}");
    }
    for marker in &c.unknown_markers {
        eprintln!("unknown port marker: {marker}");
    }
    if c.errors.is_empty() && c.unknown_markers.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

const USAGE: &str = "usage: cargo xtask gen [--check | --verify] | gen lsproto [--check] | gen api [--check] | validate | status [--out DIR]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repo_root();
    match args.first().map(String::as_str) {
        Some("gen") => match gen::run(&root, &args[1..], &read_ledger(&root).pin) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(error) => die(&error),
        },
        Some("validate") if args.len() == 1 => report(&coverage(&root)),
        Some("status") => {
            let out = match &args[1..] {
                [] => root.join("target/status"),
                [flag, dir] if flag == "--out" => PathBuf::from(dir),
                _ => die(USAGE),
            };
            let page = status::Page::read(&root);
            status::write(&out, &page);
            println!("{}", status::summary_line(&page));
            report(&page.coverage)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests;
