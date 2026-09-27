//! Test-only loader and renderer for the C4 native cases
//! (`fixtures/c4/fixtures.json`, recorded by `fixtures/c4/regenerate.py`). A
//! case is a root file and the files it depends on, checked under the flags of
//! its recorded native command, which is the one authority for what the pinned
//! `tsgo` checked.
#![allow(dead_code)]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, JsxEmit, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

/// The `react` package every JSX mode case can resolve: a classic namespace
/// and an automatic runtime, but no development runtime.
pub const REACT: &[(&str, &str)] = &[
    (
        "node_modules/react/package.json",
        include_str!("../fixtures/c4/node_modules/react/package.json"),
    ),
    (
        "node_modules/react/index.d.ts",
        include_str!("../fixtures/c4/node_modules/react/index.d.ts"),
    ),
    (
        "node_modules/react/jsx-runtime.d.ts",
        include_str!("../fixtures/c4/node_modules/react/jsx-runtime.d.ts"),
    ),
];

pub struct Case {
    pub program: Arc<Program>,
    pub owner: Arc<CheckerOwner>,
    pub generation: Generation,
    pub root: String,
    pub native: Value,
}

fn options_from_command(command: &[Value]) -> CompilerOptions {
    let mut options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        ..Default::default()
    };
    let args: Vec<&str> = command.iter().filter_map(Value::as_str).collect();
    let flags = &args[1..args.len() - 1];
    let mut i = 0;
    while i < flags.len() {
        let flag = flags[i];
        let value = flags.get(i + 1).copied().filter(|v| !v.starts_with("--"));
        let tristate = match value {
            Some("false") => Tristate::FALSE,
            _ => Tristate::TRUE,
        };
        match flag {
            "--strict" => options.strict = tristate,
            "--allowJs" => options.allow_js = tristate,
            "--checkJs" => options.check_js = tristate,
            "--skipLibCheck" => options.skip_lib_check = tristate,
            "--isolatedModules" => options.isolated_modules = tristate,
            "--experimentalDecorators" => options.experimental_decorators = tristate,
            "--emitDecoratorMetadata" => options.emit_decorator_metadata = tristate,
            "--jsx" => {
                options.jsx = match value {
                    Some("preserve") => JsxEmit::PRESERVE,
                    Some("react-native") => JsxEmit::REACT_NATIVE,
                    Some("react") => JsxEmit::REACT,
                    Some("react-jsx") => JsxEmit::REACT_JSX,
                    Some("react-jsxdev") => JsxEmit::REACT_JSX_DEV,
                    other => panic!("unsupported native jsx mode {other:?}"),
                }
            }
            "--target" => assert_eq!(value, Some("esnext")),
            "--noEmit" | "--ignoreConfig" | "--pretty" => {}
            other => panic!("unsupported native flag {other}"),
        }
        i += 1 + usize::from(value.is_some());
    }
    options
}

/// Loads `case` from its record: `files` holds the root first, then every
/// other file the record binds, as (name relative to `fixtures/c4`, text).
pub fn load(case: &str, native_json: &str, files: &[(&str, &str)]) -> Case {
    let native: Value = serde_json::from_str(native_json).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"], "{case}: fixture pin");
    let digests: serde_json::Map<String, Value> = files
        .iter()
        .map(|(name, text)| {
            (
                (*name).to_string(),
                json!(format!("{:x}", Sha256::digest(text.as_bytes()))),
            )
        })
        .collect();
    assert_eq!(
        native["sources_sha256"],
        Value::Object(digests),
        "{case}: fixture source digests"
    );
    // The recorder refuses unclean runs; the loader refuses a record whose
    // exit code disagrees with its diagnostics or that carries stderr.
    let count = native["diagnostics"].as_array().unwrap().len();
    let exit = native["exit_code"].as_i64().unwrap();
    assert!(
        (exit == 0 && count == 0) || (exit == 2 && count > 0),
        "{case}: exit code {exit} disagrees with {count} recorded diagnostics"
    );
    assert_eq!(
        native["native_stderr"].as_str().unwrap_or(""),
        "",
        "{case}: the native run wrote to stderr"
    );
    let command = native["native_command"].as_array().unwrap();
    let root = command.last().unwrap().as_str().unwrap().to_string();
    assert_eq!(root, files[0].0, "{case}: the root is the first file");
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(format!("/{name}").as_bytes(), text.as_bytes());
    }
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    options_from_command(command),
                    vec![JsString::from_bytes(format!("/{root}").as_bytes())],
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
    let (generation, owner) = checker(&program);
    Case {
        program,
        owner,
        generation,
        root,
        native,
    }
}

/// Another checker over the same program.
pub fn checker(program: &Arc<Program>) -> (Generation, Arc<CheckerOwner>) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    (generation, owner)
}

