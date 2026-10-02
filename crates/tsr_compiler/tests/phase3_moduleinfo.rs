//! The module helpers of `moduletransforms` (`externalmoduleinfo.go`,
//! `utilities.go`) against the pin. They have no transformer of their own
//! yet, so `fixtures/phase3/moduleinfo/phase3_moduleinfo_test.go`, an overlay
//! test of the pinned package, parses and binds each case's `/main.ts`, runs
//! `collectExternalModuleInfo` and the helpers over it and records a
//! deterministic description (kinds, positions, names, flags, the export maps
//! in order). This suite loads the same file, runs the ported helpers with the
//! binder's reference resolver and requires the same description.
//!
//! Regenerate the fixture by running the overlay with
//! `PHASE3_MODULEINFO_REQUESTS` (the cases without their descriptions, as a
//! JSON array) and `PHASE3_MODULEINFO_OUTPUT` set:
//! `go test -overlay <map to transformers/moduletransforms/phase3_moduleinfo_test.go>
//! -run '^TestPhase3ModuleInfo$' ./internal/transformers/moduletransforms`
//! from `upstream/tsc`.
use serde_json::Value;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::ops::ControlFlow;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::{Counters, NodeId};
use tsr_ast::{
    AstBuilder, ChildVisitor, FactoryMethods, JsString, NodeListId, NodeSlice, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_binder::name_resolver::ResolverOptions;
use tsr_binder::reference_resolver::{
    NoReferenceResolverHooks, ReferenceResolver as BinderResolver,
};
use tsr_compiler::{FileCache, Program, ProgramOptions, ProgramResolverHost};
use tsr_core::{CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, Tristate};
use tsr_jsstring::SourceText;
use tsr_printer::script_resolver::{ReferenceResolver, ResolverResult};
use tsr_printer::{generated_identifier_flags as g, AutoGenerateOptions, EmitContext};
use tsr_transformers::moduletransforms::externalmoduleinfo::{
    collect_external_module_info, contains_default_reference,
    create_external_helpers_import_declaration_if_needed, get_export_needs_import_star_helper,
    get_import_needs_import_default_helper, get_import_needs_import_star_helper,
    get_imported_helpers, get_or_create_external_helpers_module_name_if_needed,
};
use tsr_transformers::moduletransforms::utilities::{
    create_empty_imports, get_external_module_name_from_path, get_external_module_name_literal,
    is_declaration_name_of_enum_or_namespace, is_file_level_reserved_generated_identifier,
    is_simple_inlineable_expression, rewrite_module_specifier,
};
use tsr_transformers::SharedReferenceResolver;

/// `binder.NewReferenceResolver(options, binder.ReferenceResolverHooks{})`.
struct BoundResolver<'a> {
    host: ProgramResolverHost<'a>,
    resolver: BinderResolver,
}

