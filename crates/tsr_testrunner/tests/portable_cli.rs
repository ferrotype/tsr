//! A selected variant must not cross from a portable suite to native checking.
use std::path::PathBuf;
use std::process::Command;

#[test]
fn an_embed_request_cannot_select_a_native_compiler_variant() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO_BIN_EXE_tsr-testrunner"))
        .args([
            "--suite",
            "transpile",
            "--backend",
            "embed",
            "--consumer",
            "/missing-consumer",
            "run",
            "--id",
            "compiler/2dArrays.ts",
            "--local",
            "/unused-local",
        ])
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside suite transpile"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn a_missing_compiler_consumer_fails_every_requested_domain_without_fallback() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO_BIN_EXE_tsr-testrunner"))
        .args([
            "--suite",
            "compiler",
            "--backend",
            "embed",
            "--consumer",
            "/missing-phase7-consumer",
            "run",
            "--id",
            "compiler/2dArrays.ts",
            "--local",
            "/unused-local",
        ])
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    // Suite failures are result rows; a successful driver exit means that
    // the complete report was written, not that its subtests passed.
    assert!(output.status.success(), "{output:?}");
    let rows: Vec<serde_json::Value> = output
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 9, "{output:?}");
    assert!(
        rows.iter().all(|row| row["state"] == "fail"
            && row["reason"]
                .as_str()
                .unwrap()
                .contains("missing-phase7-consumer")),
        "{rows:?}"
    );
}
