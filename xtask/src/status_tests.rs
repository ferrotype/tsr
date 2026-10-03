use super::*;
use crate::tests::{Fixture, PIN};
use serde_json::json;

fn parity(f: &Fixture, suite: &str, pin: &str, total: u64, failing: &serde_json::Value) {
    f.write(
        &format!("status/parity/{suite}.json"),
        &json!({"suite": suite, "pin": pin, "total": total, "failing": failing}).to_string(),
    );
}

fn perf_run(revision: &str, at: &str, label: &str, ratios: &serde_json::Value) -> String {
    json!({
        "workload": "w", "pin": PIN, "revision": revision,
        "host": {"os": "macos", "arch": "aarch64", "cpus": 18, "label": label},
        "recorded_at": at, "ratios": ratios,
        "samples": {"rust": {}, "go": {}},
    })
    .to_string()
}

#[test]
fn suite_parity_renders_rows_failures_approvals_and_pin_flags() {
    let f = Fixture::new();
    parity(
        &f,
        "compiler",
        PIN,
        10,
        &json!({
            "compiler/b.ts/types": {"reason": "extra <T> in `a | b`"},
            "compiler/a.ts/errors": {"reason": "missing TS2322", "approved": "owner 2026-10-03, pin quirk"},
        }),
    );
    parity(&f, "tsc", &"b".repeat(40), 5, &json!({}));
    // Its file name sorts before compiler.json; suites are listed by name.
    parity(&f, "compiler-concurrent", PIN, 10, &json!({}));
    let page = Page::read(&f.0);

    let md = render_markdown(&page);
    assert!(
        md.contains(concat!(
            "| Suite | Pin | Variants | Failing sub-tests | Approved |\n|---|---|---:|---:|---:|\n",
            "| compiler | ledger pin | 10 | 2 | 1 |\n",
            "| compiler-concurrent | ledger pin | 10 | 0 | 0 |\n",
            "| tsc | **`bbbbbbbb` differs from the ledger pin** | 5 | 0 | 0 |\n",
        )),
        "{md}"
    );
    let approved = md
        .find(
            "- `compiler/a.ts/errors`: missing TS2322 **Approved:** owner 2026-10-03, pin quirk\n",
        )
        .unwrap();
    let unexplained = md
        .find("- `compiler/b.ts/types`: extra <T> in `a | b`\n")
        .unwrap();
    assert!(approved < unexplained, "failing ids are listed in id order");
    assert!(md.contains("### tsc\n\nNo failing sub-tests.\n"), "{md}");

    let html = render_html(&page);
    assert!(
        html.contains(
            "<li><code>compiler/b.ts/types</code>: extra &lt;T&gt; in <code>a | b</code></li>"
        ),
        "{html}"
    );
    assert!(html.contains("<strong>Approved:</strong> owner 2026-10-03, pin quirk</li>"));
    assert!(html.contains(
        "<td>tsc</td><td><strong><code>bbbbbbbb</code> differs from the ledger pin</strong></td><td class=\"num\">5</td>"
    ));

    assert_eq!(
        summary_line(&page),
        "status: 2 files in scope, 1 ported, 1 of 2 functions mapped; \
         compiler: 2 failing (1 approved) of 10 variants; \
         compiler-concurrent: 0 failing (0 approved) of 10 variants; \
         tsc: 0 failing (0 approved) of 5 variants"
    );
}

#[test]
fn inline_spans_nest_code_in_strong_and_close_at_the_end() {
    assert_eq!(
        inline_html("**`a` b** c"),
        "<strong><code>a</code> b</strong> c"
    );
    assert_eq!(inline_html("`x ** y` <z>"), "<code>x ** y</code> &lt;z&gt;");
    assert_eq!(
        inline_html("an **`open span"),
        "an <strong><code>open span</code></strong>"
    );
}

