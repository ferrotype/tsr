//! `splice`: insert a plan's mutant switches into a scratch workspace copy.
//!
//! Every mutant's span digest is verified against the copy before anything is
//! written; one drifted span refuses the whole splice and leaves every file
//! untouched. Insertions never contain a newline, so every existing line keeps
//! its number, and each spliced file must still parse. Each mutated crate's
//! manifest in the COPY gains a path dependency on the switch crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::json::canonical;
use crate::source::{sha256_hex, Source};

pub const SWITCH_PACKAGE: &str = "phase1_mutants";
const MARK: &str = "::phase1_mutants::";

struct Insert {
    line: usize,
    column: usize,
    order: i64,
    text: String,
}

struct Check {
    id: u64,
    span: (usize, usize),
    span_sha256: String,
}

fn number(value: &Value, name: &str) -> Result<u64, String> {
    value[name]
        .as_u64()
        .ok_or_else(|| format!("mutant field {name:?} is not a number"))
}

fn line(value: &Value, name: &str) -> Result<usize, String> {
    usize::try_from(number(value, name)?).map_err(|error| error.to_string())
}

fn text<'v>(value: &'v Value, name: &str) -> Result<&'v str, String> {
    value[name]
        .as_str()
        .ok_or_else(|| format!("mutant field {name:?} is not a string"))
}

/// A path from `from` (a directory) to `to`, both absolute and normalized.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for component in &to[common..] {
        out.push(component.as_os_str());
    }
    out
}

fn spliced_text(file: &str, source: &Source, inserts: &[Insert]) -> Result<String, String> {
    let mut positioned = Vec::with_capacity(inserts.len());
    for insert in inserts {
        if insert.text.contains('\n') {
            return Err(format!(
                "{file}:{}: an insertion contains a newline",
                insert.line
            ));
        }
        let offset = source.offset(insert.line, insert.column).ok_or_else(|| {
            format!(
                "{file}:{}:{}: insertion point is outside the line",
                insert.line, insert.column
            )
        })?;
        positioned.push((offset, insert.order, insert.text.as_str()));
    }
    // Apply from the end so earlier offsets stay valid; at one offset, texts
    // are concatenated in ascending order.
    positioned.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let mut out = source.text.clone();
    let mut index = 0;
    while index < positioned.len() {
        let offset = positioned[index].0;
        let mut joined = String::new();
        while index < positioned.len() && positioned[index].0 == offset {
            joined.push_str(positioned[index].2);
            index += 1;
        }
        out.insert_str(offset, &joined);
    }
    let after = Source::new(out);
    if after.line_count() != source.line_count() {
        return Err(format!("{file}: splicing changed the line count"));
    }
    syn::parse_file(&after.text).map_err(|error| {
        let at = error.span().start();
        format!(
            "{file}: spliced text does not parse: {error} at {}:{}",
            at.line, at.column
        )
    })?;
    Ok(after.text)
}

fn add_dependency(manifest: &str, path: &Path) -> Option<String> {
    if manifest
        .lines()
        .any(|line| line.trim_start().starts_with(SWITCH_PACKAGE))
    {
        return None;
    }
    let entry = format!(
        "{SWITCH_PACKAGE} = {{ path = {:?} }}",
        path.to_string_lossy().replace('\\', "/")
    );
    let mut lines: Vec<&str> = manifest.lines().collect();
    if let Some(at) = lines
        .iter()
        .position(|line| line.trim() == "[dependencies]")
    {
        lines.insert(at + 1, &entry);
    } else {
        lines.extend(["", "[dependencies]", &entry]);
    }
    Some(lines.join("\n") + "\n")
}

