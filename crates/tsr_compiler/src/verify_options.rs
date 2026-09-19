//! Program option verification. Diagnostics retain configuration syntax identity;
//! these checks are independent of the later checker and emitter execution.
use crate::{Error, Program};
use std::sync::Arc;
use ts_ast::{Diagnostic, NodeId};
use ts_core::{
    CompilerOptions, JsxEmit, LanguageVariant, ModuleKind, ModuleResolutionKind, ScriptTarget,
};
use ts_diagnostics::{self as d, Message};
use ts_jsstring::{JsString, SourceText};
use ts_tsoptions::TsConfigSourceFile;

struct Verifier<'a> {
    config: Option<&'a TsConfigSourceFile>,
    compiler_options: Option<NodeId>,
    diagnostics: Vec<Diagnostic>,
}
impl<'a> Verifier<'a> {
    fn new(config: Option<&'a TsConfigSourceFile>) -> Self {
        Self {
            compiler_options: config
                .and_then(|config| ts_tsoptions::find_property(config, &[b"compilerOptions"])),
            config,
            diagnostics: Vec::new(),
        }
    }
    fn at(
        &self,
        node: Option<NodeId>,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Diagnostic {
        match (self.config, node) {
            (Some(config), Some(node)) => {
                ts_tsoptions::diagnostic_for_node(config, node, message, args)
            }
            _ => Diagnostic::compiler(message, args),
        }
    }
    fn initializer(&self, node: Option<NodeId>) -> Option<NodeId> {
        let node = node?;
        self.config?
            .file
            .view()
            .node(node)
            .expect("retained config property")
            .initializer()
    }
    fn name(&self, node: Option<NodeId>) -> Option<NodeId> {
        let node = node?;
        self.config?
            .file
            .view()
            .node(node)
            .expect("retained config property")
            .name()
    }
    fn property(&self, object: Option<NodeId>, key1: &[u8], key2: &[u8]) -> Option<NodeId> {
        ts_tsoptions::find_property_in_object(self.config?, object?, &[key1, key2])
    }
    fn compiler(&mut self, message: &'static Message, args: Vec<JsString>) -> &mut Diagnostic {
        let diagnostic = self.at(self.name(self.compiler_options), message, args);
        self.diagnostics.push(diagnostic);
        self.diagnostics.last_mut().expect("just pushed diagnostic")
    }
    fn option(
        &mut self,
        on_key: bool,
        key1: &[u8],
        key2: &[u8],
        message: &'static Message,
        args: Vec<JsString>,
    ) -> &mut Diagnostic {
        let property = self.property(self.initializer(self.compiler_options), key1, key2);
        let node = if on_key {
            self.name(property)
        } else {
            self.initializer(property)
        };
        if node.is_none() {
            return self.compiler(message, args);
        }
        let diagnostic = self.at(node, message, args);
        self.diagnostics.push(diagnostic);
        self.diagnostics.last_mut().expect("just pushed diagnostic")
    }
    fn option_name(&mut self, message: &'static Message, key1: &[u8], key2: &[u8], args: &[&[u8]]) {
        let arguments = [key1, key2]
            .into_iter()
            .chain(args.iter().copied())
            .map(JsString::from_bytes)
            .collect();
        self.option(true, key1, key2, message, arguments);
    }
    fn option_value(&mut self, key: &[u8], message: &'static Message, args: &[&[u8]]) {
        self.option(false, key, b"", message, strings(args));
    }
    fn removed(&mut self, key: &[u8], value: &[u8], suggestion: &[u8]) {
        let (message, args) = if value.is_empty() {
            (
                d::Option_0_has_been_removed_Please_remove_it_from_your_configuration,
                vec![JsString::from_bytes(key)],
            )
        } else {
            (
                d::Option_0_1_has_been_removed_Please_remove_it_from_your_configuration,
                strings(&[key, value]),
            )
        };
        let diagnostic = self.option(value.is_empty(), key, b"", message, args);
        if !suggestion.is_empty() {
            diagnostic.message_chain.push(Arc::new(Diagnostic::compiler(
                d::Use_0_instead,
                vec![JsString::from_bytes(suggestion)],
            )));
        }
    }
    fn path(
        &mut self,
        on_key: bool,
        key: &[u8],
        index: Option<usize>,
        message: &'static Message,
        args: &[&[u8]],
    ) {
        let paths = self.property(self.initializer(self.compiler_options), b"paths", b"");
        let property = self.property(self.initializer(paths), key, b"");
        let mut node = if on_key {
            self.name(property)
        } else {
            self.initializer(property)
        };
        if let Some(index) = index {
            node = node.and_then(|node| {
                let config = self.config?;
                let view = config.file.view();
                let read = view.node(node).expect("retained path substitution");
                let ts_ast::NodeDataRead::ArrayLiteralExpression(data) = read.data() else {
                    return None;
                };
                let list = view
                    .list(data.elements()?)
                    .expect("retained substitution elements");
                view.node_slice(list.nodes())
                    .expect("retained substitution values")
                    .get(index)
                    .flatten()
            });
        }
        if let Some(node) = node {
            self.diagnostics
                .push(self.at(Some(node), message, strings(args)));
        } else {
            self.compiler(message, strings(args));
        }
    }
}
fn strings(args: &[&[u8]]) -> Vec<JsString> {
    args.iter().map(|arg| JsString::from_bytes(*arg)).collect()
}

/// port: tsc/internal/compiler/program.go:hasZeroOrOneAsteriskCharacter
fn has_zero_or_one_asterisk(value: &[u8]) -> bool {
    value.iter().filter(|&&byte| byte == b'*').take(2).count() < 2
}
/// port: tsc/internal/compiler/program.go:moduleResolutionSupportsPackageJsonExportsAndImports
fn supports_package_maps(value: ModuleResolutionKind) -> bool {
    (ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT).contains(&value)
        || value == ModuleResolutionKind::BUNDLER
}
/// port: tsc/internal/compiler/program.go:emitModuleKindIsNonNodeESM
fn non_node_esm(value: ModuleKind) -> bool {
    (ModuleKind::ES2015..=ModuleKind::ESNEXT).contains(&value)
}

fn removed_options(v: &mut Verifier<'_>, options: &CompilerOptions, base_url_suggestion: &[u8]) {
    if !options.base_url.is_empty() {
        v.removed(b"baseUrl", b"", base_url_suggestion);
    }
    if !options.out_file.is_empty() {
        v.removed(b"outFile", b"", b"");
    }
    if options.target == ScriptTarget::ES5 {
        v.removed(b"target", b"ES5", b"");
    }
    for (kind, name) in [
        (ModuleKind::AMD, b"AMD".as_slice()),
        (ModuleKind::SYSTEM, b"System"),
        (ModuleKind::UMD, b"UMD"),
    ] {
        if options.module == kind {
            v.removed(b"module", name, b"");
        }
    }
    if options.module_resolution == ModuleResolutionKind::CLASSIC {
        v.removed(b"moduleResolution", b"Classic", b"");
    }
    if options.always_strict.is_false() {
        v.removed(b"alwaysStrict", b"false", b"");
    }
    if options.es_module_interop.is_false() {
        v.removed(b"esModuleInterop", b"false", b"");
    }
    if options.allow_synthetic_default_imports.is_false() {
        v.removed(b"allowSyntheticDefaultImports", b"false", b"");
    }
    if options.module_resolution == ModuleResolutionKind::NODE10 {
        v.removed(b"moduleResolution", b"node10", b"");
    }
    if !options.downlevel_iteration.is_unknown() {
        v.removed(b"downlevelIteration", b"", b"");
    }
}

fn initial_options(v: &mut Verifier<'_>, options: &CompilerOptions) {
    if options.strict_property_initialization.is_true()
        && !options.strict_option_value(options.strict_null_checks)
    {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1,
            b"strictPropertyInitialization",
            b"strictNullChecks",
            &[],
        );
    }
    if options.exact_optional_property_types.is_true()
        && !options.strict_option_value(options.strict_null_checks)
    {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1,
            b"exactOptionalPropertyTypes",
            b"strictNullChecks",
            &[],
        );
    }
    if options.isolated_declarations.is_true() {
        if options.allow_js() {
            v.option_name(
                d::Option_0_cannot_be_specified_with_option_1,
                b"allowJs",
                b"isolatedDeclarations",
                &[],
            );
        }
        if !options.emit_declarations() {
            v.option_name(
                d::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                b"isolatedDeclarations",
                b"declaration",
                &[b"composite"],
            );
        }
    }
    if options.inline_source_map.is_true() {
        if options.source_map.is_true() {
            v.option_name(
                d::Option_0_cannot_be_specified_with_option_1,
                b"sourceMap",
                b"inlineSourceMap",
                &[],
            );
        }
        if !options.map_root.is_empty() {
            v.option_name(
                d::Option_0_cannot_be_specified_with_option_1,
                b"mapRoot",
                b"inlineSourceMap",
                &[],
            );
        }
    }
    if options.composite.is_true() {
        if options.declaration.is_false() {
            v.option_name(
                d::Composite_projects_may_not_disable_declaration_emit,
                b"declaration",
                b"",
                &[],
            );
        }
        if options.incremental.is_false() {
            v.option_name(
                d::Composite_projects_may_not_disable_incremental_compilation,
                b"declaration",
                b"",
                &[],
            );
        }
    }
    if options.ts_build_info_file.is_empty()
        && options.incremental.is_true()
        && options.config_file_path.is_empty()
    {
        v.compiler(d::Option_incremental_is_only_valid_with_a_known_configuration_file_like_tsconfig_json_or_when_tsBuildInfoFile_is_explicitly_provided, vec![]);
    }
}

