//! Repository tasks. `cargo xtask status` computes progress from evidence:
//! the ledger (PORTS.toml), `port:` markers in Rust sources against the Go function
//! inventory, the experiments file, the ADRs, and the sprint files. It writes STATUS.md,
//! status/status.json and docs/status.html, and with --record appends to status/history.jsonl.

mod evidence;
mod gen;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
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

#[derive(Deserialize, Clone)]
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
    #[serde(default)]
    verify: Vec<String>,
    pin: String,
    source_hash: String,
    loc: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Experiment {
    title: String,
    nature: String,
    criteria: Vec<Criterion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Criterion {
    id: String,
    metric: String,
    op: String,
    threshold: serde_json::Value,
    #[serde(default)]
    unit: String,
}
impl Criterion {
    fn check(&self) -> String {
        format!("{} {} {}", self.metric, self.op, self.threshold)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sprint {
    id: String,
    title: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    exit: Vec<String>,
    #[serde(default)]
    item: Vec<SprintItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SprintItem {
    id: String,
    title: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    r#ref: String,
    #[serde(default)]
    done_when: Vec<String>,
    #[serde(default = "required_by_default")]
    required: bool,
}
fn required_by_default() -> bool {
    true
}

// ---------------------------------------------------------------- metrics

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum Metric {
    Num(f64),
    Str(String),
    Bool(bool),
}

type Metrics = BTreeMap<String, Metric>;

fn status_rank(s: &str) -> f64 {
    match s {
        "planned" => 0.0,
        "in-progress" => 1.0,
        "ported" => 2.0,
        "verified" => 3.0,
        _ => -1.0,
    }
}

fn ratio(n: f64, d: f64) -> f64 {
    if d == 0.0 {
        0.0
    } else {
        n / d
    }
}

struct Report {
    generated: String,
    metrics: Metrics,
    ledger_pin: String,
    files_in_scope: Vec<LedgerFile>,
    unported_by_package: BTreeMap<String, Vec<String>>,
    unknown_markers: Vec<String>,
    experiments: Vec<(String, Experiment, Option<bool>)>,
    sprints: Vec<SprintResult>,
    adrs: Vec<(String, String, String)>,
    context: Option<evidence::Context>,
    evidence_states: BTreeMap<String, String>,
    evidence_artifacts: BTreeMap<String, String>,
    errors: Vec<String>,
}

struct SprintResult {
    id: String,
    title: String,
    goal: String,
    done: bool,
    exit: Vec<(String, Option<bool>)>,
    items: Vec<(String, String, Option<bool>)>,
}

// ---------------------------------------------------------------- collection

fn read_ledger(root: &Path) -> Ledger {
    let p = root.join("PORTS.toml");
    let text = fs::read_to_string(&p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    toml::from_str(&text).unwrap_or_else(|e| die(&format!("PORTS.toml: {e}")))
}

fn read_experiments(root: &Path) -> BTreeMap<String, Experiment> {
    let p = root.join("status/experiments.toml");
    match fs::read_to_string(&p) {
        Ok(text) => {
            toml::from_str(&text).unwrap_or_else(|e| die(&format!("experiments.toml: {e}")))
        }
        Err(e) => die(&format!("experiments.toml: {e}")),
    }
}

fn read_sprints(root: &Path) -> Vec<Sprint> {
    let dir = root.join("sprints");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
            .collect();
        paths.sort();
        for p in paths {
            let text = fs::read_to_string(&p).unwrap_or_default();
            match toml::from_str::<Sprint>(&text) {
                Ok(s) => out.push(s),
                Err(e) => die(&format!("{}: {e}", p.display())),
            }
        }
    }
    out
}

/// ADRs: (number, title, status) from docs/adr/NNNN-*.md files.
fn read_adrs(root: &Path) -> Vec<(String, String, String)> {
    let dir = root.join("docs/adr");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if name.len() < 5
                || !name[..4].chars().all(|c| c.is_ascii_digit())
                || !std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            let text = fs::read_to_string(&p).unwrap_or_default();
            let title = text
                .lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l[2..].trim().to_string())
                .unwrap_or(name.clone());
            let status = text
                .lines()
                .find(|l| {
                    l.to_ascii_lowercase().starts_with("status:")
                        || l.to_ascii_lowercase().starts_with("**status:**")
                        || l.to_ascii_lowercase().starts_with("- status:")
                })
                .map(|l| {
                    let s = l
                        .split(':')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .trim_end_matches("**")
                        .trim();
                    s.split([',', '(']).next().unwrap_or("").trim().to_string()
                })
                .unwrap_or_else(|| "Unknown".to_string());
            out.push((name[..4].to_string(), title, status));
        }
    }
    out
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

/// Scan crates/**/*.rs for `port: <file>:<name>` markers.
fn scan_markers(root: &Path) -> Vec<String> {
    let mut markers = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<String>) {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    walk(&p, out);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    if let Ok(text) = fs::read_to_string(&p) {
                        for line in text.lines() {
                            let t = line.trim_start();
                            if let Some(rest) = t
                                .strip_prefix("/// port:")
                                .or_else(|| t.strip_prefix("//! port:"))
                                .or_else(|| t.strip_prefix("// port:"))
                            {
                                let m = rest.trim();
                                if m.contains(':') {
                                    out.push(m.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    walk(&root.join("crates"), &mut markers);
    markers.sort();
    markers.dedup();
    markers
}

// ---------------------------------------------------------------- checks

fn parse_number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Evaluate "<metric> <op> <value>". Returns None when the metric is unknown.
fn eval_check(metrics: &Metrics, check: &str) -> Option<bool> {
    let parts: Vec<&str> = check.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let value = parts[parts.len() - 1];
    let op = parts[parts.len() - 2];
    let key = parts[..parts.len() - 2].join(" ");
    let m = metrics.get(&key)?;
    match m {
        Metric::Bool(b) => match (op, value) {
            ("==", "true") | ("!=", "false") => Some(*b),
            ("==", "false") | ("!=", "true") => Some(!*b),
            _ => None,
        },
        Metric::Num(n) => {
            let v = if let Some(v) = parse_number(value) {
                v
            } else if status_rank(value) >= 0.0 {
                status_rank(value)
            } else {
                return None;
            };
            Some(match op {
                "==" => *n == v,
                "!=" => *n != v,
                ">=" => *n >= v,
                "<=" => *n <= v,
                ">" => *n > v,
                "<" => *n < v,
                _ => return None,
            })
        }
        Metric::Str(s) => {
            // Status strings compare by rank when both sides are statuses; otherwise as strings.
            let (a, b) = (status_rank(s), status_rank(value));
            if a >= 0.0 && b >= 0.0 {
                return Some(match op {
                    "==" => a == b,
                    "!=" => a != b,
                    ">=" => a >= b,
                    "<=" => a <= b,
                    ">" => a > b,
                    "<" => a < b,
                    _ => return None,
                });
            }
            Some(match op {
                "==" => s == value,
                "!=" => s != value,
                _ => return None,
            })
        }
    }
}

// ---------------------------------------------------------------- build report

fn build_report(root: &Path) -> Report {
    build_report_in_context(root, None)
}

// Only rendering metadata is reused; no saved metrics or evidence states are trusted.
#[derive(Deserialize)]
struct ViewMetadata {
    context: evidence::Context,
    generated: String,
}

fn build_report_in_context(root: &Path, archived: Option<&ViewMetadata>) -> Report {
    let mut ledger = read_ledger(root);
    let mut metrics: Metrics = BTreeMap::new();
    let mut errors = Vec::new();
    if let Err(error) = evidence::validate_history(root) {
        errors.push(error);
    }
    let provenance = evidence::provenance(root, &ledger.pin);
    metrics.insert(
        "provenance.valid".into(),
        Metric::Num(if provenance.is_ok() { 1.0 } else { 0.0 }),
    );
    if let Err(e) = provenance {
        errors.push(e);
    }
    let context = match evidence::Context::capture(root, &ledger.pin) {
        Ok(mut c) => {
            if let Some(saved) = archived {
                if c.source_sha256 != saved.context.source_sha256
                    || c.upstream_pin != saved.context.upstream_pin
                    || c.environment.keys().ne(saved.context.environment.keys())
                {
                    errors.push("committed view context does not match source, pin or environment schema; regenerate status".into());
                }
                c.revision.clone_from(&saved.context.revision);
                c.environment.clone_from(&saved.context.environment);
                c.host.clone_from(&saved.context.host);
                c.toolchain.clone_from(&saved.context.toolchain);
            }
            Some(c)
        }
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let mut loaded = evidence::Loaded::default();
    if errors.is_empty() {
        if let Some(ctx) = &context {
            let evidence = if archived.is_some() {
                evidence::load_archived(root, ctx)
            } else {
                evidence::load(root, ctx)
            };
            match evidence {
                Ok(e) => loaded = e,
                Err(e) => errors.push(e),
            }
        }
    }
    metrics.extend(loaded.metrics.clone());
    metrics.insert(
        "upstream.ready".into(),
        Metric::Num(if evidence::upstream_ready(root, &ledger.pin) {
            1.0
        } else {
            0.0
        }),
    );
    for f in &mut ledger.file {
        if !matches!(
            f.status.as_str(),
            "planned" | "in-progress" | "ported" | "out-of-scope"
        ) {
            errors.push(format!(
                "{}: status must be implementation state; verified is computed",
                f.go
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
                .all(|p| evidence::safe_path(p) && root.join(p).is_file());
        if f.status == "ported" && !rust_exists {
            errors.push(format!("{}: ported entry needs existing Rust paths", f.go));
            f.status = "in-progress".into();
        }
        if f.status == "ported"
            && f.pin == ledger.pin
            && rust_exists
            && !f.verify.is_empty()
            && f.verify
                .iter()
                .all(|check| check.starts_with("run.") && eval_check(&metrics, check) == Some(true))
        {
            f.status = "verified".into();
        }
    }

    let in_scope: Vec<LedgerFile> = ledger
        .file
        .iter()
        .filter(|f| f.kind != "out-of-scope" && f.status != "out-of-scope")
        .cloned()
        .collect();
    let counted: Vec<&LedgerFile> = in_scope
        .iter()
        .filter(|f| f.kind == "source" || f.kind == "generated")
        .collect();
    let total = counted.len() as f64;
    let ported = counted
        .iter()
        .filter(|f| status_rank(&f.status) >= 2.0)
        .count() as f64;
    let verified = counted.iter().filter(|f| f.status == "verified").count() as f64;
    let in_progress = counted.iter().filter(|f| f.status == "in-progress").count() as f64;
    let loc_total: f64 = counted.iter().map(|f| f.loc as f64).fold(0.0, |a, x| a + x);
    let loc_verified: f64 = counted
        .iter()
        .filter(|f| f.status == "verified")
        .map(|f| f.loc as f64)
        .fold(0.0, |a, x| a + x);
    let loc_ported: f64 = counted
        .iter()
        .filter(|f| status_rank(&f.status) >= 2.0)
        .map(|f| f.loc as f64)
        .fold(0.0, |a, x| a + x);
    let stale = counted
        .iter()
        .filter(|f| {
            !f.pin.is_empty()
                && !ledger.pin.is_empty()
                && f.pin != ledger.pin
                && status_rank(&f.status) >= 2.0
        })
        .count() as f64;
    metrics.insert("ledger.files_total".into(), Metric::Num(total));
    metrics.insert("ledger.files_ported".into(), Metric::Num(ported));
    metrics.insert("ledger.files_verified".into(), Metric::Num(verified));
    metrics.insert("ledger.files_in_progress".into(), Metric::Num(in_progress));
    metrics.insert("ledger.files_stale".into(), Metric::Num(stale));
    metrics.insert("ledger.loc_total".into(), Metric::Num(loc_total));
    metrics.insert(
        "ledger.loc_ported_ratio".into(),
        Metric::Num(ratio(loc_ported, loc_total)),
    );
    metrics.insert(
        "ledger.loc_verified_ratio".into(),
        Metric::Num(ratio(loc_verified, loc_total)),
    );

    let mut by_phase: BTreeMap<i64, (f64, f64, f64)> = BTreeMap::new();
    let mut by_crate: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();
    for f in &counted {
        let e = by_phase.entry(f.phase).or_insert((0.0, 0.0, 0.0));
        e.0 += f.loc as f64;
        if f.status == "verified" {
            e.1 += f.loc as f64;
        }
        e.2 += 1.0;
        let c = by_crate.entry(f.krate.clone()).or_insert((0.0, 0.0, 0.0));
        c.0 += f.loc as f64;
        if f.status == "verified" {
            c.1 += f.loc as f64;
        }
        c.2 += 1.0;
    }
    for (ph, (t, v, n)) in &by_phase {
        metrics.insert(format!("ledger.phase[{ph}].loc_total"), Metric::Num(*t));
        metrics.insert(format!("ledger.phase[{ph}].files"), Metric::Num(*n));
        metrics.insert(
            format!("ledger.phase[{ph}].loc_verified_ratio"),
            Metric::Num(ratio(*v, *t)),
        );
    }
    for (c, (t, v, n)) in &by_crate {
        metrics.insert(format!("ledger.crate[{c}].loc_total"), Metric::Num(*t));
        metrics.insert(format!("ledger.crate[{c}].files"), Metric::Num(*n));
        metrics.insert(
            format!("ledger.crate[{c}].loc_verified_ratio"),
            Metric::Num(ratio(*v, *t)),
        );
    }
    for f in &ledger.file {
        metrics.insert(
            format!("file[{}].status", f.go),
            Metric::Str(f.status.clone()),
        );
    }

    // Function-level traceability.
    let inventory = match read_inventory(root) {
        Ok(i) => i,
        Err(e) => {
            errors.push(e);
            BTreeMap::new()
        }
    };
    let markers = scan_markers(root);
    let source_files: std::collections::HashSet<&str> = counted
        .iter()
        .filter(|f| f.kind == "source")
        .map(|f| f.go.as_str())
        .collect();
    let mut all_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut fn_total = 0.0;
    let mut per_pkg_total: BTreeMap<String, f64> = BTreeMap::new();
    for (pkg, keys) in &inventory {
        for k in keys {
            let file = k.split(':').next().unwrap_or("");
            if source_files.contains(file) {
                all_keys.insert(k.clone());
                fn_total += 1.0;
                *per_pkg_total.entry(pkg.clone()).or_insert(0.0) += 1.0;
            }
        }
    }
    let mut fn_ported = 0.0;
    let mut per_pkg_ported: BTreeMap<String, f64> = BTreeMap::new();
    let mut ported_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unknown = Vec::new();
    for m in &markers {
        if all_keys.contains(m) {
            if ported_keys.insert(m.clone()) {
                fn_ported += 1.0;
                let file = m.split(':').next().unwrap_or("");
                let pkg = file
                    .trim_start_matches("tsc/")
                    .rsplit_once('/')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_default();
                *per_pkg_ported.entry(pkg).or_insert(0.0) += 1.0;
            }
        } else {
            unknown.push(m.clone());
        }
    }
    metrics.insert("functions.total".into(), Metric::Num(fn_total));
    metrics.insert("functions.ported".into(), Metric::Num(fn_ported));
    metrics.insert(
        "functions.ratio".into(),
        Metric::Num(ratio(fn_ported, fn_total)),
    );
    let mut unported_by_package: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (pkg, keys) in &inventory {
        let t = per_pkg_total.get(pkg).copied().unwrap_or(0.0);
        if t == 0.0 {
            continue;
        }
        let p = per_pkg_ported.get(pkg).copied().unwrap_or(0.0);
        metrics.insert(format!("functions.package[{pkg}].total"), Metric::Num(t));
        metrics.insert(format!("functions.package[{pkg}].ported"), Metric::Num(p));
        metrics.insert(
            format!("functions.package[{pkg}].ratio"),
            Metric::Num(ratio(p, t)),
        );
        let missing: Vec<String> = keys
            .iter()
            .filter(|k| all_keys.contains(*k) && !ported_keys.contains(*k))
            .cloned()
            .collect();
        if !missing.is_empty() {
            unported_by_package.insert(pkg.clone(), missing);
        }
    }

    // Every typed requirement participates; notes never supply passing evidence.
    let mut experiments = Vec::new();
    for (id, e) in read_experiments(root) {
        let mut results = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        if e.criteria.is_empty() {
            errors.push(format!("{id}: experiment has no criteria"));
        }
        for c in &e.criteria {
            if !ids.insert(&c.id)
                || !(c.threshold.is_boolean() || c.threshold.as_f64().is_some_and(f64::is_finite))
                || (c.threshold.is_boolean() && !matches!(c.op.as_str(), "==" | "!="))
                || !c.metric.starts_with("run.")
                || !matches!(c.op.as_str(), "==" | "!=" | ">=" | "<=" | ">" | "<")
            {
                errors.push(format!("{id}.{}: invalid criterion", c.id));
                results.push(Some(false));
                continue;
            }
            let result = eval_check(&metrics, &c.check());
            metrics.insert(
                format!("exp.{id}.{}.pass", c.id),
                Metric::Num(if result == Some(true) { 1.0 } else { 0.0 }),
            );
            results.push(result);
        }
        let pass = if results.contains(&Some(false)) {
            Some(false)
        } else if !results.is_empty() && results.iter().all(|r| *r == Some(true)) {
            Some(true)
        } else {
            None
        };
        metrics.insert(
            format!("exp.{id}.pass"),
            Metric::Num(if pass == Some(true) { 1.0 } else { 0.0 }),
        );
        experiments.push((id, e, pass));
    }
    let exp_pass = experiments
        .iter()
        .filter(|(_, _, p)| *p == Some(true))
        .count() as f64;
    metrics.insert("exp.passed".into(), Metric::Num(exp_pass));
    metrics.insert("exp.total".into(), Metric::Num(experiments.len() as f64));

    // ADRs.
    let adrs = read_adrs(root);
    for (n, _, st) in &adrs {
        metrics.insert(format!("adr.{n}.status"), Metric::Str(st.clone()));
    }
    metrics.insert("adr.total".into(), Metric::Num(adrs.len() as f64));
    metrics.insert(
        "adr.accepted".into(),
        Metric::Num(
            adrs.iter()
                .filter(|(_, _, s)| s == "Accepted" || s == "Amended")
                .count() as f64,
        ),
    );

    // Sprints (evaluated after all other metrics exist).
    let mut sprints = Vec::new();
    let mut sprint_ids = std::collections::BTreeSet::new();
    for s in read_sprints(root) {
        if !sprint_ids.insert(s.id.clone()) {
            errors.push(format!("duplicate sprint ID {}", s.id));
        }
        let mut item_ids = std::collections::BTreeSet::new();
        for it in &s.item {
            if !item_ids.insert(&it.id) {
                errors.push(format!("{}: duplicate item ID {}", s.id, it.id));
            }
        }
        let exit: Vec<(String, Option<bool>)> = s
            .exit
            .iter()
            .map(|c| (c.clone(), eval_check(&metrics, c)))
            .collect();
        let mut done = !exit.is_empty() && exit.iter().all(|(_, r)| *r == Some(true));
        let items: Vec<(String, String, Option<bool>)> = s
            .item
            .iter()
            .map(|it| {
                let r = if it.done_when.is_empty() {
                    None
                } else {
                    let rs: Vec<Option<bool>> = it
                        .done_when
                        .iter()
                        .map(|c| eval_check(&metrics, c))
                        .collect();
                    Some(rs.iter().all(|r| *r == Some(true)))
                };
                if it.required && r != Some(true) {
                    done = false;
                }
                let label = if it.r#ref.is_empty() {
                    it.title.clone()
                } else {
                    format!("{} ({} {})", it.title, it.kind, it.r#ref)
                };
                (
                    it.id.clone(),
                    format!("{label}{}", if it.required { "" } else { " [optional]" }),
                    r,
                )
            })
            .collect();
        done &= errors.is_empty() && unknown.is_empty();
        metrics.insert(
            format!("sprint.{}.done", s.id),
            Metric::Num(if done { 1.0 } else { 0.0 }),
        );
        sprints.push(SprintResult {
            id: s.id,
            title: s.title,
            goal: s.goal,
            done,
            exit,
            items,
        });
    }

    if !errors.is_empty() || !unknown.is_empty() {
        for sprint in &mut sprints {
            sprint.done = false;
            metrics.insert(format!("sprint.{}.done", sprint.id), Metric::Num(0.0));
        }
    }
    Report {
        generated: archived.map_or_else(today, |saved| saved.generated.clone()),
        metrics,
        ledger_pin: ledger.pin,
        files_in_scope: in_scope,
        unported_by_package,
        unknown_markers: unknown,
        experiments,
        sprints,
        adrs,
        context,
        evidence_states: loaded.states,
        evidence_artifacts: loaded.artifacts,
        errors,
    }
}

fn metric_text(metric: Option<&Metric>) -> String {
    match metric {
        Some(Metric::Num(n)) => n.to_string(),
        Some(Metric::Bool(b)) => b.to_string(),
        Some(Metric::Str(s)) => s.clone(),
        None => "missing".into(),
    }
}
fn pass_text(result: Option<bool>) -> &'static str {
    match result {
        Some(true) => "pass",
        Some(false) => "fail",
        None => "pending",
    }
}
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------- outputs

fn num(m: &Metrics, k: &str) -> f64 {
    match m.get(k) {
        Some(Metric::Num(n)) => *n,
        _ => 0.0,
    }
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn today() -> String {
    // Civil date from the Unix epoch (Howard Hinnant's algorithm), UTC.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn render_status_md(r: &Report) -> String {
    let m = &r.metrics;
    let mut s = String::new();
    s.push_str("# Status\n\n");
    s.push_str(&format!("Generated by `cargo xtask status` on {} against upstream pin `{}`. Do not edit. Implementation state is declared in the ledger; verification is derived from current evidence. Function counts measure mapping, not semantic completeness.\n\n", r.generated, r.ledger_pin));
    s.push_str("## Summary\n\n| | |\n|---|---|\n");
    s.push_str(&format!(
        "| Files in scope (source and generated) | {} |\n",
        num(m, "ledger.files_total")
    ));
    s.push_str(&format!(
        "| Files ported / verified | {} / {} |\n",
        num(m, "ledger.files_ported"),
        num(m, "ledger.files_verified")
    ));
    s.push_str(&format!(
        "| Lines verified | {} of {} ({}) |\n",
        num(m, "ledger.loc_total") * num(m, "ledger.loc_verified_ratio"),
        num(m, "ledger.loc_total"),
        pct(num(m, "ledger.loc_verified_ratio"))
    ));
    s.push_str(&format!(
        "| Mapped upstream functions | {} of {} ({}) |\n",
        num(m, "functions.ported"),
        num(m, "functions.total"),
        pct(num(m, "functions.ratio"))
    ));
    s.push_str(&format!(
        "| Experiments passed | {} of {} |\n",
        num(m, "exp.passed"),
        num(m, "exp.total")
    ));
    s.push_str(&format!(
        "| ADRs accepted | {} of {} |\n",
        num(m, "adr.accepted"),
        num(m, "adr.total")
    ));
    s.push_str(&format!(
        "| Entries stale against the pin | {} |\n\n",
        num(m, "ledger.files_stale")
    ));

    s.push_str("## By phase\n\n| Phase | Files | Lines | Verified |\n|---|---:|---:|---:|\n");
    let mut phases: Vec<i64> = r
        .files_in_scope
        .iter()
        .filter(|f| f.kind == "source" || f.kind == "generated")
        .map(|f| f.phase)
        .collect();
    phases.sort_unstable();
    phases.dedup();
    for ph in phases {
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            ph,
            num(m, &format!("ledger.phase[{ph}].files")),
            num(m, &format!("ledger.phase[{ph}].loc_total")),
            pct(num(m, &format!("ledger.phase[{ph}].loc_verified_ratio")))
        ));
    }

    s.push_str("\n## By crate\n\n| Crate | Files | Lines | Verified |\n|---|---:|---:|---:|\n");
    let mut crates: Vec<String> = r
        .files_in_scope
        .iter()
        .filter(|f| f.kind == "source" || f.kind == "generated")
        .map(|f| f.krate.clone())
        .collect();
    crates.sort();
    crates.dedup();
    for c in crates {
        s.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            c,
            num(m, &format!("ledger.crate[{c}].files")),
            num(m, &format!("ledger.crate[{c}].loc_total")),
            pct(num(m, &format!("ledger.crate[{c}].loc_verified_ratio")))
        ));
    }

    s.push_str("\n## Function traceability by package\n\n[Canonical unmapped-function worklist](status/unmapped-functions.json).\n\n| Package | Functions | Mapped |\n|---|---:|---:|\n");
    let mut pkgs: Vec<String> = m
        .keys()
        .filter_map(|k| {
            k.strip_prefix("functions.package[")
                .and_then(|k| k.split(']').next())
                .map(ToString::to_string)
        })
        .collect();
    pkgs.sort();
    pkgs.dedup();
    for p in pkgs {
        s.push_str(&format!(
            "| `{}` | {} | {} |\n",
            p,
            num(m, &format!("functions.package[{p}].total")),
            pct(num(m, &format!("functions.package[{p}].ratio")))
        ));
    }
    if !r.unknown_markers.is_empty() {
        s.push_str(
            "\n**Unknown markers** (name no upstream function; stale after a pin bump?):\n\n",
        );
        for u in &r.unknown_markers {
            s.push_str(&format!("- `{u}`\n"));
        }
    }

    s.push_str("\n## Experiments\n\nEvery criterion is required. Missing or stale evidence leaves the experiment pending.\n\n| Experiment | Criterion | Required | Current | Pass |\n|---|---|---|---|---|\n");
    for (id, e, _) in &r.experiments {
        for c in &e.criteria {
            let result = eval_check(m, &c.check());
            s.push_str(&format!(
                "| {}: {} | {} | `{}` {} | {} | {} |\n",
                id,
                e.title,
                c.id,
                c.check(),
                c.unit,
                metric_text(m.get(&c.metric)),
                pass_text(result)
            ));
        }
    }
    for (id, e, _) in &r.experiments {
        s.push_str(&format!("\n{}: {}.\n", id, e.nature));
    }
    s.push_str("\n## Evidence\n\n| Run | State | Artifact |\n|---|---|---|\n");
    for (id, state) in &r.evidence_states {
        let link = r
            .evidence_artifacts
            .get(id)
            .map(|p| format!("[result]({p})"))
            .unwrap_or_else(|| "—".into());
        s.push_str(&format!("| {id} | {state} | {link} |\n"));
    }
    if !r.errors.is_empty() {
        s.push_str("\n## Invalid tracking inputs\n\n");
        for error in &r.errors {
            s.push_str(&format!("- {error}\n"));
        }
    }

    s.push_str("\n## Sprints\n\n");
    for sp in &r.sprints {
        s.push_str(&format!(
            "### {} {} {}\n\n{}\n\n",
            sp.id,
            sp.title,
            if sp.done { "(done)" } else { "(open)" },
            sp.goal
        ));
        s.push_str("Exit checks:\n\n");
        for (c, res) in &sp.exit {
            s.push_str(&format!(
                "- [{}] `{}`{}\n",
                if *res == Some(true) { "x" } else { " " },
                c,
                if res.is_none() {
                    " (unknown metric)"
                } else {
                    ""
                }
            ));
        }
        s.push_str("\nItems:\n\n");
        for (id, label, res) in &sp.items {
            let mark = match res {
                Some(true) => "x",
                Some(false) => " ",
                None => "-",
            };
            s.push_str(&format!("- [{}] {} {}\n", mark, id, label));
        }
        s.push('\n');
    }

    s.push_str("## Decisions\n\n| ADR | Title | Status |\n|---|---|---|\n");
    for (n, t, st) in &r.adrs {
        s.push_str(&format!("| {} | {} | {} |\n", n, t, st));
    }
    s
}

fn write_status_md(root: &Path, r: &Report) {
    fs::write(root.join("STATUS.md"), render_status_md(r))
        .unwrap_or_else(|e| die(&format!("STATUS.md: {e}")));
}

fn render_status_json(r: &Report) -> (String, Vec<u8>) {
    const WORKLIST: &str = "status/unmapped-functions.json";
    let mut unmapped = r.unported_by_package.clone();
    for functions in unmapped.values_mut() {
        functions.sort();
    }
    let worklist = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "pin": r.ledger_pin,
        "unmapped_functions": unmapped,
    }))
    .unwrap();

    let mut obj = serde_json::Map::new();
    obj.insert("schema_version".into(), serde_json::json!(2));
    obj.insert("context".into(), serde_json::json!(r.context));
    obj.insert(
        "evidence_states".into(),
        serde_json::json!(r.evidence_states),
    );
    obj.insert(
        "evidence_artifacts".into(),
        serde_json::json!(r.evidence_artifacts),
    );
    obj.insert("errors".into(), serde_json::json!(r.errors));
    obj.insert(
        "generated".into(),
        serde_json::Value::String(r.generated.clone()),
    );
    obj.insert(
        "pin".into(),
        serde_json::Value::String(r.ledger_pin.clone()),
    );
    let mut metrics = serde_json::Map::new();
    for (k, v) in &r.metrics {
        metrics.insert(k.clone(), serde_json::to_value(v).unwrap());
    }
    obj.insert("metrics".into(), serde_json::Value::Object(metrics));
    let unported: serde_json::Map<String, serde_json::Value> = r
        .unported_by_package
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::json!(v.len())))
        .collect();
    obj.insert(
        "unported_functions_by_package".into(),
        serde_json::Value::Object(unported),
    );
    obj.insert(
        "unmapped_functions_file".into(),
        serde_json::json!(WORKLIST),
    );
    obj.insert(
        "unmapped_functions_sha256".into(),
        serde_json::json!(evidence::hash(&worklist)),
    );
    (
        serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap(),
        worklist,
    )
}

fn write_status_json(root: &Path, r: &Report) {
    let (summary, worklist) = render_status_json(r);
    fs::create_dir_all(root.join("status"))
        .unwrap_or_else(|e| die(&format!("status directory: {e}")));
    fs::write(root.join("status/unmapped-functions.json"), worklist)
        .unwrap_or_else(|e| die(&format!("unmapped-functions.json: {e}")));
    fs::write(root.join("status/status.json"), summary)
        .unwrap_or_else(|e| die(&format!("status.json: {e}")));
}

fn history_identity(value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"source": value["context"]["source_sha256"], "pin": value["context"]["upstream_pin"], "environment": value["context"]["environment"], "evidence": value["evidence_artifacts"], "metrics": value["metrics_sha256"]})
}
fn record_history(root: &Path, r: &Report) {
    let m = &r.metrics;
    let line = serde_json::json!({
        "schema_version": 2,
        "context": r.context,
        "evidence_artifacts": r.evidence_artifacts,
        "metrics_sha256": evidence::hash(&serde_json::to_vec(&r.metrics).unwrap()),
        "date": today(),
        "pin": r.ledger_pin,
        "loc_verified_ratio": num(m, "ledger.loc_verified_ratio"),
        "loc_ported_ratio": num(m, "ledger.loc_ported_ratio"),
        "functions_ratio": num(m, "functions.ratio"),
        "files_verified": num(m, "ledger.files_verified"),
        "exp_passed": num(m, "exp.passed"),
    });
    let p = root.join("status/history.jsonl");
    let mut text = fs::read_to_string(&p).unwrap_or_default();
    let identity = history_identity(&line);
    if text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| history_identity(&v) == identity)
    {
        return;
    }
    text.push_str(&line.to_string());
    text.push('\n');
    fs::write(&p, text).unwrap_or_else(|e| die(&format!("history.jsonl: {e}")));
}