/// Splices `plan` into `root` and returns the splice report.
pub fn splice(root: &Path, plan_path: &Path, switch: &Path) -> Result<Value, String> {
    let plan_bytes =
        std::fs::read(plan_path).map_err(|error| format!("{}: {error}", plan_path.display()))?;
    let plan: Value = serde_json::from_slice(&plan_bytes)
        .map_err(|error| format!("{}: {error}", plan_path.display()))?;
    if plan["version"].as_u64() != Some(crate::plan::PLAN_VERSION) {
        return Err("unsupported plan version".to_owned());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let switch = switch
        .canonicalize()
        .map_err(|error| format!("{}: {error}", switch.display()))?;
    let switch_manifest = std::fs::read_to_string(switch.join("Cargo.toml"))
        .map_err(|error| format!("{}: {error}", switch.display()))?;
    if !switch_manifest.contains(&format!("name = \"{SWITCH_PACKAGE}\"")) {
        return Err(format!(
            "{} is not the {SWITCH_PACKAGE} crate",
            switch.display()
        ));
    }

    let mut checks: BTreeMap<String, Vec<Check>> = BTreeMap::new();
    let mut inserts: BTreeMap<String, Vec<Insert>> = BTreeMap::new();
    let mut crates = BTreeSet::new();
    let mutants = plan["mutants"]
        .as_array()
        .ok_or("plan has no mutant list")?;
    for mutant in mutants {
        let file = text(mutant, "file")?.to_owned();
        if !file.starts_with("crates/") || file.contains("..") {
            return Err(format!("refusing to splice outside crates/: {file}"));
        }
        let span = mutant["span"]
            .as_array()
            .ok_or("mutant span is not a list")?;
        let span = (
            span.first().and_then(Value::as_u64).ok_or("bad span")?,
            span.get(1).and_then(Value::as_u64).ok_or("bad span")?,
        );
        checks.entry(file.clone()).or_default().push(Check {
            id: number(mutant, "id")?,
            span: (
                usize::try_from(span.0).map_err(|error| error.to_string())?,
                usize::try_from(span.1).map_err(|error| error.to_string())?,
            ),
            span_sha256: text(mutant, "span_sha256")?.to_owned(),
        });
        for insert in mutant["insert"]
            .as_array()
            .ok_or("mutant insert is not a list")?
        {
            inserts.entry(file.clone()).or_default().push(Insert {
                line: line(insert, "line")?,
                column: line(insert, "column")?,
                order: insert["order"]
                    .as_i64()
                    .ok_or("insert order is not a number")?,
                text: text(insert, "text")?.to_owned(),
            });
        }
        crates.insert(file.split('/').take(2).collect::<Vec<_>>().join("/"));
    }

    // Verify everything before writing anything.
    let mut drift = Vec::new();
    let mut outputs = Vec::new();
    for (file, file_checks) in &checks {
        let path = root.join(file);
        let before = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if before.contains(MARK) {
            drift.push(format!("{file}: already spliced"));
            continue;
        }
        let source = Source::new(before);
        for check in file_checks {
            let found = source.span_sha256(check.span.0, check.span.1);
            if found.as_deref() != Some(check.span_sha256.as_str()) {
                drift.push(format!(
                    "{file}:{}-{}: mutant {} span drifted (planned {}, found {})",
                    check.span.0,
                    check.span.1,
                    check.id,
                    check.span_sha256,
                    found.as_deref().unwrap_or("no such lines"),
                ));
            }
        }
        let file_inserts = inserts.get(file).map_or(&[][..], Vec::as_slice);
        for insert in file_inserts {
            let inside = file_checks
                .iter()
                .any(|check| (check.span.0..=check.span.1).contains(&insert.line));
            if !inside {
                drift.push(format!(
                    "{file}:{}: insertion outside every verified span",
                    insert.line
                ));
            }
        }
        if drift.is_empty() {
            let after = spliced_text(file, &source, file_inserts)?;
            outputs.push((file.clone(), path, source.text, after, file_checks.len()));
        }
    }
    if !drift.is_empty() {
        let shown: Vec<_> = drift.iter().take(20).cloned().collect();
        return Err(format!(
            "refusing to splice: {} span check(s) failed; nothing was written\n{}",
            drift.len(),
            shown.join("\n")
        ));
    }

    let mut manifests = Vec::new();
    let mut manifest_edits = Vec::new();
    for krate in &crates {
        let manifest_path = root.join(krate).join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let dependency = relative(&root.join(krate), &switch);
        if let Some(updated) = add_dependency(&manifest, &dependency) {
            manifest_edits.push((manifest_path, updated));
        }
        manifests.push(format!("{krate}/Cargo.toml"));
    }

    let mut files = Vec::new();
    for (file, path, before, after, count) in outputs {
        std::fs::write(&path, &after).map_err(|error| format!("{}: {error}", path.display()))?;
        files.push(json!({
            "file": file,
            "mutants": count,
            "sha256_before": sha256_hex(before.as_bytes()),
            "sha256_after": sha256_hex(after.as_bytes()),
        }));
    }
    for (path, updated) in manifest_edits {
        std::fs::write(&path, updated).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(json!({
        "version": 1,
        "plan_sha256": sha256_hex(&plan_bytes),
        "root_tree": plan["root_tree"],
        "switch": switch.to_string_lossy(),
        "mutants": mutants.len(),
        "files": files,
        "manifests": manifests,
    }))
}

pub fn write_report(path: &Path, report: &Value) -> Result<(), String> {
    std::fs::write(path, canonical(report) + "\n")
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{add_dependency, relative};
    use std::path::Path;

    #[test]
    fn manifests_gain_one_path_dependency() {
        let path = Path::new("../../tools/phase1/mutation/switch");
        let added = add_dependency(
            "[package]\nname = \"x\"\n\n[dependencies]\na = \"1\"\n",
            path,
        )
        .unwrap();
        assert_eq!(
            added,
            "[package]\nname = \"x\"\n\n[dependencies]\nphase1_mutants = { path = \"../../tools/phase1/mutation/switch\" }\na = \"1\"\n"
        );
        assert_eq!(add_dependency(&added, path), None, "idempotent");
        let appended = add_dependency("[package]\nname = \"x\"\n", path).unwrap();
        assert!(appended.ends_with("\n[dependencies]\nphase1_mutants = { path = \"../../tools/phase1/mutation/switch\" }\n"));
    }

    #[test]
    fn relative_paths_climb_to_the_common_ancestor() {
        assert_eq!(
            relative(
                Path::new("/ws/crates/tsr_ast"),
                Path::new("/ws/tools/phase1/mutation/switch")
            ),
            Path::new("../../tools/phase1/mutation/switch")
        );
    }
}
