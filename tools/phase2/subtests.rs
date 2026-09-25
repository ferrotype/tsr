//! Phase 2 C0.7: the Rust side of the three runner sub-tests Phase 2 owns.
//!
//! * Union ordering (`compiler_runner.go:verifyUnionOrdering`): every union
//!   the checker interned must be reproduced by sorting its reversed list and
//!   ten shuffles (pinned seed `NewPCG(1234, 5678)`) with the production
//!   comparator. C0 has one checker per program; C6 runs it per checker.
//! * Source file parent pointers (`verifyParentPointers`): below each
//!   non-default-library source-file root, every node the generated child
//!   visitor reaches has the traversal parent as its recorded parent. The
//!   root itself is not checked. The walk stops at its first failure.
//! * Module-resolution trace (`verifyModuleResolution`,
//!   `harnessutil.TracerForBaselining`): the loader's trace, localized and
//!   sanitized exactly as the baseline tracer writes it.
//!
//! This is declared test code over production entry points; it adds no
//! checker behavior. Verdicts, not counts, are compared with Go.
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_ast::{AstView, ChildVisitor, NodeId, NodeListId, NodeSlice};
use tsr_compiler::Program;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `math/rand/v2` `PCG` with its DXSM output function and `Rand.Shuffle`
/// (Go 1.27.1 `pcg.go`/`rand.go`, 64-bit `uint64n`).
pub struct Pcg {
    hi: u64,
    lo: u64,
}

impl Pcg {
    pub fn new(seed1: u64, seed2: u64) -> Self {
        Self {
            hi: seed1,
            lo: seed2,
        }
    }

    fn next(&mut self) -> (u64, u64) {
        const MUL_HI: u64 = 2_549_297_995_355_413_924;
        const MUL_LO: u64 = 4_865_540_595_714_422_341;
        const INC_HI: u64 = 6_364_136_223_846_793_005;
        const INC_LO: u64 = 1_442_695_040_888_963_407;
        let product = u128::from(self.lo) * u128::from(MUL_LO);
        let hi = ((product >> 64) as u64)
            .wrapping_add(self.hi.wrapping_mul(MUL_LO))
            .wrapping_add(self.lo.wrapping_mul(MUL_HI));
        let (lo, carry) = (product as u64).overflowing_add(INC_LO);
        let hi = hi.wrapping_add(INC_HI).wrapping_add(u64::from(carry));
        self.lo = lo;
        self.hi = hi;
        (hi, lo)
    }

    pub fn uint64(&mut self) -> u64 {
        let (mut hi, lo) = self.next();
        const CHEAP_MUL: u64 = 0xda94_2042_e4dd_58b5;
        hi ^= hi >> 32;
        hi = hi.wrapping_mul(CHEAP_MUL);
        hi ^= hi >> 48;
        hi.wrapping_mul(lo | 1)
    }

    fn uint64n(&mut self, n: u64) -> u64 {
        // Go tests n&(n-1) == 0; Shuffle never passes n < 2.
        if n.is_power_of_two() {
            return self.uint64() & (n - 1);
        }
        let multiply = |value: u64| {
            let product = u128::from(value) * u128::from(n);
            ((product >> 64) as u64, product as u64)
        };
        let (mut hi, mut lo) = multiply(self.uint64());
        if lo < n {
            let threshold = n.wrapping_neg() % n;
            while lo < threshold {
                (hi, lo) = multiply(self.uint64());
            }
        }
        hi
    }

    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            let j = self.uint64n(i as u64 + 1) as usize;
            values.swap(i, j);
        }
    }
}

/// Rust's sort reports a comparator that is not a total order by panicking;
/// that is exactly the inconsistency the sub-test looks for.
const TOTAL_ORDER_PANIC: &str = "does not correctly implement a total order";

fn sort<T: Copy, E>(
    values: &mut [T],
    compare: &mut impl FnMut(T, T) -> std::result::Result<Ordering, E>,
) -> std::result::Result<bool, E> {
    let mut failure = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        values.sort_by(|a, b| match compare(*a, *b) {
            Ok(order) => order,
            Err(error) => {
                failure.get_or_insert(error);
                Ordering::Equal
            }
        });
    }));
    if let Err(payload) = outcome {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("");
        if message.contains(TOTAL_ORDER_PANIC) {
            return Ok(false);
        }
        std::panic::resume_unwind(payload);
    }
    failure.map_or(Ok(true), Err)
}

