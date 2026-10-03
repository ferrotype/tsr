//! `cargo xtask status`: suite parity (`status/parity/*.json`), performance
//! runs (`status/perf/**/*.json`) and port coverage, rendered as `STATUS.md`,
//! `index.html` and `unmapped-functions.json`. Both pages are written from one
//! list of blocks, so they show the same sections.

use crate::{counted, die, Coverage, LedgerFile, STATES};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- inputs

/// `status/parity/<suite>.json`: the variants the suite's runner enumerates and
/// every sub-test that does not pass, by id.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Parity {
    suite: String,
    pin: String,
    total: u64,
    failing: BTreeMap<String, Failure>,
}

/// An approved entry still fails; `approved` carries the owner's words.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    reason: String,
    #[serde(default)]
    approved: Option<String>,
}

/// `status/perf/<workload>/<run>.json`, one measurement run. Only the fields
/// the page shows are read; the samples stay in the file.
#[derive(Deserialize)]
struct PerfRun {
    revision: String,
    host: PerfHost,
    recorded_at: String,
    ratios: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct PerfHost {
    label: String,
}

struct Workload {
    name: String,
    runs: Vec<PerfRun>,
}

/// `status/perf/thresholds.toml`: a `[<workload>]` table of
/// `<ratio> = <threshold>` entries for each workload with thresholds.
type Thresholds = BTreeMap<String, BTreeMap<String, f64>>;

pub(crate) struct Page {
    generated: String,
    pub(crate) coverage: Coverage,
    suites: Vec<Parity>,
    /// `None` when `status/perf` does not exist.
    perf: Option<Vec<Workload>>,
    thresholds: Thresholds,
}

impl Page {
    pub(crate) fn read(root: &Path) -> Self {
        let perf_dir = root.join("status/perf");
        let mut suites: Vec<Parity> = json_files(&root.join("status/parity"), false)
            .iter()
            .map(|p| read_json(p))
            .collect();
        suites.sort_by(|a, b| a.suite.cmp(&b.suite));
        Self {
            generated: today(),
            coverage: crate::coverage(root),
            suites,
            perf: perf_dir.is_dir().then(|| read_workloads(&perf_dir)),
            thresholds: read_thresholds(&perf_dir.join("thresholds.toml")),
        }
    }
}

/// Every run under each workload directory, oldest first.
fn read_workloads(dir: &Path) -> Vec<Workload> {
    sorted_entries(dir)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| {
            let mut runs: Vec<PerfRun> =
                json_files(&p, true).iter().map(|p| read_json(p)).collect();
            // Stable: runs recorded at the same time keep their file-name order.
            runs.sort_by(|a, b| a.recorded_at.cmp(&b.recorded_at));
            Workload {
                name: file_name(&p),
                runs,
            }
        })
        .collect()
}

fn read_thresholds(p: &Path) -> Thresholds {
    match fs::read_to_string(p) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Thresholds::new(),
        Err(e) => die(&format!("{}: {e}", p.display())),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> T {
    let bytes = fs::read(p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())))
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

