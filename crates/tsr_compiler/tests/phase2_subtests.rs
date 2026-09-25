//! Phase 2 C0.7: direct tests of the Rust runner sub-tests in
//! `tools/phase2/subtests.rs` on constructed programs and trees.
//!
//! The PCG and shuffle vectors are Go 1.27.1 `math/rand/v2` output for
//! `NewPCG(1234, 5678)`, the seed of the pinned `verifyUnionOrdering`.
#[path = "../../../tools/phase2/subtests.rs"]
mod subtests;

use std::cell::Cell;
use std::sync::Arc;
use subtests::{first_parent_failure, inconsistent_orderings, ParentTree, Pcg, Sanitizer};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(files: &[(&str, &str)], options: CompilerOptions) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let roots = files
        .iter()
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

fn options() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        ..Default::default()
    }
}

fn checker(program: &Arc<Program>) -> Arc<CheckerOwner> {
    let counters = Counters::new();
    Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    )
}

#[test]
fn pcg_matches_go() {
    let mut pcg = Pcg::new(1234, 5678);
    let values: Vec<u64> = (0..4).map(|_| pcg.uint64()).collect();
    assert_eq!(
        values,
        [
            13_057_496_475_220_049_308,
            9_551_568_103_919_220_750,
            12_578_181_936_477_987_255,
            12_541_157_803_617_272_601
        ]
    );
}

#[test]
fn shuffle_matches_go() {
    let mut rng = Pcg::new(1234, 5678);
    let mut values: Vec<u32> = (0..13).collect();
    let expected: [[u32; 13]; 3] = [
        [2, 0, 4, 8, 1, 12, 5, 10, 3, 11, 7, 6, 9],
        [9, 10, 8, 4, 6, 7, 12, 2, 3, 11, 5, 0, 1],
        [0, 1, 9, 11, 12, 2, 8, 6, 10, 4, 3, 5, 7],
    ];
    for permutation in expected {
        rng.shuffle(&mut values);
        assert_eq!(values, permutation);
    }
    let mut rng = Pcg::new(1234, 5678);
    let mut many: Vec<u32> = (0..1000).collect();
    rng.shuffle(&mut many);
    assert_eq!(many[..8], [878, 201, 527, 7, 1, 254, 629, 179]);
    assert_eq!(many[992..], [895, 748, 138, 368, 677, 680, 517, 707]);
}

#[test]
fn ordering_check_detects_an_inconsistent_comparator() {
    let unions = vec![vec![1u32, 2, 3, 4], vec![5, 9]];
    let consistent = inconsistent_orderings(&unions, |a, b| Ok::<_, ()>(a.cmp(&b))).unwrap();
    assert_eq!(consistent, 0);
    // A comparator that disagrees with the stored order fails every check.
    let reversed = inconsistent_orderings(&unions, |a, b| Ok::<_, ()>(b.cmp(&a))).unwrap();
    assert_eq!(reversed, 22);
    // A cyclic comparator is not a total order; it is counted, never a crash.
    let cyclic = inconsistent_orderings(&[vec![0u32, 1, 2]], |a, b| {
        Ok::<_, ()>(if (a + 1) % 3 == b {
            std::cmp::Ordering::Less
        } else if a == b {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        })
    })
    .unwrap();
    assert!(cyclic > 0);
    // Comparator errors propagate instead of passing.
    assert_eq!(
        inconsistent_orderings(&unions, |_, _| Err::<std::cmp::Ordering, _>("broken")),
        Err("broken")
    );
}

#[test]
fn union_ordering_on_a_checked_program() {
    let program = program(
        &[(
            "/a.ts",
            "declare let a: string | number | undefined;\n\
             declare let b: \"x\" | \"y\" | 1 | true;\n\
             declare let c: { k: 1 } | { k: 2 } | null;\n\
             export const d = [a, b, c];\n",
        )],
        options(),
    );
    let owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let source = program.file(b"/a.ts").unwrap().source();
    op.semantic_diagnostics(source).unwrap();
    let observed = subtests::union_ordering(&op);
    assert_eq!(observed["state"], "executed");
    assert_eq!(observed["inconsistent"], 0);
    assert!(observed["unions"].as_u64().unwrap() >= 3, "{observed}");
}

#[test]
fn parent_walk_includes_user_declarations_and_skips_default_libraries() {
    let program = program(
        &[
            (
                "/a.ts",
                "export function f(x: number) { return { x, y: [x, `${x}`] }; }\n",
            ),
            (
                "/b.d.ts",
                "declare namespace N { interface I { m(): void } }\n",
            ),
        ],
        options(),
    );
    let observed = subtests::parent_pointers(&program);
    assert_eq!(observed["state"], "executed");
    assert_eq!(observed["failure"], serde_json::Value::Null);
    // Two user files; the default library files are loaded but skipped.
    assert!(program.files().len() > 2);
    assert_eq!(observed["files"], 2);
    assert!(observed["nodes"].as_u64().unwrap() > 20);
}