#[test]
fn parity_entries_reject_misspelled_fields_and_require_a_reason() {
    let file = |entry: &str| {
        serde_json::from_str::<Parity>(&format!(
            r#"{{"suite":"s","pin":"p","total":1,"failing":{{"x":{entry}}}}}"#
        ))
    };
    assert!(file(r#"{"reason":"r","approved":"o"}"#).is_ok());
    assert!(file(r#"{"reason":"r","aproved":"o"}"#).is_err());
    assert!(file(r#"{"approved":"o"}"#).is_err());
}

#[test]
fn perf_runs_render_per_workload_oldest_first_with_thresholds() {
    let f = Fixture::new();
    f.write(
        "status/perf/parse-bind/b.json",
        &perf_run(
            "0123456789abcdef",
            "2026-10-03T09:12:00Z",
            "owner quiet host",
            &json!({"one_thread_wall_time": 1.1534, "peak_rss": 0.81}),
        ),
    );
    f.write(
        "status/perf/parse-bind/a.json",
        &perf_run(
            "fedcba9876543210",
            "2026-10-02T08:00:00Z",
            "ubuntu-latest",
            &json!({"one_thread_wall_time": 1.4}),
        ),
    );
    f.write(
        "status/perf/checker/nested/run.json",
        &perf_run("abc", "2026-10-01T00:00:00Z", "a | b", &json!({"wall": 2})),
    );
    f.write(
        "status/perf/thresholds.toml",
        "[parse-bind]\none_thread_wall_time = 1.25\n",
    );
    let page = Page::read(&f.0);

    let md = render_markdown(&page);
    assert!(
        md.contains("### parse-bind\n\n| Recorded | Revision | Host | one_thread_wall_time | peak_rss |\n|---|---|---|---:|---:|\n"),
        "{md}"
    );
    let older = md
        .find(
            "| 2026-10-02T08:00:00Z | `fedcba98` | ubuntu-latest | 1.400 (threshold 1.25) | — |\n",
        )
        .unwrap();
    let newer = md
        .find("| 2026-10-03T09:12:00Z | `01234567` | owner quiet host | 1.153 (threshold 1.25) | 0.810 |\n")
        .unwrap();
    assert!(older < newer);
    assert!(
        md.contains("| 2026-10-01T00:00:00Z | `abc` | a \\| b | 2.000 |\n"),
        "{md}"
    );
    assert!(md.find("### checker").unwrap() < md.find("### parse-bind").unwrap());

    let html = render_html(&page);
    assert!(
        html.contains("<td class=\"num\">1.153 (threshold 1.25)</td>"),
        "{html}"
    );
    assert!(html.contains("<td><code>01234567</code></td><td>owner quiet host</td>"));
}

#[test]
fn absent_inputs_render_one_line_each() {
    let f = Fixture::new();
    let md = render_markdown(&Page::read(&f.0));
    assert!(
        md.contains("No expectation files under `status/parity`.\n"),
        "{md}"
    );
    assert!(md.contains("## Performance\n\nNo `status/perf` directory; no runs are recorded.\n"));
    fs::create_dir_all(f.0.join("status/perf")).unwrap();
    let page = Page::read(&f.0);
    assert!(render_markdown(&page).contains("No workloads under `status/perf`.\n"));
    assert!(render_html(&page).contains("<p>No workloads under <code>status/perf</code>.</p>"));
}

#[test]
fn tallies_count_files_and_lines_by_state_and_skip_harness_files() {
    let c = Fixture::new().coverage();
    assert_eq!(
        tallies(&c.files, |f| f.phase),
        BTreeMap::from([
            (0, [(0, 0), (0, 0), (1, 10), (1, 3)]),
            (1, [(1, 5), (0, 0), (0, 0), (0, 0)]),
        ])
    );
    assert_eq!(
        tallies(&c.files, |f| f.krate.clone()),
        BTreeMap::from([("tsr_demo".to_string(), [(1, 5), (0, 0), (1, 10), (1, 3)])])
    );
}

#[test]
fn port_coverage_renders_states_lines_packages_and_ported_tests() {
    let f = Fixture::new();
    f.write(
        "tools/watch/src/lib.rs",
        "// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileUpdate\n",
    );
    let page = Page::read(&f.0);
    let md = render_markdown(&page);
    for row in [
        "| Files in scope (source and generated) | 2 |\n",
        "| Files ported | 1 (10 of 15 lines, 66.7%) |\n",
        "| Mapped upstream functions | 1 of 2 (50.0%) |\n",
        "| Ported upstream tests | 1 |\n",
        "| Phase | Planned | Lines | In progress | Lines | Ported | Lines | Out of scope | Lines |\n",
        "| 0 | 0 | 0 | 0 | 0 | 1 | 10 | 1 | 3 |\n",
        "| 1 | 1 | 5 | 0 | 0 | 0 | 0 | 0 | 0 |\n",
        "| `tsr_demo` | 1 | 5 | 0 | 0 | 1 | 10 | 1 | 3 |\n",
        "| `internal/demo` | 2 | 1 (50.0%) | 0 |\n",
        "| `internal/fswatch` | 0 | — | 1 |\n",
        "## Unknown markers\n\nNone.\n",
    ] {
        assert!(md.contains(row), "{row}in\n{md}");
    }
    assert!(
        !md.contains("tsr_testrunner"),
        "harness files are not tallied"
    );
    assert!(!md.contains("Invalid ledger inputs"));

    let html = render_html(&page);
    assert!(html.contains("<tr><td>0</td><td class=\"num\">0</td><td class=\"num\">0</td><td class=\"num\">0</td><td class=\"num\">0</td><td class=\"num\">1</td><td class=\"num\">10</td><td class=\"num\">1</td><td class=\"num\">3</td></tr>"), "{html}");
    assert!(html.contains("<td><code>internal/fswatch</code></td><td class=\"num\">0</td><td class=\"num\">—</td><td class=\"num\">1</td>"));
}

#[test]
fn unknown_markers_and_invalid_ledger_inputs_are_listed() {
    let f = Fixture::new();
    f.write(
        "tools/x/src/lib.rs",
        "// port: tsc/internal/demo/demo.go:Gone\n",
    );
    f.replace("PORTS.toml", "status = \"ported\"", "status = \"verified\"");
    let page = Page::read(&f.0);
    let md = render_markdown(&page);
    assert!(md.contains("- `tsc/internal/demo/demo.go:Gone`\n"), "{md}");
    assert!(md.contains("## Invalid ledger inputs\n\n- tsc/internal/demo/demo.go: status must be planned, in-progress, ported or out-of-scope, not verified\n"));
    assert!(md.contains("| Files ported | 0 (0 of 15 lines, 0.0%) |\n"));
    let html = render_html(&page);
    assert!(html.contains("<li><code>tsc/internal/demo/demo.go:Gone</code></li>"));
    assert!(html.contains("<h2>Invalid ledger inputs</h2>"));
}

#[test]
fn the_page_is_one_self_contained_document_with_every_section() {
    let f = Fixture::new();
    parity(
        &f,
        "compiler",
        PIN,
        1,
        &json!({"compiler/x.ts/types": {"reason": "<script>alert(1)</script>"}}),
    );
    let html = render_html(&Page::read(&f.0));
    assert!(html.starts_with("<!doctype html>\n"));
    for absent in ["<script", "<link", "src=", "http"] {
        assert!(!html.contains(absent), "{absent}");
    }
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    for section in [
        "<h2>Suite parity</h2>",
        "<h2>Performance</h2>",
        "<h2>Port coverage</h2>",
        "<h3>By phase</h3>",
        "<h3>By crate</h3>",
        "<h3>By package</h3>",
        "<h2>Unknown markers</h2>",
        "<a href=\"unmapped-functions.json\">",
    ] {
        assert!(html.contains(section), "{section}");
    }
}

#[test]
fn status_writes_its_three_files_into_the_output_directory_only() {
    let f = Fixture::new();
    parity(&f, "compiler", PIN, 1, &json!({}));
    let out = f.0.join("target/status");
    write(&out, &Page::read(&f.0));
    let mut written: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|e| file_name(&e.unwrap().path()))
        .collect();
    written.sort();
    assert_eq!(
        written,
        ["STATUS.md", "index.html", "unmapped-functions.json"]
    );
    for tracked in [
        "STATUS.md",
        "docs",
        "status/status.json",
        "status/unmapped-functions.json",
        "status/history.jsonl",
    ] {
        assert!(!f.0.join(tracked).exists(), "{tracked}");
    }
    let worklist: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("unmapped-functions.json")).unwrap()).unwrap();
    assert_eq!(
        worklist,
        json!({
            "schema_version": 1,
            "pin": PIN,
            "unmapped_functions": {"internal/demo": ["tsc/internal/demo/demo.go:B.Map"]},
        })
    );
}

#[test]
fn the_worklist_is_sorted_and_independent_of_inventory_order() {
    let f = Fixture::new();
    let first = render_worklist(&f.coverage());
    let text = fs::read_to_string(f.0.join("data/go-functions.tsv")).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines[2..].reverse();
    f.write("data/go-functions.tsv", &(lines.join("\n") + "\n"));
    f.manifest();
    f.write("crates/demo/lib.rs", "");
    let unmapped = f.coverage();
    assert!(unmapped.errors.is_empty(), "{:?}", unmapped.errors);
    let worklist: serde_json::Value = serde_json::from_str(&render_worklist(&unmapped)).unwrap();
    assert_eq!(
        worklist["unmapped_functions"],
        json!({"internal/demo": ["tsc/internal/demo/demo.go:A.Map", "tsc/internal/demo/demo.go:B.Map"]})
    );
    f.write(
        "crates/demo/lib.rs",
        "// port: tsc/internal/demo/demo.go:A.Map\n",
    );
    assert_eq!(render_worklist(&f.coverage()), first);
    f.write(
        "crates/demo/lib.rs",
        "// port: tsc/internal/demo/demo.go:A.Map\n// port: tsc/internal/demo/demo.go:B.Map\n",
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&render_worklist(&f.coverage())).unwrap()
            ["unmapped_functions"],
        json!({})
    );
}
