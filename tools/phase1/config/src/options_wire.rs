//! Lossless typed transport to the original Go test renderer, not a JSON
//! implementation. Every public option field is sent, including zero and nil.
//! Go reflection rejects a missing/unknown field before invoking its serializer.
use serde_json::{json, Value};
use tsr_core::{collections::OrderedMap, BuildOptions, CompilerOptions};
use tsr_jsstring::JsString;
use tsr_tsoptions::ConfigValue;

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("String writer");
            s
        })
}
pub trait Wire {
    fn wire(&self) -> Value;
}
impl Wire for JsString {
    fn wire(&self) -> Value {
        json!(["string", hex(self.as_bytes())])
    }
}
impl<T: Wire> Wire for Option<T> {
    fn wire(&self) -> Value {
        self.as_ref().map_or_else(|| json!(["nil"]), Wire::wire)
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn wire(&self) -> Value {
        json!(["slice", self.iter().map(Wire::wire).collect::<Vec<_>>()])
    }
}
impl Wire for isize {
    fn wire(&self) -> Value {
        json!(["int", self])
    }
}
impl<T: Wire> Wire for OrderedMap<JsString, T> {
    fn wire(&self) -> Value {
        json!([
            "map",
            self.iter()
                .map(|(k, v)| json!([hex(k.as_bytes()), v.wire()]))
                .collect::<Vec<_>>()
        ])
    }
}
impl Wire for ConfigValue {
    fn wire(&self) -> Value {
        match self {
            Self::Null => json!(["nil"]),
            Self::EmptyStruct => json!(["struct", {}]),
            Self::Boolean(b) => json!(["bool", b]),
            Self::Number(n) => json!(["float", format!("{:016x}", n.to_bits())]),
            Self::Integer(n) => json!(["int", n]),
            Self::Enum(n) => json!(["int", n]),
            Self::String(s) => s.wire(),
            Self::Object(m) => m.wire(),
            Self::Array(None) => json!(["slice", null]),
            Self::Array(Some(a)) => a.wire(),
            Self::StringArray(a) => a.wire(),
            Self::UnorderedObject(_) => tsr_tsoptions::normalize_json_value(self.clone()).wire(),
        }
    }
}
pub fn compiler(options: &CompilerOptions) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("AllowJs".into(), json!(["int", options.allow_js.0]));
    fields.insert(
        "AllowArbitraryExtensions".into(),
        json!(["int", options.allow_arbitrary_extensions.0]),
    );
    fields.insert(
        "AllowImportingTsExtensions".into(),
        json!(["int", options.allow_importing_ts_extensions.0]),
    );
    fields.insert(
        "AllowNonTsExtensions".into(),
        json!(["int", options.allow_non_ts_extensions.0]),
    );
    fields.insert(
        "AllowUmdGlobalAccess".into(),
        json!(["int", options.allow_umd_global_access.0]),
    );
    fields.insert(
        "AllowUnreachableCode".into(),
        json!(["int", options.allow_unreachable_code.0]),
    );
    fields.insert(
        "AllowUnusedLabels".into(),
        json!(["int", options.allow_unused_labels.0]),
    );
    fields.insert(
        "AssumeChangesOnlyAffectDirectDependencies".into(),
        json!([
            "int",
            options.assume_changes_only_affect_direct_dependencies.0
        ]),
    );
    fields.insert("CheckJs".into(), json!(["int", options.check_js.0]));
    fields.insert(
        "CustomConditions".into(),
        options
            .custom_conditions
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert("Composite".into(), json!(["int", options.composite.0]));
    fields.insert(
        "EmitDeclarationOnly".into(),
        json!(["int", options.emit_declaration_only.0]),
    );
    fields.insert("EmitBOM".into(), json!(["int", options.emit_bom.0]));
    fields.insert(
        "EmitDecoratorMetadata".into(),
        json!(["int", options.emit_decorator_metadata.0]),
    );
    fields.insert("Declaration".into(), json!(["int", options.declaration.0]));
    fields.insert("DeclarationDir".into(), options.declaration_dir.wire());
    fields.insert(
        "DeclarationMap".into(),
        json!(["int", options.declaration_map.0]),
    );
    fields.insert(
        "DeduplicatePackages".into(),
        json!(["int", options.deduplicate_packages.0]),
    );
    fields.insert(
        "DisableSizeLimit".into(),
        json!(["int", options.disable_size_limit.0]),
    );
    fields.insert(
        "DisableSourceOfProjectReferenceRedirect".into(),
        json!([
            "int",
            options.disable_source_of_project_reference_redirect.0
        ]),
    );
    fields.insert(
        "DisableSolutionSearching".into(),
        json!(["int", options.disable_solution_searching.0]),
    );
    fields.insert(
        "DisableReferencedProjectLoad".into(),
        json!(["int", options.disable_referenced_project_load.0]),
    );
    fields.insert(
        "ErasableSyntaxOnly".into(),
        json!(["int", options.erasable_syntax_only.0]),
    );
    fields.insert(
        "ExactOptionalPropertyTypes".into(),
        json!(["int", options.exact_optional_property_types.0]),
    );
    fields.insert(
        "ExperimentalDecorators".into(),
        json!(["int", options.experimental_decorators.0]),
    );
    fields.insert(
        "ForceConsistentCasingInFileNames".into(),
        json!(["int", options.force_consistent_casing_in_file_names.0]),
    );
    fields.insert(
        "IsolatedModules".into(),
        json!(["int", options.isolated_modules.0]),
    );
    fields.insert(
        "IsolatedDeclarations".into(),
        json!(["int", options.isolated_declarations.0]),
    );
    fields.insert(
        "IgnoreConfig".into(),
        json!(["int", options.ignore_config.0]),
    );
    fields.insert(
        "IgnoreDeprecations".into(),
        options.ignore_deprecations.wire(),
    );
    fields.insert(
        "ImportHelpers".into(),
        json!(["int", options.import_helpers.0]),
    );
    fields.insert(
        "InlineSourceMap".into(),
        json!(["int", options.inline_source_map.0]),
    );
    fields.insert(
        "InlineSources".into(),
        json!(["int", options.inline_sources.0]),
    );
    fields.insert("Init".into(), json!(["int", options.init.0]));
    fields.insert("Incremental".into(), json!(["int", options.incremental.0]));
    fields.insert("Jsx".into(), json!(["int", options.jsx.0]));
    fields.insert("JsxFactory".into(), options.jsx_factory.wire());
    fields.insert(
        "JsxFragmentFactory".into(),
        options.jsx_fragment_factory.wire(),
    );
    fields.insert("JsxImportSource".into(), options.jsx_import_source.wire());
    fields.insert(
        "Lib".into(),
        options
            .lib
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert(
        "LibReplacement".into(),
        json!(["int", options.lib_replacement.0]),
    );
    fields.insert("Locale".into(), options.locale.wire());
    fields.insert("MapRoot".into(), options.map_root.wire());
    fields.insert("Module".into(), json!(["int", options.module.0]));
    fields.insert(
        "ModuleResolution".into(),
        json!(["int", options.module_resolution.0]),
    );
    fields.insert(
        "ModuleSuffixes".into(),
        options
            .module_suffixes
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert(
        "ModuleDetection".into(),
        json!(["int", options.module_detection.0]),
    );
    fields.insert("NewLine".into(), json!(["int", options.new_line.0]));
    fields.insert("NoEmit".into(), json!(["int", options.no_emit.0]));
    fields.insert("NoCheck".into(), json!(["int", options.no_check.0]));
    fields.insert(
        "NoErrorTruncation".into(),
        json!(["int", options.no_error_truncation.0]),
    );
    fields.insert(
        "NoFallthroughCasesInSwitch".into(),
        json!(["int", options.no_fallthrough_cases_in_switch.0]),
    );
    fields.insert(
        "NoImplicitAny".into(),
        json!(["int", options.no_implicit_any.0]),
    );
    fields.insert(
        "NoImplicitThis".into(),
        json!(["int", options.no_implicit_this.0]),
    );
    fields.insert(
        "NoImplicitReturns".into(),
        json!(["int", options.no_implicit_returns.0]),
    );
    fields.insert(
        "NoEmitHelpers".into(),
        json!(["int", options.no_emit_helpers.0]),
    );
    fields.insert("NoLib".into(), json!(["int", options.no_lib.0]));
    fields.insert(
        "NoPropertyAccessFromIndexSignature".into(),
        json!(["int", options.no_property_access_from_index_signature.0]),
    );
    fields.insert(
        "NoUncheckedIndexedAccess".into(),
        json!(["int", options.no_unchecked_indexed_access.0]),
    );
    fields.insert(
        "NoEmitOnError".into(),
        json!(["int", options.no_emit_on_error.0]),
    );
    fields.insert(
        "NoUnusedLocals".into(),
        json!(["int", options.no_unused_locals.0]),
    );
    fields.insert(
        "NoUnusedParameters".into(),
        json!(["int", options.no_unused_parameters.0]),
    );
    fields.insert("NoResolve".into(), json!(["int", options.no_resolve.0]));
    fields.insert(
        "NoImplicitOverride".into(),
        json!(["int", options.no_implicit_override.0]),
    );
    fields.insert(
        "NoUncheckedSideEffectImports".into(),
        json!(["int", options.no_unchecked_side_effect_imports.0]),
    );
    fields.insert("OutDir".into(), options.out_dir.wire());
    fields.insert(
        "Paths".into(),
        options.paths.as_ref().map_or_else(
            || json!(["nil"]),
            |map| {
                json!([
                    "map",
                    map.iter()
                        .map(|(k, v)| json!([
                            hex(k.as_bytes()),
                            v.as_ref()
                                .map_or_else(|| json!(["slice", null]), Wire::wire)
                        ]))
                        .collect::<Vec<_>>()
                ])
            },
        ),
    );
    fields.insert(
        "PreserveConstEnums".into(),
        json!(["int", options.preserve_const_enums.0]),
    );
    fields.insert(
        "PreserveSymlinks".into(),
        json!(["int", options.preserve_symlinks.0]),
    );
    fields.insert("Project".into(), options.project.wire());
    fields.insert(
        "ResolveJsonModule".into(),
        json!(["int", options.resolve_json_module.0]),
    );
    fields.insert(
        "ResolvePackageJsonExports".into(),
        json!(["int", options.resolve_package_json_exports.0]),
    );
    fields.insert(
        "ResolvePackageJsonImports".into(),
        json!(["int", options.resolve_package_json_imports.0]),
    );
    fields.insert(
        "RemoveComments".into(),
        json!(["int", options.remove_comments.0]),
    );
    fields.insert(
        "RewriteRelativeImportExtensions".into(),
        json!(["int", options.rewrite_relative_import_extensions.0]),
    );
    fields.insert("ReactNamespace".into(), options.react_namespace.wire());
    fields.insert("RootDir".into(), options.root_dir.wire());
    fields.insert(
        "RootDirs".into(),
        options
            .root_dirs
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert(
        "SkipLibCheck".into(),
        json!(["int", options.skip_lib_check.0]),
    );
    fields.insert(
        "StableTypeOrdering".into(),
        json!(["int", options.stable_type_ordering.0]),
    );
    fields.insert("Strict".into(), json!(["int", options.strict.0]));
    fields.insert(
        "StrictBindCallApply".into(),
        json!(["int", options.strict_bind_call_apply.0]),
    );
    fields.insert(
        "StrictBuiltinIteratorReturn".into(),
        json!(["int", options.strict_builtin_iterator_return.0]),
    );
    fields.insert(
        "StrictFunctionTypes".into(),
        json!(["int", options.strict_function_types.0]),
    );
    fields.insert(
        "StrictNullChecks".into(),
        json!(["int", options.strict_null_checks.0]),
    );
    fields.insert(
        "StrictPropertyInitialization".into(),
        json!(["int", options.strict_property_initialization.0]),
    );
    fields.insert(
        "StripInternal".into(),
        json!(["int", options.strip_internal.0]),
    );
    fields.insert(
        "SkipDefaultLibCheck".into(),
        json!(["int", options.skip_default_lib_check.0]),
    );
    fields.insert("SourceMap".into(), json!(["int", options.source_map.0]));
    fields.insert("SourceRoot".into(), options.source_root.wire());
    fields.insert(
        "SuppressOutputPathCheck".into(),
        json!(["int", options.suppress_output_path_check.0]),
    );
    fields.insert("Target".into(), json!(["int", options.target.0]));
    fields.insert(
        "TraceResolution".into(),
        json!(["int", options.trace_resolution.0]),
    );
    fields.insert("TsBuildInfoFile".into(), options.ts_build_info_file.wire());
    fields.insert(
        "TypeRoots".into(),
        options
            .type_roots
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert(
        "Types".into(),
        options
            .types
            .as_ref()
            .map_or_else(|| json!(["slice", null]), Wire::wire),
    );
    fields.insert(
        "UseDefineForClassFields".into(),
        json!(["int", options.use_define_for_class_fields.0]),
    );
    fields.insert(
        "UseUnknownInCatchVariables".into(),
        json!(["int", options.use_unknown_in_catch_variables.0]),
    );
    fields.insert(
        "VerbatimModuleSyntax".into(),
        json!(["int", options.verbatim_module_syntax.0]),
    );
    fields.insert(
        "MaxNodeModuleJsDepth".into(),
        options.max_node_module_js_depth.wire(),
    );
    fields.insert(
        "AllowSyntheticDefaultImports".into(),
        json!(["int", options.allow_synthetic_default_imports.0]),
    );
    fields.insert(
        "AlwaysStrict".into(),
        json!(["int", options.always_strict.0]),
    );
    fields.insert("BaseUrl".into(), options.base_url.wire());
    fields.insert(
        "DownlevelIteration".into(),
        json!(["int", options.downlevel_iteration.0]),
    );
    fields.insert(
        "ESModuleInterop".into(),
        json!(["int", options.es_module_interop.0]),
    );
    fields.insert("OutFile".into(), options.out_file.wire());
    fields.insert("ConfigFilePath".into(), options.config_file_path.wire());
    fields.insert(
        "NoDtsResolution".into(),
        json!(["int", options.no_dts_resolution.0]),
    );
    fields.insert("PathsBasePath".into(), options.paths_base_path.wire());
    fields.insert("Diagnostics".into(), json!(["int", options.diagnostics.0]));
    fields.insert(
        "ExtendedDiagnostics".into(),
        json!(["int", options.extended_diagnostics.0]),
    );
    fields.insert(
        "GenerateCpuProfile".into(),
        options.generate_cpu_profile.wire(),
    );
    fields.insert("GenerateTrace".into(), options.generate_trace.wire());
    fields.insert(
        "ListEmittedFiles".into(),
        json!(["int", options.list_emitted_files.0]),
    );
    fields.insert("ListFiles".into(), json!(["int", options.list_files.0]));
    fields.insert(
        "ExplainFiles".into(),
        json!(["int", options.explain_files.0]),
    );
    fields.insert(
        "ListFilesOnly".into(),
        json!(["int", options.list_files_only.0]),
    );
    fields.insert(
        "NoEmitForJsFiles".into(),
        json!(["int", options.no_emit_for_js_files.0]),
    );
    fields.insert(
        "PreserveWatchOutput".into(),
        json!(["int", options.preserve_watch_output.0]),
    );
    fields.insert("Pretty".into(), json!(["int", options.pretty.0]));
    fields.insert("Version".into(), json!(["int", options.version.0]));
    fields.insert("Watch".into(), json!(["int", options.watch.0]));
    fields.insert("ShowConfig".into(), json!(["int", options.show_config.0]));
    fields.insert("Build".into(), json!(["int", options.build.0]));
    fields.insert("Help".into(), json!(["int", options.help.0]));
    fields.insert("All".into(), json!(["int", options.all.0]));
    fields.insert(
        "RunExternalCode".into(),
        json!(["int", options.run_external_code.0]),
    );
    fields.insert("PprofDir".into(), options.pprof_dir.wire());
    fields.insert(
        "SingleThreaded".into(),
        json!(["int", options.single_threaded.0]),
    );
    fields.insert("Quiet".into(), json!(["int", options.quiet.0]));
    fields.insert("Checkers".into(), options.checkers.wire());
    json!(["struct", fields])
}
pub fn build(options: &BuildOptions) -> Value {
    json!(["struct", {
        "Dry": json!(["int", options.dry.0]),
        "Force": json!(["int", options.force.0]),
        "Verbose": json!(["int", options.verbose.0]),
        "Builders": options.builders.wire(),
        "StopBuildOnErrors": json!(["int", options.stop_build_on_errors.0]),
        "Clean": json!(["int", options.clean.0]),
    }])
}
