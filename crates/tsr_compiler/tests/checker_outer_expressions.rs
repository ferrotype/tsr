//! Checker copies of the pinned outer-expression skips. getSyntacticTruthySemantics
//! skips `ast.OEKAll` (checker.go:13076) and getEffectiveCheckNode skips
//! parentheses and `satisfies`, keeping a JSDoc type assertion in a JS file
//! (checker.go:9558-9565); both go through `tsr_ast::utilities::skip_outer_expressions`.
//!
//! Every expectation is the pinned tsgo binary's output (1f70213d, Go 1.27.1)
//! for the same text with the default library, `--strict --skipLibCheck
//! --target esnext --module esnext --noEmit`, plus `--allowJs --checkJs` for the
//! JS program: code, line:column and underline length, and message arguments.

use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

/// (code, 1-based line, 1-based column, length, message arguments).
type Expected<'a> = (i32, i64, i64, i64, &'a [&'a str]);

fn semantic(name: &[u8], text: &[u8], javascript: bool) -> Vec<(i32, i64, i64, i64, Vec<String>)> {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name, text);
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        module: ModuleKind::ESNEXT,
                        strict: Tristate::TRUE,
                        skip_lib_check: Tristate::TRUE,
                        allow_js: if javascript {
                            Tristate::TRUE
                        } else {
                            Tristate::UNKNOWN
                        },
                        check_js: if javascript {
                            Tristate::TRUE
                        } else {
                            Tristate::UNKNOWN
                        },
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(name)],
                ),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    let source = program.file(name).unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let starts: Vec<usize> = std::iter::once(0)
        .chain(
            text.iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(at, _)| at + 1),
        )
        .collect();
    let mut observed: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            let pos = usize::try_from(d.loc.pos()).unwrap();
            let line = starts.partition_point(|&start| start <= pos);
            (
                d.code,
                line as i64,
                (pos - starts[line - 1] + 1) as i64,
                d.loc.len(),
                d.message_args
                    .iter()
                    .map(|argument| String::from_utf8_lossy(argument.as_bytes()).into_owned())
                    .collect(),
            )
        })
        .collect();
    observed.sort();
    observed
}

fn expected(rows: &[Expected<'_>]) -> Vec<(i32, i64, i64, i64, Vec<String>)> {
    let mut rows: Vec<_> = rows
        .iter()
        .map(|&(code, line, column, length, args)| {
            (
                code,
                line,
                column,
                length,
                args.iter().map(|&a| a.to_owned()).collect(),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn truthiness_checks_skip_every_outer_expression_kind() {
    // An instantiation expression is an ExpressionWithTypeArguments, which
    // OEKAll skips: the arrow is always truthy and `undefined` always falsy.
    let text = b"if ((() => 1)<string>) {}
if (undefined<string>) {}
if ((0 as any)!) {}
";
    assert_eq!(
        semantic(b"/main.ts", text, false),
        expected(&[
            (2872, 1, 5, 17, &[]),
            (2635, 1, 15, 6, &["() => number"]),
            (2873, 2, 5, 17, &[]),
            (2635, 2, 15, 6, &["undefined"]),
        ])
    );
}

#[test]
fn argument_errors_anchor_past_parentheses_and_satisfies() {
    let text = b"declare function f(a: number): void;
f(((\"x\")));
f((\"x\" satisfies string));
";
    assert_eq!(
        semantic(b"/main.ts", text, false),
        expected(&[
            (2345, 2, 5, 3, &["string", "number"]),
            (2345, 3, 4, 3, &["string", "number"]),
        ])
    );
}

#[test]
fn argument_errors_in_checked_js_keep_a_jsdoc_type_assertion() {
    // The reparsed `/** @type {string} */ (...)` is an outer expression the JS
    // mask excludes, so the error stays on the parenthesis; plain parentheses
    // in the same file are still skipped.
    let text = b"/**
 * @param {number} a
 */
function f(a) {}
f(/** @type {string} */ (\"x\"));
f(((\"y\")));
";
    assert_eq!(
        semantic(b"/main.js", text, true),
        expected(&[
            (2345, 5, 25, 5, &["string", "number"]),
            (2345, 6, 5, 3, &["string", "number"]),
        ])
    );
}