fn json_files(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in sorted_entries(dir) {
        if p.is_dir() {
            if recursive {
                out.extend(json_files(&p, true));
            }
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- content

/// Files and lines per state, in `STATES` order.
type Tally = [(usize, i64); 4];

/// Every non-harness ledger file by `key`; a file whose kind or status is out
/// of scope counts as out of scope.
fn tallies<K: Ord>(files: &[LedgerFile], key: impl Fn(&LedgerFile) -> K) -> BTreeMap<K, Tally> {
    let mut out: BTreeMap<K, Tally> = BTreeMap::new();
    for f in files.iter().filter(|f| f.kind != "harness") {
        let state = if f.kind == "out-of-scope" {
            "out-of-scope"
        } else {
            f.status.as_str()
        };
        if let Some(i) = STATES.iter().position(|s| *s == state) {
            let cell = &mut out.entry(key(f)).or_insert([(0, 0); 4])[i];
            cell.0 += 1;
            cell.1 += f.loc;
        }
    }
    out
}

fn tally_rows<K>(tallies: BTreeMap<K, Tally>, label: impl Fn(K) -> String) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for (key, tally) in tallies {
        let mut row = vec![label(key)];
        for (files, lines) in tally {
            row.extend([files.to_string(), lines.to_string()]);
        }
        rows.push(row);
    }
    rows
}

fn coverage_summary(c: &Coverage) -> Vec<Vec<String>> {
    let files = || c.files.iter().filter(|f| counted(f));
    let ported = || files().filter(|f| f.status == "ported");
    let lines: i64 = files().map(|f| f.loc).sum();
    let ported_lines: i64 = ported().map(|f| f.loc).sum();
    let stale = ported()
        .filter(|f| !f.pin.is_empty() && !c.pin.is_empty() && f.pin != c.pin)
        .count();
    [
        (
            "Files in scope (source and generated)",
            files().count().to_string(),
        ),
        (
            "Files ported",
            format!(
                "{} ({ported_lines} of {lines} lines, {})",
                ported().count(),
                pct(ported_lines as f64, lines as f64)
            ),
        ),
        (
            "Mapped upstream functions",
            format!(
                "{} of {} ({})",
                c.mapped,
                c.functions,
                pct(c.mapped as f64, c.functions as f64)
            ),
        ),
        ("Ported upstream tests", c.tests.to_string()),
        ("Ported entries stale against the pin", stale.to_string()),
    ]
    .into_iter()
    .map(|(label, value)| vec![label.to_string(), value])
    .collect()
}

pub(crate) fn summary_line(page: &Page) -> String {
    let c = &page.coverage;
    let files = || c.files.iter().filter(|f| counted(f));
    let mut line = format!(
        "status: {} files in scope, {} ported, {} of {} functions mapped",
        files().count(),
        files().filter(|f| f.status == "ported").count(),
        c.mapped,
        c.functions
    );
    for s in &page.suites {
        line.push_str(&format!(
            "; {}: {} failing ({} approved) of {} variants",
            s.suite,
            s.failing.len(),
            approved(s),
            s.total
        ));
    }
    line
}

fn approved(s: &Parity) -> usize {
    s.failing.values().filter(|f| f.approved.is_some()).count()
}

fn pct(n: f64, d: f64) -> String {
    format!("{:.1}%", if d == 0.0 { 0.0 } else { n / d * 100.0 })
}

fn short(hex: &str) -> String {
    hex.chars().take(8).collect()
}

/// The workload's ratio names, in name order, across all its runs.
fn ratio_names(w: &Workload) -> Vec<&str> {
    let mut names: Vec<&str> = w
        .runs
        .iter()
        .flat_map(|r| r.ratios.keys().map(String::as_str))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn ratio_cell(run: &PerfRun, name: &str, thresholds: Option<&BTreeMap<String, f64>>) -> String {
    match (run.ratios.get(name), thresholds.and_then(|t| t.get(name))) {
        (None, _) => "—".into(),
        (Some(value), None) => format!("{value:.3}"),
        (Some(value), Some(threshold)) => format!("{value:.3} (threshold {threshold})"),
    }
}

/// One block of the rendered pages. Text may hold Markdown `code` and
/// **strong** spans, which the HTML page converts.
enum Block {
    Heading(usize, String),
    Text(String),
    /// Header cells, the number of leading left-aligned columns, the rows.
    Table(Vec<String>, usize, Vec<Vec<String>>),
    List(Vec<String>),
}

fn cells(texts: &[&str]) -> Vec<String> {
    texts.iter().map(ToString::to_string).collect()
}

fn blocks(page: &Page) -> Vec<Block> {
    use Block::{Heading, List, Table, Text};
    let c = &page.coverage;
    let mut b = vec![
        Heading(2, "Suite parity".into()),
        Text("Each expectation file names every failing sub-test of its suite; CI requires the observed failures to equal it. An approved entry still fails.".into()),
    ];
    if page.suites.is_empty() {
        b.push(Text("No expectation files under `status/parity`.".into()));
    } else {
        let rows = page
            .suites
            .iter()
            .map(|p| {
                let pin = if p.pin == c.pin {
                    "ledger pin".to_string()
                } else {
                    format!("**`{}` differs from the ledger pin**", short(&p.pin))
                };
                vec![
                    p.suite.clone(),
                    pin,
                    p.total.to_string(),
                    p.failing.len().to_string(),
                    approved(p).to_string(),
                ]
            })
            .collect();
        let head = ["Suite", "Pin", "Variants", "Failing sub-tests", "Approved"];
        b.push(Table(cells(&head), 2, rows));
        for p in &page.suites {
            b.push(Heading(3, p.suite.clone()));
            if p.failing.is_empty() {
                b.push(Text("No failing sub-tests.".into()));
                continue;
            }
            let items = p.failing.iter().map(|(id, f)| match &f.approved {
                Some(a) => format!("`{id}`: {} **Approved:** {}", f.reason.trim(), a.trim()),
                None => format!("`{id}`: {}", f.reason.trim()),
            });
            b.push(List(items.collect()));
        }
    }

    b.push(Heading(2, "Performance".into()));
    match &page.perf {
        None => b.push(Text(
            "No `status/perf` directory; no runs are recorded.".into(),
        )),
        Some(workloads) if workloads.is_empty() => {
            b.push(Text("No workloads under `status/perf`.".into()));
        }
        Some(workloads) => {
            b.push(Text(
                "Ratios are Rust over Go, one row per recorded run.".into(),
            ));
            for w in workloads {
                let names = ratio_names(w);
                let thresholds = page.thresholds.get(&w.name);
                let rows = w
                    .runs
                    .iter()
                    .map(|run| {
                        let mut row = vec![
                            run.recorded_at.clone(),
                            format!("`{}`", short(&run.revision)),
                            run.host.label.clone(),
                        ];
                        row.extend(names.iter().map(|n| ratio_cell(run, n, thresholds)));
                        row
                    })
                    .collect();
                b.push(Heading(3, w.name.clone()));
                b.push(Table(
                    [cells(&["Recorded", "Revision", "Host"]), cells(&names)].concat(),
                    3,
                    rows,
                ));
            }
        }
    }

    b.push(Heading(2, "Port coverage".into()));
    b.push(Text("Function counts measure mapping by `port:` markers, not semantic completeness. Ported tests are the distinct Go tests named by `source: <path>_test.go:<Name>` comments.".into()));
    b.push(Table(cells(&["", ""]), 1, coverage_summary(c)));
    let head = |first: &str| {
        cells(&[
            first,
            "Planned",
            "Lines",
            "In progress",
            "Lines",
            "Ported",
            "Lines",
            "Out of scope",
            "Lines",
        ])
    };
    b.push(Heading(3, "By phase".into()));
    let phases = tallies(&c.files, |f| f.phase);
    b.push(Table(
        head("Phase"),
        1,
        tally_rows(phases, |p| p.to_string()),
    ));
    b.push(Heading(3, "By crate".into()));
    let crates = tallies(&c.files, |f| f.krate.clone());
    b.push(Table(
        head("Crate"),
        1,
        tally_rows(crates, |k| format!("`{k}`")),
    ));
    b.push(Heading(3, "By package".into()));
    b.push(Text(
        "The unmapped functions of each package are listed in `unmapped-functions.json`, beside this page.".into(),
    ));
    let rows = c
        .packages
        .iter()
        .map(|(pkg, p)| {
            let mapped = if p.functions == 0 {
                "—".to_string()
            } else {
                format!(
                    "{} ({})",
                    p.mapped,
                    pct(p.mapped as f64, p.functions as f64)
                )
            };
            vec![
                format!("`{pkg}`"),
                p.functions.to_string(),
                mapped,
                p.tests.to_string(),
            ]
        })
        .collect();
    let head = ["Package", "Functions", "Mapped", "Ported tests"];
    b.push(Table(cells(&head), 1, rows));

    b.push(Heading(2, "Unknown markers".into()));
    if c.unknown_markers.is_empty() {
        b.push(Text("None.".into()));
    } else {
        b.push(Text(
            "These name no upstream function; stale after a pin bump?".into(),
        ));
        b.push(List(
            c.unknown_markers.iter().map(|m| format!("`{m}`")).collect(),
        ));
    }
    if !c.errors.is_empty() {
        b.push(Heading(2, "Invalid ledger inputs".into()));
        b.push(List(c.errors.clone()));
    }
    b
}

// ---------------------------------------------------------------- outputs

fn render_markdown(page: &Page) -> String {
    fn row(s: &mut String, cells: &[String]) {
        s.push('|');
        for cell in cells {
            s.push(' ');
            s.push_str(&cell.replace('|', "\\|").replace('\n', " "));
            s.push_str(" |");
        }
        s.push('\n');
    }
    let mut s = format!(
        "# Status\n\nGenerated by `cargo xtask status` on {} against upstream pin `{}`, from `status/parity`, `status/perf`, `PORTS.toml`, `data/go-functions.tsv` and the `port:` and `source:` markers; not committed. [Unmapped functions](unmapped-functions.json).\n",
        page.generated, page.coverage.pin
    );
    for block in blocks(page) {
        s.push('\n');
        match block {
            Block::Heading(level, text) => {
                s.push_str(&format!("{} {text}\n", "#".repeat(level)));
            }
            Block::Text(text) => {
                s.push_str(&text);
                s.push('\n');
            }
            Block::List(items) => {
                for item in items {
                    s.push_str(&format!("- {item}\n"));
                }
            }
            Block::Table(head, left, rows) => {
                row(&mut s, &head);
                s.push('|');
                for i in 0..head.len() {
                    s.push_str(if i < left { "---|" } else { "---:|" });
                }
                s.push('\n');
                for cells in &rows {
                    row(&mut s, cells);
                }
            }
        }
    }
    s
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escape `text` and turn its Markdown `code` and **strong** spans into HTML.
/// A strong span may hold code spans; `**` inside a code span is literal, and
/// a span left open at the end is closed there.
fn inline_html(text: &str) -> String {
    let escaped = escape_html(text);
    let (mut out, mut code, mut strong) = (String::new(), false, false);
    let mut rest = escaped.as_str();
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            out.push_str(if code { "</code>" } else { "<code>" });
            code = !code;
            rest = &rest[1..];
        } else if !code && rest.starts_with("**") {
            out.push_str(if strong { "</strong>" } else { "<strong>" });
            strong = !strong;
            rest = &rest[2..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if code {
        out.push_str("</code>");
    }
    if strong {
        out.push_str("</strong>");
    }
    out
}

const STYLE: &str = r#"
  :root { --bg:#f6f7f9; --surface:#fff; --ink:#1a2029; --ink-2:#4b5665; --line:#d7dde6; --accent:#245fa6; color-scheme: light dark; }
  @media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg:#101418; --surface:#161b21; --ink:#e4e8ee; --ink-2:#a6b0be; --line:#2a323c; --accent:#7ab4f5; } }
  :root[data-theme="dark"] { --bg:#101418; --surface:#161b21; --ink:#e4e8ee; --ink-2:#a6b0be; --line:#2a323c; --accent:#7ab4f5; }
  body { margin:0; background:var(--bg); color:var(--ink); font:400 15px/1.5 system-ui, sans-serif; }
  main { max-width:1100px; margin:0 auto; padding:32px 16px 60px; }
  h1 { font:600 30px/1.2 Georgia, serif; margin:0 0 6px; }
  h2 { font:600 21px/1.3 Georgia, serif; margin:36px 0 10px; }
  h3 { font:600 16px/1.3 system-ui, sans-serif; margin:22px 0 8px; }
  a { color:var(--accent); }
  code { font:13px ui-monospace, monospace; overflow-wrap:anywhere; }
  .eyebrow { font-size:12px; letter-spacing:.08em; text-transform:uppercase; color:var(--ink-2); }
  .scroll { overflow-x:auto; background:var(--surface); border:1px solid var(--line); border-radius:4px; padding:4px 12px; }
  table { border-collapse:collapse; width:100%; font-size:14px; }
  th, td { text-align:left; padding:7px 12px 7px 0; border-bottom:1px solid var(--line); vertical-align:top; }
  tr:last-child td { border-bottom:none; }
  th { font-size:12px; letter-spacing:.05em; text-transform:uppercase; color:var(--ink-2); }
  .num { text-align:right; font-variant-numeric:tabular-nums; white-space:nowrap; }
  li { margin:4px 0; }
"#;

fn render_html(page: &Page) -> String {
    fn row(b: &mut String, tag: &str, left: usize, cells: &[String]) {
        b.push_str("<tr>");
        for (i, cell) in cells.iter().enumerate() {
            let class = if i < left { "" } else { " class=\"num\"" };
            b.push_str(&format!("<{tag}{class}>{}</{tag}>", inline_html(cell)));
        }
        b.push_str("</tr>\n");
    }
    let mut b = String::new();
    for block in blocks(page) {
        match block {
            Block::Heading(level, text) => {
                b.push_str(&format!("<h{level}>{}</h{level}>\n", inline_html(&text)));
            }
            Block::Text(text) => b.push_str(&format!("<p>{}</p>\n", inline_html(&text))),
            Block::List(items) => {
                b.push_str("<ul>\n");
                for item in items {
                    b.push_str(&format!("<li>{}</li>\n", inline_html(&item)));
                }
                b.push_str("</ul>\n");
            }
            Block::Table(head, left, rows) => {
                b.push_str("<div class=\"scroll\"><table>\n");
                if head.iter().any(|h| !h.is_empty()) {
                    b.push_str("<thead>");
                    row(&mut b, "th", left, &head);
                    b.push_str("</thead>");
                }
                b.push_str("<tbody>\n");
                for cells in &rows {
                    row(&mut b, "td", left, cells);
                }
                b.push_str("</tbody></table></div>\n");
            }
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>ts-rust status</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n<div class=\"eyebrow\">Generated {} &middot; upstream pin <code>{}</code></div>\n<h1>Status</h1>\n<p>Rendered by <code>cargo xtask status</code> from <code>status/parity</code>, <code>status/perf</code>, <code>PORTS.toml</code>, <code>data/go-functions.tsv</code> and the markers. <a href=\"STATUS.md\">Markdown</a> &middot; <a href=\"unmapped-functions.json\">Unmapped functions</a></p>\n{b}</main>\n</body>\n</html>\n",
        escape_html(&page.generated),
        escape_html(&page.coverage.pin)
    )
}

fn render_worklist(c: &Coverage) -> String {
    let mut text = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "pin": c.pin,
        "unmapped_functions": c.unmapped,
    }))
    .expect("a map of strings serializes");
    text.push('\n');
    text
}

/// Write `STATUS.md`, `index.html` and `unmapped-functions.json` into `out`.
pub(crate) fn write(out: &Path, page: &Page) {
    fs::create_dir_all(out).unwrap_or_else(|e| die(&format!("{}: {e}", out.display())));
    for (name, text) in [
        ("STATUS.md", render_markdown(page)),
        ("index.html", render_html(page)),
        ("unmapped-functions.json", render_worklist(&page.coverage)),
    ] {
        let p = out.join(name);
        fs::write(&p, text).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }
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

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
