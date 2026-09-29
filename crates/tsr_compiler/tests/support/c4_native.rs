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
    pub name: String,
    pub program: Arc<Program>,
    pub owner: Arc<CheckerOwner>,
    pub generation: Generation,
    pub root: String,
    pub native: Value,
}

fn string_value(flag: &str, value: Option<&str>) -> JsString {
    JsString::from_bytes(
        value
            .unwrap_or_else(|| panic!("{flag} needs a value"))
            .as_bytes(),
    )
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
            "--jsxFactory" => options.jsx_factory = string_value(flag, value),
            "--jsxFragmentFactory" => options.jsx_fragment_factory = string_value(flag, value),
            "--jsxImportSource" => options.jsx_import_source = string_value(flag, value),
            "--target" => assert_eq!(value, Some("esnext")),
            "--noEmit" => options.no_emit = tristate,
            "--ignoreConfig" | "--pretty" => {}
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
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let (generation, owner) = checker(&program);
    Case {
        name: case.to_string(),
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
    let line = prefix.split(|&byte| byte == b'\n').count();
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
    for file in files {
        diagnostics.extend(case.program.syntactic_diagnostics(Some(file)).unwrap());
    }
    if diagnostics.is_empty() {
        let mut op = owner.operation().unwrap();
        diagnostics = op.global_diagnostics().unwrap();
        if diagnostics.is_empty() {
            for file in files {
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
    find_first_jsx_tag(case).expect("a JSX tag in the root file")
}

fn find_first_jsx_tag(case: &Case) -> Option<NodeId> {
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
    walk.found
}

macro_rules! fixture_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../fixtures/c4/", $name)))),*]
    };
}

macro_rules! case_records {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../fixtures/c4/", $name, ".native.json")))),*]
    };
}

/// Every file a case of `fixtures/c4/fixtures.json` names.
pub const FILES: &[(&str, &str)] = fixture_files![
    "jsx_modes.tsx",
    "node_modules/react/package.json",
    "node_modules/react/index.d.ts",
    "node_modules/react/jsx-runtime.d.ts",
    "jsx_pragmas.tsx",
    "jsx_elements.tsx",
    "jsx_generics.tsx",
    "jsx_in_js.jsx",
    "decorator_positions.ts",
    "decorator_static_block.ts",
    "decorator_contexts.ts",
    "metadata_marking.ts",
    "metadata_types.ts",
    "lifecycle.tsx",
    "jsx_deep_pragma.tsx",
    "jsx_import_source.tsx",
    "node_modules/preact/package.json",
    "node_modules/preact/index.d.ts",
    "node_modules/preact/jsx-runtime.d.ts",
    "node_modules/preact/jsx-dev-runtime.d.ts",
    "jsx_import_source_absent.tsx",
    "node_modules/bare/package.json",
    "node_modules/bare/index.d.ts",
    "jsx_import_source_option.tsx",
    "jsx_factory_options.tsx",
    "jsx_pragma_without_frag.tsx",
    "jsx_no_namespace.tsx",
    "jsx_runtime_classic.tsx",
    "jsx_runtime_automatic.tsx",
    "conditional_distribution.ts",
    "relation_excess.ts",
];

/// Every case's native diagnostics record.
pub const RECORDS: &[(&str, &str)] = case_records![
    "jsx_modes_preserve",
    "jsx_pragmas_preserve",
    "jsx_modes_react_native",
    "jsx_pragmas_react_native",
    "jsx_modes_react",
    "jsx_pragmas_react",
    "jsx_modes_react_jsx",
    "jsx_pragmas_react_jsx",
    "jsx_modes_react_jsxdev",
    "jsx_pragmas_react_jsxdev",
    "jsx_elements",
    "jsx_generics",
    "jsx_in_js",
    "decorator_positions_es",
    "decorator_positions_legacy",
    "decorator_static_block_es",
    "decorator_static_block_legacy",
    "decorator_contexts",
    "metadata_marking",
    "metadata_isolated",
    "lifecycle",
    "jsx_deep_pragma",
    "jsx_import_source_preserve",
    "jsx_import_source_absent_preserve",
    "jsx_import_source_react_native",
    "jsx_import_source_absent_react_native",
    "jsx_import_source_react",
    "jsx_import_source_absent_react",
    "jsx_import_source_react_jsx",
    "jsx_import_source_absent_react_jsx",
    "jsx_import_source_react_jsxdev",
    "jsx_import_source_absent_react_jsxdev",
    "jsx_import_source_option_react_jsx",
    "jsx_import_source_option_react_jsxdev",
    "jsx_factory_option",
    "jsx_factory_options",
    "jsx_pragmas_over_options",
    "jsx_pragma_without_frag",
    "jsx_no_flag",
    "jsx_react_missing",
    "jsx_runtime_classic_react_jsx",
    "jsx_runtime_classic_react_jsxdev",
    "jsx_runtime_automatic_react",
    "jsx_runtime_automatic_preserve",
    "metadata_plain",
    "conditional_distribution",
    "relation_excess",
];

pub const MANIFEST: &str = include_str!("../fixtures/c4/fixtures.json");

/// The case's root and bound files, root first, as the manifest lists them.
pub fn case_files(case: &str) -> Vec<(&'static str, &'static str)> {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let spec = &manifest[case];
    let root = spec["root"]
        .as_str()
        .unwrap_or_else(|| panic!("{case} is not in the manifest"));
    let mut names = vec![root.to_string()];
    for name in spec["files"].as_array().into_iter().flatten() {
        names.push(name.as_str().unwrap().to_string());
    }
    names
        .iter()
        .map(|name| {
            *FILES
                .iter()
                .find(|(file, _)| file == name)
                .unwrap_or_else(|| panic!("{case}: {name} is not in the fixture table"))
        })
        .collect()
}

/// Loads a manifest case by name.
pub fn named(case: &str) -> Case {
    let record = RECORDS
        .iter()
        .find(|(name, _)| *name == case)
        .unwrap_or_else(|| panic!("{case} has no native record in the table"))
        .1;
    load(case, record, &case_files(case))
}

/// Loads a manifest case by name and asserts its diagnostics equal the record.
pub fn assert_named(case: &str) -> Case {
    let loaded = named(case);
    assert_eq!(
        json!(observed(&loaded)),
        loaded.native["diagnostics"],
        "{case}: diagnostics differ from the pinned native observation"
    );
    loaded
}

/// The pinned checker's state for `case` (`fixtures/c4/state`, recorded by
/// its `regenerate.py` through a Go overlay of the pinned checker), after
/// checking the bound flags and sources the case loads.
pub fn recorded_state(case: &Case) -> Value {
    let record: Value =
        serde_json::from_str(include_str!("../fixtures/c4/state/native.json")).unwrap();
    let provenance: Value =
        serde_json::from_str(include_str!("../fixtures/c4/state/provenance.json")).unwrap();
    assert_eq!(provenance["pin"], case.native["pin"], "state record pin");
    assert_eq!(
        provenance["output_sha256"],
        json!(format!(
            "{:x}",
            Sha256::digest(include_bytes!("../fixtures/c4/state/native.json"))
        )),
        "state record digest"
    );
    let row = record["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(case.name))
        .unwrap_or_else(|| panic!("{}: no recorded state", case.name))
        .clone();
    let command: Vec<&Value> = case.native["native_command"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(
        row["flags"], manifest[&case.name]["flags"],
        "{}: state flags",
        case.name
    );
    for flag in row["flags"].as_array().unwrap() {
        assert!(
            command.contains(&flag),
            "{}: {flag} is not in the native command",
            case.name
        );
    }
    assert_eq!(
        row["sources_sha256"], case.native["sources_sha256"],
        "{}: state sources",
        case.name
    );
    row
}

/// The JSX entities `owner` reads at the case's first JSX tag, in the form
/// the state record keeps them: a name longer than 200 bytes by its length
/// and digest.
pub fn jsx_state(case: &Case, owner: &Arc<CheckerOwner>) -> Value {
    let tag = first_jsx_tag(case);
    let mut state = owner.operation().unwrap().jsx_link_state(tag).unwrap();
    for key in ["factory", "fragment_factory"] {
        if let Some(text) = state[key].as_str().filter(|text| text.len() > 200) {
            state[key] = json!({"bytes": text.len(), "sha256": format!("{:x}", Sha256::digest(text.as_bytes()))});
        }
    }
    state
}

/// The case's checker state equals the pinned checker's after the same
/// checks: the JSX entities at the first JSX tag and the referenced state of
/// the root's import specifiers.
pub fn assert_state(case: &Case) {
    let row = recorded_state(case);
    let expected = &row["checked"];
    if expected["jsx"].is_null() {
        assert_eq!(
            find_first_jsx_tag(case),
            None,
            "{}: the pin found no JSX tag",
            case.name
        );
    } else {
        assert_eq!(
            jsx_state(case, &case.owner),
            expected["jsx"],
            "{}: JSX state",
            case.name
        );
    }
    for (name, referenced) in expected["aliases"].as_object().unwrap() {
        let specifier = import_specifier(case, name);
        let mut op = case.owner.operation().unwrap();
        assert_eq!(
            op.alias_link_state(specifier).unwrap()["referenced"],
            *referenced,
            "{}: {name} referenced",
            case.name
        );
    }
}

/// A fresh checker over the case's program, asked before anything is
/// checked, reads the pinned checker's unchecked JSX state.
pub fn assert_unchecked_state(case: &Case) {
    let row = recorded_state(case);
    let (_generation, fresh) = checker(&case.program);
    if !row["unchecked"]["jsx"].is_null() {
        assert_eq!(
            jsx_state(case, &fresh),
            row["unchecked"]["jsx"],
            "{}: unchecked JSX state",
            case.name
        );
    }
}

/// The import specifier of the root file that imports `name`.
pub fn import_specifier(case: &Case, name: &str) -> NodeId {
    use std::ops::ControlFlow;
    use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
    struct Walk<'a> {
        view: AstView<'a>,
        name: &'a str,
        found: Option<NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if read.kind() == K::ImportSpecifier
                && self
                    .view
                    .node_text(read.name().unwrap())
                    .unwrap()
                    .as_bytes()
                    == self.name.as_bytes()
            {
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
        name,
        found: None,
    };
    let _ = walk.visit_node(file.source());
    walk.found.expect("import specifier")
}
