//! End-to-end tests of `plan` and `splice` over a synthetic workspace.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use crate::json::canonical;
use crate::plan::{hit_fn, plan, read_sites};
use crate::source::{mutant_key, Source};
use crate::splice::splice;

const PARSER: &str = r"use tsr_ast::{NodeId, NodeListId, SyntaxKind};

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.createMissingIdentifier
    pub(crate) fn create_missing_identifier(&mut self) -> NodeId {
        let node = self.new_identifier();
        self.finish_node(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.finishNode
    pub(crate) fn finish_node(&mut self, node: NodeId) -> NodeId {
        self.factory.finish(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseThing
    pub(crate) fn parse_thing(&mut self, flag: bool) -> NodeId {
        self.next_token();
        self.create_missing_identifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextToken
    pub(crate) fn next_token(&mut self) -> SyntaxKind {
        self.token = self.scan();
        self.token
    }
    // port: tsc/internal/parser/parser.go:Parser.isThing
    // port: tsc/internal/parser/parser.go:Parser.isOther
    #[inline]
    pub(crate) fn is_thing(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    /// port: tsc/internal/parser/parser.go:Parser.testOnly
    fn test_only() -> bool {
        false
    }
}
";

const FACTS: &str = r"fn compute(&mut self, node: &Node) -> u32 {
    match node.data() {
        // port: tsc/internal/ast/ast.go:PrivateIdentifier.computeSubtreeFacts
        D::PrivateIdentifier(_) => CLASS_FIELDS,
        // port: tsc/internal/ast/ast.go:BigIntLiteral.computeSubtreeFacts
        D::BigIntLiteral(_) => NONE,
        // port: tsc/internal/ast/ast.go:Decorator.computeSubtreeFacts
        D::Decorator(d) => {
            self.propagate(d) | DECORATORS
        }
        _ => NONE,
    }
}

fn statements(kind: K) -> Option<u32> {
    // port: tsc/internal/ast/utilities.go:CanHaveIllegalDecorators
    if matches!(kind, K::A | K::B) {
        return None;
    }
    // port: tsc/internal/ast/utilities.go:ShouldTransformImportCall
    let module = kind.module();
    Some(module)
}

// port: tsc/internal/ast/utilities.go:IsEmittableImport
#[allow(
    clippy::match_same_arms,
)]
fn emittable(node: NodeId) -> bool {
    false
}

macro_rules! reads {
    ($node_id:ty) => {
        /// port: tsc/internal/ast/ast.go:Node.Expression
        $visibility fn expression(&self) -> Option<$node_id> {
            None
        }
    };
}
";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "phase1-mutation-splicer-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in [
            ("crates/tsr_parser/Cargo.toml", "[package]\nname = \"tsr_parser\"\n\n[dependencies]\ntsr_ast = { path = \"../tsr_ast\" }\n"),
            ("crates/tsr_parser/src/lib.rs", PARSER),
            ("crates/tsr_ast/Cargo.toml", "[package]\nname = \"tsr_ast\"\nversion = \"0.1.0\"\n"),
            ("crates/tsr_ast/src/subtree_facts.rs", FACTS),
            ("tools/phase1/mutation/switch/Cargo.toml", "[package]\nname = \"phase1_mutants\"\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(root)
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.0.join(path)).unwrap()
    }

    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.0.join(path), text).unwrap();
    }

    fn plan_file(&self, plan: &Value) -> PathBuf {
        let path = self.0.join("plan.json");
        std::fs::write(&path, canonical(plan)).unwrap();
        path
    }

    fn switch(&self) -> PathBuf {
        self.0.join("tools/phase1/mutation/switch")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fn_site(op: &str, file: &str, marker_line: usize, function: &str, fn_line: usize) -> Value {
    json!({"op": op, "file": file, "marker_line": marker_line, "marker_kind": "fn",
           "function": function, "fn_line": fn_line})
}

fn statement_site(op: &str, file: &str, marker_line: usize) -> Value {
    json!({"op": op, "file": file, "marker_line": marker_line, "marker_kind": "statement"})
}

const P: &str = "crates/tsr_parser/src/lib.rs";
const F: &str = "crates/tsr_ast/src/subtree_facts.rs";

fn sites() -> Vec<Value> {
    vec![
        fn_site(
            "tsc/internal/parser/parser.go:Parser.createMissingIdentifier",
            P,
            4,
            "create_missing_identifier",
            5,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.finishNode",
            P,
            9,
            "finish_node",
            10,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.parseThing",
            P,
            13,
            "parse_thing",
            14,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.nextToken",
            P,
            18,
            "next_token",
            19,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.isThing",
            P,
            23,
            "is_thing",
            26,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.isOther",
            P,
            24,
            "is_thing",
            26,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.testOnly",
            P,
            33,
            "test_only",
            34,
        ),
        statement_site(
            "tsc/internal/ast/ast.go:PrivateIdentifier.computeSubtreeFacts",
            F,
            3,
        ),
        statement_site(
            "tsc/internal/ast/ast.go:BigIntLiteral.computeSubtreeFacts",
            F,
            5,
        ),
        statement_site(
            "tsc/internal/ast/ast.go:Decorator.computeSubtreeFacts",
            F,
            7,
        ),
        statement_site(
            "tsc/internal/ast/utilities.go:CanHaveIllegalDecorators",
            F,
            16,
        ),
        statement_site(
            "tsc/internal/ast/utilities.go:ShouldTransformImportCall",
            F,
            20,
        ),
        statement_site("tsc/internal/ast/utilities.go:IsEmittableImport", F, 25),
        fn_site(
            "tsc/internal/ast/ast.go:Node.Expression",
            F,
            35,
            "expression",
            36,
        ),
    ]
}

fn run_plan(workspace: &Workspace, sites: &[Value]) -> Value {
    let document =
        json!({"sites": sites, "unsited": [{"op": "tsc/x.go:none", "reason": "no_marker: test"}]});
    let (sites, unsited) = read_sites(&document).unwrap();
    plan(&workspace.0, &sites, &unsited, Some("tree")).unwrap()
}

fn operators_of(plan: &Value) -> Vec<(String, String)> {
    plan["mutants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mutant| {
            (
                mutant["function"].as_str().unwrap().to_owned(),
                mutant["operator"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn sites_resolve_to_functions_arms_statements_and_macro_bodies() {
    let workspace = Workspace::new();
    let planned = run_plan(&workspace, &sites());
    let chain: Vec<&str> = planned["missing_node_chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert_eq!(
        chain,
        [
            "create_missing_identifier",
            "create_missing_list",
            "finish_node",
            "new_identifier"
        ]
    );
    let found: Vec<(String, String)> = operators_of(&planned);
    let expected = [
        ("compute", "arm:0"),
        ("compute", "arm:!0"),
        ("compute", "arm:!0"),
        ("compute", "arm:0"),
        ("compute", "arm:!0"),
        ("statements", "negate_condition"),
        ("expression", "return:None"),
        (
            "create_missing_identifier",
            "wrap_flags:THIS_NODE_HAS_ERROR",
        ),
        ("finish_node", "wrap_param:node"),
        ("finish_node", "wrap_flags:THIS_NODE_HAS_ERROR"),
        (
            "parse_thing",
            "wrap_result:self.create_missing_identifier()",
        ),
        ("parse_thing", "param:flag:!flag"),
        ("parse_thing", "control"),
        ("next_token", "wrap_token:SyntaxKind::Unknown"),
        ("next_token", "wrap_result:SyntaxKind::Unknown"),
        ("is_thing", "return:false"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(function, operator)| ((*function).to_owned(), (*operator).to_owned()))
        .collect();
    assert_eq!(found, expected);
    let mutants = planned["mutants"].as_array().unwrap();
    let kinds: Vec<&str> = mutants
        .iter()
        .map(|mutant| mutant["site_kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "arm", "arm", "arm", "arm", "arm", "stmt", "macro_fn", "fn", "fn", "fn", "fn", "fn",
            "fn", "fn", "fn", "fn"
        ]
    );
    let shared = mutants.last().unwrap();
    assert_eq!(
        shared["ops"],
        json!([
            "tsc/internal/parser/parser.go:Parser.isOther",
            "tsc/internal/parser/parser.go:Parser.isThing"
        ])
    );
    assert_eq!(shared["op"], "tsc/internal/parser/parser.go:Parser.isOther");
    assert_eq!(
        shared["span"],
        json!([23, 28]),
        "both stacked markers are inside the span"
    );
    assert_eq!(shared["hit_fn"], "hit_parser");
    assert_eq!(mutants[0]["hit_fn"], "hit");
    assert_eq!(mutants[0]["crate"], "tsr_ast");
    assert_eq!(mutants[0]["span"], json!([3, 4]));

    let reasons: Vec<(String, String)> = planned["unsupported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let reason = entry["reason"].as_str().unwrap();
            (
                entry["op"]
                    .as_str()
                    .unwrap()
                    .rsplit(':')
                    .next()
                    .unwrap()
                    .to_owned(),
                reason.split(':').next().unwrap().to_owned(),
            )
        })
        .collect();
    let expected = [
        ("ShouldTransformImportCall", "no_operator"),
        ("IsEmittableImport", "statement_marker_unresolved"),
        ("Parser.testOnly", "cfg_test"),
        ("none", "no_marker"),
    ];
    let mut expected: Vec<(String, String)> = expected
        .iter()
        .map(|(op, reason)| ((*op).to_owned(), (*reason).to_owned()))
        .collect();
    expected.sort();
    let mut reasons = reasons;
    reasons.sort();
    assert_eq!(reasons, expected);
    let chain_root = mutants
        .iter()
        .find(|mutant| mutant["function"] == "create_missing_identifier")
        .unwrap();
    assert_eq!(
        chain_root["control"],
        Value::Null,
        "a flag toggle allocates nothing, so it has no control"
    );
}

#[test]
fn allocating_mutants_are_paired_with_controls_at_their_site() {
    let workspace = Workspace::new();
    let planned = run_plan(&workspace, &sites());
    let mutants = planned["mutants"].as_array().unwrap();
    let by_id = |id: &Value| {
        mutants
            .iter()
            .find(|mutant| mutant["id"] == *id)
            .unwrap()
            .clone()
    };
    let paired: Vec<&Value> = mutants
        .iter()
        .filter(|mutant| !mutant["control"].is_null())
        .collect();
    assert_eq!(paired.len(), 1);
    let mutant = paired[0];
    assert_eq!(
        mutant["operator"],
        "wrap_result:self.create_missing_identifier()"
    );
    let control = by_id(&mutant["control"]);
    assert_eq!(control["operator"], "control");
    assert_eq!(control["control_of"], mutant["id"]);
    assert_eq!(control["control"], Value::Null);
    for field in [
        "op",
        "ops",
        "file",
        "function",
        "site_kind",
        "site_line",
        "markers",
        "span",
        "span_sha256",
        "crate",
        "hit_fn",
        "return_category",
    ] {
        assert_eq!(control[field], mutant[field], "{field}");
    }
    assert_eq!(
        control["key"],
        mutant_key(
            control["op"].as_str().unwrap(),
            control["file"].as_str().unwrap(),
            control["function"].as_str().unwrap(),
            usize::try_from(control["site_line"].as_u64().unwrap()).unwrap(),
            "control",
            control["span_sha256"].as_str().unwrap(),
        )
    );
    let texts = |entry: &Value| -> Vec<String> {
        entry["insert"]
            .as_array()
            .unwrap()
            .iter()
            .map(|insert| insert["text"].as_str().unwrap().to_owned())
            .collect()
    };
    let control_texts = texts(&control).join("");
    assert!(control_texts.contains("{ let _ = self.create_missing_identifier(); }"));
    assert!(
        !control_texts.contains("return"),
        "the real result is returned"
    );
    assert!(texts(mutant)
        .join("")
        .contains("{ return self.create_missing_identifier(); }"));
    assert_eq!(planned["counts"]["controls"], 1);
    assert_eq!(planned["counts"]["mutants"], 15);
}

#[test]
fn homes_are_every_production_marker_site() {
    let workspace = Workspace::new();
    let planned = run_plan(&workspace, &sites());
    let homes = planned["homes"].as_object().unwrap();
    let mutant_keys = |function: &str| -> Vec<Value> {
        planned["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|mutant| mutant["function"] == function && mutant.get("control_of").is_none())
            .map(|mutant| mutant["key"].clone())
            .collect()
    };
    let requested: Vec<String> = sites()
        .iter()
        .map(|site| site["op"].as_str().unwrap().to_owned())
        .chain(["tsc/x.go:none".to_owned()])
        .collect();
    let mut keys: Vec<&String> = homes.keys().collect();
    keys.sort();
    let mut expected_keys: Vec<&String> = requested.iter().collect();
    expected_keys.sort();
    expected_keys.dedup();
    assert_eq!(
        keys, expected_keys,
        "every requested operation has a home list"
    );
    assert_eq!(homes["tsc/x.go:none"], json!([]), "no marker, no home");
    assert_eq!(
        homes["tsc/internal/parser/parser.go:Parser.testOnly"],
        json!([]),
        "test-only code is never a home"
    );
    let thing = &homes["tsc/internal/parser/parser.go:Parser.parseThing"][0];
    assert_eq!(thing["site_kind"], "fn");
    assert_eq!(thing["function"], "parse_thing");
    assert_eq!(thing["site_line"], 14);
    assert_eq!(thing["marker_line"], 13);
    assert_eq!(thing["reason"], Value::Null);
    assert_eq!(
        thing["mutants"].as_array().unwrap(),
        &mutant_keys("parse_thing"),
        "controls are not home mutants"
    );
    assert_eq!(thing["mutants"].as_array().unwrap().len(), 2);
    let other = &homes["tsc/internal/parser/parser.go:Parser.isOther"][0];
    let this = &homes["tsc/internal/parser/parser.go:Parser.isThing"][0];
    assert_eq!(
        other["mutants"], this["mutants"],
        "one site, two operations"
    );
    assert_eq!(
        (other["marker_line"].clone(), this["marker_line"].clone()),
        (json!(24), json!(23))
    );
    let arm = &homes["tsc/internal/ast/ast.go:BigIntLiteral.computeSubtreeFacts"][0];
    assert_eq!(
        (arm["site_kind"].clone(), arm["function"].clone()),
        (json!("arm"), json!("compute"))
    );
    assert_eq!(arm["mutants"].as_array().unwrap().len(), 1);
    let unmutated = &homes["tsc/internal/ast/utilities.go:ShouldTransformImportCall"][0];
    assert_eq!(unmutated["site_kind"], "stmt");
    assert_eq!(unmutated["mutants"], json!([]));
    assert!(unmutated["reason"]
        .as_str()
        .unwrap()
        .starts_with("no_operator"));
    let unresolved = &homes["tsc/internal/ast/utilities.go:IsEmittableImport"][0];
    assert_eq!(
        (
            unresolved["site_kind"].clone(),
            unresolved["function"].clone(),
            unresolved["site_line"].clone(),
            unresolved["span"].clone()
        ),
        (
            json!("stmt"),
            json!("emittable"),
            json!(26),
            json!([25, 26])
        ),
        "a marker above a multi-line attribute names the item it annotates"
    );
    assert!(unresolved["reason"]
        .as_str()
        .unwrap()
        .starts_with("statement_marker_unresolved"));
    let source = Source::new(workspace.read(F));
    assert_eq!(
        unresolved["span_sha256"].as_str(),
        source.span_sha256(25, 26).as_deref()
    );
    assert_eq!(
        planned["counts"]["homes"], 13,
        "14 markers, one of them test-only"
    );
    assert_eq!(planned["counts"]["homes_without_mutants"], 2);
}

#[test]
fn ids_and_keys_are_deterministic() {
    let workspace = Workspace::new();
    let first = canonical(&run_plan(&workspace, &sites()));
    let mut reversed = sites();
    reversed.reverse();
    let second = canonical(&run_plan(&workspace, &reversed));
    assert_eq!(first, second, "site order does not change the plan");
    let planned: Value = serde_json::from_str(&first).unwrap();
    for (index, mutant) in planned["mutants"].as_array().unwrap().iter().enumerate() {
        assert_eq!(mutant["id"], index + 1);
        let key = mutant_key(
            mutant["op"].as_str().unwrap(),
            mutant["file"].as_str().unwrap(),
            mutant["function"].as_str().unwrap(),
            usize::try_from(mutant["site_line"].as_u64().unwrap()).unwrap(),
            mutant["operator"].as_str().unwrap(),
            mutant["span_sha256"].as_str().unwrap(),
        );
        assert_eq!(mutant["key"], key);
        let span = mutant["span"].as_array().unwrap();
        let source = Source::new(workspace.read(mutant["file"].as_str().unwrap()));
        let digest = source.span_sha256(
            usize::try_from(span[0].as_u64().unwrap()).unwrap(),
            usize::try_from(span[1].as_u64().unwrap()).unwrap(),
        );
        assert_eq!(digest.as_deref(), mutant["span_sha256"].as_str());
    }
    // An edit inside one span changes that mutant's key and nothing else's id.
    workspace.write(
        P,
        &workspace
            .read(P)
            .replace("        true\n", "        true // edited\n"),
    );
    let edited = run_plan(&workspace, &sites());
    let keys = |plan: &Value| -> Vec<String> {
        plan["mutants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let (before, after) = (keys(&planned), keys(&edited));
    assert_eq!(before.len(), after.len());
    let changed: Vec<usize> = (0..before.len())
        .filter(|at| before[*at] != after[*at])
        .collect();
    assert_eq!(changed, [before.len() - 1]);
}

fn spliced_workspace() -> (Workspace, PathBuf) {
    let workspace = Workspace::new();
    let planned = run_plan(&workspace, &sites());
    let path = workspace.plan_file(&planned);
    (workspace, path)
}

#[test]
fn splicing_keeps_every_line_and_adds_the_switch_dependency() {
    let (workspace, plan_path) = spliced_workspace();
    let before = workspace.read(P);
    let report = splice(&workspace.0, &plan_path, &workspace.switch()).unwrap();
    assert_eq!(report["mutants"], 16);
    let after = workspace.read(P);
    assert_eq!(before.lines().count(), after.lines().count());
    for (old, new) in before.lines().zip(after.lines()) {
        assert!(
            new.contains(old.trim()) || new.starts_with(&old[..old.len().min(20)]),
            "{old} -> {new}"
        );
    }
    let line = |text: &str, number: usize| text.lines().nth(number - 1).unwrap().to_owned();
    assert_eq!(
        line(&after, 14),
        "    pub(crate) fn parse_thing(&mut self, flag: bool) -> NodeId {let flag = if ::phase1_mutants::hit_parser(12) { !flag } else { flag }; \
         let __phase1_mutant_result = (|| -> NodeId { let __phase1_mutant_result = (|| -> NodeId { "
    );
    assert_eq!(
        line(&after, 17),
        "     })(); if ::phase1_mutants::hit_parser(13) { let _ = self.create_missing_identifier(); } __phase1_mutant_result  \
         })(); if ::phase1_mutants::hit_parser(11) { return self.create_missing_identifier(); } __phase1_mutant_result }"
    );
    assert_eq!(
        line(&after, 10),
        "    pub(crate) fn finish_node(&mut self, node: NodeId) -> NodeId {let __phase1_mutant_param_9 = node; \
         let __phase1_mutant_result = (|| -> NodeId { let __phase1_mutant_result = (|| -> NodeId { "
    );
    assert_eq!(
        line(&after, 12),
        "     })(); if ::phase1_mutants::hit_parser(10) { return { let __phase1_flags = \
         ::tsr_ast::Factory::node(&self.factory, __phase1_mutant_result).flags(); \
         ::tsr_ast::Factory::set_node_flags(&mut self.factory, __phase1_mutant_result, \
         __phase1_flags ^ ::tsr_ast::node_flags::THIS_NODE_HAS_ERROR); __phase1_mutant_result }; } \
         __phase1_mutant_result  })(); if ::phase1_mutants::hit_parser(9) { return __phase1_mutant_param_9; } \
         __phase1_mutant_result }"
    );
    assert_eq!(
        line(&after, 22),
        "     })(); if ::phase1_mutants::hit_parser(15) { return SyntaxKind::Unknown; } __phase1_mutant_result  \
         })(); if ::phase1_mutants::hit_parser(14) { return if __phase1_mutant_result == SyntaxKind::EndOfFile \
         { __phase1_mutant_result } else { self.token = SyntaxKind::Unknown; SyntaxKind::Unknown }; } \
         __phase1_mutant_result }"
    );
    let facts = workspace.read(F);
    assert_eq!(
        line(&facts, 6),
        "        D::BigIntLiteral(_) => if ::phase1_mutants::hit(3) { !0 } else { NONE },"
    );
    assert_eq!(
        line(&facts, 17),
        "    if ::phase1_mutants::hit(6) != (matches!(kind, K::A | K::B)) {"
    );
    assert_eq!(
        line(&facts, 4),
        "        D::PrivateIdentifier(_) => if ::phase1_mutants::hit(1) { 0 } else { if ::phase1_mutants::hit(2) { !0 } else { CLASS_FIELDS } },"
    );
    assert!(line(&facts, 36).ends_with("{if ::phase1_mutants::hit(7) { return None; } "));
    let manifest = workspace.read("crates/tsr_parser/Cargo.toml");
    assert!(manifest.contains(
        "[dependencies]\nphase1_mutants = { path = \"../../tools/phase1/mutation/switch\" }\ntsr_ast"
    ));
    assert!(workspace.read("crates/tsr_ast/Cargo.toml").ends_with(
        "\n[dependencies]\nphase1_mutants = { path = \"../../tools/phase1/mutation/switch\" }\n"
    ));
    let error = splice(&workspace.0, &plan_path, &workspace.switch()).unwrap_err();
    assert!(error.contains("already spliced"), "{error}");
}

#[test]
fn a_drifted_span_refuses_the_whole_splice() {
    let (workspace, plan_path) = spliced_workspace();
    let original_facts = workspace.read(F);
    // Edit one line inside the `is_thing` span only.
    workspace.write(
        P,
        &workspace
            .read(P)
            .replace("        true\n", "        !false\n"),
    );
    let parser = workspace.read(P);
    let error = splice(&workspace.0, &plan_path, &workspace.switch()).unwrap_err();
    assert!(error.contains("span drifted"), "{error}");
    assert!(error.contains("nothing was written"), "{error}");
    assert_eq!(workspace.read(P), parser, "no file was touched");
    assert_eq!(
        workspace.read(F),
        original_facts,
        "not even the files that still match"
    );
    assert!(!workspace
        .read("crates/tsr_ast/Cargo.toml")
        .contains("phase1_mutants"));

    // Moving code (a new line above a span) is drift too at splice time.
    let (workspace, plan_path) = spliced_workspace();
    workspace.write(F, &format!("// a new first line\n{}", workspace.read(F)));
    assert!(splice(&workspace.0, &plan_path, &workspace.switch()).is_err());

    // An edit outside every span does not block the splice.
    let (workspace, plan_path) = spliced_workspace();
    workspace.write(
        F,
        &workspace
            .read(F)
            .replace("    Some(module)\n", "    Some(module + 0)\n"),
    );
    assert!(splice(&workspace.0, &plan_path, &workspace.switch()).is_ok());
}

#[test]
fn splice_refuses_the_wrong_switch_crate() {
    let (workspace, plan_path) = spliced_workspace();
    let error = splice(
        &workspace.0,
        &plan_path,
        &workspace.0.join("crates/tsr_ast"),
    )
    .unwrap_err();
    assert!(error.contains("is not the phase1_mutants crate"), "{error}");
    let _: &Path = &workspace.0;
}

#[test]
fn two_wrappers_on_one_function_nest_in_id_order() {
    let workspace = Workspace::new();
    let text = "impl<F> Parser<'_, F> {\n    /// port: tsc/internal/parser/parser.go:Parser.parseOptional\n    fn parse_optional(&mut self) -> Option<NodeId> {\n        self.parse()\n    }\n}\n";
    workspace.write(P, text);
    let sites = [fn_site(
        "tsc/internal/parser/parser.go:Parser.parseOptional",
        P,
        2,
        "parse_optional",
        3,
    )];
    let planned = run_plan(&workspace, &sites);
    assert_eq!(
        operators_of(&planned),
        [
            ("parse_optional".to_owned(), "wrap_result:None".to_owned()),
            (
                "parse_optional".to_owned(),
                "wrap_present:self.create_missing_identifier()".to_owned()
            ),
            ("parse_optional".to_owned(), "control".to_owned()),
        ]
    );
    let plan_path = workspace.plan_file(&planned);
    splice(&workspace.0, &plan_path, &workspace.switch()).unwrap();
    let after = workspace.read(P);
    let lines: Vec<&str> = after.lines().collect();
    let open = "let __phase1_mutant_result = (|| -> Option < NodeId > { ";
    assert_eq!(
        lines[2],
        format!("    fn parse_optional(&mut self) -> Option<NodeId> {{{open}{open}{open}")
    );
    assert_eq!(
        lines[4],
        "     })(); if ::phase1_mutants::hit_parser(3) { let _ = __phase1_mutant_result.map(|_| \
         self.create_missing_identifier()); } __phase1_mutant_result  })(); if ::phase1_mutants::hit_parser(2) \
         { return __phase1_mutant_result.map(|_| self.create_missing_identifier()); } __phase1_mutant_result  \
         })(); if ::phase1_mutants::hit_parser(1) { return None; } __phase1_mutant_result }"
    );
}

#[test]
fn only_sites_whose_every_operation_is_internal_parser_stay_live_while_observing() {
    let workspace = Workspace::new();
    let parser = "impl<F> Parser<'_, F> {
    /// port: tsc/internal/ast/utilities.go:NodeIsMissing
    fn node_is_missing(&self) -> bool {
        true
    }
    // port: tsc/internal/parser/parser.go:Parser.isShared
    // port: tsc/internal/ast/utilities.go:IsShared
    fn is_shared(&self) -> bool {
        true
    }
    /// port: tsc/internal/parser/parser.go:Parser.isParserOnly
    fn is_parser_only(&self) -> bool {
        true
    }
}
";
    let ast = "/// port: tsc/internal/parser/utilities.go:isAstHome
fn is_ast_home() -> bool {
    true
}
";
    workspace.write(P, parser);
    workspace.write("crates/tsr_ast/src/lib.rs", ast);
    let sites = [
        fn_site(
            "tsc/internal/ast/utilities.go:NodeIsMissing",
            P,
            2,
            "node_is_missing",
            3,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.isShared",
            P,
            6,
            "is_shared",
            8,
        ),
        fn_site(
            "tsc/internal/ast/utilities.go:IsShared",
            P,
            7,
            "is_shared",
            8,
        ),
        fn_site(
            "tsc/internal/parser/parser.go:Parser.isParserOnly",
            P,
            11,
            "is_parser_only",
            12,
        ),
        fn_site(
            "tsc/internal/parser/utilities.go:isAstHome",
            "crates/tsr_ast/src/lib.rs",
            1,
            "is_ast_home",
            2,
        ),
    ];
    let planned = run_plan(&workspace, &sites);
    let mut found: Vec<(String, String)> = Vec::new();
    for mutant in planned["mutants"].as_array().unwrap() {
        let hit_fn = mutant["hit_fn"].as_str().unwrap();
        let call = format!("::phase1_mutants::{hit_fn}({})", mutant["id"]);
        for insert in mutant["insert"].as_array().unwrap() {
            let text = insert["text"].as_str().unwrap();
            if text.contains("::phase1_mutants::") {
                assert!(text.contains(&call), "{text} does not call {call}");
            }
        }
        let pair = (
            mutant["function"].as_str().unwrap().to_owned(),
            hit_fn.to_owned(),
        );
        if !found.contains(&pair) {
            found.push(pair);
        }
    }
    let expected = [
        ("is_ast_home", "hit"),
        ("node_is_missing", "hit"),
        ("is_shared", "hit"),
        ("is_parser_only", "hit_parser"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(function, hit_fn)| ((*function).to_owned(), (*hit_fn).to_owned()))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn the_switch_rule_needs_parser_sources_and_only_parser_operations() {
    let ops = |ops: &[&str]| -> Vec<String> { ops.iter().map(|op| (*op).to_owned()).collect() };
    let parser_op = "tsc/internal/parser/parser.go:Parser.a";
    let ast_op = "tsc/internal/ast/utilities.go:NodeIsMissing";
    let cases = [
        (P, ops(&[parser_op]), "hit_parser"),
        (
            P,
            ops(&[parser_op, "tsc/internal/parser/jsdoc.go:Parser.b"]),
            "hit_parser",
        ),
        (P, ops(&[ast_op]), "hit"),
        (P, ops(&[parser_op, ast_op]), "hit"),
        (F, ops(&[parser_op]), "hit"),
        (P, ops(&[]), "hit"),
    ];
    for (file, ops, expected) in cases {
        assert_eq!(hit_fn(file, &ops), expected, "{file} {ops:?}");
    }
}
