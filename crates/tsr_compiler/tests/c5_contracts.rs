//! Phase 2 C5.8: direct contracts of the declaration and services surface over
//! production entry points (docs/PHASE2-C5-plan.md, C5.8). Each contract names
//! its pinned counterpart. Where the pin's answer is compared, it is the
//! pinned one: the native Phase 2 capture's declaration diagnostics
//! (`target/phase2/native`), or the calls the pinned fourslash suite made into
//! the pinned checker (`data/phase2/services-replay.json.xz`), whose contract
//! subset `scripts/phase2_services.py fixture` writes to
//! `fixtures/c5/services-subset.ndjson` and the replay compares here.
#[path = "../../../tools/phase2/services/replay/mod.rs"]
mod replay;

use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K};
use tsr_checker::{type_format_flags as tff, CheckerOwner, SignatureKind, TypeRef, UnionReduction};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const SUBSET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/c5/services-subset.ndjson"
);

fn program(files: &[(&str, &str)], options: CompilerOptions) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let roots = files
        .iter()
        .filter(|(name, _)| !name.contains("/node_modules/"))
        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
        .collect();
    Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, roots),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

fn strict() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        ..Default::default()
    }
}

fn checker(program: &Arc<Program>, counters: &Counters) -> (Generation, Arc<CheckerOwner>) {
    let generation = Generation::new(counters);
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), counters),
            counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    (generation, owner)
}

struct Collect<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
}
impl ChildVisitor for Collect<'_> {
    fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
        self.nodes.push(id);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, id: NodeListId) -> ControlFlow<()> {
        let list = self.view.list(id).unwrap();
        self.visit_node_slice(list.nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.nodes
            .extend(self.view.node_slice(nodes).unwrap().iter().flatten());
        ControlFlow::Continue(())
    }
}

/// Every node of `file` in preorder, with its kind and its text for names.
fn nodes(program: &Program, file: &str) -> Vec<(NodeId, K, String)> {
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    let mut result = Vec::new();
    let mut work = vec![file.source()];
    while let Some(id) = work.pop() {
        let node = view.node(id).unwrap();
        let kind = node.kind().known().unwrap();
        let text = if matches!(kind, K::Identifier | K::StringLiteral) {
            String::from_utf8_lossy(view.node_text(id).unwrap().as_bytes()).into_owned()
        } else {
            String::new()
        };
        result.push((id, kind, text));
        let mut children = Collect {
            view,
            nodes: Vec::new(),
        };
        let _ = node.for_each_child(&mut children);
        work.extend(children.nodes.into_iter().rev());
    }
    result
}

/// The `nth` node of `kind` in `file` (and, for names, with `text`).
fn find(program: &Program, file: &str, kind: K, text: &str, nth: usize) -> NodeId {
    nodes(program, file)
        .into_iter()
        .filter(|(_, k, t)| *k == kind && (text.is_empty() || t == text))
        .nth(nth)
        .unwrap_or_else(|| panic!("no {kind:?} {text:?} #{nth} in {file}"))
        .0
}