fn path_and_emit_options(v: &mut Verifier<'_>, options: &CompilerOptions) {
    for (key, values) in options.paths.iter().flatten() {
        let key = key.as_bytes();
        if !has_zero_or_one_asterisk(key) {
            v.path(
                true,
                key,
                None,
                d::Pattern_0_can_have_at_most_one_Asterisk_character,
                &[key],
            );
        }
        if values.is_none() {
            v.path(
                false,
                key,
                None,
                d::Substitutions_for_pattern_0_should_be_an_array,
                &[key],
            );
        } else if values.as_ref().is_some_and(Vec::is_empty) {
            v.path(
                false,
                key,
                None,
                d::Substitutions_for_pattern_0_shouldn_t_be_an_empty_array,
                &[key],
            );
        }
        for (index, value) in values.iter().flatten().enumerate() {
            let value = value.as_bytes();
            if !has_zero_or_one_asterisk(value) {
                v.path(
                    false,
                    key,
                    Some(index),
                    d::Substitution_0_in_pattern_1_can_have_at_most_one_Asterisk_character,
                    &[value, key],
                );
            }
            if !ts_tspath::is_relative(value) && ts_tspath::root_length(value) == 0 {
                v.path(
                    false,
                    key,
                    Some(index),
                    d::Non_relative_paths_are_not_allowed_Did_you_forget_a_leading_Slash,
                    &[],
                );
            }
        }
    }
    if options.source_map.is_false_or_unknown() && options.inline_source_map.is_false_or_unknown() {
        if options.inline_sources.is_true() {
            v.option_name(d::Option_0_can_only_be_used_when_either_option_inlineSourceMap_or_option_sourceMap_is_provided, b"inlineSources", b"", &[]);
        }
        if !options.source_root.is_empty() {
            v.option_name(d::Option_0_can_only_be_used_when_either_option_inlineSourceMap_or_option_sourceMap_is_provided, b"sourceRoot", b"", &[]);
        }
    }
    if !(options.map_root.is_empty()
        || options.source_map.is_true()
        || options.declaration_map.is_true())
    {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
            b"mapRoot",
            b"sourceMap",
            &[b"declarationMap"],
        );
    }
    if !options.declaration_dir.is_empty() && !options.emit_declarations() {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
            b"declarationDir",
            b"declaration",
            &[b"composite"],
        );
    }
    if options.declaration_map.is_true() && !options.emit_declarations() {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
            b"declarationMap",
            b"declaration",
            &[b"composite"],
        );
    }
    if options.lib.is_some() && options.no_lib.is_true() {
        v.option_name(
            d::Option_0_cannot_be_specified_with_option_1,
            b"lib",
            b"noLib",
            &[],
        );
    }
    if (options.isolated_modules.is_true() || options.verbatim_module_syntax.is_true())
        && options.preserve_const_enums.is_false()
    {
        let option = if options.verbatim_module_syntax.is_true() {
            b"verbatimModuleSyntax".as_slice()
        } else {
            b"isolatedModules"
        };
        v.option_name(
            d::Option_preserveConstEnums_cannot_be_disabled_when_0_is_enabled,
            option,
            b"preserveConstEnums",
            &[],
        );
    }
}