impl ReferenceResolver for BoundResolver<'_> {
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
        Ok(self.resolver.get_referenced_export_container(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
            prefix_locals,
        )?)
    }
    fn get_referenced_import_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        Ok(self.resolver.get_referenced_import_declaration(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>> {
        Ok(self.resolver.get_referenced_value_declaration(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>> {
        Ok(self.resolver.get_referenced_value_declarations(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_element_access_expression_name(
        &mut self,
        expression: NodeId,
    ) -> ResolverResult<JsString> {
        Ok(self
            .resolver
            .get_element_access_expression_name(&mut NoReferenceResolverHooks, Some(expression))?)
    }
    fn get_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        Ok(self.resolver.get_referenced_member_value_declaration(
            &self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
}

fn module_kind(name: &str) -> ModuleKind {
    match name {
        "none" => ModuleKind::NONE,
        "commonjs" => ModuleKind::COMMON_JS,
        "system" => ModuleKind::SYSTEM,
        "es2015" => ModuleKind::ES2015,
        "esnext" => ModuleKind::ESNEXT,
        "node16" => ModuleKind::NODE16,
        "preserve" => ModuleKind::PRESERVE,
        _ => panic!("unknown module kind {name}"),
    }
}

fn tristate(value: &Value) -> Tristate {
    if value.as_bool().expect("bool") {
        Tristate::TRUE
    } else {
        Tristate::UNKNOWN
    }
}

fn options(case: &Value) -> CompilerOptions {
    let mut options = CompilerOptions {
        module: module_kind(case["module"].as_str().expect("module")),
        import_helpers: tristate(&case["import_helpers"]),
        rewrite_relative_import_extensions: tristate(&case["rewrite_relative_import_extensions"]),
        no_lib: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    if case["jsx_preserve"].as_bool().expect("bool") {
        options.jsx = JsxEmit::PRESERVE;
    }
    if case["module_detection_force"].as_bool().expect("bool") {
        options.module_detection = ModuleDetectionKind::FORCE;
    }
    options
}

fn program(source: &str, options: CompilerOptions) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", source.as_bytes());
    Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                vec![JsString::from_bytes(&b"/main.ts"[..])],
            ),
            host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
            current_directory: JsString::from_bytes(&b"/"[..]),
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            skip_module_resolution: false,
            single_threaded: Tristate::UNKNOWN,
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .expect("program")
}

/// Go's `%q` for the texts of these cases.
fn quote(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).expect("UTF-8 text");
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => write!(out, "\\x{:02x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The immediate children of `node`, as `ForEachChild` visits them.
fn children(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    struct Collect<'f> {
        factory: &'f dyn RuntimeFactory,
        nodes: Vec<NodeId>,
    }
    impl ChildVisitor for Collect<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            self.nodes.push(node);
            ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.factory.read_list(list).nodes())
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
            self.nodes
                .extend(self.factory.read_nodes(nodes).iter().flatten());
            ControlFlow::Continue(())
        }
    }
    let mut collect = Collect {
        factory,
        nodes: Vec::new(),
    };
    let _ = factory.node(node).for_each_child(&mut collect);
    collect.nodes
}

fn text(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<u8> {
    factory
        .ast_view()
        .expect("builder view")
        .node_text(node)
        .expect("node text")
        .as_bytes()
        .to_vec()
}

struct Describer<'c> {
    context: &'c EmitContext,
}

impl Describer<'_> {
    fn node(&self, factory: &dyn RuntimeFactory, node: Option<NodeId>) -> String {
        let Some(node) = node else {
            return "nil".into();
        };
        if let Some(info) = self.context.auto_generate_info(node) {
            if let Some(source) = info.node {
                return format!("gen({}:{})", info.flags, self.node(factory, Some(source)));
            }
            return format!("gen({}:{})", info.flags, quote(&text(factory, node)));
        }
        let read = factory.node(node);
        let kind = read.kind();
        let text = match kind.known() {
            Some(
                K::Identifier
                | K::StringLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::NumericLiteral,
            ) => format!(":{}", quote(&text(factory, node))),
            _ => String::new(),
        };
        format!("{kind}@{}{text}", read.pos())
    }

    fn nodes(&self, factory: &dyn RuntimeFactory, nodes: &[NodeId]) -> String {
        let parts: Vec<String> = nodes
            .iter()
            .map(|&node| self.node(factory, Some(node)))
            .collect();
        format!("[{}]", parts.join(" "))
    }

    fn synthesized(&self, factory: &dyn RuntimeFactory, node: Option<NodeId>) -> String {
        let Some(node) = node else {
            return "nil".into();
        };
        let kind = factory.node(node).kind();
        let mut out = format!("{kind}{{flags={}", self.context.emit_flags(node));
        if self.context.has_auto_generate_info(node) {
            out.push(' ');
            out.push_str(&self.node(factory, Some(node)));
        } else if matches!(kind.known(), Some(K::Identifier | K::StringLiteral)) {
            out.push(' ');
            out.push_str(&quote(&text(factory, node)));
        }
        for child in children(factory, node) {
            out.push(' ');
            out.push_str(&self.synthesized(factory, Some(child)));
        }
        out.push('}');
        out
    }
}

#[allow(clippy::too_many_lines)]
fn describe(case: &Value) -> String {
    let options = options(case);
    let program = program(case["source"].as_str().expect("source"), options.clone());
    let source = program
        .source_file(b"/main.ts")
        .map(tsr_compiler::ProgramFile::source)
        .expect("main.ts");
    let counters = Counters::new();
    let mut context = EmitContext::new();
    let mut output =
        AstBuilder::with_hooks(SourceText::default(), &counters, context.factory_hooks());
    for file in program.files() {
        output.retain_completed(file.bound());
    }
    let resolver: SharedReferenceResolver<'_> = Rc::new(RefCell::new(BoundResolver {
        host: program.resolver_host(&counters),
        resolver: BinderResolver::new(ResolverOptions {
            emit_script_target: options.emit_script_target(),
            isolated_modules: options.isolated_modules(),
            verbatim_module_syntax: options.verbatim_module_syntax.is_true(),
            emit_standard_class_fields: options.emit_standard_class_fields(),
        }),
    }));
    let f: &mut dyn RuntimeFactory = &mut output;
    let described_context = context.clone();
    let describer = Describer {
        context: &described_context,
    };
    let d = &describer;
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };

    // collectExternalModuleInfo
    let info = collect_external_module_info(f, source, &options, &context, &resolver)
        .expect("collectExternalModuleInfo");
    line(format!(
        "externalImports {}",
        d.nodes(f, &info.external_imports)
    ));
    let mut specifier_keys: Vec<&JsString> = info.export_specifiers.keys().collect();
    specifier_keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for key in specifier_keys {
        line(format!(
            "exportSpecifiers {} {}",
            quote(key.as_bytes()),
            d.nodes(f, info.export_specifiers.get(key))
        ));
    }
    let mut bindings: Vec<(String, &[NodeId])> = info
        .exported_bindings
        .keys()
        .map(|key| (d.node(f, Some(*key)), info.exported_bindings.get(key)))
        .collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, values) in bindings {
        line(format!("exportedBindings {key} {}", d.nodes(f, values)));
    }
    line(format!(
        "exportedNames {}",
        d.nodes(f, &info.exported_names)
    ));
    let functions: Vec<NodeId> = info.exported_functions.values().copied().collect();
    line(format!("exportedFunctions {}", d.nodes(f, &functions)));
    line(format!("exportEquals {}", d.node(f, info.export_equals)));
    line(format!(
        "hasExportStarsToExportValues {}",
        info.has_export_stars_to_export_values
    ));

    // Per statement helpers.
    let statements = {
        let list = f.node(source).statement_list().expect("statements");
        let nodes = f.read_list(list).nodes();
        f.read_nodes(nodes).iter().flatten().collect::<Vec<_>>()
    };
    for statement in statements {
        let kind = f.node(statement).kind().known();
        match kind {
            Some(K::ImportDeclaration) => line(format!(
                "import {} star={} default={}",
                d.node(f, Some(statement)),
                get_import_needs_import_star_helper(f, statement).unwrap(),
                get_import_needs_import_default_helper(f, statement).unwrap()
            )),
            Some(K::ExportDeclaration) => {
                let clause = f
                    .node(statement)
                    .data_source()
                    .as_export_declaration()
                    .unwrap()
                    .export_clause();
                line(format!(
                    "export {} star={} containsDefault={}",
                    d.node(f, Some(statement)),
                    get_export_needs_import_star_helper(f, statement).unwrap(),
                    contains_default_reference(f, clause).unwrap()
                ));
            }
            _ => {}
        }
        match kind {
            Some(K::ImportDeclaration | K::ExportDeclaration | K::ImportEqualsDeclaration) => {
                let literal =
                    get_external_module_name_literal(f, statement, source, None, &options).unwrap();
                line(format!(
                    "moduleNameLiteral {} {}",
                    d.node(f, Some(statement)),
                    d.synthesized(f, literal)
                ));
                let specifier = tsr_ast::utilities_modules::get_external_module_name(
                    f.ast_view().unwrap(),
                    statement,
                )
                .unwrap();
                let rewritten = rewrite_module_specifier(&mut context, f, specifier, &options);
                line(format!(
                    "rewriteModuleSpecifier {} {} same={}",
                    d.node(f, specifier),
                    d.synthesized(f, rewritten),
                    rewritten == specifier
                ));
            }
            Some(K::ExpressionStatement) => {
                let expression = f.node(statement).expression().unwrap();
                line(format!(
                    "simpleInlineable {} {}",
                    d.node(f, Some(expression)),
                    is_simple_inlineable_expression(f, expression)
                ));
            }
            _ => {}
        }
    }

    // Every identifier: the declaration name of an enum or namespace?
    fn walk(context: &EmitContext, f: &dyn RuntimeFactory, node: NodeId, names: &mut Vec<NodeId>) {
        if f.node(node).kind() == K::Identifier
            && is_declaration_name_of_enum_or_namespace(context, f, node)
        {
            names.push(node);
        }
        for child in children(f, node) {
            walk(context, f, child, names);
        }
    }
    let mut names = Vec::new();
    for child in children(f, source) {
        walk(&context, f, child, &mut names);
    }
    line(format!("enumOrNamespaceNames {}", d.nodes(f, &names)));

    // Generated names.
    let mut reserved = Vec::new();
    for flags in [
        0,
        g::FILE_LEVEL,
        g::FILE_LEVEL | g::OPTIMISTIC,
        g::FILE_LEVEL | g::RESERVED_IN_NESTED_SCOPES,
        g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES,
        g::FILE_LEVEL | g::OPTIMISTIC | g::RESERVED_IN_NESTED_SCOPES,
    ] {
        let name = context.new_unique_name_ex(
            f,
            JsString::from_bytes(&b"n"[..]),
            AutoGenerateOptions {
                flags,
                ..AutoGenerateOptions::default()
            },
        );
        reserved.push(is_file_level_reserved_generated_identifier(&context, name).to_string());
    }
    let plain = f.new_identifier(JsString::from_bytes(&b"plain"[..]));
    reserved.push(is_file_level_reserved_generated_identifier(&context, plain).to_string());
    line(format!("fileLevelReserved [{}]", reserved.join(" ")));
    let empty = create_empty_imports(f);
    line(format!("emptyImports {}", d.synthesized(f, Some(empty))));
    line(format!(
        "externalModuleNameFromPath {}",
        quote(get_external_module_name_from_path(b"/a.ts", b"/b.ts").as_bytes())
    ));

    // The tslib import.
    for helper in case["helpers"].as_array().expect("helpers") {
        let expression = f.new_identifier(JsString::from_bytes(&b"e"[..]));
        match helper.as_str().expect("helper") {
            "importStar" => {
                context.new_import_star_helper(f, expression);
            }
            "importDefault" => {
                context.new_import_default_helper(f, expression);
            }
            "exportStar" => {
                let exports = f.new_identifier(JsString::from_bytes(&b"exports"[..]));
                context.new_export_star_helper(f, expression, exports);
            }
            "await" => {
                context.new_await_helper(f, expression);
            }
            "asyncSuper" => {
                context.add_emit_helper(source, &[&tsr_printer::emit_helpers::ASYNC_SUPER_HELPER]);
                continue;
            }
            other => panic!("unknown helper {other}"),
        }
        let helpers = context.read_emit_helpers();
        context.add_emit_helper(source, &helpers);
    }
    let imported: Vec<&str> = get_imported_helpers(&context, source)
        .iter()
        .map(|helper| std::str::from_utf8(helper.name).unwrap())
        .collect();
    line(format!("importedHelpers [{}]", imported.join(" ")));
    let file_module_kind = module_kind(case["file_module_kind"].as_str().expect("kind"));
    let flag = |name: &str| case[name].as_bool().expect("bool");
    let declaration = create_external_helpers_import_declaration_if_needed(
        &mut context,
        f,
        source,
        &options,
        file_module_kind,
        flag("has_export_stars"),
        flag("has_import_star"),
        flag("has_import_default"),
    )
    .expect("createExternalHelpersImportDeclarationIfNeeded");
    line(format!(
        "externalHelpersImport {}",
        d.synthesized(f, declaration)
    ));
    line(format!("sourceFileFlags {}", context.emit_flags(source)));
    let again = get_or_create_external_helpers_module_name_if_needed(
        &mut context,
        f,
        source,
        &options,
        &[],
        false,
        false,
        file_module_kind,
    );
    line(format!(
        "externalHelpersModuleName {}",
        d.synthesized(f, again)
    ));
    out
}

#[test]
fn module_helpers_describe_what_the_pinned_ones_describe() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/phase3/moduleinfo/moduleinfo.json");
    let document: Value =
        serde_json::from_slice(&std::fs::read(path).expect("fixture")).expect("fixture JSON");
    assert_eq!(document["version"], 1);
    let only = std::env::var("PHASE3_MODULEINFO_CASE").ok();
    let mut failures = Vec::new();
    let mut compared = 0;
    for case in document["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("id");
        if only.as_deref().is_some_and(|only| only != id) {
            continue;
        }
        let expected = case["description"]
            .as_str()
            .expect("the pin described every case");
        let actual = describe(case);
        compared += 1;
        if actual != expected {
            failures.push(format!("{id}:\n--- pinned\n{expected}--- ported\n{actual}"));
        }
    }
    assert!(
        compared >= 30 || only.is_some(),
        "too few cases: {compared}"
    );
    assert!(
        failures.is_empty(),
        "{} of {compared} cases differ:\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}
