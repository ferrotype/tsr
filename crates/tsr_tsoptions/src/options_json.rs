//! `json.Marshal` of the pinned `core.CompilerOptions`: every field in struct
//! order under its JSON name, zero values left out (`omitzero`). The content
//! mapper host sends the whole object to a mapper's `openProject`, and a
//! mapper's transform identity folds in the fields its manifest declares.
use tsr_core::{CompilerOptions, Tristate};
use tsr_json::{Encode, Encoder, Error, Token};
use tsr_jsstring::JsString;

/// One option's non-zero value.
pub enum OptionValue<'a> {
    Bool(bool),
    Null,
    String(&'a JsString),
    Strings(&'a [JsString]),
    Int(i64),
    Paths(&'a tsr_core::compiler_options::PathMappings),
}

impl Encode for OptionValue<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        match self {
            Self::Bool(value) => out.boolean(*value),
            Self::Null => out.null(),
            Self::String(value) => value.encode(out),
            Self::Strings(values) => out.array(values.iter()),
            Self::Int(value) => out.int(*value),
            Self::Paths(paths) => {
                out.write_token(Token::BeginObject)?;
                for (key, value) in *paths {
                    key.encode(out)?;
                    match value {
                        Some(values) => out.array(values.iter())?,
                        None => out.null()?,
                    }
                }
                out.write_token(Token::EndObject)
            }
        }
    }
}

/// `Tristate.MarshalJSON`; the unknown state is zero and omitted.
fn tristate(value: Tristate) -> Option<OptionValue<'static>> {
    match value {
        Tristate::UNKNOWN => None,
        Tristate::TRUE => Some(OptionValue::Bool(true)),
        Tristate::FALSE => Some(OptionValue::Bool(false)),
        _ => Some(OptionValue::Null),
    }
}

fn string(value: &JsString) -> Option<OptionValue<'_>> {
    (!value.is_empty()).then_some(OptionValue::String(value))
}

/// A nil slice is zero; an empty non-nil slice is not.
fn strings(value: Option<&[JsString]>) -> Option<OptionValue<'_>> {
    value.map(OptionValue::Strings)
}

fn enumeration(value: i32) -> Option<OptionValue<'static>> {
    (value != 0).then_some(OptionValue::Int(i64::from(value)))
}

type Field = (
    &'static str,
    for<'a> fn(&'a CompilerOptions) -> Option<OptionValue<'a>>,
);