fn final_options(v: &mut Verifier<'_>, options: &CompilerOptions) {
    if options.check_js.is_true() && !options.allow_js() {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1,
            b"checkJs",
            b"allowJs",
            &[],
        );
    }
    if options.emit_declaration_only.is_true() && !options.emit_declarations() {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
            b"emitDeclarationOnly",
            b"declaration",
            &[b"composite"],
        );
    }
    if options.emit_decorator_metadata.is_true()
        && options.experimental_decorators.is_false_or_unknown()
    {
        v.option_name(
            d::Option_0_cannot_be_specified_without_specifying_option_1,
            b"emitDecoratorMetadata",
            b"experimentalDecorators",
            &[],
        );
    }
    let automatic_jsx = options.jsx == JsxEmit::REACT_JSX || options.jsx == JsxEmit::REACT_JSX_DEV;
    let jsx_name = || jsx_name(options.jsx);
    if !options.jsx_factory.is_empty() {
        if !options.react_namespace.is_empty() {
            v.option_name(
                d::Option_0_cannot_be_specified_with_option_1,
                b"reactNamespace",
                b"jsxFactory",
                &[],
            );
        }
        if automatic_jsx {
            v.option_name(
                d::Option_0_cannot_be_specified_when_option_jsx_is_1,
                b"jsxFactory",
                jsx_name(),
                &[],
            );
        }
        if ts_parser::parse_isolated_entity_name(SourceText::from_loaded_bytes(
            options.jsx_factory.as_bytes(),
        ))
        .is_none()
        {
            v.option_value(
                b"jsxFactory",
                d::Invalid_value_for_jsxFactory_0_is_not_a_valid_identifier_or_qualified_name,
                &[options.jsx_factory.as_bytes()],
            );
        }
    } else if !options.react_namespace.is_empty()
        && !ts_scanner::is_identifier_text(
            options.react_namespace.as_bytes(),
            LanguageVariant::STANDARD,
        )
    {
        v.option_value(
            b"reactNamespace",
            d::Invalid_value_for_reactNamespace_0_is_not_a_valid_identifier,
            &[options.react_namespace.as_bytes()],
        );
    }
    if !options.jsx_fragment_factory.is_empty() {
        if options.jsx_factory.is_empty() {
            v.option_name(
                d::Option_0_cannot_be_specified_without_specifying_option_1,
                b"jsxFragmentFactory",
                b"jsxFactory",
                &[],
            );
        }
        if automatic_jsx {
            v.option_name(
                d::Option_0_cannot_be_specified_when_option_jsx_is_1,
                b"jsxFragmentFactory",
                jsx_name(),
                &[],
            );
        }
        if ts_parser::parse_isolated_entity_name(SourceText::from_loaded_bytes(
            options.jsx_fragment_factory.as_bytes(),
        ))
        .is_none()
        {
            v.option_value(b"jsxFragmentFactory", d::Invalid_value_for_jsxFragmentFactory_0_is_not_a_valid_identifier_or_qualified_name, &[options.jsx_fragment_factory.as_bytes()]);
        }
    }
    if !options.react_namespace.is_empty() && automatic_jsx {
        v.option_name(
            d::Option_0_cannot_be_specified_when_option_jsx_is_1,
            b"reactNamespace",
            jsx_name(),
            &[],
        );
    }
    if !options.jsx_import_source.is_empty() && options.jsx == JsxEmit::REACT {
        v.option_name(
            d::Option_0_cannot_be_specified_when_option_jsx_is_1,
            b"jsxImportSource",
            jsx_name(),
            &[],
        );
    }
    if options.allow_importing_ts_extensions.is_true()
        && !(options.no_emit.is_true()
            || options.emit_declaration_only.is_true()
            || options.rewrite_relative_import_extensions.is_true())
    {
        v.option_value(b"allowImportingTsExtensions", d::Option_allowImportingTsExtensions_can_only_be_used_when_one_of_noEmit_emitDeclarationOnly_or_rewriteRelativeImportExtensions_is_set, &[]);
    }
    let resolution = options.module_resolution_kind();
    if !supports_package_maps(resolution) {
        for (enabled, name) in [
            (
                options.resolve_package_json_exports.is_true(),
                b"resolvePackageJsonExports".as_slice(),
            ),
            (
                options.resolve_package_json_imports.is_true(),
                b"resolvePackageJsonImports",
            ),
            (options.custom_conditions.is_some(), b"customConditions"),
        ] {
            if enabled {
                v.option_name(d::Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler, name, b"", &[]);
            }
        }
    }
    let module = options.emit_module_kind();
    if resolution == ModuleResolutionKind::BUNDLER
        && !non_node_esm(module)
        && module != ModuleKind::PRESERVE
        && module != ModuleKind::COMMON_JS
    {
        v.option_value(
            b"moduleResolution",
            d::Option_0_can_only_be_used_when_module_is_set_to_preserve_commonjs_or_es2015_or_later,
            &[b"bundler"],
        );
    }
    if (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module)
        && !(ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT).contains(&resolution)
    {
        let required = if module == ModuleKind::NODE_NEXT {
            b"NodeNext".as_slice()
        } else {
            b"Node16"
        };
        v.option_value(b"moduleResolution", d::Option_moduleResolution_must_be_set_to_0_or_left_unspecified_when_option_module_is_set_to_1, &[required, module_name(module).as_bytes()]);
    } else if (ModuleResolutionKind::NODE16..=ModuleResolutionKind::NODE_NEXT).contains(&resolution)
        && !(ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module)
    {
        let required = resolution_name(resolution);
        v.option_value(
            b"module",
            d::Option_module_must_be_set_to_0_when_option_moduleResolution_is_set_to_1,
            &[required, required],
        );
    }
}
fn jsx_name(value: JsxEmit) -> &'static [u8] {
    match value {
        JsxEmit::REACT_JSX => b"react-jsx",
        JsxEmit::REACT_JSX_DEV => b"react-jsxdev",
        JsxEmit::REACT => b"react",
        _ => b"",
    }
}
fn module_name(value: ModuleKind) -> JsString {
    let name = match value {
        ModuleKind::NODE16 => b"Node16".as_slice(),
        ModuleKind::NODE18 => b"Node18",
        ModuleKind::NODE20 => b"Node20",
        ModuleKind::NODE_NEXT => b"NodeNext",
        _ => return JsString::from_bytes(format!("ModuleKind({})", value.0).into_bytes()),
    };
    JsString::from_bytes(name)
}
fn resolution_name(value: ModuleResolutionKind) -> &'static [u8] {
    match value {
        ModuleResolutionKind::NODE16 => b"Node16",
        ModuleResolutionKind::NODE_NEXT => b"NodeNext",
        _ => panic!("unhandled case in ModuleResolutionKind.String"),
    }
}

