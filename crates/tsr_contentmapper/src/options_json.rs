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