/// The option fields in `core.CompilerOptions` order, by JSON name.
#[allow(clippy::cast_possible_wrap)]
const FIELDS: &[Field] = &[
    ("allowJs", |o| tristate(o.allow_js)),
    ("allowArbitraryExtensions", |o| {
        tristate(o.allow_arbitrary_extensions)
    }),
    ("allowImportingTsExtensions", |o| {
        tristate(o.allow_importing_ts_extensions)
    }),
    ("allowNonTsExtensions", |o| {
        tristate(o.allow_non_ts_extensions)
    }),
    ("allowUmdGlobalAccess", |o| {
        tristate(o.allow_umd_global_access)
    }),
    ("allowUnreachableCode", |o| {
        tristate(o.allow_unreachable_code)
    }),
    ("allowUnusedLabels", |o| tristate(o.allow_unused_labels)),
    ("assumeChangesOnlyAffectDirectDependencies", |o| {
        tristate(o.assume_changes_only_affect_direct_dependencies)
    }),
    ("checkJs", |o| tristate(o.check_js)),
    ("customConditions", |o| {
        strings(o.custom_conditions.as_deref())
    }),
    ("composite", |o| tristate(o.composite)),
    ("emitDeclarationOnly", |o| tristate(o.emit_declaration_only)),
    ("emitBOM", |o| tristate(o.emit_bom)),
    ("emitDecoratorMetadata", |o| {
        tristate(o.emit_decorator_metadata)
    }),
    ("declaration", |o| tristate(o.declaration)),
    ("declarationDir", |o| string(&o.declaration_dir)),
    ("declarationMap", |o| tristate(o.declaration_map)),
    ("deduplicatePackages", |o| tristate(o.deduplicate_packages)),
    ("disableSizeLimit", |o| tristate(o.disable_size_limit)),
    ("disableSourceOfProjectReferenceRedirect", |o| {
        tristate(o.disable_source_of_project_reference_redirect)
    }),
    ("disableSolutionSearching", |o| {
        tristate(o.disable_solution_searching)
    }),
    ("disableReferencedProjectLoad", |o| {
        tristate(o.disable_referenced_project_load)
    }),
    ("erasableSyntaxOnly", |o| tristate(o.erasable_syntax_only)),
    ("exactOptionalPropertyTypes", |o| {
        tristate(o.exact_optional_property_types)
    }),
    ("experimentalDecorators", |o| {
        tristate(o.experimental_decorators)
    }),
    ("forceConsistentCasingInFileNames", |o| {
        tristate(o.force_consistent_casing_in_file_names)
    }),
    ("isolatedModules", |o| tristate(o.isolated_modules)),
    ("isolatedDeclarations", |o| {
        tristate(o.isolated_declarations)
    }),
    ("ignoreConfig", |o| tristate(o.ignore_config)),
    ("ignoreDeprecations", |o| string(&o.ignore_deprecations)),
    ("importHelpers", |o| tristate(o.import_helpers)),
    ("inlineSourceMap", |o| tristate(o.inline_source_map)),
    ("inlineSources", |o| tristate(o.inline_sources)),
    ("init", |o| tristate(o.init)),
    ("incremental", |o| tristate(o.incremental)),
    ("jsx", |o| enumeration(o.jsx.0)),
    ("jsxFactory", |o| string(&o.jsx_factory)),
    ("jsxFragmentFactory", |o| string(&o.jsx_fragment_factory)),
    ("jsxImportSource", |o| string(&o.jsx_import_source)),
    ("lib", |o| strings(o.lib.as_deref())),
    ("libReplacement", |o| tristate(o.lib_replacement)),
    ("locale", |o| string(&o.locale)),
    ("mapRoot", |o| string(&o.map_root)),
    ("module", |o| enumeration(o.module.0)),
    ("moduleResolution", |o| enumeration(o.module_resolution.0)),
    ("moduleSuffixes", |o| strings(o.module_suffixes.as_deref())),
    ("moduleDetection", |o| enumeration(o.module_detection.0)),
    ("newLine", |o| enumeration(o.new_line.0)),
    ("noEmit", |o| tristate(o.no_emit)),
    ("noCheck", |o| tristate(o.no_check)),
    ("noErrorTruncation", |o| tristate(o.no_error_truncation)),
    ("noFallthroughCasesInSwitch", |o| {
        tristate(o.no_fallthrough_cases_in_switch)
    }),
    ("noImplicitAny", |o| tristate(o.no_implicit_any)),
    ("noImplicitThis", |o| tristate(o.no_implicit_this)),
    ("noImplicitReturns", |o| tristate(o.no_implicit_returns)),
    ("noEmitHelpers", |o| tristate(o.no_emit_helpers)),
    ("noLib", |o| tristate(o.no_lib)),
    ("noPropertyAccessFromIndexSignature", |o| {
        tristate(o.no_property_access_from_index_signature)
    }),
    ("noUncheckedIndexedAccess", |o| {
        tristate(o.no_unchecked_indexed_access)
    }),
    ("noEmitOnError", |o| tristate(o.no_emit_on_error)),
    ("noUnusedLocals", |o| tristate(o.no_unused_locals)),
    ("noUnusedParameters", |o| tristate(o.no_unused_parameters)),
    ("noResolve", |o| tristate(o.no_resolve)),
    ("noImplicitOverride", |o| tristate(o.no_implicit_override)),
    ("noUncheckedSideEffectImports", |o| {
        tristate(o.no_unchecked_side_effect_imports)
    }),
    ("outDir", |o| string(&o.out_dir)),
    ("paths", |o| o.paths.as_ref().map(OptionValue::Paths)),
    ("preserveConstEnums", |o| tristate(o.preserve_const_enums)),
    ("preserveSymlinks", |o| tristate(o.preserve_symlinks)),
    ("project", |o| string(&o.project)),
    ("resolveJsonModule", |o| tristate(o.resolve_json_module)),
    ("resolvePackageJsonExports", |o| {
        tristate(o.resolve_package_json_exports)
    }),
    ("resolvePackageJsonImports", |o| {
        tristate(o.resolve_package_json_imports)
    }),
    ("removeComments", |o| tristate(o.remove_comments)),
    ("rewriteRelativeImportExtensions", |o| {
        tristate(o.rewrite_relative_import_extensions)
    }),
    ("reactNamespace", |o| string(&o.react_namespace)),
    ("rootDir", |o| string(&o.root_dir)),
    ("rootDirs", |o| strings(o.root_dirs.as_deref())),
    ("skipLibCheck", |o| tristate(o.skip_lib_check)),
    ("stableTypeOrdering", |o| tristate(o.stable_type_ordering)),
    ("strict", |o| tristate(o.strict)),
    ("strictBindCallApply", |o| {
        tristate(o.strict_bind_call_apply)
    }),
    ("strictBuiltinIteratorReturn", |o| {
        tristate(o.strict_builtin_iterator_return)
    }),
    ("strictFunctionTypes", |o| tristate(o.strict_function_types)),
    ("strictNullChecks", |o| tristate(o.strict_null_checks)),
    ("strictPropertyInitialization", |o| {
        tristate(o.strict_property_initialization)
    }),
    ("stripInternal", |o| tristate(o.strip_internal)),
    ("skipDefaultLibCheck", |o| {
        tristate(o.skip_default_lib_check)
    }),
    ("sourceMap", |o| tristate(o.source_map)),
    ("sourceRoot", |o| string(&o.source_root)),
    ("suppressOutputPathCheck", |o| {
        tristate(o.suppress_output_path_check)
    }),
    ("target", |o| enumeration(o.target.0)),
    ("traceResolution", |o| tristate(o.trace_resolution)),
    ("tsBuildInfoFile", |o| string(&o.ts_build_info_file)),
    ("typeRoots", |o| strings(o.type_roots.as_deref())),
    ("types", |o| strings(o.types.as_deref())),
    ("useDefineForClassFields", |o| {
        tristate(o.use_define_for_class_fields)
    }),
    ("useUnknownInCatchVariables", |o| {
        tristate(o.use_unknown_in_catch_variables)
    }),
    ("verbatimModuleSyntax", |o| {
        tristate(o.verbatim_module_syntax)
    }),
    ("maxNodeModuleJsDepth", |o| {
        o.max_node_module_js_depth
            .map(|value| OptionValue::Int(value as i64))
    }),
    ("allowSyntheticDefaultImports", |o| {
        tristate(o.allow_synthetic_default_imports)
    }),
    ("alwaysStrict", |o| tristate(o.always_strict)),
    ("baseUrl", |o| string(&o.base_url)),
    ("downlevelIteration", |o| tristate(o.downlevel_iteration)),
    ("esModuleInterop", |o| tristate(o.es_module_interop)),
    ("outFile", |o| string(&o.out_file)),
    ("configFilePath", |o| string(&o.config_file_path)),
    ("noDtsResolution", |o| tristate(o.no_dts_resolution)),
    ("pathsBasePath", |o| string(&o.paths_base_path)),
    ("diagnostics", |o| tristate(o.diagnostics)),
    ("extendedDiagnostics", |o| tristate(o.extended_diagnostics)),
    ("generateCpuProfile", |o| string(&o.generate_cpu_profile)),
    ("generateTrace", |o| string(&o.generate_trace)),
    ("listEmittedFiles", |o| tristate(o.list_emitted_files)),
    ("listFiles", |o| tristate(o.list_files)),
    ("explainFiles", |o| tristate(o.explain_files)),
    ("listFilesOnly", |o| tristate(o.list_files_only)),
    ("noEmitForJsFiles", |o| tristate(o.no_emit_for_js_files)),
    ("preserveWatchOutput", |o| tristate(o.preserve_watch_output)),
    ("pretty", |o| tristate(o.pretty)),
    ("version", |o| tristate(o.version)),
    ("watch", |o| tristate(o.watch)),
    ("showConfig", |o| tristate(o.show_config)),
    ("build", |o| tristate(o.build)),
    ("help", |o| tristate(o.help)),
    ("all", |o| tristate(o.all)),
    ("runExternalCode", |o| tristate(o.run_external_code)),
    ("pprofDir", |o| string(&o.pprof_dir)),
    ("singleThreaded", |o| tristate(o.single_threaded)),
    ("quiet", |o| tristate(o.quiet)),
    ("checkers", |o| {
        o.checkers.map(|value| OptionValue::Int(value as i64))
    }),
];