fn position(text: &[u8], offset: usize) -> (usize, usize) {
    let prefix = &text[..offset];
    let line = prefix.iter().filter(|&&byte| byte == b'\n').count() + 1;
    let start = prefix
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1);
    let column = String::from_utf8_lossy(&prefix[start..])
        .encode_utf16()
        .count()
        + 1;
    (line, column)
}

type SortKey = (String, i64, i64, i32, i32, String);

/// The case's diagnostics the way the pinned command line composes and
/// prints them (`GetDiagnosticsOfAnyProgram`, then
/// `SortAndDeduplicateDiagnostics`): every file's syntactic diagnostics
/// alone when there are any; otherwise the global diagnostics alone when the
/// checker starts with any; otherwise the bind and checker diagnostics of every
/// file that is type checked (declaration files are skipped under
/// `--skipLibCheck`), with the global diagnostics found while checking.
pub fn observed_with(case: &Case, owner: &Arc<CheckerOwner>) -> Vec<Value> {
    let files = case.program.files();
    let mut diagnostics = Vec::new();
    for file in files.iter() {
        diagnostics.extend(case.program.syntactic_diagnostics(Some(file)).unwrap());
    }
    if diagnostics.is_empty() {
        let mut op = owner.operation().unwrap();
        diagnostics = op.global_diagnostics().unwrap();
        if diagnostics.is_empty() {
            for file in files.iter() {
                if file
                    .bound()
                    .view()
                    .source_file()
                    .unwrap()
                    .is_declaration_file
                {
                    continue;
                }
                diagnostics.extend(case.program.bind_diagnostics(Some(file.source())).unwrap());
                diagnostics.extend(op.semantic_diagnostics(file.source()).unwrap());
            }
            diagnostics.extend(op.global_diagnostics().unwrap());
        }
    }
    let file_of = |id: NodeId| {
        files
            .iter()
            .find(|file| file.source() == id)
            .expect("diagnostic source retained")
    };
    let mut rendered: Vec<(SortKey, Value)> = diagnostics
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.category, 1);
            let message =
                String::from_utf8(tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap()).unwrap();
            let (path, file, line, column) = match diagnostic.file {
                Some(id) => {
                    let source = file_of(id).bound().view().source_file().unwrap();
                    let name = String::from_utf8(source.file_name().to_vec()).unwrap();
                    let (line, column) = position(
                        source.text().as_bytes(),
                        usize::try_from(diagnostic.loc.pos()).unwrap(),
                    );
                    let printed = name.strip_prefix('/').unwrap_or(&name).to_string();
                    (name, json!(printed), json!(line), json!(column))
                }
                None => (String::new(), Value::Null, Value::Null, Value::Null),
            };
            let key = (
                path,
                diagnostic.loc.pos(),
                diagnostic.loc.end(),
                diagnostic.code,
                diagnostic.category,
                message.clone(),
            );
            (
                key,
                json!({"file": file, "line": line, "column": column, "code": diagnostic.code, "message": message}),
            )
        })
        .collect();
    rendered.sort_by(|a, b| a.0.cmp(&b.0));
    rendered.dedup_by(|a, b| a.1 == b.1);
    rendered.into_iter().map(|(_, value)| value).collect()
}

pub fn observed(case: &Case) -> Vec<Value> {
    observed_with(case, &case.owner)
}

/// Loads the case and asserts its diagnostics equal the native record.
pub fn assert_case(case: &str, native_json: &str, files: &[(&str, &str)]) -> Case {
    let loaded = load(case, native_json, files);
    assert_eq!(
        json!(observed(&loaded)),
        loaded.native["diagnostics"],
        "{case}: diagnostics differ from the pinned native observation"
    );
    loaded
}

/// The first JSX opening element, self-closing element or opening fragment
/// in the root file, in source order.
pub fn first_jsx_tag(case: &Case) -> NodeId {
    use std::ops::ControlFlow;
    use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
    struct Walk<'a> {
        view: AstView<'a>,
        found: Option<NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if matches!(
                read.kind().known(),
                Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxOpeningFragment)
            ) {
                self.found = Some(node);
                return ControlFlow::Break(());
            }
            read.for_each_child(self)
        }
        fn visit_list(&mut self, list: tsr_ast::NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.view.list(list).unwrap().nodes())
        }
        fn visit_node_slice(&mut self, slice: tsr_ast::NodeSlice) -> ControlFlow<()> {
            for node in self.view.node_slice(slice).unwrap().iter().flatten() {
                self.visit_node(node)?;
            }
            ControlFlow::Continue(())
        }
    }
    let file = case
        .program
        .file(format!("/{}", case.root).as_bytes())
        .unwrap();
    let mut walk = Walk {
        view: file.bound().view().ast(),
        found: None,
    };
    let _ = walk.visit_node(file.source());
    walk.found.expect("a JSX tag in the root file")
}
