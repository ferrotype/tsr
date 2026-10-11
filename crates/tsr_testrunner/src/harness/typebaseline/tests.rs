use super::walker::rows::{Display, Row as Observation, SymbolDeclaration};
use super::*;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

#[test]
fn host_symbol_envelope_escapes_masks_and_truncates_observed_positions() {
    // Exact pinned Symbol/Decl envelope, composed without checker access.
    let row = decorate::observation_row(Observation {
        line: 2,
        source_text: b"value".to_vec(),
        display: Display::Symbol {
            name: b"\xfevalue".to_vec(),
            declarations: vec![
                SymbolDeclaration {
                    file: b"/.lib/lib.es5.d.ts".to_vec(),
                    line: 100,
                    character: 20,
                },
                SymbolDeclaration {
                    file: b"/.src/a.ts".to_vec(),
                    line: 0,
                    character: 13,
                },
                SymbolDeclaration {
                    file: b"/.src/b.ts".to_vec(),
                    line: 1,
                    character: 2,
                },
                SymbolDeclaration {
                    file: b"/.src/c.ts".to_vec(),
                    line: 3,
                    character: 4,
                },
                SymbolDeclaration {
                    file: b"/.src/d.ts".to_vec(),
                    line: 5,
                    character: 6,
                },
            ],
            declaration_count: 7,
        },
    })
    .unwrap();
    assert_eq!(row.line, 2);
    assert_eq!(row.source_text, b"value");
    assert_eq!(row.display, b"Symbol(__value, Decl(lib.es5.d.ts, --, --), Decl(a.ts, 0, 13), Decl(b.ts, 1, 2), Decl(c.ts, 3, 4), Decl(d.ts, 5, 6) ... and 2 more)");
}

#[test]
fn host_refuses_incomplete_symbol_declaration_observations() {
    let row = Observation {
        line: 0,
        source_text: b"value".to_vec(),
        display: Display::Symbol {
            name: b"value".to_vec(),
            declarations: vec![],
            declaration_count: 1,
        },
    };
    let error = decorate::observation_row(row)
        .err()
        .expect("missing positions must fail");
    assert!(error
        .to_string()
        .contains("incomplete declaration positions"));
}

#[test]
fn collected_native_schedule_keeps_types_before_symbols_and_decorates_on_host() {
    const SOURCE: &[u8] = b"export const value: number = 1;\n";
    let counters = Counters::new();
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/entry.ts", SOURCE);
    let program = Arc::new(
        Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/entry.ts".as_slice())],
                ),
                host: Arc::new(host.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/lib".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let source = program.file(b"/entry.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut operation = pool.checker_for_file_exclusive(source).unwrap();
    let mut trace = Trace::default();
    let result = generate(
        &program,
        &mut operation,
        &[InputFile {
            name: b"entry.ts",
            content: SOURCE,
        }],
        b"entry.ts",
        false,
        &mut trace,
    );
    assert_eq!(result["state"], "executed", "{result}");
    let operations: Vec<_> = trace
        .queries
        .iter()
        .map(|row| row["operation"].as_str().unwrap())
        .collect();
    assert_eq!(
        operations,
        [
            "GetTypeAtLocation",
            "TypeToTypeNode",
            "GetTypeAtLocation",
            "TypeToTypeNode",
            "GetSymbolAtLocation",
            "SymbolToStringEx",
            "GetSymbolAtLocation",
        ]
    );
    assert_eq!(trace.queries[0]["pos"], 12);
    assert_eq!(trace.queries[2]["pos"], 28);
    assert_eq!(trace.queries[6]["absent"], true);
    let expected_types = b"//// [entry.ts] ////\r\n\r\n=== entry.ts ===\r\nexport const value: number = 1;\r\n>value : number\r\n>1 : 1\r\n\r\n";
    let expected_symbols = b"//// [entry.ts] ////\r\n\r\n=== entry.ts ===\r\nexport const value: number = 1;\r\n>value : Symbol(value, Decl(entry.ts, 0, 12))\r\n\r\n";
    assert_eq!(result["types"]["text_hex"], hex(expected_types));
    assert_eq!(result["symbols"]["text_hex"], hex(expected_symbols));
}
