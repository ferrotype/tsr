//! Pinned frontends export facts; these Rust emitters own Rust syntax only.
mod ast;
mod ast_compact;
mod ast_local_read;
mod ast_read;
mod ast_runtime;
mod diagnostics;
mod encoder;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const SCHEMAS: &[&str] = &[
    "ast",
    "diagnostics",
    "encoder",
    "api",
    "api-wire-fixtures",
    "client-files",
];
const MANIFEST: &str = "data/s03/generated.json";

fn json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn workspace_edition(root: &Path) -> Result<String, String> {
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("workspace manifest: {e}"))?;
    manifest
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("edition"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "missing workspace.package.edition for generated Rust".into())
}

fn format_rust(root: &Path, source: &str, edition: &str) -> Result<Vec<u8>, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition, "--config-path"])
        .arg(root.join("rustfmt.toml"))
        .args(["--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("start rustfmt: {e}"))?;
    // Write concurrently: generated sources can exceed the stdout pipe capacity.
    let mut stdin = child.stdin.take().ok_or("rustfmt stdin unavailable")?;
    let input = source.as_bytes().to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|e| format!("rustfmt: {e}"))?;
    writer
        .join()
        .map_err(|_| "rustfmt input writer panicked")?
        .map_err(|e| format!("rustfmt stdin: {e}"))?;
    if !output.status.success() {
        return Err(format!("rustfmt failed: {}", output.status));
    }
    Ok(output.stdout)
}

fn outputs(root: &Path, stage: &Path, pin: &str) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let edition = workspace_edition(root)?;
    let mut files = BTreeMap::new();
    for name in SCHEMAS {
        let value = read_json(&stage.join(format!("{name}.json")))?;
        let mut rust = match *name {
            "ast" => ast::emit(&value, pin)?,
            "diagnostics" => diagnostics::emit(&value, pin)?,
            "encoder" => encoder::emit(&value, pin)?,
            _ => BTreeMap::new(),
        };
        if *name == "ast" {
            let runtime = ast_runtime::emit(&value, pin)?;
            rust.extend(runtime.files);
            files.insert(
                PathBuf::from("data/s06/generated-ast-scope.json"),
                json_bytes(&runtime.scope)?,
            );
        }
        if *name == "encoder" {
            let ast_schema = read_json(&stage.join("ast.json"))?;
            rust.extend(encoder::emit_runtime(&ast_schema, &value, pin)?);
        }
        for (path, source) in rust {
            if files
                .insert(path.clone(), format_rust(root, &source, &edition)?)
                .is_some()
            {
                return Err(format!("duplicate emitted file: {}", path.display()));
            }
        }
        files.insert(
            PathBuf::from(format!("data/s03/schema/{name}.json")),
            json_bytes(&value)?,
        );
    }
    let hashes: BTreeMap<_, _> = files
        .iter()
        .map(|(path, bytes)| {
            (
                path.to_string_lossy().into_owned(),
                super::evidence::hash(bytes),
            )
        })
        .collect();
    files.insert(
        PathBuf::from(MANIFEST),
        json_bytes(&json!({
            "version": 1, "upstreamPin": pin, "files": hashes,
        }))?,
    );
    Ok(files)
}

fn managed(path: &Path) -> bool {
    if path
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return false;
    }
    let text = path.to_string_lossy();
    text == "data/s06/generated-ast-scope.json"
        || (text.starts_with("data/s03/schema/") && text.ends_with(".json"))
        || (["tsr_ast", "tsr_diagnostics", "tsr_encoder"]
            .iter()
            .any(|krate| text.starts_with(&format!("crates/{krate}/src/")))
            && (text.ends_with("/generated.rs") || text.ends_with("_generated.rs")))
}

fn previous_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.join(MANIFEST).exists() {
        return Ok(Vec::new());
    }
    let manifest = read_json(&root.join(MANIFEST))?;
    if manifest["version"] != 1 {
        return Err("unsupported S03 generated manifest".into());
    }
    manifest["files"]
        .as_object()
        .ok_or("invalid generated file inventory")?
        .keys()
        .map(|path| {
            let path = PathBuf::from(path);
            if managed(&path) {
                Ok(path)
            } else {
                Err(format!("unsafe managed path: {}", path.display()))
            }
        })
        .collect()
}

fn existing_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
        if !directory.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                visit(root, &path, paths)?;
            } else {
                let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
                if managed(relative) {
                    if kind.is_symlink() {
                        return Err(format!(
                            "managed output is a symlink: {}",
                            relative.display()
                        ));
                    }
                    paths.push(relative.to_path_buf());
                }
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    for directory in [
        "data/s03/schema",
        "data/s06",
        "crates/tsr_ast/src",
        "crates/tsr_diagnostics/src",
        "crates/tsr_encoder/src",
    ] {
        visit(root, &root.join(directory), &mut paths)?;
    }
    Ok(paths)
}

