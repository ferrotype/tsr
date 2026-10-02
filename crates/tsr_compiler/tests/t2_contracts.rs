//! Phase 3 T2 contracts (docs/PHASE3-plan.md, sections 4 and 5): each
//! output's source map is generated for that output alone, and the printer
//! records source-map positions through its growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_core::{CompilerOptions, JsxEmit, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn mapped() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        source_map: Tristate::TRUE,
        inline_sources: Tristate::TRUE,
        declaration: Tristate::TRUE,
        declaration_map: Tristate::TRUE,
        ..CompilerOptions::default()
    }
}

/// A file that imports another and exports a value and a function, so its
/// maps would name the other file's source if a generator were shared.
fn file_sources(name: &str) -> String {
    format!(
        "import {{ shared }} from \"./shared\";\nexport const {name}: number = shared + 1;\nexport function {name}f(p: number): number {{ return p * {name}; }}\n"
    )
}

/// The source-map generator is per output (`emitter.printSourceFile` creates
/// one for each file it writes, `emitJSFile` and `emitDeclarationFile` each
/// print their own): with `sourceMap`, `inlineSources` and `declarationMap`,
/// `EmitResult.SourceMaps` holds one map per script and per declaration file,
/// in input order, and each names its own output and only its own source,
/// with that source's text inlined for a script map. Each written map is the
/// recorded one, and each output ends with the URL of its own map. Both modes
/// produce the same result.
#[test]
fn each_output_maps_only_its_own_source_in_input_order() {
    let names = ["a", "b", "c", "d"];
    let mut program = files(&[
        ("/lib.d.ts", LIB),
        ("/shared.ts", "export const shared: number = 1;\n"),
    ]);
    for name in names {
        program.push((format!("/{name}.ts"), file_sources(name)));
    }
    let mut observations = Vec::new();
    for mode in Mode::BOTH {
        let (checked, _) = support::checked(&program, &mapped(), mode);
        let observed = support::emit_all(&checked);
        let mut expected = Vec::new();
        for name in std::iter::once("shared").chain(names) {
            expected.push(format!("/{name}.js"));
            expected.push(format!("/{name}.d.ts"));
        }
        let generated: Vec<String> = observed
            .source_maps
            .iter()
            .map(|(file, _, _)| file.clone())
            .collect();
        assert_eq!(generated, expected, "{mode:?}");
        for (generated, inputs, map) in &observed.source_maps {
            let stem = generated
                .trim_start_matches('/')
                .split('.')
                .next()
                .expect("a stem");
            let source = format!("{stem}.ts");
            assert_eq!(inputs, &vec![format!("/{source}")], "{mode:?} {generated}");
            assert_eq!(
                map.sources,
                vec![JsString::from_bytes(source.as_bytes())],
                "{mode:?} {generated}"
            );
            let output = generated.trim_start_matches('/');
            assert_eq!(map.file.as_bytes(), output.as_bytes(), "{mode:?}");
            if extension(generated) == "js" {
                let text = program
                    .iter()
                    .find(|(file, _)| file == &format!("/{source}"))
                    .map(|(_, text)| text.as_bytes())
                    .expect("the source");
                assert_eq!(
                    map.sources_content,
                    Some(vec![Some(JsString::from_bytes(text))]),
                    "{mode:?} {generated}"
                );
            } else {
                assert_eq!(map.sources_content, None, "{mode:?} {generated}");
            }
            let url = format!("//# sourceMappingURL={output}.map");
            assert!(
                observed.text(generated).ends_with(url.as_bytes()),
                "{mode:?} {generated}"
            );
            let mut written = tsr_sourcemap::RawSourceMap::default();
            tsr_json::unmarshal(
                observed.text(&format!("{generated}.map")),
                &mut written,
                tsr_json::Options::default(),
            )
            .expect("a source map");
            assert_eq!(&written, map, "{mode:?} {generated}.map");
        }
        observations.push(observed);
    }
    assert_eq!(observations[0], observations[1], "the modes differ");
}

/// The extension of a file name.
fn extension(name: &str) -> &str {
    std::path::Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
}

/// The mappings of `map` decoded: their count, with no decoding error.
fn segments(map: &tsr_sourcemap::RawSourceMap) -> usize {
    let mut decoder = tsr_sourcemap::decode_mappings(map.mappings.clone());
    let count = decoder.values().count();
    assert!(decoder.error().is_none(), "the mappings decode");
    count
}

/// Depth of the deep inputs printed with source maps.
const DEPTH: usize = 500;

/// Deep inputs printed with a source-map generator on both outputs (ADR
/// 0011): a left-nested binary chain, a call chain, nested blocks, nested
/// JSX (printed, `jsx: preserve`) and a nested object type in a declaration,
/// `DEPTH` levels deep. The printer records a position before and after each
/// nested node, inside the same guarded recursion; single-threaded on a 256
/// KiB thread and concurrently on the work group's reserved stacks the
/// outputs and maps are the same, and each map holds a mapping for every
/// level.
#[test]
fn deep_inputs_map_through_the_growth_guards() {
    let n = DEPTH;
    let options = CompilerOptions {
        jsx: JsxEmit::PRESERVE,
        ..mapped()
    };
    let cases = [
        (
            "/binary.ts",
            format!(
                "declare const a: number;\nexport const x: number = {};\n",
                support::repeat("a", n, " + ")
            ),
        ),
        (
            "/call.ts",
            format!(
                "declare const a: any;\nexport const x: number = a{};\n",
                "()".repeat(n)
            ),
        ),
        (
            "/blocks.ts",
            format!(
                "export function f(): void {{ {}let v: number = 1;{} }}\n",
                "{".repeat(n),
                "}".repeat(n)
            ),
        ),
        (
            "/jsx.tsx",
            format!(
                "declare namespace JSX {{ interface IntrinsicElements {{ [name: string]: any }} }}\nexport const x = {}{};\n",
                "<a>".repeat(n),
                "</a>".repeat(n)
            ),
        ),
        (
            "/types.ts",
            format!(
                "export type T = {}number{};\n",
                "{ a: ".repeat(n),
                " }".repeat(n)
            ),
        ),
    ];
    for (name, text) in &cases {
        let observed = support::emit_deep(&files(&[("/lib.d.ts", LIB), (name, text)]), &options);
        let stem = name.trim_end_matches(".tsx").trim_end_matches(".ts");
        let script = if extension(name) == "tsx" {
            format!("{stem}.jsx")
        } else {
            format!("{stem}.js")
        };
        let declaration = format!("{stem}.d.ts");
        let map = |generated: &str| {
            &observed
                .source_maps
                .iter()
                .find(|(file, _, _)| file == generated)
                .unwrap_or_else(|| panic!("{name}: no map for {generated}"))
                .2
        };
        let deepest = if name.ends_with("types.ts") {
            segments(map(&declaration))
        } else {
            segments(map(&script))
        };
        assert!(deepest >= n, "{name}: {deepest} mappings for {n} levels");
    }
}
