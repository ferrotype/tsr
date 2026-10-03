use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const PIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A repository with one ported source file, one planned generated file, one
/// out-of-scope file and one harness file, and the inventory and upstream
/// manifest that agree with them.
pub(crate) struct Fixture(pub(crate) PathBuf);
impl Fixture {
    pub(crate) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "xtask-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        let f = Self(dir);
        let entry = |go: &str, krate: &str, phase: u8, kind: &str, status: &str, loc: u32| {
            format!(
                "\n[[file]]\ngo = \"{go}\"\npackage = \"{}\"\ncrate = \"{krate}\"\nphase = {phase}\nkind = \"{kind}\"\nstatus = \"{status}\"\npin = \"{PIN}\"\nsource_hash = \"{}\"\nloc = {loc}\n",
                go_package(go),
                "b".repeat(64)
            )
        };
        let ledger = [
            format!("pin = \"{PIN}\"\n"),
            entry("tsc/internal/demo/demo.go", "tsr_demo", 0, "source", "ported", 10).replace(
                "status = \"ported\"\n",
                "status = \"ported\"\nrust = [\"crates/demo/lib.rs\"]\nverify = [\"run.proof.ok == true\"]\n",
            ),
            entry("tsc/internal/demo/gen.go", "tsr_demo", 1, "generated", "planned", 5),
            entry("tsc/internal/demo/demo_windows.go", "tsr_demo", 0, "out-of-scope", "out-of-scope", 3),
            entry("tsc/internal/testutil/harness.go", "tsr_testrunner", 1, "harness", "planned", 7),
        ];
        f.write("PORTS.toml", &ledger.concat());
        f.write(
            "crates/demo/lib.rs",
            "// port: tsc/internal/demo/demo.go:A.Map\npub fn map() {}\n",
        );
        let row = |file: &str, receiver: &str, name: &str, id: &str| {
            format!(
                "{file}\t{}\t{receiver}\t{name}\t1\t2\t{file}:{id}\n",
                go_package(file)
            )
        };
        f.write(
            "data/go-functions.tsv",
            &[
                format!("# upstream {PIN}\nfile\tpackage\treceiver\tname\tstart\tend\tid\n"),
                row("tsc/internal/demo/demo.go", "*A", "Map", "A.Map"),
                row("tsc/internal/demo/demo.go", "*B", "Map", "B.Map"),
                row("tsc/internal/demo/gen.go", "", "Table", "Table"),
                row("tsc/internal/testutil/harness.go", "", "Serve", "Serve"),
            ]
            .concat(),
        );
        f.manifest();
        f
    }
    pub(crate) fn write(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    pub(crate) fn replace(&self, path: &str, from: &str, to: &str) {
        let text = fs::read_to_string(self.0.join(path)).unwrap();
        assert!(text.contains(from), "{path} has no {from:?}");
        self.write(path, &text.replace(from, to));
    }
    /// Rewrite data/upstream.json for the current generated fields and inventory.
    pub(crate) fn manifest(&self) {
        let value = serde_json::json!({
            "schema_version": 2,
            "pin": PIN,
            "ledger_generated_sha256": ledger_generated_hash(&self.0).unwrap(),
            "inventory_sha256": hash(&fs::read(self.0.join("data/go-functions.tsv")).unwrap()),
        });
        self.write("data/upstream.json", &value.to_string());
    }
    pub(crate) fn coverage(&self) -> Coverage {
        coverage(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn state_of<'a>(c: &'a Coverage, go: &str) -> &'a str {
    &c.files.iter().find(|f| f.go == go).unwrap().status
}

#[test]
fn the_ledger_parses_with_and_without_the_retired_verify_field() {
    let entry = |extra: &str| {
        format!(
            "pin = \"{PIN}\"\n[[file]]\ngo = \"tsc/a.go\"\npackage = \"a\"\ncrate = \"tsr\"\nphase = 0\nkind = \"source\"\nstatus = \"planned\"\nrust = []\n{extra}pin = \"{PIN}\"\nsource_hash = \"{}\"\nloc = 1\n",
            "b".repeat(64)
        )
    };
    for extra in ["", "verify = []\n", "verify = [\"run.e1.parity == 1\"]\n"] {
        let ledger: Ledger = toml::from_str(&entry(extra)).unwrap();
        assert_eq!(ledger.file[0].go, "tsc/a.go");
    }
    // Every other unknown field is still a schema error.
    assert!(toml::from_str::<Ledger>(&entry("verified = true\n")).is_err());
    assert!(toml::from_str::<Ledger>(&entry("rusts = []\n")).is_err());
}

#[test]
fn the_fixture_validates_and_counts_mapped_functions() {
    let c = Fixture::new().coverage();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(c.unknown_markers.is_empty(), "{:?}", c.unknown_markers);
    // Only the counted source file's functions count: not the generated or
    // the harness file's.
    assert_eq!((c.functions, c.mapped, c.tests), (2, 1, 0));
    let demo = &c.packages["internal/demo"];
    assert_eq!((demo.functions, demo.mapped, demo.tests), (2, 1, 0));
    assert!(!c.packages.contains_key("internal/testutil"));
    assert_eq!(
        c.unmapped,
        BTreeMap::from([(
            "internal/demo".to_string(),
            vec!["tsc/internal/demo/demo.go:B.Map".to_string()]
        )])
    );
    assert_eq!(report(&c), ExitCode::SUCCESS);
}

#[test]
fn verified_and_other_unknown_states_are_errors_counted_as_planned() {
    for state in ["verified", "done"] {
        let f = Fixture::new();
        f.replace(
            "PORTS.toml",
            "status = \"ported\"",
            &format!("status = \"{state}\""),
        );
        let c = f.coverage();
        assert_eq!(
            c.errors,
            [format!(
                "tsc/internal/demo/demo.go: status must be planned, in-progress, ported or out-of-scope, not {state}"
            )]
        );
        assert_eq!(state_of(&c, "tsc/internal/demo/demo.go"), "planned");
        assert_eq!(report(&c), ExitCode::from(1));
    }
}

#[test]
fn a_ported_entry_needs_existing_rust_paths() {
    for rust in [
        "[]",
        "[\"crates/demo/missing.rs\"]",
        "[\"../demo/lib.rs\"]",
        "[\"crates/demo\"]",
    ] {
        let f = Fixture::new();
        f.replace(
            "PORTS.toml",
            "rust = [\"crates/demo/lib.rs\"]",
            &format!("rust = {rust}"),
        );
        let c = f.coverage();
        assert_eq!(
            c.errors,
            ["tsc/internal/demo/demo.go: ported entry needs existing Rust paths"],
            "{rust}"
        );
        assert_eq!(state_of(&c, "tsc/internal/demo/demo.go"), "in-progress");
    }
}

#[test]
fn provenance_binds_generated_fields_inventory_and_pin_but_not_editable_state() {
    let f = Fixture::new();
    // status and rust are the editable fields; the manifest does not hash them.
    f.replace(
        "PORTS.toml",
        "status = \"planned\"",
        "status = \"in-progress\"",
    );
    assert!(f.coverage().errors.is_empty());
    f.replace("PORTS.toml", "loc = 10", "loc = 11");
    assert_eq!(
        f.coverage().errors,
        ["generated ledger fields/inventory changed: regenerate their upstream manifest"]
    );
    f.manifest();
    assert!(f.coverage().errors.is_empty());
    f.replace(
        "data/go-functions.tsv",
        "\t1\t2\ttsc/internal/demo/gen.go",
        "\t1\t3\ttsc/internal/demo/gen.go",
    );
    assert_eq!(f.coverage().errors.len(), 1);
    f.manifest();
    f.replace("data/upstream.json", PIN, &"c".repeat(40));
    assert_eq!(
        f.coverage().errors,
        ["upstream manifest pin/version mismatch"]
    );
    f.manifest();
    f.replace(
        "data/go-functions.tsv",
        &format!("# upstream {PIN}"),
        &format!("# upstream {}", "c".repeat(40)),
    );
    f.manifest();
    assert_eq!(f.coverage().errors, ["inventory pin does not match ledger"]);
}

#[test]
fn invalid_source_provenance_and_a_malformed_inventory_are_errors() {
    let f = Fixture::new();
    f.replace("data/go-functions.tsv", "\tTable\t1\t2\t", "\tTable\t1\t");
    f.manifest();
    assert_eq!(f.coverage().errors, ["malformed inventory row"]);
    let f = Fixture::new();
    f.replace(
        "PORTS.toml",
        "package = \"internal/testutil\"",
        "package = \"\"",
    );
    assert_eq!(
        f.coverage().errors,
        [
            "missing/invalid generated field package",
            "tsc/internal/testutil/harness.go: invalid source provenance"
        ]
    );
}

#[test]
fn generated_projection_has_the_same_canonical_hash_as_python() {
    let f = Fixture::new();
    for verify in ["verify = []\n", ""] {
        f.write(
            "PORTS.toml",
            &format!(
                r#"pin = "{PIN}"
[[file]]
go = "tsc/internal/é.go"
package = "internal"
crate = "tsr_core"
phase = 0
kind = "source"
pin = "{PIN}"
source_hash = "{}"
loc = 2
status = "planned"
rust = []
{verify}"#,
                "b".repeat(64)
            ),
        );
        assert_eq!(
            ledger_generated_hash(&f.0).unwrap(),
            "637f37b9dba865420809290bc99927d2ab80f1b50231c1ed8a3445a24ab9b528"
        );
    }
}

#[test]
fn port_markers_are_scanned_in_crates_and_tools() {
    let f = Fixture::new();
    f.write(
        "tools/harness/src/lib.rs",
        "/// port: tsc/internal/demo/demo.go:B.Map\nfn b() {}\n",
    );
    let c = f.coverage();
    assert_eq!(c.mapped, 2);
    assert!(c.unmapped.is_empty());

    // A marker naming a function of an uncounted (harness) file is known but
    // not counted; one naming no inventory function is unknown, in either tree.
    f.write(
        "tools/harness/src/lib.rs",
        "//! port: tsc/internal/testutil/harness.go:Serve\n    // port: tsc/internal/demo/demo.go:Gone\n",
    );
    f.write(
        "crates/demo/src/more.rs",
        "/// port: tsc/internal/demo/demo.go:A.Map\n/// port: tsc/internal/demo/demo.go:Missing\n",
    );
    let c = f.coverage();
    assert_eq!(c.mapped, 1);
    assert_eq!(
        c.unknown_markers,
        [
            "tsc/internal/demo/demo.go:Gone",
            "tsc/internal/demo/demo.go:Missing"
        ]
    );
    assert_eq!(report(&c), ExitCode::from(1));
}

#[test]
fn marker_syntax_is_exact_and_target_directories_are_skipped() {
    let f = Fixture::new();
    for (path, text) in [
        (
            "tools/x/target/debug/build.rs",
            "// port: tsc/internal/demo/demo.go:Nope\n",
        ),
        (
            "crates/demo/target/out.rs",
            "// port: tsc/internal/demo/demo.go:Nope\n",
        ),
        (
            "tools/x/notes.md",
            "// port: tsc/internal/demo/demo.go:Nope\n",
        ),
        (
            "tools/x/src/lib.rs",
            concat!(
                "//port: tsc/internal/demo/demo.go:Nope\n",
                "//  port: tsc/internal/demo/demo.go:Nope\n",
                "//// port: tsc/internal/demo/demo.go:Nope\n",
                "let x = 1; // port: tsc/internal/demo/demo.go:Nope\n",
                "// port: no-colon-means-no-marker\n",
                "// ports: tsc/internal/demo/demo.go:Nope\n",
            ),
        ),
    ] {
        f.write(path, text);
    }
    let c = f.coverage();
    assert!(c.unknown_markers.is_empty(), "{:?}", c.unknown_markers);
    assert_eq!(c.mapped, 1);
}

#[test]
fn ported_tests_count_distinct_go_test_identities_by_package() {
    let f = Fixture::new();
    f.write(
        "crates/demo/src/tests.rs",
        concat!(
            "// source: tsc/internal/demo/demo_test.go:TestMap\n",
            "/// source: tsc/internal/demo/demo_test.go:TestOther (one subtest)\n",
            "// source: tsc/internal/demo/demo.go:A.Map\n",
            "// source: tsc/internal/demo/demo_test.go\n",
            "// source: tsc/internal/demo/demo_test.go:\n",
        ),
    );
    f.write(
        "tools/watch/src/lib.rs",
        concat!(
            "    // source: tsc/internal/demo/demo_test.go:TestMap\n",
            "//! source: tsc/internal/fswatch/watcher_test.go:TestWatchFileUpdate\n",
            "// source: tsc/cmd/tsc/sys_unix_test.go:TestSys\n",
        ),
    );
    let c = f.coverage();
    assert_eq!(c.tests, 4);
    assert_eq!(c.packages["internal/demo"].tests, 2);
    // A package with ported tests but no counted functions still has a row.
    let fswatch = &c.packages["internal/fswatch"];
    assert_eq!(
        (fswatch.functions, fswatch.mapped, fswatch.tests),
        (0, 0, 1)
    );
    assert_eq!(c.packages["cmd/tsc"].tests, 1);
    assert!(c.errors.is_empty() && c.unknown_markers.is_empty());
}