fn differences(root: &Path, files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<Vec<PathBuf>, String> {
    let mut changed = Vec::new();
    for (path, bytes) in files {
        match fs::read(root.join(path)) {
            Ok(existing) if existing == *bytes => {}
            Ok(_) => changed.push(path.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => changed.push(path.clone()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    for path in previous_paths(root)?
        .into_iter()
        .chain(existing_paths(root)?)
    {
        if !files.contains_key(&path) {
            changed.push(path);
        }
    }
    changed.sort();
    changed.dedup();
    Ok(changed)
}

fn write_outputs(root: &Path, files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<(), String> {
    let previous = previous_paths(root)?;
    for path in existing_paths(root)? {
        if !files.contains_key(&path) && !previous.contains(&path) {
            return Err(format!(
                "unrecognized generated output {}; review/remove it before regenerating",
                path.display()
            ));
        }
    }
    for path in previous {
        if !files.contains_key(&path) && root.join(&path).exists() {
            fs::remove_file(root.join(path)).map_err(|e| e.to_string())?;
        }
    }
    for (path, bytes) in files {
        let destination = root.join(path);
        fs::create_dir_all(destination.parent().ok_or("output lacks parent")?)
            .map_err(|e| e.to_string())?;
        fs::write(&destination, bytes).map_err(|e| format!("{}: {e}", destination.display()))?;
    }
    Ok(())
}

fn stage_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("s03-stage-") else {
        return false;
    };
    let parts: Vec<_> = suffix.split('-').collect();
    // Recognize both the original PID-only format and current PID/nonce names.
    matches!(parts.len(), 1 | 2)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn exclusive_lock(path: &Path) -> Result<fs::File, String> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    lock.lock()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(lock)
}

fn new_stage(root: &Path) -> Result<(PathBuf, fs::File), String> {
    let target = root.join("target");
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    // Hold through Rust emission and output publication, not only the frontend.
    // Another generator cannot prune an active run's inputs. OS locks also
    // release after a crash, so a stale marker cannot block the next run.
    let lock = exclusive_lock(&target.join("s03-generation.lock"))?;
    // A frontend can outlive a killed Rust parent. Its existing Python flock
    // must finish before pruning. Drop this short-lived lock before spawning
    // our own frontend, which acquires the same lock independently.
    let _tooling_lock = exclusive_lock(&target.join("s03-tooling.lock"))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage = target.join(format!("s03-stage-{}-{nonce}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    // Keep this attempt, whether it succeeds or fails, and prune older owned
    // stages. Do not follow symlinks or remove similarly named user directories.
    for entry in fs::read_dir(&target).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path() != stage
            && entry.file_type().map_err(|e| e.to_string())?.is_dir()
            && entry.file_name().to_str().is_some_and(stage_name)
        {
            fs::remove_dir_all(entry.path()).map_err(|e| format!("prune S03 stage: {e}"))?;
        }
    }
    Ok((stage, lock))
}

fn ast_schema(summary: &Value) -> Result<bool, String> {
    summary["ast_schema"]
        .as_bool()
        .ok_or_else(|| "missing or invalid frontend ast_schema measurement".into())
}

pub(super) fn run(root: &Path, args: &[String], pin: &str) -> Result<bool, String> {
    let mode = match args {
        [] => "write",
        [option] if option == "--check" || option == "--verify" => option,
        _ => return Err("usage: cargo xtask gen [--check | --verify]".into()),
    };
    let (stage, _generation_lock) = new_stage(root)?;
    let output = Command::new("python3")
        .arg(root.join("scripts/s03.py"))
        .arg("prepare")
        .arg("--output")
        .arg(&stage)
        .arg("--pin")
        .arg(pin)
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("S03 frontend: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "S03 frontend failed ({}); stage: {}",
            output.status,
            stage.display()
        ));
    }
    let summary: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("invalid frontend summary: {e}"))?;
    if !ast_schema(&summary)? {
        eprintln!("S03 AST export failed; stage: {}", stage.display());
        if mode == "--verify" {
            // Downstream outputs were not measured; do not invent drift/client
            // results just to fill in the rest of the sprint's metric names.
            println!("{}", json!({"metrics": summary}));
            return Ok(true);
        }
        return Ok(false);
    }
    let patches = summary["patches_apply"]
        .as_bool()
        .ok_or("missing patches_apply")?;
    let client = summary["client_identical"]
        .as_bool()
        .ok_or("missing client_identical")?;
    let files = outputs(root, &stage, pin)?;
    let changed = differences(root, &files)?;
    for path in &changed {
        eprintln!("S03 drift: {}", path.display());
    }
    let mut metrics = summary
        .as_object()
        .ok_or("frontend summary must be an object")?
        .clone();
    metrics.insert("drift".into(), json!(!changed.is_empty()));
    metrics.insert("generated_files".into(), json!(files.len()));
    let passed = patches && client && changed.is_empty();
    if mode == "--verify" {
        println!("{}", json!({"metrics": metrics}));
        return Ok(true); // Successful measurement; consumers enforce individual metrics.
    }
    if mode == "write" && patches && client {
        write_outputs(root, &files)?;
        eprintln!("generated {} files from {pin}", files.len());
        return Ok(true);
    }
    if passed {
        eprintln!(
            "S03: all {} generated files and pinned client bytes match",
            files.len()
        );
    }
    Ok(passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ts-rust-gen-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn stages_are_bounded_and_an_active_attempt_holds_the_lock() {
        let root = fixture("stage");
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        fs::create_dir(target.join("s03-stage-1")).unwrap();
        fs::create_dir(target.join("s03-stage-2-3")).unwrap();
        fs::create_dir(target.join("s03-stage-not-generated")).unwrap();
        let (first, lock) = new_stage(&root).unwrap();
        fs::write(first.join("failed-export.json"), b"diagnostic input").unwrap();
        let contender = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.join("s03-generation.lock"))
            .unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
        assert!(!target.join("s03-stage-1").exists());
        assert!(!target.join("s03-stage-2-3").exists());
        assert!(target.join("s03-stage-not-generated").exists());
        assert!(first.join("failed-export.json").exists());
        drop(lock);
        let (second, lock) = new_stage(&root).unwrap();
        assert_ne!(first, second);
        assert!(!first.exists());
        assert!(second.is_dir());
        assert!(fs::read_dir(&second).unwrap().next().is_none());
        drop(lock);
        drop(contender);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_surviving_python_frontend_keeps_its_stage_until_it_exits() {
        use std::io::{BufRead, BufReader};
        use std::sync::mpsc;
        use std::time::Duration;

        let root = fixture("orphan-frontend");
        let old_stage = root.join("target/s03-stage-1-2");
        fs::create_dir_all(&old_stage).unwrap();
        let mut child = Command::new("python3").args(["-u", "-c",
            "import fcntl,sys; f=open(sys.argv[1], 'w'); fcntl.flock(f,fcntl.LOCK_EX); print('ready'); sys.stdin.readline()"])
            .arg(root.join("target/s03-tooling.lock"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let stage = new_stage(&worker_root).unwrap();
            finished_tx.send(stage).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            finished_rx.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(old_stage.exists());
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let (new_stage, lock) = finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
        assert!(!old_stage.exists());
        assert!(new_stage.exists());
        drop(lock);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn measured_ast_failure_does_not_require_downstream_outputs() {
        let root = fixture("ast-failure");
        fs::create_dir(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/s03.py"),
            "print('{\"patches_apply\": true, \"ast_schema\": false}')\n",
        )
        .unwrap();
        assert!(!run(&root, &["--check".into()], "pin").unwrap());
        assert!(run(&root, &["--verify".into()], "pin").unwrap());
        assert!(!root.join("crates").exists());
        assert!(!root.join(MANIFEST).exists());
        for value in [
            json!({}),
            json!({"ast_schema": "false"}),
            json!({"ast_schema": 1}),
        ] {
            assert!(ast_schema(&value).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_formatting_uses_the_workspace_edition() {
        let root = fixture("edition");
        fs::write(root.join("rustfmt.toml"), "").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nedition = '2024'\n",
        )
        .unwrap();
        let edition = workspace_edition(&root).unwrap();
        // `gen` is reserved in 2024, but was an ordinary identifier in 2021.
        // This fails if the formatter silently keeps its old edition setting.
        let source = "fn gen() {}\n";
        assert_eq!(edition, "2024");
        assert!(format_rust(&root, source, "2021").is_ok());
        assert!(format_rust(&root, source, &edition).is_err());
        fs::write(root.join("Cargo.toml"), "[workspace.package]\n").unwrap();
        assert!(workspace_edition(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_paths_cannot_escape_or_delete_handwritten_code() {
        for path in [
            "../Cargo.toml",
            "/tmp/generated.rs",
            "data/s03/schema/../../Cargo.toml",
            "crates/tsr_ast/src/lib.rs",
            "Cargo.toml",
        ] {
            assert!(!managed(Path::new(path)), "{path}");
        }
        for path in [
            "data/s03/schema/ast.json",
            "crates/tsr_ast/src/data_generated.rs",
            "crates/tsr_encoder/src/generated.rs",
        ] {
            assert!(managed(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn drift_catches_missing_changed_and_removed_outputs_without_writing() {
        let root = std::env::temp_dir().join(format!("ts-rust-gen-test-{}", std::process::id()));
        fs::create_dir_all(root.join("data/s03/schema")).unwrap();
        fs::write(
            root.join(MANIFEST),
            br#"{"version":1,"files":{"data/s03/schema/old.json":"hash"}}"#,
        )
        .unwrap();
        fs::write(root.join("data/s03/schema/changed.json"), b"old").unwrap();
        fs::write(root.join("data/s03/schema/untracked.json"), b"extra").unwrap();
        let files = BTreeMap::from([
            (
                PathBuf::from("data/s03/schema/changed.json"),
                b"new".to_vec(),
            ),
            (
                PathBuf::from("data/s03/schema/missing.json"),
                b"new".to_vec(),
            ),
        ]);
        assert_eq!(differences(&root, &files).unwrap().len(), 4);
        assert!(write_outputs(&root, &files)
            .unwrap_err()
            .contains("unrecognized"));
        assert_eq!(
            fs::read(root.join("data/s03/schema/changed.json")).unwrap(),
            b"old"
        );
        assert!(!root.join("data/s03/schema/missing.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
