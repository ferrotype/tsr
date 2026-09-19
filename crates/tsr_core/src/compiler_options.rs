//! Immutable-input compiler option values from the pinned core package.
use crate::{ScriptTarget, Tristate};
use ts_jsstring::JsString;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKind(pub i32);
impl ModuleKind {
    pub const NONE: Self = Self(0);
    pub const COMMON_JS: Self = Self(1);
    pub const AMD: Self = Self(2);
    pub const UMD: Self = Self(3);
    pub const SYSTEM: Self = Self(4);
    pub const ES2015: Self = Self(5);
    pub const ES2020: Self = Self(6);
    pub const ES2022: Self = Self(7);
    pub const ESNEXT: Self = Self(99);
    pub const NODE16: Self = Self(100);
    pub const NODE18: Self = Self(101);
    pub const NODE20: Self = Self(102);
    pub const NODE_NEXT: Self = Self(199);
    pub const PRESERVE: Self = Self(200);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleResolutionKind(pub i32);
impl ModuleResolutionKind {
    pub const UNKNOWN: Self = Self(0);
    pub const CLASSIC: Self = Self(1);
    pub const NODE10: Self = Self(2);
    pub const NODE16: Self = Self(3);
    pub const NODE_NEXT: Self = Self(99);
    pub const BUNDLER: Self = Self(100);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleDetectionKind(pub i32);
impl ModuleDetectionKind {
    pub const NONE: Self = Self(0);
    pub const AUTO: Self = Self(1);
    pub const LEGACY: Self = Self(2);
    pub const FORCE: Self = Self(3);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsxEmit(pub i32);
impl JsxEmit {
    pub const NONE: Self = Self(0);
    pub const PRESERVE: Self = Self(1);
    pub const REACT: Self = Self(3);
    pub const REACT_NATIVE: Self = Self(2);
    pub const REACT_JSX: Self = Self(4);
    pub const REACT_JSX_DEV: Self = Self(5);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NewLineKind(pub i32);
impl NewLineKind {
    pub const NONE: Self = Self(0);
    pub const CRLF: Self = Self(1);
    pub const LF: Self = Self(2);
}
pub type ResolutionMode = ModuleKind;

pub type PathMappings = Vec<(JsString, Option<Vec<JsString>>)>;

/// Slices preserve nil versus nonnil empty; paths preserve insertion order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompilerOptions {
    pub allow_js: Tristate,
    pub allow_arbitrary_extensions: Tristate,
    pub allow_importing_ts_extensions: Tristate,
    pub allow_non_ts_extensions: Tristate,
    pub allow_umd_global_access: Tristate,
    pub allow_unreachable_code: Tristate,
    pub allow_unused_labels: Tristate,
    pub assume_changes_only_affect_direct_dependencies: Tristate,
    pub check_js: Tristate,
    pub custom_conditions: Option<Vec<JsString>>,
    pub composite: Tristate,
    pub emit_declaration_only: Tristate,
    pub emit_bom: Tristate,
    pub emit_decorator_metadata: Tristate,
    pub declaration: Tristate,
    pub declaration_dir: JsString,
    pub declaration_map: Tristate,
    pub deduplicate_packages: Tristate,
    pub disable_size_limit: Tristate,
    pub disable_source_of_project_reference_redirect: Tristate,
    pub disable_solution_searching: Tristate,
    pub disable_referenced_project_load: Tristate,
    pub erasable_syntax_only: Tristate,
    pub exact_optional_property_types: Tristate,
    pub experimental_decorators: Tristate,
    pub force_consistent_casing_in_file_names: Tristate,
    pub isolated_modules: Tristate,
    pub isolated_declarations: Tristate,
    pub ignore_config: Tristate,
    pub ignore_deprecations: JsString,
    pub import_helpers: Tristate,
    pub inline_source_map: Tristate,
    pub inline_sources: Tristate,
    pub init: Tristate,
    pub incremental: Tristate,
    pub jsx: JsxEmit,
    pub jsx_factory: JsString,
    pub jsx_fragment_factory: JsString,
    pub jsx_import_source: JsString,
    pub lib: Option<Vec<JsString>>,
    pub lib_replacement: Tristate,
    pub locale: JsString,
    pub map_root: JsString,
    pub module: ModuleKind,
    pub module_resolution: ModuleResolutionKind,
    pub module_suffixes: Option<Vec<JsString>>,
    pub module_detection: ModuleDetectionKind,
    pub new_line: NewLineKind,
    pub no_emit: Tristate,
    pub no_check: Tristate,
    pub no_error_truncation: Tristate,
    pub no_fallthrough_cases_in_switch: Tristate,
    pub no_implicit_any: Tristate,
    pub no_implicit_this: Tristate,
    pub no_implicit_returns: Tristate,
    pub no_emit_helpers: Tristate,
    pub no_lib: Tristate,
    pub no_property_access_from_index_signature: Tristate,
    pub no_unchecked_indexed_access: Tristate,
    pub no_emit_on_error: Tristate,
    pub no_unused_locals: Tristate,
    pub no_unused_parameters: Tristate,
    pub no_resolve: Tristate,
    pub no_implicit_override: Tristate,
    pub no_unchecked_side_effect_imports: Tristate,
    pub out_dir: JsString,
    pub paths: Option<PathMappings>,
    pub preserve_const_enums: Tristate,
    pub preserve_symlinks: Tristate,
    pub project: JsString,
    pub resolve_json_module: Tristate,
    pub resolve_package_json_exports: Tristate,
    pub resolve_package_json_imports: Tristate,
    pub remove_comments: Tristate,
    pub rewrite_relative_import_extensions: Tristate,
    pub react_namespace: JsString,
    pub root_dir: JsString,
    pub root_dirs: Option<Vec<JsString>>,
    pub skip_lib_check: Tristate,
    pub stable_type_ordering: Tristate,
    pub strict: Tristate,
    pub strict_bind_call_apply: Tristate,
    pub strict_builtin_iterator_return: Tristate,
    pub strict_function_types: Tristate,
    pub strict_null_checks: Tristate,
    pub strict_property_initialization: Tristate,
    pub strip_internal: Tristate,
    pub skip_default_lib_check: Tristate,
    pub source_map: Tristate,
    pub source_root: JsString,
    pub suppress_output_path_check: Tristate,
    pub target: ScriptTarget,
    pub trace_resolution: Tristate,
    pub ts_build_info_file: JsString,
    pub type_roots: Option<Vec<JsString>>,
    pub types: Option<Vec<JsString>>,
    pub use_define_for_class_fields: Tristate,
    pub use_unknown_in_catch_variables: Tristate,
    pub verbatim_module_syntax: Tristate,
    pub max_node_module_js_depth: Option<isize>,
    pub allow_synthetic_default_imports: Tristate,
    pub always_strict: Tristate,
    pub base_url: JsString,
    pub downlevel_iteration: Tristate,
    pub es_module_interop: Tristate,
    pub out_file: JsString,
    pub config_file_path: JsString,
    pub no_dts_resolution: Tristate,
    pub paths_base_path: JsString,
    pub diagnostics: Tristate,
    pub extended_diagnostics: Tristate,
    pub generate_cpu_profile: JsString,
    pub generate_trace: JsString,
    pub list_emitted_files: Tristate,
    pub list_files: Tristate,
    pub explain_files: Tristate,
    pub list_files_only: Tristate,
    pub no_emit_for_js_files: Tristate,
    pub preserve_watch_output: Tristate,
    pub pretty: Tristate,
    pub version: Tristate,
    pub watch: Tristate,
    pub show_config: Tristate,
    pub build: Tristate,
    pub help: Tristate,
    pub all: Tristate,
    pub run_external_code: Tristate,
    pub pprof_dir: JsString,
    pub single_threaded: Tristate,
    pub quiet: Tristate,
    pub checkers: Option<isize>,
}

impl CompilerOptions {
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEmitScriptTarget
    pub fn emit_script_target(&self) -> ScriptTarget {
        if self.target == ScriptTarget::NONE {
            ScriptTarget::LATEST_STANDARD
        } else {
            self.target
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEmitModuleKind
    pub fn emit_module_kind(&self) -> ModuleKind {
        if self.module != ModuleKind::NONE {
            return self.module;
        }
        let target = self.emit_script_target();
        if target == ScriptTarget::ESNEXT {
            ModuleKind::ESNEXT
        } else if target >= ScriptTarget::ES2022 {
            ModuleKind::ES2022
        } else if target >= ScriptTarget::ES2020 {
            ModuleKind::ES2020
        } else if target >= ScriptTarget::ES2015 {
            ModuleKind::ES2015
        } else {
            ModuleKind::COMMON_JS
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetModuleResolutionKind
    pub fn module_resolution_kind(&self) -> ModuleResolutionKind {
        if matches!(
            self.module_resolution,
            ModuleResolutionKind::UNKNOWN
                | ModuleResolutionKind::CLASSIC
                | ModuleResolutionKind::NODE10
        ) {
            match self.emit_module_kind() {
                ModuleKind::NODE16 | ModuleKind::NODE18 | ModuleKind::NODE20 => {
                    ModuleResolutionKind::NODE16
                }
                ModuleKind::NODE_NEXT => ModuleResolutionKind::NODE_NEXT,
                _ => ModuleResolutionKind::BUNDLER,
            }
        } else {
            self.module_resolution
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEmitModuleDetectionKind
    pub fn emit_module_detection_kind(&self) -> ModuleDetectionKind {
        if self.module_detection != ModuleDetectionKind::NONE {
            return self.module_detection;
        }
        let module = self.emit_module_kind();
        if ModuleKind::NODE16 <= module && module <= ModuleKind::NODE_NEXT {
            ModuleDetectionKind::FORCE
        } else {
            ModuleDetectionKind::AUTO
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetResolvePackageJsonExports
    pub fn resolve_package_json_exports(&self) -> bool {
        self.resolve_package_json_exports.is_true_or_unknown()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetResolvePackageJsonImports
    pub fn resolve_package_json_imports(&self) -> bool {
        self.resolve_package_json_imports.is_true_or_unknown()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetAllowImportingTsExtensions
    pub fn allow_importing_ts_extensions(&self) -> bool {
        self.allow_importing_ts_extensions.is_true()
            || self.rewrite_relative_import_extensions.is_true()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.AllowImportingTsExtensionsFrom
    pub fn allow_importing_ts_extensions_from(&self, file_name: &[u8]) -> bool {
        self.allow_importing_ts_extensions() || crate::path::is_declaration_file_name(file_name)
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetResolveJsonModule
    pub fn resolve_json_module(&self) -> bool {
        if self.resolve_json_module != Tristate::UNKNOWN {
            return self.resolve_json_module == Tristate::TRUE;
        }
        matches!(
            self.emit_module_kind(),
            ModuleKind::NODE20 | ModuleKind::NODE_NEXT
        ) || self.module_resolution_kind() == ModuleResolutionKind::BUNDLER
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.ShouldPreserveConstEnums
    pub fn should_preserve_const_enums(&self) -> bool {
        self.preserve_const_enums == Tristate::TRUE || self.isolated_modules()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetAllowJS
    pub fn allow_js(&self) -> bool {
        if self.allow_js == Tristate::UNKNOWN {
            self.check_js == Tristate::TRUE
        } else {
            self.allow_js == Tristate::TRUE
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetJSXTransformEnabled
    pub fn jsx_transform_enabled(&self) -> bool {
        matches!(
            self.jsx,
            JsxEmit::REACT | JsxEmit::REACT_JSX | JsxEmit::REACT_JSX_DEV
        )
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetStrictOptionValue
    pub fn strict_option_value(&self, value: Tristate) -> bool {
        if value == Tristate::UNKNOWN {
            self.strict != Tristate::FALSE
        } else {
            value == Tristate::TRUE
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.UsesWildcardTypes
    pub fn uses_wildcard_types(&self) -> bool {
        self.types
            .as_ref()
            .is_some_and(|types| types.iter().any(|name| name.as_bytes() == b"*"))
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetIsolatedModules
    pub fn isolated_modules(&self) -> bool {
        self.isolated_modules == Tristate::TRUE || self.verbatim_module_syntax == Tristate::TRUE
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.IsIncremental
    pub fn is_incremental(&self) -> bool {
        self.incremental.is_true() || self.composite.is_true()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEmitStandardClassFields
    pub fn emit_standard_class_fields(&self) -> bool {
        self.use_define_for_class_fields != Tristate::FALSE
            && self.emit_script_target() >= ScriptTarget::ES2022
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetUseDefineForClassFields
    pub fn use_define_for_class_fields(&self) -> bool {
        if self.use_define_for_class_fields == Tristate::UNKNOWN {
            self.emit_script_target() >= ScriptTarget::ES2022
        } else {
            self.use_define_for_class_fields == Tristate::TRUE
        }
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEmitDeclarations
    pub fn emit_declarations(&self) -> bool {
        self.declaration.is_true() || self.composite.is_true()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetAreDeclarationMapsEnabled
    pub fn declaration_maps_enabled(&self) -> bool {
        self.declaration_map == Tristate::TRUE && self.emit_declarations()
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.HasJsonModuleEmitEnabled
    pub fn json_module_emit_enabled(&self) -> bool {
        !matches!(
            self.emit_module_kind(),
            ModuleKind::SYSTEM | ModuleKind::UMD
        )
    }
    /// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetPathsBasePath
    pub fn paths_base_path<'a>(&'a self, current_directory: &'a [u8]) -> &'a [u8] {
        if self.paths.as_ref().is_none_or(Vec::is_empty) {
            b""
        } else if !self.paths_base_path.is_empty() {
            self.paths_base_path.as_bytes()
        } else {
            current_directory
        }
    }
}
impl ModuleKind {
    /// port: tsc/internal/core/compileroptions.go:ModuleKind.IsNonNodeESM
    pub fn is_non_node_esm(self) -> bool {
        Self::ES2015 <= self && self <= Self::ESNEXT
    }
    /// port: tsc/internal/core/compileroptions.go:ModuleKind.SupportsImportAttributes
    pub fn supports_import_attributes(self) -> bool {
        Self::NODE18 <= self && self <= Self::NODE_NEXT
            || self == Self::PRESERVE
            || self == Self::ESNEXT
    }
}
