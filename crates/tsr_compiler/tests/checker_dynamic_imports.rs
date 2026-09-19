//! Native P6 observations of dynamic-import results and semantic diagnostics.
//! Nested property/signature displays remain part of the separate full comparison.
use serde_json::{json, Value};
use std::{fmt::Write as _, sync::Arc};
use ts_arena::{CheckerIdentity, Counters, Generation, NodeId};
use ts_ast::{Diagnostic, SyntaxKind as K};
use ts_checker::CheckerOwner;
use ts_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use ts_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use ts_jsstring::JsString;

fn hex(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

fn payload(program: &Program, diagnostic: &Diagnostic) -> Value {
    let file = diagnostic.file.map(|source| {
        let file = program
            .files()
            .iter()
            .find(|file| file.source() == source)
            .unwrap();
        String::from_utf8(
            file.bound()
                .view()
                .ast()
                .source_file(source)
                .unwrap()
                .file_name()
                .to_vec(),
        )
        .unwrap()
    });
    let args = if diagnostic.message_args.is_empty() {
        Value::Null
    } else {
        json!(diagnostic
            .message_args
            .iter()
            .map(|arg| std::str::from_utf8(arg.as_bytes()).unwrap())
            .collect::<Vec<_>>())
    };
    json!({"file":file, "pos":diagnostic.loc.pos(), "end":diagnostic.loc.end(),
        "code":diagnostic.code, "category":diagnostic.category, "args":args,
        "key_hex":hex(diagnostic.message_key.as_bytes()), "text_hex":hex(diagnostic.message_text.as_bytes()),
        "source_hex":hex(diagnostic.source.as_bytes()),
        "chain":diagnostic.message_chain.iter().map(|d|payload(program,d)).collect::<Vec<_>>(),
        "related":diagnostic.related_information.iter().map(|d|payload(program,d)).collect::<Vec<_>>(),
        "unnecessary":diagnostic.reports_unnecessary,"deprecated":diagnostic.reports_deprecated,
        "skipped_on_no_emit":diagnostic.skipped_on_no_emit})
}

fn declaration(program: &Program, file: &str, name: &str) -> NodeId {
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    for statement in view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
    {
        let read = view.node(statement).unwrap();
        if read.kind() != K::VariableStatement {
            continue;
        }
        let list = read
            .as_variable_statement()
            .unwrap()
            .declaration_list()
            .unwrap();
        let list = view
            .node(list)
            .unwrap()
            .as_variable_declaration_list()
            .unwrap()
            .declarations()
            .unwrap();
        for declaration in view
            .node_slice(view.list(list).unwrap().nodes())
            .unwrap()
            .iter()
            .flatten()
        {
            let read = view.node(declaration).unwrap();
            if view.node_text(read.name().unwrap()).unwrap().as_bytes() == name.as_bytes() {
                return declaration;
            }
        }
    }
    panic!("missing fixture declaration {name}");
}

#[test]
fn dynamic_import_result_types_and_diagnostics_match_native() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/dynamic-import-tests.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 11);
    for case in cases {
        let request = &case["program"];
        let id = request["id"].as_str().unwrap();
        let counters = Counters::new();
        let generation = Generation::new(&counters);
        let mut fs = ts_vfs::MemoryBuilder::new(b"/", true);
        for (name, source) in request["files"].as_object().unwrap() {
            fs.insert_loaded(name.as_bytes(), source.as_str().unwrap().as_bytes());
        }
        let libraries = case["options"]["noLib"] == false;
        let host: Arc<dyn ts_vfs::FileSystem> = if libraries {
            Arc::new(ts_bundled::BundledFs::new(Arc::new(fs.finish())))
        } else {
            Arc::new(fs.finish())
        };
        let module = match case["options"]["module"].as_str().unwrap() {
            "NodeNext" => ModuleKind::NODE_NEXT,
            "CommonJS" => ModuleKind::COMMON_JS,
            "ESNext" => ModuleKind::ESNEXT,
            other => panic!("unsupported fixture module {other}"),
        };
        let program = Arc::new(
            Program::load(
                ProgramOptions {
                    config: ts_tsoptions::ParsedCommandLine::new(
                        CompilerOptions {
                            target: ScriptTarget::ESNEXT,
                            module,
                            strict: Tristate::TRUE,
                            no_lib: if libraries {
                                Tristate::FALSE
                            } else {
                                Tristate::TRUE
                            },
                            skip_lib_check: if libraries {
                                Tristate::TRUE
                            } else {
                                Tristate::UNKNOWN
                            },
                            resolve_json_module: if case["options"]["resolveJsonModule"] == true {
                                Tristate::TRUE
                            } else {
                                Tristate::UNKNOWN
                            },
                            ..Default::default()
                        },
                        request["roots"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|path| JsString::from_bytes(path.as_str().unwrap().as_bytes()))
                            .collect(),
                    ),
                    host,
                    current_directory: JsString::from_bytes(b"/".as_slice()),
                    default_library_path: JsString::from_bytes(if libraries {
                        ts_bundled::LIB_PATH
                    } else {
                        b"/no-default-lib"
                    }),
                    skip_module_resolution: false,
                },
                &mut FileCache::new(),
                &counters,
            )
            .unwrap(),
        );
        let owner = CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap();
        let owner = Arc::new(owner);
        let mut op = owner.operation().unwrap();
        let queries = request["queries"].as_array().unwrap();
        let expected_types = case["types"].as_array().unwrap();
        assert_eq!(queries.len(), expected_types.len(), "{id}: query count");
        for (query, expected) in queries.iter().zip(expected_types) {
            assert_eq!(query["id"], expected["id"], "{id}: query order");
            let decl = declaration(
                &program,
                query["file"].as_str().unwrap(),
                query["declaration"].as_str().unwrap(),
            );
            let file = program
                .file(query["file"].as_str().unwrap().as_bytes())
                .unwrap();
            let view = file.bound().view().ast();
            let read = view.node(decl).unwrap();
            let node = match query["target"].as_str().unwrap() {
                "initializer" => read.initializer().unwrap(),
                "name" => read.name().unwrap(),
                other => panic!("unsupported query target {other}"),
            };
            let ty = op.get_type_at_location(node).unwrap();
            let flags = ts_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                | ts_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
            let actual = op.type_to_string(ty, flags).unwrap();
            assert_eq!(
                hex(actual.as_bytes()),
                expected["display_hex"],
                "{id}: {}",
                query["id"]
            );
            // Repeat through the public API, including options/global-type caches.
            let again = op.get_type_at_location(node).unwrap();
            assert_eq!(
                op.type_to_string(again, flags).unwrap(),
                actual,
                "{id}: repeated query"
            );
        }
        let mut diagnostics = Vec::new();
        for file in program.files() {
            if program.skip_type_checking(file, false).unwrap() {
                continue;
            }
            diagnostics.extend(op.semantic_diagnostics(file.source()).unwrap());
        }
        let diagnostics = program
            .sort_and_deduplicate_diagnostics(&diagnostics)
            .unwrap();
        assert_eq!(
            json!(diagnostics
                .iter()
                .map(|d| payload(&program, d))
                .collect::<Vec<_>>()),
            case["semantic"],
            "{id}: semantic diagnostics"
        );
    }
}