/// The JSON of one option by its JSON name, `None` when the name is not an
/// option or the value is zero (`MarshalDeclaredOptions`' reflective lookup).
pub fn option_json(options: &CompilerOptions, name: &str) -> Result<Option<Vec<u8>>, Error> {
    let Some((_, field)) = FIELDS.iter().find(|(field, _)| *field == name) else {
        return Ok(None);
    };
    field(options)
        .map(|value| tsr_json::marshal(&value, tsr_json::Options::default()))
        .transpose()
}

/// Encodes the whole options object.
pub struct CompilerOptionsJson<'a>(pub &'a CompilerOptions);

impl Encode for CompilerOptionsJson<'_> {
    fn type_name(&self) -> &'static str {
        "core.CompilerOptions"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        for (name, field) in FIELDS {
            if let Some(value) = field(self.0) {
                out.string(name.as_bytes())?;
                out.value(&value)?;
            }
        }
        out.write_token(Token::EndObject)
    }
}

/// `json.Marshal(options)`.
pub fn marshal_compiler_options(options: &CompilerOptions) -> Result<Vec<u8>, Error> {
    tsr_json::marshal(&CompilerOptionsJson(options), tsr_json::Options::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_options_are_left_out_and_the_rest_keep_struct_order() {
        let mut options = CompilerOptions::default();
        assert_eq!(marshal_compiler_options(&options).unwrap(), b"{}");
        options.target = tsr_core::ScriptTarget::ESNEXT;
        options.allow_js = Tristate::FALSE;
        options.strict = Tristate::TRUE;
        options.jsx = tsr_core::compiler_options::JsxEmit::REACT_JSX;
        options.lib = Some(Vec::new());
        options.config_file_path = JsString::from_bytes(b"/tsconfig.json".as_slice());
        options.max_node_module_js_depth = Some(0);
        assert_eq!(
            String::from_utf8(marshal_compiler_options(&options).unwrap()).unwrap(),
            r#"{"allowJs":false,"jsx":4,"lib":[],"strict":true,"target":99,"maxNodeModuleJsDepth":0,"configFilePath":"/tsconfig.json"}"#
        );
        assert_eq!(
            option_json(&options, "target").unwrap(),
            Some(b"99".to_vec())
        );
        assert_eq!(option_json(&options, "module").unwrap(), None);
        assert_eq!(option_json(&options, "notAnOption").unwrap(), None);
    }
}

/// Decodes one option by its JSON name, as json v2 decodes the struct field
/// the name selects; names that are not options are skipped.
type Setter = fn(&mut CompilerOptions, &mut tsr_json::Decoder<'_>) -> Result<(), Error>;

fn set_tristate(target: &mut Tristate, input: &mut tsr_json::Decoder<'_>) -> Result<(), Error> {
    use tsr_json::Decode as _;
    target.decode(input)
}
fn set_string(target: &mut JsString, input: &mut tsr_json::Decoder<'_>) -> Result<(), Error> {
    input.value(target)
}
/// A JSON null is a nil slice; an array, even empty, is a non-nil one.
fn set_strings(
    target: &mut Option<Vec<JsString>>,
    input: &mut tsr_json::Decoder<'_>,
) -> Result<(), Error> {
    input.value(target)
}
fn set_int(target: &mut i32, input: &mut tsr_json::Decoder<'_>) -> Result<(), Error> {
    input.value(target)
}
/// Go `*int`: null leaves the option unset.
fn set_optional_int(
    target: &mut Option<isize>,
    input: &mut tsr_json::Decoder<'_>,
) -> Result<(), Error> {
    input.value(target)
}
fn set_paths(
    target: &mut Option<tsr_core::compiler_options::PathMappings>,
    input: &mut tsr_json::Decoder<'_>,
) -> Result<(), Error> {
    if input.peek_kind() == tsr_json::Kind::Null {
        input.read_token()?;
        *target = None;
        return Ok(());
    }
    let mut paths = tsr_core::compiler_options::PathMappings::default();
    input.object(|key, input| {
        let mut value: Option<Vec<JsString>> = None;
        input.value(&mut value)?;
        if paths.insert(JsString::from_bytes(key), value).is_some() {
            return Err(Error::Message(format!(
                "duplicate name {:?} in object",
                String::from_utf8_lossy(key)
            )));
        }
        Ok(())
    })?;
    *target = Some(paths);
    Ok(())
}

macro_rules! setters {
    ($( $name:literal => $kind:ident $field:ident ),* $(,)?) => {
        const SETTERS: &[(&str, Setter)] = &[
            $( ($name, |o, input| setters!(@set $kind, o.$field, input)), )*
        ];
    };
    (@set tristate, $target:expr, $input:expr) => { set_tristate(&mut $target, $input) };
    (@set string, $target:expr, $input:expr) => { set_string(&mut $target, $input) };
    (@set strings, $target:expr, $input:expr) => { set_strings(&mut $target, $input) };
    (@set enumeration, $target:expr, $input:expr) => { set_int(&mut $target.0, $input) };
    (@set optional_int, $target:expr, $input:expr) => { set_optional_int(&mut $target, $input) };
    (@set paths, $target:expr, $input:expr) => { set_paths(&mut $target, $input) };
}

setters!(
    "allowJs" => tristate allow_js,
    "allowArbitraryExtensions" => tristate allow_arbitrary_extensions,
    "allowImportingTsExtensions" => tristate allow_importing_ts_extensions,
    "allowNonTsExtensions" => tristate allow_non_ts_extensions,
    "allowUmdGlobalAccess" => tristate allow_umd_global_access,
    "allowUnreachableCode" => tristate allow_unreachable_code,
    "allowUnusedLabels" => tristate allow_unused_labels,
    "assumeChangesOnlyAffectDirectDependencies" => tristate assume_changes_only_affect_direct_dependencies,
    "checkJs" => tristate check_js,
    "customConditions" => strings custom_conditions,
    "composite" => tristate composite,
    "emitDeclarationOnly" => tristate emit_declaration_only,
    "emitBOM" => tristate emit_bom,
    "emitDecoratorMetadata" => tristate emit_decorator_metadata,
    "declaration" => tristate declaration,
    "declarationDir" => string declaration_dir,
    "declarationMap" => tristate declaration_map,
    "deduplicatePackages" => tristate deduplicate_packages,
    "disableSizeLimit" => tristate disable_size_limit,
    "disableSourceOfProjectReferenceRedirect" => tristate disable_source_of_project_reference_redirect,
    "disableSolutionSearching" => tristate disable_solution_searching,
    "disableReferencedProjectLoad" => tristate disable_referenced_project_load,
    "erasableSyntaxOnly" => tristate erasable_syntax_only,
    "exactOptionalPropertyTypes" => tristate exact_optional_property_types,
    "experimentalDecorators" => tristate experimental_decorators,
    "forceConsistentCasingInFileNames" => tristate force_consistent_casing_in_file_names,
    "isolatedModules" => tristate isolated_modules,
    "isolatedDeclarations" => tristate isolated_declarations,
    "ignoreConfig" => tristate ignore_config,
    "ignoreDeprecations" => string ignore_deprecations,
    "importHelpers" => tristate import_helpers,
    "inlineSourceMap" => tristate inline_source_map,
    "inlineSources" => tristate inline_sources,
    "init" => tristate init,
    "incremental" => tristate incremental,
    "jsx" => enumeration jsx,
    "jsxFactory" => string jsx_factory,
    "jsxFragmentFactory" => string jsx_fragment_factory,
    "jsxImportSource" => string jsx_import_source,
    "lib" => strings lib,
    "libReplacement" => tristate lib_replacement,
    "locale" => string locale,
    "mapRoot" => string map_root,
    "module" => enumeration module,
    "moduleResolution" => enumeration module_resolution,
    "moduleSuffixes" => strings module_suffixes,
    "moduleDetection" => enumeration module_detection,
    "newLine" => enumeration new_line,
    "noEmit" => tristate no_emit,
    "noCheck" => tristate no_check,
    "noErrorTruncation" => tristate no_error_truncation,
    "noFallthroughCasesInSwitch" => tristate no_fallthrough_cases_in_switch,
    "noImplicitAny" => tristate no_implicit_any,
    "noImplicitThis" => tristate no_implicit_this,
    "noImplicitReturns" => tristate no_implicit_returns,
    "noEmitHelpers" => tristate no_emit_helpers,
    "noLib" => tristate no_lib,
    "noPropertyAccessFromIndexSignature" => tristate no_property_access_from_index_signature,
    "noUncheckedIndexedAccess" => tristate no_unchecked_indexed_access,
    "noEmitOnError" => tristate no_emit_on_error,
    "noUnusedLocals" => tristate no_unused_locals,
    "noUnusedParameters" => tristate no_unused_parameters,
    "noResolve" => tristate no_resolve,
    "noImplicitOverride" => tristate no_implicit_override,
    "noUncheckedSideEffectImports" => tristate no_unchecked_side_effect_imports,
    "outDir" => string out_dir,
    "paths" => paths paths,
    "preserveConstEnums" => tristate preserve_const_enums,
    "preserveSymlinks" => tristate preserve_symlinks,
    "project" => string project,
    "resolveJsonModule" => tristate resolve_json_module,
    "resolvePackageJsonExports" => tristate resolve_package_json_exports,
    "resolvePackageJsonImports" => tristate resolve_package_json_imports,
    "removeComments" => tristate remove_comments,
    "rewriteRelativeImportExtensions" => tristate rewrite_relative_import_extensions,
    "reactNamespace" => string react_namespace,
    "rootDir" => string root_dir,
    "rootDirs" => strings root_dirs,
    "skipLibCheck" => tristate skip_lib_check,
    "stableTypeOrdering" => tristate stable_type_ordering,
    "strict" => tristate strict,
    "strictBindCallApply" => tristate strict_bind_call_apply,
    "strictBuiltinIteratorReturn" => tristate strict_builtin_iterator_return,
    "strictFunctionTypes" => tristate strict_function_types,
    "strictNullChecks" => tristate strict_null_checks,
    "strictPropertyInitialization" => tristate strict_property_initialization,
    "stripInternal" => tristate strip_internal,
    "skipDefaultLibCheck" => tristate skip_default_lib_check,
    "sourceMap" => tristate source_map,
    "sourceRoot" => string source_root,
    "suppressOutputPathCheck" => tristate suppress_output_path_check,
    "target" => enumeration target,
    "traceResolution" => tristate trace_resolution,
    "tsBuildInfoFile" => string ts_build_info_file,
    "typeRoots" => strings type_roots,
    "types" => strings types,
    "useDefineForClassFields" => tristate use_define_for_class_fields,
    "useUnknownInCatchVariables" => tristate use_unknown_in_catch_variables,
    "verbatimModuleSyntax" => tristate verbatim_module_syntax,
    "maxNodeModuleJsDepth" => optional_int max_node_module_js_depth,
    "allowSyntheticDefaultImports" => tristate allow_synthetic_default_imports,
    "alwaysStrict" => tristate always_strict,
    "baseUrl" => string base_url,
    "downlevelIteration" => tristate downlevel_iteration,
    "esModuleInterop" => tristate es_module_interop,
    "outFile" => string out_file,
    "configFilePath" => string config_file_path,
    "noDtsResolution" => tristate no_dts_resolution,
    "pathsBasePath" => string paths_base_path,
    "diagnostics" => tristate diagnostics,
    "extendedDiagnostics" => tristate extended_diagnostics,
    "generateCpuProfile" => string generate_cpu_profile,
    "generateTrace" => string generate_trace,
    "listEmittedFiles" => tristate list_emitted_files,
    "listFiles" => tristate list_files,
    "explainFiles" => tristate explain_files,
    "listFilesOnly" => tristate list_files_only,
    "noEmitForJsFiles" => tristate no_emit_for_js_files,
    "preserveWatchOutput" => tristate preserve_watch_output,
    "pretty" => tristate pretty,
    "version" => tristate version,
    "watch" => tristate watch,
    "showConfig" => tristate show_config,
    "build" => tristate build,
    "help" => tristate help,
    "all" => tristate all,
    "runExternalCode" => tristate run_external_code,
    "pprofDir" => string pprof_dir,
    "singleThreaded" => tristate single_threaded,
    "quiet" => tristate quiet,
    "checkers" => optional_int checkers,
);

/// `json.Unmarshal` into `core.CompilerOptions`: a null is the zero value,
/// unknown names are skipped, each known name decodes into its field.
pub fn decode_compiler_options(
    options: &mut CompilerOptions,
    input: &mut tsr_json::Decoder<'_>,
) -> Result<(), Error> {
    if input.peek_kind() == tsr_json::Kind::Null {
        input.read_token()?;
        *options = CompilerOptions::default();
        return Ok(());
    }
    input.object(
        |name, input| match SETTERS.iter().find(|(json, _)| json.as_bytes() == name) {
            Some((_, set)) => set(options, input),
            None => input.skip_value(),
        },
    )
}

#[cfg(test)]
mod decode_tests {
    use super::*;

    #[test]
    fn setters_cover_every_encoded_field() {
        let encoded: Vec<_> = FIELDS.iter().map(|(name, _)| *name).collect();
        let decoded: Vec<_> = SETTERS.iter().map(|(name, _)| *name).collect();
        assert_eq!(encoded, decoded);
    }

    #[test]
    fn decoded_options_round_trip_through_the_encoder() {
        let text = br#"{"allowJs":true,"strict":false,"target":99,"lib":[],"paths":{"@a/*":["src/*"],"b":null},"maxNodeModuleJsDepth":2,"unknownOption":1,"customConditions":null}"#;
        let mut options = CompilerOptions::default();
        decode_compiler_options(&mut options, &mut tsr_json::Decoder::from_slice(text)).unwrap();
        assert_eq!(options.allow_js, Tristate::TRUE);
        assert_eq!(options.strict, Tristate::FALSE);
        assert_eq!(options.target.0, 99);
        assert_eq!(options.lib, Some(Vec::new()));
        assert_eq!(options.max_node_module_js_depth, Some(2));
        assert_eq!(options.custom_conditions, None);
        assert_eq!(
            marshal_compiler_options(&options).unwrap(),
            br#"{"allowJs":true,"lib":[],"paths":{"@a/*":["src/*"],"b":null},"strict":false,"target":99,"maxNodeModuleJsDepth":2}"#
        );
        let mut again = options.clone();
        decode_compiler_options(&mut again, &mut tsr_json::Decoder::from_slice(b"null")).unwrap();
        assert_eq!(again, CompilerOptions::default());
    }
}
