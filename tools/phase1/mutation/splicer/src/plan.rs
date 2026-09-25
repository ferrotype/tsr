//! `plan`: resolve the marker sites the Python planner found to functions,
//! match arms and statements, choose operators, pair every allocating mutant
//! with a control, number the mutants and record every operation's homes.
//!
//! The plan is deterministic: the same sources and the same site list (in any
//! order) produce byte-identical output. Mutants are numbered 1.. in (file,
//! site line, site column, site kind, rank) order, a control ranking after the
//! site's mutants, and each carries the stable key
//! sha256(`op|file|function|site_line|operator|span_sha256`)[..16].
//!
//! plan.json (canonical JSON):
//!
//! ```text
//! {"version": 1, "root_tree": <git tree id of the planned sources>,
//!  "missing_node_chain": [Parser methods that never return a missing node],
//!  "counts": {"mutants", "controls", "sites", "homes", "homes_without_mutants",
//!             "ops_with_mutants", "ops_without_mutants"},
//!  "mutants": [{"id", "key", "op" (first of "ops"), "ops" (every operation whose
//!     marker stands on the site), "file", "function", "site_kind" (fn | macro_fn |
//!     arm | stmt), "site_line", "markers" {op: marker line}, "span" [start, end]
//!     (1-based, inclusive, from the topmost marker), "span_sha256", "operator",
//!     "return_category", "crate", "hit_fn" (hit_parser only for a tsr_parser
//!     site whose every operation is under tsc/internal/parser/, else hit),
//!     "control" (the paired control's id, or null),
//!     "insert": [{"line", "column" (UTF-8 byte offset in the line), "order", "text"}]}
//!     A control is an entry of its own with "operator": "control" and
//!     "control_of": <the mutant's id>, at the mutant's site and span.],
//!  "homes": {op: [{"file", "function", "site_kind", "site_line", "span",
//!     "span_sha256", "marker_line", "mutants" [keys, controls excluded],
//!     "reason" (why the home has no mutant, else null)}]},
//!  "unsupported": [{"op", "file", "function", "reason"}]}
//! ```
//!
//! `insert` is a list because an arm, a negated condition and a wrapper each
//! need an opening and a closing text; at one position, texts apply in
//! ascending `order`.
//!
//! Homes are every production marker site of every requested operation:
//! function, macro-body function, match arm and statement sites, including
//! sites with no operator and statement markers that resolve to no statement
//! (those carry `mutants: []` and a reason). An operation with no marker has an
//! empty home list. Test-only code is never a home.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::index::{index_file, FileIndex, FnInfo, Pos, StmtKind};
use crate::operators::{self, Choice, Operator, Scope, Shape, Site, MAX_OPERATORS, PARAM, RESULT};
use crate::source::{is_comment_or_blank, is_marker_line, mutant_key, Source};
use crate::types::{type_head, type_names, TypeIndex};

pub const PLAN_VERSION: u64 = 1;
/// The sources whose sites may call `hit_parser`.
pub const PARSER_SOURCES: &str = "crates/tsr_parser/src/";
/// The operations Go counts while observing (lazy JSDoc parsing during e1
/// encoding): `phase1_mutation_go` `observe_counts` of the e1 oracle.
pub const PARSER_OPERATIONS: &str = "tsc/internal/parser/";
/// Orders a wrapper's opening text after every guard at the same position.
const WRAP_ORDER: i64 = 1 << 40;
const CHAIN_ROOTS: [&str; 2] = ["create_missing_identifier", "create_missing_list"];
/// The operator name of a control entry.
pub const CONTROL: &str = "control";
const RESULT_VAR: &str = "__phase1_mutant_result";

/// One `port:` marker the Python planner resolved with the scope's rules.
#[derive(Clone, Debug)]
pub struct SiteRequest {
    pub op: String,
    pub file: String,
    pub marker_line: usize,
    /// `fn` (matched by `phase1_scope._PORT_ANNOTATION`) or `statement` (a
    /// line-prefix marker the function rule does not attribute).
    pub marker_kind: String,
    pub function: Option<String>,
    pub fn_line: Option<usize>,
}

/// An operation with no usable site, and why.
#[derive(Clone, Debug)]
pub struct Unsited {
    pub op: String,
    pub reason: String,
}