/// The pinned check over each union: reversed, then ten shuffles, each sorted
/// back must equal the stored order. Returns the number of failed checks.
pub fn inconsistent_orderings<T: Copy + PartialEq, E>(
    unions: &[Vec<T>],
    mut compare: impl FnMut(T, T) -> std::result::Result<Ordering, E>,
) -> std::result::Result<u64, E> {
    let mut inconsistent = 0;
    for types in unions {
        let mut reversed: Vec<T> = types.iter().rev().copied().collect();
        if !sort(&mut reversed, &mut compare)? || reversed != *types {
            inconsistent += 1;
        }
        let mut shuffled = types.clone();
        let mut rng = Pcg::new(1234, 5678);
        for _ in 0..10 {
            rng.shuffle(&mut shuffled);
            if !sort(&mut shuffled, &mut compare)? || shuffled != *types {
                inconsistent += 1;
            }
        }
    }
    Ok(inconsistent)
}

pub fn union_ordering(op: &tsr_checker::Operation<'_>) -> Value {
    let result = (|| -> Result<Value> {
        let unions = op
            .union_types()
            .into_iter()
            .map(|union| op.constituents(union))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let inconsistent =
            inconsistent_orderings(&unions, |a, b| op.compare_type_order(Some(a), Some(b)))?;
        Ok(
            json!({"state":"executed","checkers":1,"unions":unions.len(),"inconsistent":inconsistent}),
        )
    })();
    result.unwrap_or_else(|error| failure(error, "checker_error"))
}

/// A tree the parent walk reads; the program adapter and tests implement it.
pub trait ParentTree<T> {
    fn parent(&self, node: T) -> Result<Option<T>>;
    fn children(&self, node: T) -> Result<Vec<T>>;
    fn describe(&self, node: T) -> String;
}

/// Go's recursive `ForEachChild` walk in the same pre-order. Returns the first
/// failure's message, as the pinned assertion would report it.
pub fn first_parent_failure<T: Copy + PartialEq>(
    tree: &impl ParentTree<T>,
    root: T,
    nodes: &mut u64,
) -> Result<Option<String>> {
    let mut work: Vec<(T, T)> = tree
        .children(root)?
        .into_iter()
        .rev()
        .map(|child| (child, root))
        .collect();
    while let Some((node, parent)) = work.pop() {
        *nodes += 1;
        match tree.parent(node)? {
            None => return Ok(Some("parent node does not exist".into())),
            Some(recorded) if recorded != parent => {
                return Ok(Some(format!(
                    "parent node does not match traversed parent: {}",
                    tree.describe(node)
                )))
            }
            Some(_) => {}
        }
        work.extend(
            tree.children(node)?
                .into_iter()
                .rev()
                .map(|child| (child, node)),
        );
    }
    Ok(None)
}

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<tsr_arena::Error>,
}

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        match self.view.list(id) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

struct FileTree<'a>(AstView<'a>);

impl ParentTree<NodeId> for FileTree<'_> {
    fn parent(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(self.0.node(node)?.parent())
    }
    fn children(&self, node: NodeId) -> Result<Vec<NodeId>> {
        let mut children = Children {
            view: self.0,
            nodes: Vec::new(),
            error: None,
        };
        let _ = self.0.node(node)?.for_each_child(&mut children);
        match children.error {
            Some(error) => Err(error.into()),
            None => Ok(children.nodes),
        }
    }
    fn describe(&self, node: NodeId) -> String {
        self.0.node(node).map_or_else(
            |_| "unreadable node".into(),
            |read| format!("{:?}", read.kind()),
        )
    }
}

pub fn parent_pointers(program: &Program) -> Value {
    let result = (|| -> Result<Value> {
        let (mut files, mut nodes, mut failure) = (0u64, 0u64, None);
        for file in program.files() {
            let path = file
                .bound()
                .view()
                .source_file()?
                .parse_options()
                .path
                .as_bytes()
                .to_vec();
            if program.is_lib(&path) {
                continue;
            }
            files += 1;
            let tree = FileTree(file.bound().view().ast());
            if let Some(message) = first_parent_failure(&tree, file.source(), &mut nodes)? {
                failure = Some(message);
                break;
            }
        }
        Ok(json!({"state":"executed","files":files,"nodes":nodes,"failure":failure}))
    })();
    result.unwrap_or_else(|error| failure(error, "compiler_error"))
}