/// A source include explanation requested by option verification. The owning
/// Program's include processor supplies the existing include-reason chain.
#[derive(Clone, Debug)]
pub struct FileIncludeDiagnostic {
    pub file: JsString,
    pub message: &'static Message,
    pub args: Vec<JsString>,
}
#[derive(Clone, Debug)]
pub struct OptionVerification {
    pub diagnostics: Vec<Diagnostic>,
    pub include_diagnostics: Vec<FileIncludeDiagnostic>,
    pub blocked_output_paths: std::collections::BTreeSet<JsString>,
}

/// Port the source's verification writes: direct diagnostics, include-explanation
/// requests, and blocked output paths. This does not perform checker or emit work.
/// port: tsc/internal/compiler/program.go:Program.verifyCompilerOptions
pub fn verify_compiler_options(program: &Program) -> Result<OptionVerification, Error> {
    use crate::output_paths as output;
    use ts_tspath as path;
    let options = program.options();
    let config = program.config().config_file.as_deref();
    let config_path = config
        .map(|config| {
            config
                .file
                .view()
                .source_file(config.root)
                .expect("retained config source")
                .parse_options()
                .file_name
                .clone()
        })
        .unwrap_or_default();
    let mut v = Verifier::new(config);
    let suggestion = if !options.base_url.is_empty() && !config_path.is_empty() {
        let mut relative = path::relative_from_file(
            config_path.as_bytes(),
            options.base_url.as_bytes(),
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        );
        if !(relative.starts_with(b"./") || relative.starts_with(b"../")) {
            relative = [b"./".as_slice(), &relative].concat();
        }
        let pattern = path::combine(&relative, &[b"*"]);
        [
            br#""paths": {"*": ["#.as_slice(),
            &json_string(&pattern),
            b"]}",
        ]
        .concat()
    } else {
        Vec::new()
    };
    removed_options(&mut v, options, &suggestion);
    initial_options(&mut v, options);
    // The frozen S07 operation boundary rejects project references before
    // Program construction. No reference verification result is manufactured.
    let emitted: Vec<_> = program
        .files()
        .iter()
        .map(|file| output::may_emit(file, program).map(|emit| (file, emit)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(file, emit)| emit.then_some(file))
        .collect();
    let names: Vec<_> = emitted
        .iter()
        .map(|file| {
            file.bound()
                .view()
                .source_file()
                .map(|source| source.parse_options().file_name.clone())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let common = output::common_directory(program, &names);
    let mut includes = Vec::new();
    let to_path = |name: &[u8]| {
        path::to_path(
            name,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        )
    };
    if options.composite.is_true() {
        let roots: std::collections::BTreeSet<_> = program
            .config()
            .root_file_names
            .iter()
            .map(|name| to_path(name.as_bytes()))
            .collect();
        for file in &emitted {
            let source = file.bound().view().source_file()?;
            if !roots.contains(&source.parse_options().path) {
                includes.push(FileIncludeDiagnostic {file: source.parse_options().path.clone(), message: d::File_0_is_not_listed_within_the_file_list_of_project_1_Projects_must_list_all_files_or_use_an_include_pattern, args: vec![source.parse_options().file_name.clone(),config_path.clone()]});
            }
        }
    }
    path_and_emit_options(&mut v, options);
    let needs_common = !options.out_dir.is_empty()
        || !options.root_dir.is_empty()
        || !options.source_root.is_empty()
        || !options.map_root.is_empty()
        || options.emit_declarations() && !options.declaration_dir.is_empty();
    // Source CommonSourceDirectory is memoized; membership writes happen only
    // at its first actual use, even when later output checks ask again.
    let mut common_checked = false;
    let mut check_common = |includes: &mut Vec<FileIncludeDiagnostic>| {
        if common_checked {
            return;
        }
        common_checked = true;
        let root = if !options.root_dir.is_empty() {
            options.root_dir.as_bytes().to_vec()
        } else if !options.config_file_path.is_empty() {
            path::directory(options.config_file_path.as_bytes())
        } else {
            return;
        };
        for name in &names {
            if !path::contains_path(
                &root,
                name.as_bytes(),
                program.current_directory(),
                program.host().use_case_sensitive_file_names(),
            ) {
                includes.push(FileIncludeDiagnostic {file: to_path(name.as_bytes()), message: d::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files, args: vec![name.clone(),JsString::from_bytes(root.as_slice())]});
            }
        }
    };
    if needs_common {
        check_common(&mut includes);
        if !options.out_dir.is_empty()
            && common.is_empty()
            && program.files().iter().any(|file| {
                file.bound().view().source_file().is_ok_and(|source| {
                    path::root_length(source.parse_options().file_name.as_bytes()) > 1
                })
            })
        {
            v.option_name(
                d::Cannot_find_the_common_subdirectory_path_for_the_input_files,
                b"outDir",
                b"",
                &[],
            );
        }
    }
    if !options.no_emit.is_true()
        && !options.composite.is_true()
        && options.root_dir.is_empty()
        && !options.config_file_path.is_empty()
        && (!options.out_dir.is_empty()
            || options.emit_declarations() && !options.declaration_dir.is_empty()
            || !options.out_file.is_empty())
    {
        check_common(&mut includes);
        let previous = output::computed_common_directory(&names, program);
        if !previous.is_empty()
            && path::canonical(&previous, program.host().use_case_sensitive_file_names())
                != path::canonical(&common, program.host().use_case_sensitive_file_names())
        {
            let key1: &[u8] = if !options.out_file.is_empty() {
                b"outFile".as_slice()
            } else if !options.out_dir.is_empty() {
                b"outDir"
            } else {
                b"declarationDir"
            };
            let key2 = if options.out_file.is_empty() && !options.out_dir.is_empty() {
                b"declarationDir".as_slice()
            } else {
                b""
            };
            let relative = path::relative_from_file(
                options.config_file_path.as_bytes(),
                &previous,
                program.current_directory(),
                program.host().use_case_sensitive_file_names(),
            );
            let diagnostic = v.option(true,key1,key2,d::The_common_source_directory_of_0_is_1_The_rootDir_setting_must_be_explicitly_set_to_this_or_another_path_to_adjust_your_output_s_file_layout,strings(&[path::base_name(options.config_file_path.as_bytes()),&relative]));
            diagnostic.message_chain.push(Arc::new(Diagnostic::compiler(
                d::Visit_https_Colon_Slash_Slashaka_ms_Slashts6_for_migration_information,
                vec![],
            )));
        }
    }
    final_options(&mut v, options);
    let mut blocked = std::collections::BTreeSet::new();
    if !options.no_emit.is_true() && !options.suppress_output_path_check.is_true() {
        let mut seen = std::collections::BTreeSet::new();
        let mut verify_path = |name: Vec<u8>| {
            if name.is_empty() {
                return;
            }
            let canonical = to_path(&name);
            if program.file(canonical.as_bytes()).is_some() {
                let mut diagnostic = Diagnostic::compiler(
                    d::Cannot_write_file_0_because_it_would_overwrite_input_file,
                    vec![JsString::from_bytes(name.as_slice())],
                );
                if config_path.is_empty() {
                    diagnostic.message_chain.push(Arc::new(Diagnostic::compiler(d::Adding_a_tsconfig_json_file_will_help_organize_projects_that_contain_both_TypeScript_and_JavaScript_files_Learn_more_at_https_Colon_Slash_Slashaka_ms_Slashtsconfig,vec![])));
                }
                blocked.insert(canonical.clone());
                v.diagnostics.push(diagnostic);
            }
            if !seen.insert(canonical.clone()) {
                blocked.insert(canonical);
                v.diagnostics.push(Diagnostic::compiler(
                    d::Cannot_write_file_0_because_it_would_be_overwritten_by_multiple_input_files,
                    vec![JsString::from_bytes(name)],
                ));
            }
        };
        for file in emitted {
            if !options.out_dir.is_empty()
                || options.emit_declarations() && !options.declaration_dir.is_empty()
            {
                check_common(&mut includes);
            }
            for name in output::output_names(file, program, &common)? {
                verify_path(name);
            }
        }
        verify_path(output::build_info_file(
            options,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        ));
    }
    Ok(OptionVerification {
        diagnostics: v.diagnostics,
        include_diagnostics: includes,
        blocked_output_paths: blocked,
    })
}

// Go encoding/json string output (the baseUrl migration suggestion) replaces
// each malformed UTF-8 byte and escapes HTML and the two JavaScript separators.
fn json_string(mut input: &[u8]) -> Vec<u8> {
    use ts_jsstring::wtf8::decode_utf8;
    let mut output = vec![b'"'];
    while !input.is_empty() {
        let (rune, width) = decode_utf8(input);
        input = &input[width..];
        match rune {
            0x22 => output.extend_from_slice(br#"\""#),
            0x5c => output.extend_from_slice(br"\\"),
            0x08 => output.extend_from_slice(br"\b"),
            0x0c => output.extend_from_slice(br"\f"),
            0x0a => output.extend_from_slice(br"\n"),
            0x0d => output.extend_from_slice(br"\r"),
            0x09 => output.extend_from_slice(br"\t"),
            0..=0x1f | 0x3c | 0x3e | 0x26 | 0x2028 | 0x2029 => {
                output.extend_from_slice(format!("\\u{rune:04x}").as_bytes());
            }
            _ => {
                let mut bytes = [0; 4];
                output.extend_from_slice(
                    char::from_u32(rune as u32)
                        .expect("Go UTF-8 decoding returns a Unicode scalar")
                        .encode_utf8(&mut bytes)
                        .as_bytes(),
                );
            }
        }
    }
    output.push(b'"');
    output
}