fn render_dashboard(root: &Path, r: &Report) -> String {
    let m = &r.metrics;
    let history = fs::read_to_string(root.join("status/history.jsonl")).unwrap_or_default();
    let points: Vec<(String, f64, f64)> = history
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["pin"].as_str() == Some(r.ledger_pin.as_str()) && v["schema_version"] == 2)
        .map(|v| {
            (
                v["date"].as_str().unwrap_or("").to_string(),
                v["loc_verified_ratio"].as_f64().unwrap_or(0.0),
                v["functions_ratio"].as_f64().unwrap_or(0.0),
            )
        })
        .collect();

    let chart = {
        let w = 720.0;
        let h = 220.0;
        let n = points.len().max(2) as f64;
        let mut poly_loc = String::new();
        let mut poly_fn = String::new();
        for (i, (_, loc, f)) in points.iter().enumerate() {
            let x = 40.0 + (w - 60.0) * (i as f64) / (n - 1.0);
            poly_loc.push_str(&format!("{:.1},{:.1} ", x, 10.0 + (h - 40.0) * (1.0 - loc)));
            poly_fn.push_str(&format!("{:.1},{:.1} ", x, 10.0 + (h - 40.0) * (1.0 - f)));
        }
        let first = points.first().map(|p| p.0.clone()).unwrap_or_default();
        let last = points.last().map(|p| p.0.clone()).unwrap_or_default();
        format!(
            r#"<svg viewBox="0 0 {w} {h}" width="100%" role="img" aria-label="Verified lines and mapped functions over time">
  <line x1="40" y1="10" x2="40" y2="{y0}" stroke="var(--line)"/><line x1="40" y1="{y0}" x2="{x1}" y2="{y0}" stroke="var(--line)"/>
  <text x="4" y="14" class="ax">100%</text><text x="4" y="{ymid}" class="ax">50%</text><text x="12" y="{y0}" class="ax">0%</text>
  <text x="40" y="{yl}" class="ax">{first}</text><text x="{x1}" y="{yl}" class="ax" text-anchor="end">{last}</text>
  <polyline fill="none" stroke="var(--accent)" stroke-width="2" points="{poly_loc}"/>
  <polyline fill="none" stroke="var(--rust)" stroke-width="2" stroke-dasharray="4 3" points="{poly_fn}"/>
</svg>
<div class="legend"><span class="acc">verified lines</span><span class="rust">mapped functions</span></div>"#,
            w = w,
            h = h,
            y0 = h - 30.0,
            ymid = 10.0 + (h - 40.0) / 2.0,
            yl = h - 12.0,
            x1 = w - 20.0
        )
    };

    let mut phase_rows = String::new();
    let mut phases: Vec<i64> = r
        .files_in_scope
        .iter()
        .filter(|f| f.kind == "source" || f.kind == "generated")
        .map(|f| f.phase)
        .collect();
    phases.sort_unstable();
    phases.dedup();
    for ph in phases {
        let v = num(m, &format!("ledger.phase[{ph}].loc_verified_ratio"));
        phase_rows.push_str(&format!(
            "<tr><td>Phase {ph}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td><div class=\"bar\"><div style=\"width:{:.1}%\"></div></div></td><td class=\"num\">{}</td></tr>\n",
            num(m, &format!("ledger.phase[{ph}].files")),
            num(m, &format!("ledger.phase[{ph}].loc_total")),
            v * 100.0,
            pct(v)
        ));
    }
    let mut exp_rows = String::new();
    for (id, e, pass) in &r.experiments {
        let cls = match pass {
            Some(true) => "ok",
            Some(false) => "crit",
            None => "warn",
        };
        let criteria = e
            .criteria
            .iter()
            .map(|c| escape_html(&c.check()))
            .collect::<Vec<_>>()
            .join("<br>");
        let measurements = e
            .criteria
            .iter()
            .map(|c| format!("{}: {}", escape_html(&c.id), metric_text(m.get(&c.metric))))
            .collect::<Vec<_>>()
            .join("<br>");
        exp_rows.push_str(&format!("<tr><td>{id}</td><td>{}</td><td>{criteria}</td><td>{measurements}</td><td><span class=\"chip {cls}\">{}</span></td></tr>\n", escape_html(&e.title), pass_text(*pass)));
    }
    let mut sprint_rows = String::new();
    for sp in &r.sprints {
        let passed = sp.exit.iter().filter(|(_, r)| *r == Some(true)).count();
        sprint_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"num\">{} / {}</td><td><span class=\"chip {}\">{}</span></td></tr>\n",
            sp.id,
            sp.title,
            passed,
            sp.exit.len(),
            if sp.done { "ok" } else { "warn" },
            if sp.done { "done" } else { "open" }
        ));
    }

    let evidence_rows = r
        .evidence_states
        .iter()
        .map(|(id, state)| {
            let artifact = r
                .evidence_artifacts
                .get(id)
                .map(|p| format!("<a href=\"../{}\">result</a>", escape_html(p)))
                .unwrap_or_else(|| "—".into());
            format!(
                "<tr><td>{}</td><td>{}</td><td>{artifact}</td></tr>",
                escape_html(id),
                escape_html(state)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let errors_html = r
        .errors
        .iter()
        .map(|e| format!("<p class=\"crit\">{}</p>", escape_html(e)))
        .collect::<Vec<_>>()
        .join("\n");
    let html = format!(
        r#"<title>Corsa in Rust Status</title>
<style>
  :root {{ --bg:#f6f7f9; --surface:#fff; --ink:#1a2029; --ink-2:#4b5665; --line:#d7dde6; --accent:#245fa6; --rust:#b7410e; --ok:#2f6f3e; --ok-soft:#e1f2e5; --warn:#8a5a00; --warn-soft:#fbefd2; --crit:#b42318; --crit-soft:#fde8e6; color-scheme: light dark; }}
  @media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{ --bg:#101418; --surface:#161b21; --ink:#e4e8ee; --ink-2:#a6b0be; --line:#2a323c; --accent:#7ab4f5; --rust:#e8875a; --ok:#7cc58f; --ok-soft:#1a3322; --warn:#e2b93b; --warn-soft:#3a2f12; --crit:#f08a80; --crit-soft:#3d1d1a; }} }}
  :root[data-theme="dark"] {{ --bg:#101418; --surface:#161b21; --ink:#e4e8ee; --ink-2:#a6b0be; --line:#2a323c; --accent:#7ab4f5; --rust:#e8875a; --ok:#7cc58f; --ok-soft:#1a3322; --warn:#e2b93b; --warn-soft:#3a2f12; --crit:#f08a80; --crit-soft:#3d1d1a; }}
  body {{ margin:0; background:var(--bg); color:var(--ink); font:400 15px/1.5 "IBM Plex Sans", system-ui, sans-serif; }}
  .page {{ max-width:1000px; margin:0 auto; padding:32px 24px 60px; }}
  h1 {{ font:600 30px/1.2 "IBM Plex Serif", Georgia, serif; margin:0 0 6px; }}
  h2 {{ font:600 20px/1.3 "IBM Plex Serif", Georgia, serif; margin:28px 0 10px; }}
  .eyebrow {{ font-size:12px; letter-spacing:.08em; text-transform:uppercase; color:var(--ink-2); }}
  .tiles {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(180px,1fr)); gap:10px; margin:18px 0; }}
  .tile {{ background:var(--surface); border:1px solid var(--line); border-radius:4px; padding:12px 14px; }}
  .tile b {{ display:block; font:500 24px/1.1 "JetBrains Mono", ui-monospace, monospace; margin-bottom:4px; }}
  .tile span {{ color:var(--ink-2); font-size:13px; }}
  table {{ border-collapse:collapse; width:100%; font-size:14px; }}
  th, td {{ text-align:left; padding:7px 10px 7px 0; border-bottom:1px solid var(--line); vertical-align:top; }}
  th {{ font-size:12px; letter-spacing:.05em; text-transform:uppercase; color:var(--ink-2); }}
  td.num {{ text-align:right; font-variant-numeric:tabular-nums; white-space:nowrap; padding-right:16px; }}
  .bar {{ background:var(--line); height:8px; border-radius:2px; min-width:160px; }}
  .bar div {{ background:var(--accent); height:8px; border-radius:2px; }}
  .chip {{ display:inline-block; font-size:11px; letter-spacing:.04em; text-transform:uppercase; padding:3px 6px; border-radius:3px; }}
  .chip.ok {{ background:var(--ok-soft); color:var(--ok); }} .chip.warn {{ background:var(--warn-soft); color:var(--warn); }} .chip.crit {{ background:var(--crit-soft); color:var(--crit); }}
  .ax {{ font:11px ui-monospace, monospace; fill:var(--ink-2); }}
  .legend {{ font-size:13px; color:var(--ink-2); display:flex; gap:16px; }}
  .legend span::before {{ content:""; display:inline-block; width:18px; height:3px; margin-right:6px; vertical-align:middle; background:var(--accent); }}
  .legend .rust::before {{ background:var(--rust); }}
  .chart {{ background:var(--surface); border:1px solid var(--line); border-radius:4px; padding:12px; }}
</style>
<div class="page">
  <div class="eyebrow">Corsa in Rust &middot; generated {date} &middot; upstream pin {pin}</div>
  <h1>Status</h1>
  <p>Function mappings measure traceability. Baseline parity and required gates establish behavior. <a href="../STATUS.md">Detailed report</a> · <a href="TRACKING.md">Tracking contract</a> · <a href="../status/unmapped-functions.json">Unmapped-function worklist</a></p>
{errors_html}
  <div class="tiles">
    <div class="tile"><b>{loc_pct}</b><span>lines verified, of {loc_total}</span></div>
    <div class="tile"><b>{files_verified} / {files_total}</b><span>files verified</span></div>
    <div class="tile"><b>{fn_pct}</b><span>functions with a Rust counterpart, of {fn_total}</span></div>
    <div class="tile"><b>{exp_passed} / {exp_total}</b><span>experiments passed</span></div>
    <div class="tile"><b>{adr_acc} / {adr_total}</b><span>decisions accepted</span></div>
  </div>
  <h2>Over time</h2>
  <div class="chart">{chart}</div>
  <h2>By phase</h2>
  <table><thead><tr><th>Phase</th><th class="num">Files</th><th class="num">Lines</th><th>Verified</th><th class="num"></th></tr></thead><tbody>
{phase_rows}</tbody></table>
  <h2>Experiments</h2>
  <table><thead><tr><th></th><th>Title</th><th>Threshold</th><th>Measured</th><th>Result</th></tr></thead><tbody>
{exp_rows}</tbody></table>
  <h2>Evidence</h2>
  <table><thead><tr><th>Run</th><th>State</th><th>Artifact</th></tr></thead><tbody>{evidence_rows}</tbody></table>
  <h2>Sprints</h2>
  <table><thead><tr><th>Sprint</th><th>Title</th><th class="num">Exit checks</th><th>State</th></tr></thead><tbody>
{sprint_rows}</tbody></table>
</div>
"#,
        date = r.generated,
        pin = r.ledger_pin,
        loc_pct = pct(num(m, "ledger.loc_verified_ratio")),
        loc_total = num(m, "ledger.loc_total"),
        files_verified = num(m, "ledger.files_verified"),
        files_total = num(m, "ledger.files_total"),
        fn_pct = pct(num(m, "functions.ratio")),
        fn_total = num(m, "functions.total"),
        exp_passed = num(m, "exp.passed"),
        exp_total = num(m, "exp.total"),
        adr_acc = num(m, "adr.accepted"),
        adr_total = num(m, "adr.total"),
    );
    html
}

fn write_dashboard(root: &Path, r: &Report) {
    fs::create_dir_all(root.join("docs")).ok();
    fs::write(root.join("docs/status.html"), render_dashboard(root, r))
        .unwrap_or_else(|e| die(&format!("status.html: {e}")));
}

/// Verify the saved rendering without replacing it or approving live execution gates.
fn check_committed_views(root: &Path) -> Result<(), String> {
    let bytes = fs::read(root.join("status/status.json")).map_err(|e| e.to_string())?;
    let saved: ViewMetadata = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let date = saved.generated.as_bytes();
    if date.len() != 10
        || date[4] != b'-'
        || date[7] != b'-'
        || !date
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
        || saved.context.revision.len() != 40
        || !saved
            .context
            .revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || saved.context.host.trim().is_empty()
        || saved.context.toolchain.trim().is_empty()
    {
        return Err("invalid committed rendering date, revision or execution identity".into());
    }
    let report = build_report_in_context(root, Some(&saved));
    if !report.errors.is_empty() || !report.unknown_markers.is_empty() {
        return Err(format!(
            "invalid committed view inputs: {:?}; unknown markers: {:?}",
            report.errors, report.unknown_markers
        ));
    }
    let (summary, worklist) = render_status_json(&report);
    for (path, expected) in [
        ("STATUS.md", render_status_md(&report).into_bytes()),
        ("status/status.json", summary.into_bytes()),
        (
            "docs/status.html",
            render_dashboard(root, &report).into_bytes(),
        ),
        ("status/unmapped-functions.json", worklist),
    ] {
        let actual = fs::read(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        if actual != expected {
            return Err(format!(
                "committed view differs: {path}; run cargo xtask status"
            ));
        }
    }
    Ok(())
}

fn checks_pass(report: &Report, checks: &[String]) -> bool {
    !checks.is_empty()
        && report.errors.is_empty()
        && report.unknown_markers.is_empty()
        && checks
            .iter()
            .all(|check| eval_check(&report.metrics, check) == Some(true))
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repo_root();
    match args.first().map(String::as_str) {
        Some("gen") => match gen::run(&root, &args[1..], &read_ledger(&root).pin) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(error) => die(&error),
        },
        Some("status") => {
            if args.iter().any(|arg| arg == "--check-committed") {
                if args.len() != 2 {
                    die("usage: cargo xtask status --check-committed");
                }
                return match check_committed_views(&root) {
                    Ok(()) => {
                        println!(
                            "committed views match validated evidence in their recorded context"
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("xtask: {error}");
                        ExitCode::from(1)
                    }
                };
            }
            let r = build_report(&root);
            if !r.unknown_markers.is_empty() {
                eprintln!(
                    "xtask: {} unknown port markers (listed in STATUS.md)",
                    r.unknown_markers.len()
                );
            }
            write_status_md(&root, &r);
            write_status_json(&root, &r);
            if args.iter().any(|a| a == "--record")
                && r.errors.is_empty()
                && r.unknown_markers.is_empty()
            {
                record_history(&root, &r);
            }
            write_dashboard(&root, &r);
            let m = &r.metrics;
            println!(
                "status: {} files in scope, {} verified ({} of lines), {} of {} functions mapped, {} of {} experiments passed, {} sprints",
                num(m, "ledger.files_total"),
                num(m, "ledger.files_verified"),
                pct(num(m, "ledger.loc_verified_ratio")),
                num(m, "functions.ported"),
                num(m, "functions.total"),
                num(m, "exp.passed"),
                num(m, "exp.total"),
                r.sprints.len()
            );
            for e in &r.errors {
                eprintln!("xtask: {e}");
            }
            if r.unknown_markers.is_empty() && r.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Some("run") => {
            let id = args
                .get(1)
                .unwrap_or_else(|| die("usage: cargo xtask run <run-id>"));
            match evidence::run(&root, id, &read_ledger(&root).pin) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(e) => die(&e),
            }
        }
        Some("validate") => {
            let r = build_report(&root);
            for e in &r.errors {
                eprintln!("xtask: {e}");
            }
            for marker in &r.unknown_markers {
                eprintln!("unknown port marker: {marker}");
            }
            if r.errors.is_empty() && r.unknown_markers.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Some("check-metrics") => {
            let report = build_report(&root);
            let checks = &args[1..];
            if checks.is_empty() {
                die("usage: cargo xtask check-metrics '<metric> <op> <value>' ...");
            }
            for check in checks {
                println!("{} {check}", pass_text(eval_check(&report.metrics, check)));
            }
            for error in &report.errors {
                eprintln!("xtask: {error}");
            }
            for marker in &report.unknown_markers {
                eprintln!("unknown port marker: {marker}");
            }
            if checks_pass(&report, checks) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Some("check") => {
            let id = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| die("usage: xtask check <sprint id>"));
            let r = build_report(&root);
            match r.sprints.iter().find(|s| s.id == id) {
                Some(s) => {
                    for (c, res) in &s.exit {
                        println!(
                            "{} {}",
                            match res {
                                Some(true) => "pass ",
                                Some(false) => "FAIL ",
                                None => "??   ",
                            },
                            c
                        );
                    }
                    for (id, label, result) in &s.items {
                        println!("{} {} {}", pass_text(*result), id, label);
                    }
                    for e in &r.errors {
                        eprintln!("xtask: {e}");
                    }
                    if s.done && r.errors.is_empty() && r.unknown_markers.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        if num(&r.metrics, "upstream.ready") == 0.0 {
                            eprintln!(
                                "xtask: upstream is not ready; run `git submodule update --init upstream`, then verify it is clean at pin {}",
                                r.ledger_pin
                            );
                        }
                        ExitCode::from(1)
                    }
                }
                None => die(&format!("no sprint {id}")),
            }
        }
        _ => {
            eprintln!("usage: cargo xtask status [--record | --check-committed] | gen [--check | --verify] | validate | run <run-id> | check <sprint-id> | check-metrics '<metric> <op> <value>' ...");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod worklist_tests;