#[test]
fn parent_walk_reaches_reparsed_jsdoc_children() {
    let js = |text: &str| {
        let program = program(
            &[("/a.js", text)],
            CompilerOptions {
                allow_js: Tristate::TRUE,
                check_js: Tristate::TRUE,
                ..options()
            },
        );
        let observed = subtests::parent_pointers(&program);
        assert_eq!(observed["failure"], serde_json::Value::Null, "{observed}");
        observed["nodes"].as_u64().unwrap()
    };
    // The reparsed JSDoc type is a real child of the declaration and must
    // carry it as parent, as in Go's ForEachChild.
    assert!(js("/** @type {string | number} */\nvar x = 1;\n") > js("var x = 1;\n"));
}

/// A tree with an explicit parent table, so wrong and missing parents can be
/// constructed; the production AST cannot represent them.
struct Table {
    children: Vec<Vec<u32>>,
    parents: Vec<Option<u32>>,
    reads: Cell<u32>,
}

impl ParentTree<u32> for Table {
    fn parent(&self, node: u32) -> Result<Option<u32>, Box<dyn std::error::Error>> {
        self.reads.set(self.reads.get() + 1);
        Ok(self.parents[node as usize])
    }
    fn children(&self, node: u32) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
        Ok(self.children[node as usize].clone())
    }
    fn describe(&self, node: u32) -> String {
        format!("node {node}")
    }
}

#[test]
fn parent_walk_accepts_a_parentless_root_and_stops_at_the_first_failure() {
    // 0 -> [1, 2], 1 -> [3], 2 -> [4]
    let tree = |parents: Vec<Option<u32>>| Table {
        children: vec![vec![1, 2], vec![3], vec![4], vec![], vec![]],
        parents,
        reads: Cell::new(0),
    };
    let mut nodes = 0;
    let valid = tree(vec![None, Some(0), Some(0), Some(1), Some(2)]);
    assert_eq!(first_parent_failure(&valid, 0, &mut nodes).unwrap(), None);
    assert_eq!(nodes, 4);

    let mut nodes = 0;
    let missing = tree(vec![None, Some(0), Some(0), None, Some(2)]);
    assert_eq!(
        first_parent_failure(&missing, 0, &mut nodes)
            .unwrap()
            .as_deref(),
        Some("parent node does not exist")
    );
    // Pre-order: 1, then 3 fails; 2 and 4 are never read.
    assert_eq!(nodes, 2);
    assert_eq!(missing.reads.get(), 2);

    let mut nodes = 0;
    let wrong = tree(vec![None, Some(0), Some(0), Some(1), Some(1)]);
    assert_eq!(
        first_parent_failure(&wrong, 0, &mut nodes)
            .unwrap()
            .as_deref(),
        Some("parent node does not match traversed parent: node 4")
    );
    assert_eq!(nodes, 4);
}

#[test]
fn trace_renders_the_loader_trace_like_the_baseline_tracer() {
    let files = [
        ("/a.ts", "import { x } from \"./b\";\nx;\n"),
        ("/b.ts", "export const x = 1;\n"),
    ];
    let traced = program(
        &files,
        CompilerOptions {
            trace_resolution: Tristate::TRUE,
            ..options()
        },
    );
    let text = String::from_utf8(subtests::trace_text(&traced)).unwrap();
    assert!(
        text.starts_with("======== Resolving module './b' from '/a.ts'. ========\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "======== Module name './b' was successfully resolved to '/b.ts'. ========\n"
        ),
        "{text}"
    );
    assert_eq!(subtests::trace(&traced, true)["state"], "content");
    assert_eq!(subtests::trace(&traced, false)["state"], "disabled");
    let quiet = program(&files, options());
    assert_eq!(subtests::trace(&quiet, true)["state"], "no_content");
}

#[test]
fn sanitizer_follows_the_pinned_tracer() {
    let mut sanitizer = Sanitizer::new(b"/", true);
    let version = format!("Resolving with version '{}' now.", tsr_core::version());
    assert_eq!(
        sanitizer.sanitize(version.as_bytes(), true),
        b"Resolving with version 'FakeTSVersion' now."
    );
    // Cached lookups become the uncached message once, then stay cached.
    let cached = b"File '/p/package.json' does not exist according to earlier cached lookups.";
    assert_eq!(
        sanitizer.sanitize(cached, true),
        b"File '/p/package.json' does not exist."
    );
    assert_eq!(sanitizer.sanitize(cached, true), cached.to_vec());
    let exists = b"File '/e/package.json' exists according to earlier cached lookups.";
    assert_eq!(
        sanitizer.sanitize(exists, true),
        b"Found 'package.json' at '/e/package.json'."
    );
    // Uncached lookups are kept once and then rendered as cached.
    let missing = b"File '/q/package.json' does not exist.";
    assert_eq!(sanitizer.sanitize(missing, true), missing.to_vec());
    assert_eq!(
        sanitizer.sanitize(missing, true),
        b"File '/q/package.json' does not exist according to earlier cached lookups."
    );
    let found = b"Found 'package.json' at '/r/package.json'.";
    assert_eq!(sanitizer.sanitize(found, true), found.to_vec());
    assert_eq!(
        sanitizer.sanitize(found, true),
        b"File '/r/package.json' exists according to earlier cached lookups."
    );
    // Without the package.json cache, only the cached-lookup rewrites apply.
    assert_eq!(sanitizer.sanitize(missing, false), missing.to_vec());
}