/// `harnessutil.TracerForBaselining.sanitizeTrace`, including its per-tracer
/// package.json cache (one per compile, as the harness host is).
pub struct Sanitizer {
    current_directory: Vec<u8>,
    case_sensitive: bool,
    package_json: HashMap<Vec<u8>, bool>,
}

impl Sanitizer {
    pub fn new(current_directory: &[u8], case_sensitive: bool) -> Self {
        Self {
            current_directory: current_directory.to_vec(),
            case_sensitive,
            package_json: HashMap::new(),
        }
    }

    fn path(&self, file: &[u8]) -> Vec<u8> {
        tsr_tspath::to_path(file, &self.current_directory, self.case_sensitive)
            .as_bytes()
            .to_vec()
    }

    pub fn sanitize(&mut self, message: &[u8], use_package_json_cache: bool) -> Vec<u8> {
        let version = format!("'{}'", tsr_core::version()).into_bytes();
        if let Some(at) = find(message, &version) {
            let mut result = message[..at].to_vec();
            result.extend_from_slice(b"'FakeTSVersion'");
            result.extend_from_slice(&message[at + version.len()..]);
            return result;
        }
        if let Some(rest) = message
            .strip_suffix(b"' does not exist according to earlier cached lookups.".as_slice())
        {
            let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
            if use_package_json_cache {
                let path = self.path(file);
                if self.package_json.contains_key(&path) {
                    return message.to_vec();
                }
                self.package_json.insert(path, false);
            }
            return [b"File '".as_slice(), file, b"' does not exist."].concat();
        }
        if let Some(rest) =
            message.strip_suffix(b"' exists according to earlier cached lookups.".as_slice())
        {
            let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
            if use_package_json_cache {
                let path = self.path(file);
                if self.package_json.contains_key(&path) {
                    return message.to_vec();
                }
                self.package_json.insert(path, true);
            }
            return [b"Found 'package.json' at '".as_slice(), file, b"'."].concat();
        }
        if use_package_json_cache {
            if let Some(rest) = message.strip_suffix(b"' does not exist.".as_slice()) {
                let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
                let path = self.path(file);
                if let Entry::Vacant(entry) = self.package_json.entry(path) {
                    entry.insert(false);
                    return message.to_vec();
                }
                return [
                    b"File '".as_slice(),
                    file,
                    b"' does not exist according to earlier cached lookups.",
                ]
                .concat();
            }
            if let Some(rest) = message.strip_prefix(b"Found 'package.json' at '".as_slice()) {
                let file = rest.strip_suffix(b"'.".as_slice()).unwrap_or(rest);
                let path = self.path(file);
                if let Entry::Vacant(entry) = self.package_json.entry(path) {
                    entry.insert(true);
                    return message.to_vec();
                }
                return [
                    b"File '".as_slice(),
                    file,
                    b"' exists according to earlier cached lookups.",
                ]
                .concat();
            }
        }
        message.to_vec()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The `.trace.json` text the pinned harness would baseline for this program.
pub fn trace_text(program: &Program) -> Vec<u8> {
    let mut sanitizer = Sanitizer::new(
        program.current_directory(),
        program.host().use_case_sensitive_file_names(),
    );
    let mut text = Vec::new();
    for entry in program.trace() {
        let args: Vec<Vec<u8>> = entry
            .args
            .iter()
            .map(|arg| match arg {
                tsr_module::TraceArg::Text(value) => value.as_bytes().to_vec(),
                tsr_module::TraceArg::Bool(value) => value.to_string().into_bytes(),
            })
            .collect();
        let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
        let message =
            tsr_diagnostics::localize(&tsr_locale::DEFAULT, Some(entry.message), b"", &refs);
        text.extend(sanitizer.sanitize(&message, true));
        text.push(b'\n');
    }
    text
}

pub fn trace(program: &Program, trace_resolution: bool) -> Value {
    if !trace_resolution {
        return json!({"state":"disabled"});
    }
    let text = trace_text(program);
    if text.is_empty() {
        json!({"state":"no_content"})
    } else {
        json!({"state":"content","text_hex":hex(&text)})
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}

fn failure(reason: impl std::fmt::Display, class: &str) -> Value {
    json!({"state":"failed","class":class,"reason":reason.to_string()})
}