/// The replay report lines of `tests` from the contract subset (or of the
/// given record text), one per test.
fn replay_tests(label: &str, record: Option<&str>, tests: &[&str]) -> Vec<Value> {
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("c5_contracts");
    std::fs::create_dir_all(&directory).unwrap();
    let input = match record {
        Some(text) => {
            let path = directory.join(format!("{label}.input.ndjson"));
            std::fs::write(&path, text).unwrap();
            path.to_string_lossy().into_owned()
        }
        None => SUBSET.to_string(),
    };
    let output = directory.join(format!("{label}.ndjson"));
    let tests: Vec<String> = tests.iter().map(|name| (*name).to_string()).collect();
    replay::replay_file(&input, &output.to_string_lossy(), &tests).unwrap();
    std::fs::read_to_string(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Every call of every replayed test matched; the calls per operation.
fn matched_calls(lines: &[Value], tests: &[&str]) -> BTreeMap<String, u64> {
    assert_eq!(lines.len(), tests.len(), "every requested test replayed");
    let mut calls = BTreeMap::new();
    for line in lines {
        for status in line["programs"].as_object().unwrap().values() {
            assert!(
                status["same_files"] == true && status["same_order"] == true,
                "{}: {status}",
                line["test"]
            );
        }
        for (op, counts) in line["operations"].as_object().unwrap() {
            for (outcome, count) in counts.as_object().unwrap() {
                assert_eq!(
                    outcome,
                    "match",
                    "{}: {op}: {}",
                    line["test"],
                    serde_json::to_string(&line["details"][op]).unwrap()
                );
                *calls.entry(op.clone()).or_default() += count.as_u64().unwrap();
            }
        }
    }
    calls
}

/// Contract 1 (ADR 0012; the pin's per-request node builder
/// `Checker.getNodeBuilderEx`, its checker-lifetime `typeToStringNodebuilder`
/// and `EmitResolver` links, and the language service's snapshot ownership):
/// operation-local builder output is released with the builder; the checker's
/// own diagnostic builder and resolver marks are built once and reused by the
/// next operation; a retained result keeps a retired checker's storage alive,
/// the retired generation refuses the owner at once, and the counters return
/// to their baseline only after the last root drops.
#[test]
fn owner_retention_follows_three_lifetimes() {
    let program = program(
        &[(
            "/a.ts",
            "export const x = { a: 1, b: [1, 2] };\nexport function f(v: string) { return x; }\nconst n: number = \"s\";\n",
        )],
        strict(),
    );
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let (generation, owner) = checker(&program, &counters);
    let x = find(&program, "/a.ts", K::Identifier, "x", 0);
    let source = program.file(b"/a.ts").unwrap().source();
    {
        let mut op = owner.operation().unwrap();
        let ty = op.get_type_at_location(x).unwrap();
        let before = counters.snapshot();
        {
            let mut builder = op.node_builder();
            let node = builder.type_to_type_node(ty, None, 0, 0).unwrap();
            assert!(node.is_some());
        }
        assert_eq!(
            counters.snapshot(),
            before,
            "builder output is operation-local"
        );
    }
    let first = {
        let mut op = owner.operation().unwrap();
        let diagnostics = op.get_diagnostics(source).unwrap();
        op.mark_linked_references_recursively(source).unwrap();
        diagnostics
    };
    assert_eq!(first.len(), 1, "TS2322 names the two types it displays");
    let cached = counters.snapshot();
    let second = {
        let mut op = owner.operation().unwrap();
        let diagnostics = op.get_diagnostics(source).unwrap();
        op.mark_linked_references_recursively(source).unwrap();
        diagnostics
    };
    assert_eq!(first, second);
    assert_eq!(
        counters.snapshot(),
        cached,
        "the checker's diagnostic builder and marks are reused, not rebuilt"
    );
    let retained = {
        let mut op = owner.operation().unwrap();
        let ty = op.get_type_at_location(x).unwrap();
        op.retain_type(ty).unwrap()
    };
    generation.retire();
    assert!(matches!(
        owner.operation(),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    let (_other_generation, other) = checker(&program, &Counters::new());
    assert!(matches!(
        other.operation().unwrap().import_type(&retained),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
    ));
    drop(owner);
    drop(generation);
    assert_ne!(
        counters.snapshot(),
        baseline,
        "the retained root keeps the retired storage"
    );
    drop(retained);
    assert_eq!(
        counters.snapshot(),
        baseline,
        "the last root returns every count"
    );
}

/// Contract 2 (`Program.GetDeclarationDiagnostics`, `getDeclarationDiagnostics`
/// in emitter.go, and the declaration transformer's late statements): a file's
/// declaration diagnostics come from that file's view, as the native capture
/// of `globalThisDeclarationEmit` reports them (TS4025 at 52..62 of index.ts,
/// none for variable.ts); and the late-declaration case of the augmentation
/// row, `declarationEmitAugmentationUsesCorrectSourceFile`, whose
/// visibility pass late-marks a declaration of the augmented `.d.ts`,
/// completes with the pin's diagnostics: none.
#[test]
fn declaration_diagnostics_come_from_each_files_view() {
    let options = CompilerOptions {
        declaration: Tristate::TRUE,
        module: ModuleKind::COMMON_JS,
        target: ScriptTarget::ES2015,
        ..Default::default()
    };
    let program = program(
        &[
            (
                "/.src/index.ts",
                "import { variable } from \"./variable\";\nexport const globalThis = variable;\n",
            ),
            ("/.src/variable.ts", "export const variable = globalThis;"),
        ],
        options,
    );
    let counters = Counters::new();
    let (_generation, owner) = checker(&program, &counters);
    let mut op = owner.operation().unwrap();
    let index = program.file(b"/.src/index.ts").unwrap();
    let variable = program.file(b"/.src/variable.ts").unwrap();
    let diagnostics = program
        .declaration_diagnostics_with_checker(&mut op, index)
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.file, Some(index.source()));
    assert_eq!(
        (diagnostic.code, diagnostic.loc.pos(), diagnostic.loc.end()),
        (4025, 52, 62)
    );
    assert_eq!(
        diagnostic
            .message_args
            .iter()
            .map(|arg| String::from_utf8_lossy(arg.as_bytes()).into_owned())
            .collect::<Vec<_>>(),
        ["globalThis", "globalThis", ""]
    );
    assert!(program
        .declaration_diagnostics_with_checker(&mut op, variable)
        .unwrap()
        .is_empty());
    drop(op);

    let random = "// A bunch of random text to move the positions forward\n".repeat(9);
    let knex = format!(
        "\n{random}\ntype ShouldJustBeAny = [any][0];\n\ndeclare namespace knex {{\n  export {{ Knex }};\n}}\n\ndeclare namespace Knex {{\n  interface Interface {{\n    method(): ShouldJustBeAny;\n  }}\n}}\n\nexport = knex;\n"
    );
    let options = CompilerOptions {
        declaration: Tristate::TRUE,
        module: ModuleKind::PRESERVE,
        strict: Tristate::TRUE,
        ..Default::default()
    };
    let program = self::program(
        &[
            ("/node_modules/knex/index.d.ts", &knex),
            (
                "/index.ts",
                "import \"knex\";\ndeclare module \"knex\" {\n  namespace Knex {\n    function newFunc(): Knex.Interface;\n  }\n}\n",
            ),
        ],
        options,
    );
    let (_generation, owner) = checker(&program, &counters);
    let mut op = owner.operation().unwrap();
    let index = program.file(b"/index.ts").unwrap();
    assert!(program
        .declaration_diagnostics_with_checker(&mut op, index)
        .unwrap()
        .is_empty());
}

/// Contract 3 (symbolaccessibility.go: `GetAccessibleSymbolChain`,
/// `IsSymbolAccessible`, `getAccessibleSymbolChain`'s chain shapes): the
/// pinned fourslash suite's accessibility queries replay with equal chains
/// and results.
#[test]
fn accessibility_chains_replay_as_the_pin_answered() {
    let tests = [
        "TestCompletionForComputedStringProperties",
        "TestQualifyModuleTypeNames",
    ];
    let calls = matched_calls(&replay_tests("accessibility", None, &tests), &tests);
    assert!(calls["Checker.GetAccessibleSymbolChain"] > 0);
    assert!(calls["Checker.IsSymbolAccessible"] > 0);
}

/// Contract 4 (nodebuilderimpl.go `serializeTypeForDeclaration`,
/// `serializeReturnTypeForSignature`, `signatureToSignatureDeclarationHelper`
/// and the truncation of printer.go `typeToStringEx`): the declaration
/// serializations a code fix asks for replay with equal syntax (annotated
/// declarations reuse their type nodes, inferred ones serialize), and so do
/// the truncated hover strings; `noErrorTruncation` removes the truncation
/// that the same display otherwise applies.
#[test]
fn serialization_reuse_and_truncation_follow_the_pin() {
    let tests = [
        "TestCodeFixMissingTypeAnnotationOnExports11",
        "TestCodeFixMissingTypeAnnotationOnExports44_default_export",
        "TestCodeFixMissingTypeAnnotationOnExports55_generator_return",
        "TestCodeFixClassImplementInterfaceWithAmbientSignatures2",
        "TestQuickinfoVerbosityToplevelTruncation1",
    ];
    let calls = matched_calls(&replay_tests("serialization", None, &tests), &tests);
    for op in [
        "EmitResolver.CreateTypeOfDeclaration",
        "EmitResolver.CreateReturnTypeOfSignatureDeclaration",
        "EmitResolver.CreateTypeOfExpression",
        "NodeBuilder.SignatureToSignatureDeclaration",
        "Checker.ExpandSymbolForHover",
    ] {
        assert!(calls.get(op).copied().unwrap_or(0) > 0, "{op}");
    }
    let members: String = (0..60).fold(String::new(), |mut text, i| {
        text.push_str(&format!("\"member{i}\" | "));
        text
    });
    let text = format!("export declare const wide: {members}\"last\";\n");
    let display = |truncation: Tristate| {
        let program = program(
            &[("/wide.ts", &text)],
            CompilerOptions {
                no_error_truncation: truncation,
                ..strict()
            },
        );
        let (_generation, owner) = checker(&program, &Counters::new());
        let mut op = owner.operation().unwrap();
        let wide = find(&program, "/wide.ts", K::Identifier, "wide", 0);
        let ty = op.get_type_at_location(wide).unwrap();
        let default = op.type_to_string_ex(ty, None, 0, None).unwrap();
        let full = op
            .type_to_string_ex(ty, None, tff::NO_TRUNCATION, None)
            .unwrap();
        (
            String::from_utf8_lossy(default.as_bytes()).into_owned(),
            String::from_utf8_lossy(full.as_bytes()).into_owned(),
        )
    };
    let (truncated, full) = display(Tristate::FALSE);
    assert!(
        truncated.contains(" more ...") && truncated.len() < full.len(),
        "{truncated}"
    );
    assert!(!full.contains(" more ..."));
    let (untruncated, _) = display(Tristate::TRUE);
    assert_eq!(untruncated, full);
}

/// Contract 5 (nodebuilder_hover.go `expandSymbolForHover` and printer.go
/// `ExpandSymbolForHover`): a class, an interface, an enum, a namespace and the
/// type aliases a namespace holds expand as the pinned fourslash hovers
/// recorded them, and a type alias's verbosity display follows the pin's.
#[test]
fn hover_expansion_replays_as_the_pin_recorded() {
    let tests = [
        "TestQuickinfoVerbosityClassWithMixinBase",
        "TestQuickinfoVerbosityInterfaceMemberOrdering",
        "TestQuickinfoVerbosityConstEnum",
        "TestQuickinfoVerbosityNestedNamespace",
        "TestQuickinfoVerbosityNamespaceTypeAliases",
        "TestQuickinfoVerbosityConditionalType",
    ];
    let lines = replay_tests("hover", None, &tests);
    let calls = matched_calls(&lines, &tests);
    for line in &lines {
        if line["test"] == "TestQuickinfoVerbosityConditionalType" {
            continue;
        }
        assert!(
            line["operations"]["Checker.ExpandSymbolForHover"]["match"]
                .as_u64()
                .unwrap()
                > 0,
            "{}",
            line["test"]
        );
    }
    assert!(calls["Checker.TypeToStringEx"] > 0);
}

/// Contract 6 (the C5.5 reference, `scripts/phase2_services.py replay`): the
/// whole contract subset of the recorded fourslash calls replays with every
/// call matching, and a changed recorded result fails the replay.
#[test]
fn the_services_replay_detects_a_changed_result() {
    let record = std::fs::read_to_string(SUBSET).unwrap();
    let names: Vec<String> = record
        .lines()
        .filter(|line| line.starts_with(r#"{"e":"test""#))
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let tests: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(tests.len(), 20);
    let calls = matched_calls(&replay_tests("subset", None, &tests), &tests);
    assert!(calls.values().sum::<u64>() > 400);
    // Change one recorded display string.
    let mut target = String::new();
    let mut current = String::new();
    let mut changed = false;
    let mutated: Vec<String> = record
        .lines()
        .map(|line| {
            if line.starts_with(r#"{"e":"test""#) {
                current = serde_json::from_str::<Value>(line).unwrap()["name"]
                    .as_str()
                    .unwrap()
                    .to_string();
            }
            if !changed && line.contains(r#""op":"Checker.SymbolToStringEx""#) {
                target.clone_from(&current);
                let mut event: Value = serde_json::from_str(line).unwrap();
                let text = event["results"][0].as_str().unwrap().to_string();
                event["results"][0] = Value::String(text + " ");
                changed = true;
                return serde_json::to_string(&event).unwrap();
            }
            line.to_string()
        })
        .collect();
    assert!(changed, "the subset holds a signature display to change");
    let lines = replay_tests("mutated", Some(&(mutated.join("\n") + "\n")), &[&target]);
    assert_eq!(
        lines[0]["operations"]["Checker.SymbolToStringEx"]["mismatch"],
        1
    );
}

/// Contract 7 (exports.go): every public query entry point answers over a
/// checked program, and answers the same again after an unrelated query.
#[test]
fn the_public_query_surface_answers_and_answers_again() {
    const SOURCE: &str = r#"export interface Point { readonly x: number; y?: string; [key: string]: unknown }
export class Base { protected id = 0 }
export class Box<T extends object = {}> extends Base implements Point {
    readonly x = 1;
    y?: string;
    [key: string]: unknown;
    constructor(public value: T) { super(); }
    method(this: Box<T>, a: number, ...rest: string[]): T { return this.value; }
}
export enum Color { Red, Green = "g" }
export type Pair<T> = [T, T];
export type Cond<T> = T extends string ? "s" : "n";
export function isString(v: unknown): v is string { return typeof v === "string"; }
export const box = new Box({ a: 1 });
export const pair: Pair<number> = [1, 2];
export const maybe = undefined as string | undefined;
export const promise = Promise.resolve(1);
export const list = [1, 2, 3];
/** @deprecated */
export function old() {}
const obj = { m: 1, n: "a" };
isString(obj);
box.method(1, "a");
"#;
    let program = program(
        &[
            ("/q.ts", SOURCE),
            ("/other.ts", "export const other: number = 1;\n"),
        ],
        strict(),
    );
    let (_generation, owner) = checker(&program, &Counters::new());
    let source = program.file(b"/q.ts").unwrap().source();
    let other = program.file(b"/other.ts").unwrap().source();
    let name = |text: &str| find(&program, "/q.ts", K::Identifier, text, 0);
    // After `super()` and `Promise.resolve(1)`.
    let call = find(&program, "/q.ts", K::CallExpression, "", 2);
    let method_call = find(&program, "/q.ts", K::CallExpression, "", 3);
    let object = find(&program, "/q.ts", K::ObjectLiteralExpression, "", 1);
    let object_member = find(&program, "/q.ts", K::PropertyAssignment, "", 2);
    let pair_node = find(&program, "/q.ts", K::TypeReference, "", 3);
    let old = find(&program, "/q.ts", K::FunctionDeclaration, "", 1);
    let answers = || -> Vec<String> {
        let mut op = owner.operation().unwrap();
        let mut out = Vec::new();
        let show = |op: &mut tsr_checker::Operation<'_>, ty: TypeRef| {
            String::from_utf8_lossy(op.type_to_string_default(ty).unwrap().as_bytes()).into_owned()
        };
        let symbol = |op: &mut tsr_checker::Operation<'_>, text: &str| {
            op.get_symbol_at_location(name(text)).unwrap().unwrap()
        };
        let box_type = op.get_type_at_location(name("box")).unwrap();
        let point = symbol(&mut op, "Point");
        let box_class = symbol(&mut op, "Box");
        let color = symbol(&mut op, "Color");
        let pair_type = op.get_type_at_location(name("pair")).unwrap();
        let pair_annotation = op.get_type_from_type_node(pair_node).unwrap();
        let maybe = op.get_type_at_location(name("maybe")).unwrap();
        let promise = op.get_type_at_location(name("promise")).unwrap();
        let list = op.get_type_at_location(name("list")).unwrap();
        let cond_symbol = symbol(&mut op, "Cond");
        let cond = op.get_declared_type_of_symbol(cond_symbol).unwrap();
        let declared_box = op.get_declared_type_of_symbol(box_class).unwrap();
        let point_type = op.get_declared_type_of_symbol(point).unwrap();
        let string = op.get_string_type();
        let number = op.get_number_type();
        out.push(show(&mut op, box_type));
        out.push(show(&mut op, pair_annotation));
        for ty in [
            op.get_boolean_type(),
            op.get_undefined_type(),
            op.get_any_type(),
            op.get_never_type(),
            op.get_big_int_type(),
            op.get_non_primitive_type(),
            op.get_union_type(&[string, number]).unwrap(),
            op.get_union_type_ex(&[string, number], UnionReduction::Subtype)
                .unwrap(),
            op.get_non_nullable_type(maybe).unwrap(),
            op.get_non_optional_type(maybe).unwrap(),
            op.remove_missing_or_undefined_type(maybe).unwrap(),
            op.get_apparent_type(string).unwrap(),
            op.get_reduced_type(maybe).unwrap(),
            op.get_widened_type(list).unwrap(),
            op.get_widened_literal_type(list).unwrap(),
            op.get_base_constructor_type_of_class(declared_box).unwrap(),
            op.get_true_type_of_conditional_type(cond).unwrap(),
            op.get_false_type_of_conditional_type(cond).unwrap(),
            op.get_promised_type_of_promise(promise).unwrap().unwrap(),
            op.get_element_type_of_array_type(list).unwrap().unwrap(),
            op.get_type_of_symbol(color).unwrap(),
            op.get_non_missing_type_of_symbol(point).unwrap(),
        ] {
            out.push(show(&mut op, ty));
        }
        out.push(format!(
            "{:?}",
            op.get_properties_of_type(point_type).unwrap().len()
        ));
        out.push(format!(
            "{:?}",
            op.get_apparent_properties(box_type).unwrap().len()
        ));
        out.push(format!(
            "{:?}",
            op.get_base_types(declared_box).unwrap().len()
        ));
        out.push(format!(
            "{:?}",
            op.get_type_arguments(pair_type).unwrap().len()
        ));
        out.push(format!(
            "{:?}",
            op.get_index_infos_of_type(point_type).unwrap().len()
        ));
        out.push(format!(
            "{:?}",
            op.get_index_info_of_type(point_type, string)
                .unwrap()
                .is_some()
        ));
        out.push(format!(
            "{:?}",
            op.get_string_index_type(point_type)
                .unwrap()
                .map(TypeRef::id)
        ));
        out.push(format!(
            "{:?}",
            op.get_number_index_type(point_type)
                .unwrap()
                .map(TypeRef::id)
        ));
        out.push(format!(
            "{:?}",
            op.get_type_of_property_of_type(point_type, b"x")
                .unwrap()
                .map(TypeRef::id)
        ));
        out.push(format!(
            "{:?}",
            op.get_property_of_type(point_type, b"y")
                .unwrap()
                .map(tsr_checker::SymbolRef::id)
        ));
        out.push(format!(
            "{:?} {:?} {:?} {:?} {:?} {:?}",
            op.is_array_like_type(list).unwrap(),
            op.is_array_type(list).unwrap(),
            op.is_tuple_type(pair_type).unwrap(),
            op.is_nullable_type(maybe).unwrap(),
            op.is_empty_anonymous_object_type(box_type).unwrap(),
            op.type_has_call_or_construct_signatures(declared_box)
                .unwrap(),
        ));
        out.push(format!(
            "{:?}",
            op.is_type_assignable_to(string, maybe).unwrap()
        ));
        let box_constructor = op.get_type_of_symbol(box_class).unwrap();
        let signatures = op
            .get_signatures_of_type(box_constructor, SignatureKind::Construct)
            .unwrap();
        out.push(format!("{}", signatures.len()));
        let resolved = op.get_resolved_signature(method_call).unwrap();
        let returned = op.get_return_type_of_signature(resolved).unwrap();
        out.push(format!(
            "{:?} {:?} {}",
            op.has_effective_rest_parameter(resolved).unwrap(),
            op.get_expanded_parameters(resolved, false).unwrap().len(),
            show(&mut op, returned),
        ));
        let guard = op.get_resolved_signature(call).unwrap();
        let predicate = op.get_type_predicate_of_signature(guard).unwrap().unwrap();
        out.push(
            String::from_utf8_lossy(op.type_predicate_to_string(predicate).unwrap().as_bytes())
                .into_owned(),
        );
        out.push(format!(
            "{:?}",
            op.get_contextual_type_for_object_literal_element(object_member, 0)
                .unwrap()
                .map(TypeRef::id)
        ));
        out.push(format!("{:?}", op.is_context_sensitive(object).unwrap()));
        out.push(format!(
            "{:?}",
            op.get_contextual_type_for_argument_at_index(call, 0)
                .unwrap()
                .map(TypeRef::id)
        ));
        let x = op.get_property_of_type(point_type, b"x").unwrap().unwrap();
        out.push(format!(
            "{:?} {:?} {:?}",
            op.get_symbol_flags(color).unwrap(),
            op.is_readonly_symbol(x).unwrap(),
            op.get_declaration_modifier_flags_from_symbol(point)
                .unwrap(),
        ));
        out.push(
            String::from_utf8_lossy(op.get_fully_qualified_name(color).unwrap().as_bytes())
                .into_owned(),
        );
        out.push(format!(
            "{:?}",
            op.get_local_type_parameters_of_class_or_interface_or_type_alias(box_class)
                .unwrap()
                .len()
        ));
        out.push(format!(
            "{:?}",
            op.resolve_name(b"Box", Some(source), tsr_ast::symbol_flags::VALUE, false)
                .unwrap()
                .map(tsr_checker::SymbolRef::id)
        ));
        out.push(format!(
            "{:?}",
            op.get_global_symbol(b"Array", tsr_ast::symbol_flags::TYPE, None)
                .unwrap()
                .is_some()
        ));
        out.push(format!("{:?}", op.get_merged_symbol(point).unwrap().id()));
        out.push(format!("{:?}", op.get_ambient_modules().unwrap().len()));
        out.push(format!("{:?}", op.is_deprecated_declaration(old).unwrap()));
        out.push(format!("{:?}", op.get_diagnostics(source).unwrap().len()));
        out.push(format!(
            "{:?}",
            op.get_suggestion_diagnostics(source).unwrap().len()
        ));
        out.push(format!("{:?}", op.get_global_diagnostics().unwrap().len()));
        out
    };
    let first = answers();
    {
        let mut op = owner.operation().unwrap();
        let unrelated = find(&program, "/other.ts", K::Identifier, "other", 0);
        op.get_type_at_location(unrelated).unwrap();
        op.get_diagnostics(other).unwrap();
    }
    let second = answers();
    assert_eq!(first, second);
    assert_eq!(first[0], "Box<{ a: number; }>");
}

/// Contract 8 (emitresolver.go `IsReferencedAliasDeclaration`,
/// `IsValueAliasDeclaration`, `MarkLinkedReferencesRecursively`,
/// `GetTypeReferenceSerializationKind` over the marking state of C3's
/// contract 6 and C4's JSX and metadata contracts, and the recorded resolver
/// visibility queries): an import used as a value is referenced once its file
/// is marked and one used only as a type is not; a JSX factory import is
/// referenced by the elements; a parameter type under decorator metadata
/// serializes as a class constructor; and the pinned code fixes' resolver
/// queries replay with equal answers.
#[test]
fn resolver_queries_read_the_marking_state() {
    let options = CompilerOptions {
        experimental_decorators: Tristate::TRUE,
        emit_decorator_metadata: Tristate::TRUE,
        jsx: tsr_core::JsxEmit::REACT,
        ..strict()
    };
    let program = program(
        &[
            (
                "/a.ts",
                "export const value = 1;\nexport interface Shape { n: number }\nexport class Service {}\n",
            ),
            (
                "/b.ts",
                "import { value, Shape, Service } from \"./a\";\nexport const used = value;\nexport const typed: Shape = { n: 1 };\ndeclare function inject(target: object, key: string, index: number): void;\nexport class Consumer { constructor(@inject service: Service) {} }\n",
            ),
            (
                "/c.tsx",
                "import * as React from \"./react\";\nexport const element = <div />;\n",
            ),
            (
                "/react.ts",
                "export function createElement(...args: unknown[]): unknown { return args; }\ndeclare global { namespace JSX { interface IntrinsicElements { div: {} } } }\n",
            ),
        ],
        options,
    );
    let (_generation, owner) = checker(&program, &Counters::new());
    let mut op = owner.operation().unwrap();
    let b = program.file(b"/b.ts").unwrap().source();
    let c = program.file(b"/c.tsx").unwrap().source();
    op.mark_linked_references_recursively(b).unwrap();
    op.mark_linked_references_recursively(c).unwrap();
    let specifier = |text: &str| {
        let name = find(&program, "/b.ts", K::Identifier, text, 0);
        program
            .file(b"/b.ts")
            .unwrap()
            .bound()
            .view()
            .ast()
            .node(name)
            .unwrap()
            .parent()
            .unwrap()
    };
    assert!(op
        .is_referenced_alias_declaration(specifier("value"))
        .unwrap());
    assert!(op.is_value_alias_declaration(specifier("value")).unwrap());
    assert!(!op
        .is_referenced_alias_declaration(specifier("Shape"))
        .unwrap());
    assert!(!op.is_value_alias_declaration(specifier("Shape")).unwrap());
    let namespace = find(&program, "/c.tsx", K::NamespaceImport, "", 0);
    assert!(op.is_referenced_alias_declaration(namespace).unwrap());
    let parameter_type = find(&program, "/b.ts", K::TypeReference, "", 1);
    let type_name = program
        .file(b"/b.ts")
        .unwrap()
        .bound()
        .view()
        .ast()
        .node(parameter_type)
        .unwrap()
        .data_source()
        .as_type_reference_node()
        .and_then(|data| data.type_name())
        .unwrap();
    assert_eq!(
        op.type_reference_serialization_kind(Some(type_name), Some(b))
            .unwrap(),
        tsr_checker::TypeReferenceSerializationKind::TypeWithConstructSignatureAndValue
    );
    drop(op);
    let tests = [
        "TestReferences01",
        "TestCodeFixMissingTypeAnnotationOnExports11",
    ];
    let calls = matched_calls(&replay_tests("resolver", None, &tests), &tests);
    assert!(calls["EmitResolver.IsDeclarationVisible"] > 0);
}

/// Contract 9 (the pin's checker pool, `CheckerPool` giving each request its
/// own checker, and ADR 0012's retirement): two checkers over one program
/// build their own syntax, caches and resolver marks, and a panic while one
/// checker's builder is live retires only that checker.
#[test]
fn two_checkers_are_independent_and_a_builder_panic_retires_one() {
    let program = program(
        &[(
            "/m.ts",
            "import { value } from \"./v\";\nexport const shown = { a: value, b: [value] };\nconst wrong: number = \"s\";\n",
        ), ("/v.ts", "export const value = 1;\n")],
        strict(),
    );
    let (generation_a, a) = checker(&program, &Counters::new());
    let (generation_b, b) = checker(&program, &Counters::new());
    let source = program.file(b"/m.ts").unwrap().source();
    let shown = find(&program, "/m.ts", K::Identifier, "shown", 0);
    let import = find(&program, "/m.ts", K::ImportSpecifier, "", 0);
    let text = |owner: &Arc<CheckerOwner>| {
        let mut op = owner.operation().unwrap();
        let ty = op.get_type_at_location(shown).unwrap();
        String::from_utf8_lossy(op.type_to_string_default(ty).unwrap().as_bytes()).into_owned()
    };
    assert_eq!(text(&a), text(&b));
    {
        let mut op = a.operation().unwrap();
        assert_eq!(op.get_diagnostics(source).unwrap().len(), 1);
        op.mark_linked_references_recursively(source).unwrap();
        assert!(op.is_referenced_alias_declaration(import).unwrap());
    }
    {
        // B's types are its own: A refuses them before any read.
        let mut op = b.operation().unwrap();
        let ty = op.get_type_at_location(shown).unwrap();
        drop(op);
        let a_op = a.operation().unwrap();
        assert!(matches!(
            a_op.retain_type(ty),
            Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
        ));
    }
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut op = a.operation().unwrap();
        let ty = op.get_type_at_location(shown).unwrap();
        let mut builder = op.node_builder();
        builder.type_to_type_node(ty, None, 0, 0).unwrap();
        panic!("injected panic inside a builder request");
    }));
    assert!(panic.is_err());
    assert_eq!(generation_a.validate(), Err(tsr_arena::Error::Retired));
    assert!(matches!(
        a.operation(),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    assert!(generation_b.validate().is_ok());
    let mut op = b.operation().unwrap();
    assert_eq!(op.get_diagnostics(source).unwrap().len(), 1);
    op.mark_linked_references_recursively(source).unwrap();
    assert!(op.is_referenced_alias_declaration(import).unwrap());
    drop(op);
    assert!(text(&b).starts_with("{ a: number;"));
}
