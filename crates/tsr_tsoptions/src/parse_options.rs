//! Source config value assignment. Enum/type validation belongs to the caller;
//! this layer preserves the original ParseCompilerOptions conversion boundary.
use crate::{option_declaration, ConfigValue};
use ts_core::{CompilerOptions, PathMappings, Tristate};
use ts_jsstring::JsString;

/// port: tsc/internal/tsoptions/parsinghelpers.go:ParseTristate
pub fn parse_tristate(value: &ConfigValue) -> Tristate {
    match value {
        ConfigValue::Null => Tristate::UNKNOWN,
        ConfigValue::Boolean(true) => Tristate::TRUE,
        _ => Tristate::FALSE,
    }
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:ParseStringArray
pub fn parse_string_array(value: &ConfigValue) -> Option<Vec<JsString>> {
    let ConfigValue::Array(values) = value else {
        return None;
    };
    values.as_ref().map(|values| {
        values
            .iter()
            .filter_map(|value| value.as_string().cloned())
            .collect()
    })
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:ParseString
pub fn parse_string(value: &ConfigValue) -> JsString {
    value.as_string().cloned().unwrap_or_default()
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:parseStringMap
pub fn parse_string_map(value: &ConfigValue) -> Option<PathMappings> {
    value.as_object().map(|entries| {
        entries
            .iter()
            .map(|(name, value)| (name.clone(), parse_string_array(value)))
            .collect()
    })
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:parseNumber
pub fn parse_number(value: &ConfigValue) -> Option<isize> {
    match value {
        ConfigValue::Integer(value) => Some(*value as isize),
        ConfigValue::Number(value) => Some(*value as isize),
        _ => None,
    }
}
fn parse_enum(value: &ConfigValue) -> i32 {
    match value {
        ConfigValue::Enum(value) => *value,
        ConfigValue::Number(value) => *value as i32,
        _ => panic!("compiler option enum requires a converted enum or float64"),
    }
}

/// port: tsc/internal/tsoptions/parsinghelpers.go:ParseCompilerOptions
pub fn parse_compiler_options(key: &[u8], value: &ConfigValue, options: &mut CompilerOptions) {
    if value.is_null() {
        return;
    }
    let key = option_declaration(key, false).map_or(key, |option| option.name.as_bytes());
    match key {
        b"allowJs" => options.allow_js = parse_tristate(value),
        b"allowImportingTsExtensions" => {
            options.allow_importing_ts_extensions = parse_tristate(value);
        }
        b"allowSyntheticDefaultImports" => {
            options.allow_synthetic_default_imports = parse_tristate(value);
        }
        b"allowNonTsExtensions" => options.allow_non_ts_extensions = parse_tristate(value),
        b"allowUmdGlobalAccess" => options.allow_umd_global_access = parse_tristate(value),
        b"allowUnreachableCode" => options.allow_unreachable_code = parse_tristate(value),
        b"allowUnusedLabels" => options.allow_unused_labels = parse_tristate(value),
        b"allowArbitraryExtensions" => options.allow_arbitrary_extensions = parse_tristate(value),
        b"alwaysStrict" => options.always_strict = parse_tristate(value),
        b"assumeChangesOnlyAffectDirectDependencies" => {
            options.assume_changes_only_affect_direct_dependencies = parse_tristate(value);
        }
        b"baseUrl" => options.base_url = parse_string(value),
        b"build" => options.build = parse_tristate(value),
        b"checkJs" => options.check_js = parse_tristate(value),
        b"customConditions" => options.custom_conditions = parse_string_array(value),
        b"composite" => options.composite = parse_tristate(value),
        b"declarationDir" => options.declaration_dir = parse_string(value),
        b"deduplicatePackages" => options.deduplicate_packages = parse_tristate(value),
        b"diagnostics" => options.diagnostics = parse_tristate(value),
        b"disableSizeLimit" => options.disable_size_limit = parse_tristate(value),
        b"disableSourceOfProjectReferenceRedirect" => {
            options.disable_source_of_project_reference_redirect = parse_tristate(value);
        }
        b"disableSolutionSearching" => options.disable_solution_searching = parse_tristate(value),
        b"disableReferencedProjectLoad" => {
            options.disable_referenced_project_load = parse_tristate(value);
        }
        b"declarationMap" => options.declaration_map = parse_tristate(value),
        b"declaration" => options.declaration = parse_tristate(value),
        b"downlevelIteration" => options.downlevel_iteration = parse_tristate(value),
        b"erasableSyntaxOnly" => options.erasable_syntax_only = parse_tristate(value),
        b"emitDeclarationOnly" => options.emit_declaration_only = parse_tristate(value),
        b"extendedDiagnostics" => options.extended_diagnostics = parse_tristate(value),
        b"emitDecoratorMetadata" => options.emit_decorator_metadata = parse_tristate(value),
        b"emitBOM" => options.emit_bom = parse_tristate(value),
        b"esModuleInterop" => options.es_module_interop = parse_tristate(value),
        b"exactOptionalPropertyTypes" => {
            options.exact_optional_property_types = parse_tristate(value);
        }
        b"explainFiles" => options.explain_files = parse_tristate(value),
        b"experimentalDecorators" => options.experimental_decorators = parse_tristate(value),
        b"forceConsistentCasingInFileNames" => {
            options.force_consistent_casing_in_file_names = parse_tristate(value);
        }
        b"generateCpuProfile" => options.generate_cpu_profile = parse_string(value),
        b"generateTrace" => options.generate_trace = parse_string(value),
        b"isolatedModules" => options.isolated_modules = parse_tristate(value),
        b"ignoreConfig" => options.ignore_config = parse_tristate(value),
        b"ignoreDeprecations" => options.ignore_deprecations = parse_string(value),
        b"importHelpers" => options.import_helpers = parse_tristate(value),
        b"incremental" => options.incremental = parse_tristate(value),
        b"init" => options.init = parse_tristate(value),
        b"inlineSourceMap" => options.inline_source_map = parse_tristate(value),
        b"inlineSources" => options.inline_sources = parse_tristate(value),
        b"isolatedDeclarations" => options.isolated_declarations = parse_tristate(value),
        b"jsx" => options.jsx = ts_core::JsxEmit(parse_enum(value)),
        b"jsxFactory" => options.jsx_factory = parse_string(value),
        b"jsxFragmentFactory" => options.jsx_fragment_factory = parse_string(value),
        b"jsxImportSource" => options.jsx_import_source = parse_string(value),
        b"lib" => options.lib = parse_string_array(value),
        b"libReplacement" => options.lib_replacement = parse_tristate(value),
        b"listEmittedFiles" => options.list_emitted_files = parse_tristate(value),
        b"listFiles" => options.list_files = parse_tristate(value),
        b"listFilesOnly" => options.list_files_only = parse_tristate(value),
        b"locale" => options.locale = parse_string(value),
        b"mapRoot" => options.map_root = parse_string(value),
        b"module" => options.module = ts_core::ModuleKind(parse_enum(value)),
        b"moduleResolution" => {
            options.module_resolution = ts_core::ModuleResolutionKind(parse_enum(value));
        }
        b"moduleSuffixes" => options.module_suffixes = parse_string_array(value),
        b"moduleDetection" | b"moduleDetectionKind" => {
            options.module_detection = ts_core::ModuleDetectionKind(parse_enum(value));
        }
        b"noCheck" => options.no_check = parse_tristate(value),
        b"noFallthroughCasesInSwitch" => {
            options.no_fallthrough_cases_in_switch = parse_tristate(value);
        }
        b"noEmitForJsFiles" => options.no_emit_for_js_files = parse_tristate(value),
        b"noErrorTruncation" => options.no_error_truncation = parse_tristate(value),
        b"noImplicitAny" => options.no_implicit_any = parse_tristate(value),
        b"noImplicitThis" => options.no_implicit_this = parse_tristate(value),
        b"noLib" => options.no_lib = parse_tristate(value),
        b"noPropertyAccessFromIndexSignature" => {
            options.no_property_access_from_index_signature = parse_tristate(value);
        }
        b"noUncheckedIndexedAccess" => options.no_unchecked_indexed_access = parse_tristate(value),
        b"noEmitHelpers" => options.no_emit_helpers = parse_tristate(value),
        b"noEmitOnError" => options.no_emit_on_error = parse_tristate(value),
        b"noImplicitReturns" => options.no_implicit_returns = parse_tristate(value),
        b"noUnusedLocals" => options.no_unused_locals = parse_tristate(value),
        b"noUnusedParameters" => options.no_unused_parameters = parse_tristate(value),
        b"noImplicitOverride" => options.no_implicit_override = parse_tristate(value),
        b"noUncheckedSideEffectImports" => {
            options.no_unchecked_side_effect_imports = parse_tristate(value);
        }
        b"outFile" => options.out_file = parse_string(value),
        b"noResolve" => options.no_resolve = parse_tristate(value),
        b"paths" => options.paths = parse_string_map(value),
        b"preserveWatchOutput" => options.preserve_watch_output = parse_tristate(value),
        b"preserveConstEnums" => options.preserve_const_enums = parse_tristate(value),
        b"preserveSymlinks" => options.preserve_symlinks = parse_tristate(value),
        b"project" => options.project = parse_string(value),
        b"pretty" => options.pretty = parse_tristate(value),
        b"resolveJsonModule" => options.resolve_json_module = parse_tristate(value),
        b"resolvePackageJsonExports" => {
            options.resolve_package_json_exports = parse_tristate(value);
        }
        b"resolvePackageJsonImports" => {
            options.resolve_package_json_imports = parse_tristate(value);
        }
        b"reactNamespace" => options.react_namespace = parse_string(value),
        b"rewriteRelativeImportExtensions" => {
            options.rewrite_relative_import_extensions = parse_tristate(value);
        }
        b"rootDir" => options.root_dir = parse_string(value),
        b"rootDirs" => options.root_dirs = parse_string_array(value),
        b"removeComments" => options.remove_comments = parse_tristate(value),
        b"stableTypeOrdering" => options.stable_type_ordering = parse_tristate(value),
        b"strict" => options.strict = parse_tristate(value),
        b"strictBindCallApply" => options.strict_bind_call_apply = parse_tristate(value),
        b"strictBuiltinIteratorReturn" => {
            options.strict_builtin_iterator_return = parse_tristate(value);
        }
        b"strictFunctionTypes" => options.strict_function_types = parse_tristate(value),
        b"strictNullChecks" => options.strict_null_checks = parse_tristate(value),
        b"strictPropertyInitialization" => {
            options.strict_property_initialization = parse_tristate(value);
        }
        b"skipDefaultLibCheck" => options.skip_default_lib_check = parse_tristate(value),
        b"sourceMap" => options.source_map = parse_tristate(value),
        b"sourceRoot" => options.source_root = parse_string(value),
        b"stripInternal" => options.strip_internal = parse_tristate(value),
        b"suppressOutputPathCheck" => options.suppress_output_path_check = parse_tristate(value),
        b"target" => options.target = ts_core::ScriptTarget(parse_enum(value)),
        b"traceResolution" => options.trace_resolution = parse_tristate(value),
        b"tsBuildInfoFile" => options.ts_build_info_file = parse_string(value),
        b"typeRoots" => options.type_roots = parse_string_array(value),
        b"types" => options.types = parse_string_array(value),
        b"useDefineForClassFields" => options.use_define_for_class_fields = parse_tristate(value),
        b"useUnknownInCatchVariables" => {
            options.use_unknown_in_catch_variables = parse_tristate(value);
        }
        b"verbatimModuleSyntax" => options.verbatim_module_syntax = parse_tristate(value),
        b"version" => options.version = parse_tristate(value),
        b"help" => options.help = parse_tristate(value),
        b"all" => options.all = parse_tristate(value),
        b"maxNodeModuleJsDepth" => options.max_node_module_js_depth = parse_number(value),
        b"skipLibCheck" => options.skip_lib_check = parse_tristate(value),
        b"noEmit" => options.no_emit = parse_tristate(value),
        b"showConfig" => options.show_config = parse_tristate(value),
        b"configFilePath" => options.config_file_path = parse_string(value),
        b"noDtsResolution" => options.no_dts_resolution = parse_tristate(value),
        b"pathsBasePath" => options.paths_base_path = parse_string(value),
        b"outDir" => options.out_dir = parse_string(value),
        b"newLine" => options.new_line = ts_core::NewLineKind(parse_enum(value)),
        b"watch" => options.watch = parse_tristate(value),
        b"pprofDir" => options.pprof_dir = parse_string(value),
        b"singleThreaded" => options.single_threaded = parse_tristate(value),
        b"quiet" => options.quiet = parse_tristate(value),
        b"checkers" => options.checkers = parse_number(value),
        b"runExternalCode" => options.run_external_code = parse_tristate(value),
        _ => {}
    }
}