fn field<'v>(value: &'v Value, name: &str) -> Result<&'v Value, String> {
    value
        .get(name)
        .ok_or_else(|| format!("missing field {name:?} in {value}"))
}

fn text_field(value: &Value, name: &str) -> Result<String, String> {
    field(value, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("field {name:?} is not a string in {value}"))
}

fn line_field(value: &Value, name: &str) -> Result<usize, String> {
    field(value, name)?
        .as_u64()
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)
        .ok_or_else(|| format!("field {name:?} is not a line number in {value}"))
}

/// Reads a site document: a bare list of sites, or `{"sites": [..],
/// "unsited": [{"op", "reason"}]}`.
pub fn read_sites(document: &Value) -> Result<(Vec<SiteRequest>, Vec<Unsited>), String> {
    let (sites, unsited) = match document {
        Value::Array(sites) => (sites.as_slice(), &[][..]),
        Value::Object(_) => (
            field(document, "sites")?
                .as_array()
                .ok_or("sites is not a list")?
                .as_slice(),
            document
                .get("unsited")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice),
        ),
        _ => return Err("the site document is neither a list nor an object".to_owned()),
    };
    let sites = sites
        .iter()
        .map(|site| {
            let marker_kind = text_field(site, "marker_kind")?;
            let (function, fn_line) = match marker_kind.as_str() {
                "fn" => (
                    Some(text_field(site, "function")?),
                    Some(line_field(site, "fn_line")?),
                ),
                "statement" => (None, None),
                other => return Err(format!("unknown marker_kind {other:?}")),
            };
            Ok(SiteRequest {
                op: text_field(site, "op")?,
                file: text_field(site, "file")?,
                marker_line: line_field(site, "marker_line")?,
                marker_kind,
                function,
                fn_line,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let unsited = unsited
        .iter()
        .map(|entry| {
            Ok(Unsited {
                op: text_field(entry, "op")?,
                reason: text_field(entry, "reason")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((sites, unsited))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Target {
    Fn(usize),
    Arm(usize),
    Stmt(usize),
}

struct Unsupported {
    op: String,
    file: Option<String>,
    function: Option<String>,
    reason: String,
}

impl Unsupported {
    fn to_json(&self) -> Value {
        json!({"op": self.op, "file": self.file, "function": self.function, "reason": self.reason})
    }

    fn sort_key(&self) -> (String, String, String, String) {
        (
            self.op.clone(),
            self.file.clone().unwrap_or_default(),
            self.function.clone().unwrap_or_default(),
            self.reason.clone(),
        )
    }
}

/// Where a mutant stands: its file, site position and site kind.
type SiteKey = (String, Pos, &'static str);

/// The non-control mutants of each site, as (id, key) in id order.
type SiteMutants = BTreeMap<SiteKey, Vec<(usize, String)>>;

/// One text insertion: (line, byte column, order, text). `{ID}` and `{HIT}`
/// are filled in once ids are assigned.
type Insert = (usize, usize, i64, String);

struct Draft {
    file: String,
    site_kind: &'static str,
    site: Pos,
    rank: usize,
    function: String,
    ops: Vec<String>,
    markers: BTreeMap<String, usize>,
    span: (usize, usize),
    span_sha256: String,
    operator: String,
    category: String,
    krate: String,
    hit_fn: &'static str,
    /// For an allocating mutant: its control's rank.
    control_rank: Option<usize>,
    /// For a control: the rank of the mutant it controls.
    control_of_rank: Option<usize>,
    inserts: Vec<Insert>,
}

impl Draft {
    fn site_key(&self) -> SiteKey {
        (self.file.clone(), self.site, self.site_kind)
    }
}

/// One marker site of one or more operations.
struct HomeDraft {
    file: String,
    function: Option<String>,
    site_kind: &'static str,
    site: Pos,
    span: (usize, usize),
    span_sha256: String,
    markers: BTreeMap<String, usize>,
    reason: Option<String>,
}

pub(crate) fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut paths: Vec<_> = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    paths.sort();
    for path in paths {
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The `Parser` methods reachable from `create_missing_identifier` and
/// `create_missing_list` through `self.method(..)` calls: none of them may
/// return a missing node, or creating one would recurse.
pub fn missing_node_chain(root: &Path) -> Result<BTreeSet<String>, String> {
    let dir = root.join("crates/tsr_parser/src");
    let mut graph: HashMap<String, BTreeSet<String>> = HashMap::new();
    if dir.is_dir() {
        let mut files = Vec::new();
        rust_files(&dir, &mut files)?;
        for path in files {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let index =
                index_file(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            for function in index.fns {
                if type_head(&function.self_ty) == "Parser" {
                    graph
                        .entry(function.name)
                        .or_default()
                        .extend(function.self_calls);
                }
            }
        }
    }
    let mut chain: BTreeSet<String> = CHAIN_ROOTS.iter().map(|root| (*root).to_owned()).collect();
    let mut pending: Vec<String> = chain.iter().cloned().collect();
    while let Some(name) = pending.pop() {
        for callee in graph.get(&name).into_iter().flatten() {
            if chain.insert(callee.clone()) {
                pending.push(callee.clone());
            }
        }
    }
    Ok(chain)
}

/// (crate directory, package name) of a file under crates/*/src.
fn package_name(root: &Path, file: &str) -> Result<(String, String), String> {
    let mut parts = file.split('/');
    let (Some("crates"), Some(dir), Some("src")) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(format!("{file} is not under crates/*/src/"));
    };
    let manifest = root.join("crates").join(dir).join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(value) = line.strip_prefix("name") {
                let value = value.trim_start();
                if let Some(value) = value.strip_prefix('=') {
                    return Ok((dir.to_owned(), value.trim().trim_matches('"').to_owned()));
                }
            }
        }
    }
    Err(format!("{}: no package name", manifest.display()))
}

fn crate_dir(file: &str) -> &str {
    file.split('/').nth(1).unwrap_or_default()
}

/// The switch function a site's guards call. `hit_parser` records reach and
/// stays selectable while observing, which is sound only where Go counts the
/// same observation-time entries: a `tsr_parser` site whose every operation is
/// an `internal/parser` function. Every other site calls the production-only
/// `hit`, including a `tsr_parser` port of an `internal/ast` utility and a site
/// shared by a parser and a non-parser operation.
pub fn hit_fn<'o>(file: &str, ops: impl IntoIterator<Item = &'o String>) -> &'static str {
    let mut ops = ops.into_iter().peekable();
    let parser = file.starts_with(PARSER_SOURCES)
        && ops.peek().is_some()
        && ops.all(|op| op.starts_with(PARSER_OPERATIONS));
    if parser {
        "hit_parser"
    } else {
        "hit"
    }
}

/// The first line of a site's span: the syntactic start, extended upward to the
/// topmost `port:` marker in the comment and attribute block directly above.
fn span_start(source: &Source, syntactic: usize) -> usize {
    let mut top = syntactic;
    while top > 1 {
        let line = source.line(top - 1).unwrap_or_default().trim();
        let attribute = line.starts_with("#[") && line.ends_with(']');
        if line.starts_with("//") || attribute {
            top -= 1;
        } else {
            break;
        }
    }
    (top..syntactic)
        .find(|line| source.line(*line).is_some_and(is_marker_line))
        .unwrap_or(syntactic)
}

/// The first code line after a statement marker.
fn code_after(source: &Source, marker_line: usize) -> Option<usize> {
    (marker_line + 1..=source.line_count())
        .find(|line| !source.line(*line).is_some_and(is_comment_or_blank))
}

fn resolve(index: &FileIndex, source: &Source, site: &SiteRequest) -> Result<Target, String> {
    if site.marker_kind == "fn" {
        let name = site.function.as_deref().unwrap_or_default();
        let line = site.fn_line.unwrap_or_default();
        return index
            .fns
            .iter()
            .position(|function| function.name == name && function.fn_line == line)
            .map(Target::Fn)
            .ok_or_else(|| {
                format!("fn_marker_unresolved: no function `{name}` with its `fn` on line {line}")
            });
    }
    let target = code_after(source, site.marker_line)
        .ok_or("statement_marker_unresolved: no code follows the marker")?;
    let indent = source.line(target).map_or(0, |line| {
        line.chars().take_while(|c| c.is_whitespace()).count()
    });
    let arms = index
        .arms
        .iter()
        .enumerate()
        .filter(|(_, arm)| arm.start == (target, indent))
        .map(|(at, arm)| (arm.end_line, 1, Target::Arm(at)));
    let stmts = index
        .stmts
        .iter()
        .enumerate()
        .filter(|(_, stmt)| stmt.start == (target, indent))
        .map(|(at, stmt)| (stmt.end_line, 0, Target::Stmt(at)));
    arms.chain(stmts)
        .max()
        .map(|(_, _, target)| target)
        .ok_or_else(|| {
            format!(
                "statement_marker_unresolved: line {target} does not start a statement or match \
                 arm inside a function body"
            )
        })
}

/// The Rust text of a closure return annotation for `function`'s result, or
/// nothing when the written type cannot annotate a closure (an `impl` type, an
/// elided or anonymous lifetime).
fn closure_annotation(function: &FnInfo) -> String {
    let Some(text) = function.ret_text.as_deref() else {
        return String::new();
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    if words
        .iter()
        .any(|word| *word == "impl" || word.contains("'_"))
    {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    for (at, c) in chars.iter().enumerate() {
        if *c == '&' {
            let next = chars[at + 1..].iter().find(|c| !c.is_whitespace());
            if next != Some(&'\'') {
                return String::new();
            }
        }
    }
    format!("-> {text} ")
}

struct FilePlan<'a> {
    root: &'a Path,
    file: String,
    source: Source,
    index: FileIndex,
    /// Resolved targets with the markers standing on each.
    targets: BTreeMap<Target, BTreeMap<String, usize>>,
    /// Statement or function markers that resolved to no site.
    unresolved: Vec<(SiteRequest, String)>,
}

struct Planner<'a> {
    chain: &'a BTreeSet<String>,
    types: &'a TypeIndex,
    packages: HashMap<String, (String, String)>,
    drafts: Vec<Draft>,
    homes: Vec<HomeDraft>,
    unsupported: Vec<Unsupported>,
}

impl FilePlan<'_> {
    fn function_of(&self, target: Target) -> &FnInfo {
        let at = match target {
            Target::Fn(at) => at,
            Target::Arm(at) => self.index.arms[at].function,
            Target::Stmt(at) => self.index.stmts[at].function,
        };
        &self.index.fns[at]
    }

    fn byte_pos(&self, (line, column): Pos) -> Result<(usize, usize), String> {
        let byte = self
            .source
            .byte_column(line, column)
            .ok_or_else(|| format!("{}:{line}:{column} is outside the file", self.file))?;
        Ok((line, byte))
    }

    fn choose(&self, target: Target, chain: &BTreeSet<String>, types: &TypeIndex) -> Choice {
        let function = self.function_of(target);
        let site = Site {
            scope: Scope::of(&self.file, function, chain.contains(&function.name)),
            function,
            types,
            krate: crate_dir(&self.file),
        };
        match target {
            Target::Fn(_) => operators::for_function(site),
            Target::Arm(at) => operators::for_arm(&self.index.arms[at], site),
            Target::Stmt(at) => operators::for_stmt(&self.index.stmts[at]),
        }
    }

    /// The insertions of `operator` at `target`, as the mutant itself or, with
    /// `control`, as its control (the replacement is computed and discarded).
    fn inserts(
        &self,
        target: Target,
        operator: &Operator,
        control: bool,
    ) -> Result<Vec<Insert>, String> {
        let guard = "if ::phase1_mutants::{HIT}({ID})";
        let fill = |value: &str| {
            value
                .replace(RESULT, RESULT_VAR)
                .replace(PARAM, "__phase1_mutant_param_{ID}")
        };
        let use_value = |value: &str| {
            if control {
                format!("{guard} {{ let _ = {}; }}", fill(value))
            } else {
                format!("{guard} {{ return {}; }}", fill(value))
            }
        };
        let mut out = Vec::new();
        match (target, &operator.shape) {
            (Target::Fn(at), shape) => {
                let function = &self.index.fns[at];
                let (line, column) = self.byte_pos(function.open)?;
                match shape {
                    Shape::Guard(None) if !control => {
                        out.push((line, column, 0, format!("{guard} {{ return; }} ")));
                    }
                    Shape::Guard(Some(value)) => {
                        out.push((line, column, 0, format!("{} ", use_value(value))));
                    }
                    Shape::Param { name, mutable, alt } if !control => {
                        // A `mut` parameter is reassigned rather than shadowed, so
                        // its own `mut` stays used.
                        let binding = if *mutable { "" } else { "let " };
                        out.push((
                            line,
                            column,
                            0,
                            format!("{binding}{name} = {guard} {{ {alt} }} else {{ {name} }}; "),
                        ));
                    }
                    Shape::Wrap { value, capture } => {
                        let capture = capture.as_ref().map_or_else(String::new, |param| {
                            format!("let __phase1_mutant_param_{{ID}} = {param}; ")
                        });
                        out.push((
                            line,
                            column,
                            WRAP_ORDER,
                            format!(
                                "{capture}let {RESULT_VAR} = (|| {}{{ ",
                                closure_annotation(function)
                            ),
                        ));
                        let (close_line, close_column) = self.byte_pos(function.close)?;
                        out.push((
                            close_line,
                            close_column,
                            -1,
                            format!(" }})(); {} {RESULT_VAR} ", use_value(value)),
                        ));
                    }
                    _ => return Err(format!("operator {} has no control form", operator.name)),
                }
            }
            (Target::Arm(at), Shape::Arm(value)) => {
                let arm = &self.index.arms[at];
                let (line, column) = self.byte_pos(arm.body_start)?;
                let (end_line, end_column) = self.byte_pos(arm.body_end)?;
                if control {
                    out.push((line, column, 0, format!("{{ {} ", use_value(value))));
                } else {
                    out.push((line, column, 0, format!("{guard} {{ {value} }} else {{ ")));
                }
                out.push((end_line, end_column, -1, " }".to_owned()));
            }
            (Target::Stmt(at), Shape::Negate) if !control => {
                let (StmtKind::If { cond } | StmtKind::LetIf { cond }) = &self.index.stmts[at].kind
                else {
                    return Err("not a conditional statement".to_owned());
                };
                let (line, column) = self.byte_pos(cond.0)?;
                let (end_line, end_column) = self.byte_pos(cond.1)?;
                out.push((
                    line,
                    column,
                    0,
                    "::phase1_mutants::{HIT}({ID}) != (".to_owned(),
                ));
                out.push((end_line, end_column, -1, ")".to_owned()));
            }
            (Target::Stmt(at), Shape::Skip) if !control => {
                let StmtKind::Unit { span } = &self.index.stmts[at].kind else {
                    return Err("not a unit statement".to_owned());
                };
                let (line, column) = self.byte_pos(span.0)?;
                let (end_line, end_column) = self.byte_pos(span.1)?;
                out.push((
                    line,
                    column,
                    0,
                    "if !::phase1_mutants::{HIT}({ID}) { ".to_owned(),
                ));
                out.push((end_line, end_column, -1, " }".to_owned()));
            }
            _ => return Err(format!("operator {} does not fit its site", operator.name)),
        }
        Ok(out)
    }

    /// The enclosing function of a line, or the function item that starts on
    /// the first code line after it.
    fn function_near(&self, marker_line: usize) -> Option<String> {
        let after = code_after(&self.source, marker_line);
        self.index
            .fns
            .iter()
            .filter(|function| function.has_body && !function.macro_body)
            .filter(|function| {
                (function.start_line..=function.end_line).contains(&marker_line)
                    || Some(function.start_line) == after
            })
            .min_by_key(|function| function.end_line - function.start_line)
            .map(|function| function.name.clone())
    }
}

impl Planner<'_> {
    fn reject(
        &mut self,
        plan: &FilePlan<'_>,
        target: Target,
        markers: &BTreeMap<String, usize>,
        reason: &str,
    ) {
        let function = plan.function_of(target).name.clone();
        for op in markers.keys() {
            self.unsupported.push(Unsupported {
                op: op.clone(),
                file: Some(plan.file.clone()),
                function: Some(function.clone()),
                reason: reason.to_owned(),
            });
        }
    }

    fn draft(
        &mut self,
        plan: &FilePlan<'_>,
        target: Target,
        markers: &BTreeMap<String, usize>,
    ) -> Result<(), String> {
        let function = plan.function_of(target);
        if function.cfg_test {
            self.reject(
                plan,
                target,
                markers,
                "cfg_test: the site is test-only code",
            );
            return Ok(());
        }
        let (site_kind, site, end) = match target {
            Target::Fn(_) if function.macro_body => {
                ("macro_fn", (function.fn_line, 0), function.end_line)
            }
            Target::Fn(_) => ("fn", (function.fn_line, 0), function.end_line),
            Target::Arm(at) => (
                "arm",
                plan.index.arms[at].start,
                plan.index.arms[at].end_line,
            ),
            Target::Stmt(at) => (
                "stmt",
                plan.index.stmts[at].start,
                plan.index.stmts[at].end_line,
            ),
        };
        let syntactic = match target {
            Target::Fn(_) => function.start_line,
            _ => site.0,
        };
        let first_marker = markers.values().copied().min().unwrap_or(syntactic);
        let start = span_start(&plan.source, syntactic).min(first_marker);
        let span_sha256 = plan
            .source
            .span_sha256(start, end)
            .ok_or_else(|| format!("{}: invalid span {start}..={end}", plan.file))?;
        let mut home = HomeDraft {
            file: plan.file.clone(),
            function: Some(function.name.clone()),
            site_kind,
            site,
            span: (start, end),
            span_sha256: span_sha256.clone(),
            markers: markers.clone(),
            reason: None,
        };
        if !function.has_body {
            let reason = "no_body: a trait method declaration has no body to mutate";
            self.reject(plan, target, markers, reason);
            home.reason = Some(reason.to_owned());
            self.homes.push(home);
            return Ok(());
        }
        let choice = plan.choose(target, self.chain, self.types);
        if choice.operators.is_empty() {
            let mut reason = format!(
                "no_operator: {} site of category `{}`",
                site_kind, choice.category
            );
            if let Some(ret) = function
                .ret
                .as_deref()
                .filter(|_| matches!(target, Target::Fn(_)))
            {
                reason.push_str(&format!(
                    " (returns `{ret}`), and no bool, Option or integer parameter"
                ));
            }
            for note in &choice.notes {
                reason.push_str("; ");
                reason.push_str(note);
            }
            self.reject(plan, target, markers, &reason);
            home.reason = Some(reason);
            self.homes.push(home);
            return Ok(());
        }
        self.homes.push(home);
        if !self.packages.contains_key(&plan.file) {
            self.packages
                .insert(plan.file.clone(), package_name(plan.root, &plan.file)?);
        }
        let krate = self.packages[&plan.file].1.clone();
        let hit_fn = hit_fn(&plan.file, markers.keys());
        let draft = |rank: usize, operator: String, inserts: Vec<Insert>| Draft {
            file: plan.file.clone(),
            site_kind,
            site,
            rank,
            function: function.name.clone(),
            ops: markers.keys().cloned().collect(),
            markers: markers.clone(),
            span: (start, end),
            span_sha256: span_sha256.clone(),
            operator,
            category: choice.category.clone(),
            krate: krate.clone(),
            hit_fn,
            control_rank: None,
            control_of_rank: None,
            inserts,
        };
        for (rank, operator) in choice.operators.iter().enumerate() {
            let mut mutant = draft(
                rank,
                operator.name.clone(),
                plan.inserts(target, operator, false)?,
            );
            if operator.allocates {
                let control_rank = MAX_OPERATORS + rank;
                mutant.control_rank = Some(control_rank);
                let mut control = draft(
                    control_rank,
                    CONTROL.to_owned(),
                    plan.inserts(target, operator, true)?,
                );
                control.control_of_rank = Some(rank);
                self.drafts.push(control);
            }
            self.drafts.push(mutant);
        }
        Ok(())
    }

    fn unresolved(&mut self, plan: &FilePlan<'_>, request: &SiteRequest, reason: &str) {
        self.unsupported.push(Unsupported {
            op: request.op.clone(),
            file: Some(plan.file.clone()),
            function: request.function.clone(),
            reason: reason.to_owned(),
        });
        let (site_kind, site_line) = if request.marker_kind == "fn" {
            ("fn", request.fn_line.unwrap_or(request.marker_line))
        } else {
            (
                "stmt",
                code_after(&plan.source, request.marker_line).unwrap_or(request.marker_line),
            )
        };
        let span = (request.marker_line, site_line.max(request.marker_line));
        self.homes.push(HomeDraft {
            file: plan.file.clone(),
            function: request
                .function
                .clone()
                .or_else(|| plan.function_near(request.marker_line)),
            site_kind,
            site: (site_line, usize::MAX),
            span,
            span_sha256: plan.source.span_sha256(span.0, span.1).unwrap_or_default(),
            markers: BTreeMap::from([(request.op.clone(), request.marker_line)]),
            reason: Some(reason.to_owned()),
        });
    }
}

fn read_file_plans<'a>(root: &'a Path, sites: &[SiteRequest]) -> Result<Vec<FilePlan<'a>>, String> {
    let mut by_file: BTreeMap<&str, Vec<&SiteRequest>> = BTreeMap::new();
    for site in sites {
        by_file.entry(site.file.as_str()).or_default().push(site);
    }
    let mut plans = Vec::new();
    for (file, requests) in by_file {
        let path = root.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let index = index_file(&text).map_err(|error| format!("{file}: {error}"))?;
        let mut plan = FilePlan {
            root,
            file: file.to_owned(),
            source: Source::new(text),
            index,
            targets: BTreeMap::new(),
            unresolved: Vec::new(),
        };
        for request in requests {
            match resolve(&plan.index, &plan.source, request) {
                Ok(target) => {
                    let marker = plan
                        .targets
                        .entry(target)
                        .or_default()
                        .entry(request.op.clone())
                        .or_insert(request.marker_line);
                    *marker = (*marker).min(request.marker_line);
                }
                Err(reason) => plan.unresolved.push((request.clone(), reason)),
            }
        }
        plans.push(plan);
    }
    Ok(plans)
}

/// Every capitalized name in the types an operator may need to look up.
fn wanted_types(plans: &[FilePlan<'_>]) -> BTreeSet<String> {
    let mut wanted = BTreeSet::new();
    for plan in plans {
        for target in plan.targets.keys() {
            let function = plan.function_of(*target);
            for ty in function
                .ret
                .iter()
                .chain(function.params.iter().map(|param| &param.ty))
            {
                wanted.extend(type_names(ty));
            }
            if let Target::Arm(at) = target {
                if let Some(ty) = &plan.index.arms[*at].value_ty {
                    wanted.extend(type_names(ty));
                }
            }
        }
    }
    wanted
}

fn number(drafts: Vec<Draft>) -> Result<(Vec<Value>, SiteMutants), String> {
    let mut drafts = drafts;
    drafts.sort_by(|left, right| {
        (&left.file, left.site, left.site_kind, left.rank).cmp(&(
            &right.file,
            right.site,
            right.site_kind,
            right.rank,
        ))
    });
    let ids: HashMap<(SiteKey, usize), usize> = drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| ((draft.site_key(), draft.rank), index + 1))
        .collect();
    let linked = |draft: &Draft, rank: Option<usize>| -> Result<Option<usize>, String> {
        rank.map(|rank| {
            ids.get(&(draft.site_key(), rank))
                .copied()
                .ok_or_else(|| format!("{}:{}: unpaired control", draft.file, draft.site.0))
        })
        .transpose()
    };
    let mut mutants = Vec::with_capacity(drafts.len());
    let mut keys = BTreeSet::new();
    let mut by_site: SiteMutants = BTreeMap::new();
    for (index, draft) in drafts.iter().enumerate() {
        let id = index + 1;
        let key = mutant_key(
            &draft.ops[0],
            &draft.file,
            &draft.function,
            draft.site.0,
            &draft.operator,
            &draft.span_sha256,
        );
        if !keys.insert(key.clone()) {
            return Err(format!(
                "duplicate mutant key {key} at {}:{}",
                draft.file, draft.site.0
            ));
        }
        let signed = i64::try_from(id).map_err(|error| error.to_string())?;
        let inserts: Vec<Value> = draft
            .inserts
            .iter()
            .map(|(line, column, order, text)| {
                let order = if *order < 0 { -signed } else { order + signed };
                json!({
                    "line": line,
                    "column": column,
                    "order": order,
                    "text": text.replace("{HIT}", draft.hit_fn).replace("{ID}", &id.to_string()),
                })
            })
            .collect();
        let mut entry = json!({
            "id": id,
            "key": key,
            "op": draft.ops[0],
            "ops": draft.ops,
            "file": draft.file,
            "function": draft.function,
            "site_kind": draft.site_kind,
            "site_line": draft.site.0,
            "markers": draft.markers,
            "span": [draft.span.0, draft.span.1],
            "span_sha256": draft.span_sha256,
            "operator": draft.operator,
            "return_category": draft.category,
            "crate": draft.krate,
            "hit_fn": draft.hit_fn,
            "control": linked(draft, draft.control_rank)?,
            "insert": inserts,
        });
        if let Some(of) = linked(draft, draft.control_of_rank)? {
            entry["control_of"] = json!(of);
        } else {
            by_site.entry(draft.site_key()).or_default().push((id, key));
        }
        mutants.push(entry);
    }
    Ok((mutants, by_site))
}

fn homes_json(
    homes: &[HomeDraft],
    by_site: &SiteMutants,
    ops: &BTreeSet<String>,
) -> BTreeMap<String, Vec<Value>> {
    let mut out: BTreeMap<String, Vec<(SiteKey, Value)>> = BTreeMap::new();
    for home in homes {
        let key: SiteKey = (home.file.clone(), home.site, home.site_kind);
        let mutants: Vec<&str> = by_site
            .get(&key)
            .into_iter()
            .flatten()
            .map(|(_, key)| key.as_str())
            .collect();
        for (op, marker_line) in &home.markers {
            out.entry(op.clone()).or_default().push((
                key.clone(),
                json!({
                    "file": home.file,
                    "function": home.function,
                    "site_kind": home.site_kind,
                    "site_line": home.site.0,
                    "span": [home.span.0, home.span.1],
                    "span_sha256": home.span_sha256,
                    "marker_line": marker_line,
                    "mutants": mutants,
                    "reason": if mutants.is_empty() { json!(home.reason.clone().unwrap_or_else(|| "no_mutant".to_owned())) } else { Value::Null },
                }),
            ));
        }
    }
    ops.iter()
        .map(|op| {
            let mut rows = out.remove(op).unwrap_or_default();
            rows.sort_by(|left, right| left.0.cmp(&right.0));
            (op.clone(), rows.into_iter().map(|(_, row)| row).collect())
        })
        .collect()
}

/// Builds the plan document.
pub fn plan(
    root: &Path,
    sites: &[SiteRequest],
    unsited: &[Unsited],
    root_tree: Option<&str>,
) -> Result<Value, String> {
    let chain = missing_node_chain(root)?;
    let plans = read_file_plans(root, sites)?;
    let types = TypeIndex::build(root, &wanted_types(&plans))?;
    let mut planner = Planner {
        chain: &chain,
        types: &types,
        packages: HashMap::new(),
        drafts: Vec::new(),
        homes: Vec::new(),
        unsupported: unsited
            .iter()
            .map(|entry| Unsupported {
                op: entry.op.clone(),
                file: None,
                function: None,
                reason: entry.reason.clone(),
            })
            .collect(),
    };
    for plan in &plans {
        for (target, markers) in &plan.targets {
            planner.draft(plan, *target, markers)?;
        }
        for (request, reason) in &plan.unresolved {
            planner.unresolved(plan, request, reason);
        }
    }
    let Planner {
        drafts,
        homes,
        mut unsupported,
        ..
    } = planner;
    let (mutants, by_site) = number(drafts)?;
    unsupported.sort_by_key(Unsupported::sort_key);
    unsupported.dedup_by(|left, right| left.sort_key() == right.sort_key());
    let ops: BTreeSet<String> = sites
        .iter()
        .map(|site| site.op.clone())
        .chain(unsited.iter().map(|entry| entry.op.clone()))
        .collect();
    let homes = homes_json(&homes, &by_site, &ops);
    let is_control = |mutant: &&Value| mutant.get("control_of").is_some();
    let with_mutants: BTreeSet<&str> = mutants
        .iter()
        .filter(|mutant| !is_control(mutant))
        .flat_map(|mutant| mutant["ops"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let without: BTreeSet<&str> = ops
        .iter()
        .map(String::as_str)
        .filter(|op| !with_mutants.contains(op))
        .collect();
    let sites_with_mutants: BTreeSet<&SiteKey> = by_site.keys().collect();
    let home_rows: Vec<&Value> = homes.values().flatten().collect();
    let controls = mutants.iter().filter(is_control).count();
    Ok(json!({
        "version": PLAN_VERSION,
        "root_tree": root_tree,
        "missing_node_chain": chain,
        "counts": {
            "mutants": mutants.len() - controls,
            "controls": controls,
            "sites": sites_with_mutants.len(),
            "homes": home_rows.len(),
            "homes_without_mutants": home_rows.iter().filter(|home| home["mutants"].as_array().is_none_or(Vec::is_empty)).count(),
            "ops_with_mutants": with_mutants.len(),
            "ops_without_mutants": without.len(),
        },
        "mutants": mutants,
        "homes": homes,
        "unsupported": unsupported.iter().map(Unsupported::to_json).collect::<Vec<_>>(),
    }))
}
