//! The transpile runner's witness (docs/PHASE3-plan.md, T8 and section 5):
//! every configuration of the recorded native capture of the pinned runner
//! (`data/phase3/transpile-native.json`, `scripts/phase3_native.py
//! transpile --record`) runs through `tsr_transpile` with the runner's
//! inputs, and the harness port of the runner
//! (`tools/phase3/harness/transpile.rs`) composes each baseline. Every
//! baseline must equal the pin's by name and text, and every unit's output,
//! source map and diagnostics the pin's. `scripts/phase3_transpile.py` runs
//! the same harness against a native capture directory and grades it.
#[path = "../../../tools/s08/p5/errors.rs"]
#[allow(dead_code)]
mod errors;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[path = "../../../tools/phase3/harness/transpile.rs"]
mod transpile_runner;

use serde_json::Value;
use std::path::Path;
use transpile_runner::{run_test, run_value, unhex, Configuration, Unit};

fn native() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/phase3/transpile-native.json");
    serde_json::from_slice(&std::fs::read(path).expect("the recorded native transpile capture"))
        .expect("native JSON")
}

fn text(value: &Value) -> String {
    String::from_utf8_lossy(&unhex(value.as_str().unwrap_or_default()).expect("hex")).into_owned()
}

/// The differences between one native configuration and its Rust runs.
fn differences(row: &Value) -> Vec<String> {
    let id = row["id"].as_str().expect("id");
    let units: Vec<Unit> = row["units"]
        .as_array()
        .expect("units")
        .iter()
        .map(|unit| Unit {
            name: unhex(unit["name_hex"].as_str().expect("name")).expect("hex"),
            content: unhex(unit["content_hex"].as_str().expect("content")).expect("hex"),
        })
        .collect();
    let options = tsr_tsoptions::raw::compiler_options(&row["options"]).expect("options");
    let file = row["file"].as_str().expect("file").as_bytes();
    let configuration_name = row["configuration_name"].as_str().expect("name").as_bytes();
    let (configured_name, _) = transpile_runner::configured_name(file, configuration_name);
    let mut found = Vec::new();
    if configured_name
        != row["configured_name"]
            .as_str()
            .expect("configured")
            .as_bytes()
    {
        found.push(format!(
            "{id}: configured name {}",
            String::from_utf8_lossy(&configured_name)
        ));
    }
    let runs = match run_test(&Configuration {
        file,
        name: configuration_name,
        units: &units,
        options: &options,
        report_diagnostics: row["harness_options"]["ReportDiagnostics"] == true,
    }) {
        Ok(runs) => runs,
        Err(reason) => return vec![format!("{id}: failed: {reason}")],
    };
    let native_runs = row["runs"].as_array().expect("runs");
    if runs.len() != native_runs.len() {
        found.push(format!(
            "{id}: {} runs, the pin made {}",
            runs.len(),
            native_runs.len()
        ));
    }
    for (native_run, run) in native_runs.iter().zip(&runs) {
        let ported = run_value(run).expect("the run encodes");
        let name = native_run["baseline"]["name"].as_str().expect("name");
        if ported["declaration"] != native_run["declaration"] {
            found.push(format!("{id}: run kinds differ"));
            continue;
        }
        if ported["baseline"] != native_run["baseline"] {
            found.push(format!(
                "{name}: the baseline differs (ported {})\n--- pinned\n{}\n--- ported\n{}",
                ported["baseline"]["name"],
                text(&native_run["baseline"]["text_hex"]),
                text(&ported["baseline"]["text_hex"])
            ));
        }
        for (native_unit, unit) in native_run["units"]
            .as_array()
            .expect("units")
            .iter()
            .zip(ported["units"].as_array().expect("units"))
        {
            for field in [
                "unit_hex",
                "output_name_hex",
                "output_hex",
                "source_map_hex",
                "diagnostics",
            ] {
                if native_unit[field] != unit[field] {
                    found.push(format!(
                        "{name}: unit {} differs in {field}",
                        text(&native_unit["unit_hex"])
                    ));
                }
            }
        }
    }
    found
}

#[test]
fn the_transpile_runner_baselines_match_the_pin() {
    let document = native();
    let rows = document["rows"].as_array().expect("rows");
    let mut failures = Vec::new();
    let mut baselines = 0;
    for row in rows {
        baselines += row["runs"].as_array().expect("runs").len();
        failures.extend(differences(row));
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert_eq!(rows.len(), 28, "configurations");
    assert_eq!(baselines, 41, "baselines");
    assert_eq!(document["baselines"], 41);
}

/// The runner's naming: the vary-by names are respelled, the extension is
/// the test file's last one.
#[test]
fn configured_names_follow_the_runner() {
    for (file, configuration, expected) in [
        (
            "transpile/declarationBasicSyntax.ts",
            "declarationmap=true",
            "declarationBasicSyntax(declarationMap=true)",
        ),
        (
            "transpile/jsWithInlineSourceMapBasic.ts",
            "inlinesourcemap=false",
            "jsWithInlineSourceMapBasic(inlineSourceMap=false)",
        ),
        (
            "transpile/jsWithSourceMapBasic.ts",
            "sourcemap=true",
            "jsWithSourceMapBasic(sourceMap=true)",
        ),
        ("transpile/syntheticImports.tsx", "", "syntheticImports"),
    ] {
        let (name, extension) =
            transpile_runner::configured_name(file.as_bytes(), configuration.as_bytes());
        assert_eq!(String::from_utf8_lossy(&name), expected);
        assert_eq!(
            extension,
            tsr_tspath::any_extension_from_path::<&[u8]>(file.as_bytes(), &[], false)
        );
    }
}
