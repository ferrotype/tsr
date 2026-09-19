//! Strict source CompilerOptions wire decoding. This deliberately does not
//! masquerade as tsconfig interpretation: targets/module kinds are numeric,
//! tristates preserve the source boolean/null interpretation, and unknown fields are rejected.
use serde_json::Value;
use ts_core::{
    CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, NewLineKind,
    ScriptTarget, Tristate,
};
use ts_jsstring::JsString;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    ExpectedObject,
    UnknownField(String),
    InvalidField(String),
}
pub fn compiler_options(value: &Value) -> Result<CompilerOptions, Error> {
    let map = value.as_object().ok_or(Error::ExpectedObject)?;
    let mut result = CompilerOptions::default();
    for (key, value) in map {
        if value.is_null() {
            continue;
        }
        match key.as_str() {
            "allowJs" => {
                result.allow_js = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowArbitraryExtensions" => {
                result.allow_arbitrary_extensions = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowImportingTsExtensions" => {
                result.allow_importing_ts_extensions = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowNonTsExtensions" => {
                result.allow_non_ts_extensions = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowUmdGlobalAccess" => {
                result.allow_umd_global_access = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowUnreachableCode" => {
                result.allow_unreachable_code = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "allowUnusedLabels" => {
                result.allow_unused_labels = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "assumeChangesOnlyAffectDirectDependencies" => {
                result.assume_changes_only_affect_direct_dependencies = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "checkJs" => {
                result.check_js = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "customConditions" => result.custom_conditions = Some(texts(value, key)?),
            "composite" => {
                result.composite = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "emitDeclarationOnly" => {
                result.emit_declaration_only = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "emitBOM" => {
                result.emit_bom = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "emitDecoratorMetadata" => {
                result.emit_decorator_metadata = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "declaration" => {
                result.declaration = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "declarationDir" => result.declaration_dir = text(value, key)?,
            "declarationMap" => {
                result.declaration_map = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "deduplicatePackages" => {
                result.deduplicate_packages = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "disableSizeLimit" => {
                result.disable_size_limit = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "disableSourceOfProjectReferenceRedirect" => {
                result.disable_source_of_project_reference_redirect = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "disableSolutionSearching" => {
                result.disable_solution_searching = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "disableReferencedProjectLoad" => {
                result.disable_referenced_project_load = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "erasableSyntaxOnly" => {
                result.erasable_syntax_only = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "exactOptionalPropertyTypes" => {
                result.exact_optional_property_types = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "experimentalDecorators" => {
                result.experimental_decorators = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "forceConsistentCasingInFileNames" => {
                result.force_consistent_casing_in_file_names = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "isolatedModules" => {
                result.isolated_modules = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "isolatedDeclarations" => {
                result.isolated_declarations = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "ignoreConfig" => {
                result.ignore_config = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "ignoreDeprecations" => result.ignore_deprecations = text(value, key)?,
            "importHelpers" => {
                result.import_helpers = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "inlineSourceMap" => {
                result.inline_source_map = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "inlineSources" => {
                result.inline_sources = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "init" => {
                result.init = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "incremental" => {
                result.incremental = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "jsx" => {
                result.jsx = JsxEmit(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "jsxFactory" => result.jsx_factory = text(value, key)?,
            "jsxFragmentFactory" => result.jsx_fragment_factory = text(value, key)?,
            "jsxImportSource" => result.jsx_import_source = text(value, key)?,
            "lib" => result.lib = Some(texts(value, key)?),
            "libReplacement" => {
                result.lib_replacement = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "locale" => result.locale = text(value, key)?,
            "mapRoot" => result.map_root = text(value, key)?,
            "module" => {
                result.module = ModuleKind(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "moduleResolution" => {
                result.module_resolution = ModuleResolutionKind(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "moduleSuffixes" => result.module_suffixes = Some(texts(value, key)?),
            "moduleDetection" => {
                result.module_detection = ModuleDetectionKind(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "newLine" => {
                result.new_line = NewLineKind(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "noEmit" => {
                result.no_emit = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noCheck" => {
                result.no_check = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noErrorTruncation" => {
                result.no_error_truncation = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noFallthroughCasesInSwitch" => {
                result.no_fallthrough_cases_in_switch = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noImplicitAny" => {
                result.no_implicit_any = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noImplicitThis" => {
                result.no_implicit_this = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noImplicitReturns" => {
                result.no_implicit_returns = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noEmitHelpers" => {
                result.no_emit_helpers = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noLib" => {
                result.no_lib = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noPropertyAccessFromIndexSignature" => {
                result.no_property_access_from_index_signature = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noUncheckedIndexedAccess" => {
                result.no_unchecked_indexed_access = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noEmitOnError" => {
                result.no_emit_on_error = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noUnusedLocals" => {
                result.no_unused_locals = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noUnusedParameters" => {
                result.no_unused_parameters = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noResolve" => {
                result.no_resolve = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noImplicitOverride" => {
                result.no_implicit_override = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noUncheckedSideEffectImports" => {
                result.no_unchecked_side_effect_imports = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "outDir" => result.out_dir = text(value, key)?,
            "paths" => {
                result.paths = Some(
                    value
                        .as_object()
                        .ok_or_else(|| Error::InvalidField(key.clone()))?
                        .iter()
                        .map(|(pattern, value)| {
                            Ok((
                                JsString::from_bytes(pattern.as_bytes()),
                                if value.is_null() {
                                    None
                                } else {
                                    Some(texts(value, key)?)
                                },
                            ))
                        })
                        .collect::<Result<_, Error>>()?,
                );
            }
            "preserveConstEnums" => {
                result.preserve_const_enums = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "preserveSymlinks" => {
                result.preserve_symlinks = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "project" => result.project = text(value, key)?,
            "resolveJsonModule" => {
                result.resolve_json_module = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "resolvePackageJsonExports" => {
                result.resolve_package_json_exports = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "resolvePackageJsonImports" => {
                result.resolve_package_json_imports = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "removeComments" => {
                result.remove_comments = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "rewriteRelativeImportExtensions" => {
                result.rewrite_relative_import_extensions = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "reactNamespace" => result.react_namespace = text(value, key)?,
            "rootDir" => result.root_dir = text(value, key)?,
            "rootDirs" => result.root_dirs = Some(texts(value, key)?),
            "skipLibCheck" => {
                result.skip_lib_check = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "stableTypeOrdering" => {
                result.stable_type_ordering = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strict" => {
                result.strict = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strictBindCallApply" => {
                result.strict_bind_call_apply = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strictBuiltinIteratorReturn" => {
                result.strict_builtin_iterator_return = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strictFunctionTypes" => {
                result.strict_function_types = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strictNullChecks" => {
                result.strict_null_checks = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "strictPropertyInitialization" => {
                result.strict_property_initialization = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "stripInternal" => {
                result.strip_internal = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "skipDefaultLibCheck" => {
                result.skip_default_lib_check = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "sourceMap" => {
                result.source_map = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "sourceRoot" => result.source_root = text(value, key)?,
            "suppressOutputPathCheck" => {
                result.suppress_output_path_check = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "target" => {
                result.target = ScriptTarget(
                    i32::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "traceResolution" => {
                result.trace_resolution = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "tsBuildInfoFile" => result.ts_build_info_file = text(value, key)?,
            "typeRoots" => result.type_roots = Some(texts(value, key)?),
            "types" => result.types = Some(texts(value, key)?),
            "useDefineForClassFields" => {
                result.use_define_for_class_fields = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "useUnknownInCatchVariables" => {
                result.use_unknown_in_catch_variables = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "verbatimModuleSyntax" => {
                result.verbatim_module_syntax = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "maxNodeModuleJsDepth" => {
                result.max_node_module_js_depth = Some(
                    isize::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            "allowSyntheticDefaultImports" => {
                result.allow_synthetic_default_imports = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "alwaysStrict" => {
                result.always_strict = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "baseUrl" => result.base_url = text(value, key)?,
            "downlevelIteration" => {
                result.downlevel_iteration = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "esModuleInterop" => {
                result.es_module_interop = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "outFile" => result.out_file = text(value, key)?,
            "configFilePath" => result.config_file_path = text(value, key)?,
            "noDtsResolution" => {
                result.no_dts_resolution = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "pathsBasePath" => result.paths_base_path = text(value, key)?,
            "diagnostics" => {
                result.diagnostics = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "extendedDiagnostics" => {
                result.extended_diagnostics = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "generateCpuProfile" => result.generate_cpu_profile = text(value, key)?,
            "generateTrace" => result.generate_trace = text(value, key)?,
            "listEmittedFiles" => {
                result.list_emitted_files = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "listFiles" => {
                result.list_files = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "explainFiles" => {
                result.explain_files = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "listFilesOnly" => {
                result.list_files_only = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "noEmitForJsFiles" => {
                result.no_emit_for_js_files = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "preserveWatchOutput" => {
                result.preserve_watch_output = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "pretty" => {
                result.pretty = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "version" => {
                result.version = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "watch" => {
                result.watch = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "showConfig" => {
                result.show_config = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "build" => {
                result.build = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "help" => {
                result.help = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "all" => {
                result.all = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "runExternalCode" => {
                result.run_external_code = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "pprofDir" => result.pprof_dir = text(value, key)?,
            "singleThreaded" => {
                result.single_threaded = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "quiet" => {
                result.quiet = match value.as_bool() {
                    Some(value) => Tristate::from(value),
                    None => Tristate::UNKNOWN,
                }
            }
            "checkers" => {
                result.checkers = Some(
                    isize::try_from(integer(value, key)?)
                        .map_err(|_| Error::InvalidField(key.clone()))?,
                );
            }
            _ => return Err(Error::UnknownField(key.clone())),
        }
    }
    Ok(result)
}
fn integer(value: &Value, key: &str) -> Result<i64, Error> {
    value
        .as_i64()
        .ok_or_else(|| Error::InvalidField(key.into()))
}
fn text(value: &Value, key: &str) -> Result<JsString, Error> {
    value
        .as_str()
        .map(|s| JsString::from_bytes(s.as_bytes()))
        .ok_or_else(|| Error::InvalidField(key.into()))
}
fn texts(value: &Value, key: &str) -> Result<Vec<JsString>, Error> {
    value
        .as_array()
        .ok_or_else(|| Error::InvalidField(key.into()))?
        .iter()
        .map(|value| text(value, key))
        .collect()
}
