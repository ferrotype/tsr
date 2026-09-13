//! Package maps preserve ordered conditions and the source's distinction between
//! continuing a search (`None`) and explicitly blocking it (`Some(unresolved)`).
use crate::resolver::{extension, parse_package_name, truthy, DTS, JS, JSON, TS};
use crate::trace::trace;
use crate::{get_conditions, Error, PackageJson, ResolvedModule, Resolver};
use serde_json::{Map, Value};
use ts_core::{CompilerOptions, ModuleKind, ModuleResolutionKind, TextRange};
use ts_diagnostics as diagnostics;
use ts_jsstring::JsString;
use ts_tspath as path;

#[derive(Clone)]
pub(super) struct Context {
    pub esm: bool,
    pub exports: bool,
    pub imports: bool,
    pub imports_pattern_root: bool,
    pub extensions: u8,
    pub conditions: Vec<JsString>,
}
impl Context {
    pub fn new(options: &CompilerOptions, mode: ModuleKind) -> Self {
        let resolution = options.module_resolution_kind();
        // Pinned newResolutionState applies boolean feature overrides only to
        // Bundler; Node16/NodeNext select their fixed feature sets directly.
        Self {
            esm: resolution != ModuleResolutionKind::BUNDLER && mode == ModuleKind::ESNEXT,
            exports: resolution != ModuleResolutionKind::BUNDLER
                || options.resolve_package_json_exports(),
            imports: resolution != ModuleResolutionKind::BUNDLER
                || options.resolve_package_json_imports(),
            imports_pattern_root: resolution != ModuleResolutionKind::NODE16,
            extensions: TS
                | JS
                | if options.no_dts_resolution.is_true() {
                    0
                } else {
                    DTS
                }
                | if options.resolve_json_module() {
                    JSON
                } else {
                    0
                },
            conditions: get_conditions(options, mode),
        }
    }
}
impl Resolver {
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromSelfNameReference
    pub(super) fn self_name(
        &mut self,
        name: &[u8],
        directory: &[u8],
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        let Some(scope) = self.package_scope(directory)? else {
            return Ok(None);
        };
        if !scope.contents.get("exports").is_some_and(truthy) {
            return Ok(None);
        }
        let Some(package_name) = scope.string("name") else {
            return Ok(None);
        };
        let subpath = if name == package_name {
            b".".to_vec()
        } else if let Some(rest) = name
            .strip_prefix(package_name)
            .and_then(|s| s.strip_prefix(b"/"))
        {
            path::combine(b".", &[rest])
        } else {
            return Ok(None);
        };
        if self.options.allow_js() && !directory.windows(14).any(|w| w == b"/node_modules/") {
            return self.exports(&scope, context.extensions, &subpath, context);
        }
        if let Some(result) =
            self.exports(&scope, context.extensions & (TS | DTS), &subpath, context)?
        {
            return Ok(Some(result));
        }
        self.exports(&scope, context.extensions & !(TS | DTS), &subpath, context)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromImports
    pub(super) fn package_imports(
        &mut self,
        name: &[u8],
        directory: &[u8],
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        if name == b"#" || name.starts_with(b"#/") && !context.imports_pattern_root {
            trace!(
                self,
                diagnostics::Invalid_import_specifier_0_has_no_possible_resolutions,
                name
            );
            return Ok(None);
        }
        let Some(scope) = self.package_scope(directory)? else {
            trace!(self,diagnostics::Directory_0_has_no_containing_package_json_scope_Imports_will_not_resolve,directory);
            return Ok(None);
        };
        let Some(table) = scope.contents.get("imports").and_then(Value::as_object) else {
            trace!(
                self,
                diagnostics::X_package_json_scope_0_has_no_imports_defined,
                &scope.directory
            );
            return Ok(None);
        };
        let result = self.map_entries(context.extensions, name, table, &scope, true, context)?;
        if result.is_none() {
            trace!(
                self,
                diagnostics::Import_specifier_0_does_not_exist_in_package_json_scope_at_path_1,
                name,
                &scope.directory
            );
        }
        Ok(result)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromExports
    fn exports(
        &mut self,
        scope: &PackageJson,
        ext: u8,
        subpath: &[u8],
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        let Some(exports) = scope.contents.get("exports").filter(|v| truthy(v)) else {
            return Ok(None);
        };
        if subpath == b"." {
            let main = match exports {
                Value::String(_) | Value::Array(_) => Some(exports),
                Value::Object(table) if table.keys().all(|k| !k.starts_with('.')) => Some(exports),
                Value::Object(table) => table.get("."),
                _ => None,
            };
            if let Some(main) = main {
                return self
                    .map_target(ext, scope, false, main, b"", false, subpath, b".", context);
            }
        } else if let Value::Object(table) = exports {
            if table.keys().all(|k| k.starts_with('.')) {
                let result = self.map_entries(ext, subpath, table, scope, false, context)?;
                if result.is_some() {
                    return Ok(result);
                }
            }
        }
        trace!(
            self,
            diagnostics::Export_specifier_0_does_not_exist_in_package_json_scope_at_path_1,
            subpath,
            &scope.directory
        );
        Ok(None)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromExportsOrImports
    fn map_entries(
        &mut self,
        ext: u8,
        name: &[u8],
        table: &Map<String, Value>,
        scope: &PackageJson,
        imports: bool,
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        if !name.ends_with(b"/") && !name.contains(&b'*') {
            if let Some(target) = table
                .iter()
                .find_map(|(key, value)| (key.as_bytes() == name).then_some(value))
            {
                return self
                    .map_target(ext, scope, imports, target, b"", false, name, name, context);
            }
        }
        let mut keys: Vec<_> = table
            .keys()
            .filter(|key| key.bytes().filter(|&b| b == b'*').count() == 1 || key.ends_with('/'))
            .collect();
        keys.sort_by(|a, b| compare_pattern_keys(a.as_bytes(), b.as_bytes()));
        for key in keys {
            let pattern = key.as_bytes();
            if let Some(star) = pattern.iter().position(|&c| c == b'*') {
                let prefix = &pattern[..star];
                let suffix = &pattern[star + 1..];
                if name.len() >= prefix.len() + suffix.len()
                    && name.starts_with(prefix)
                    && name.ends_with(suffix)
                {
                    return self.map_target(
                        ext,
                        scope,
                        imports,
                        &table[key],
                        &name[prefix.len()..name.len() - suffix.len()],
                        true,
                        name,
                        pattern,
                        context,
                    );
                }
            } else if let Some(rest) = name.strip_prefix(pattern) {
                return self.map_target(
                    ext,
                    scope,
                    imports,
                    &table[key],
                    rest,
                    false,
                    name,
                    pattern,
                    context,
                );
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromTargetExportOrImport
    // Recursive target state mirrors one ordered package map branch; every argument affects fallback semantics.
    #[allow(clippy::too_many_arguments)]
    fn map_target(
        &mut self,
        ext: u8,
        scope: &PackageJson,
        imports: bool,
        target: &Value,
        subpath: &[u8],
        pattern: bool,
        module_name: &[u8],
        key: &[u8],
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        match target {
            Value::Null => {
                trace!(
                    self,
                    diagnostics::X_package_json_scope_0_explicitly_maps_specifier_1_to_null,
                    &scope.directory,
                    module_name
                );
                Ok(Some(ResolvedModule::default()))
            }
            Value::Array(array) => {
                for target in array {
                    if let Some(result) = self.map_target(
                        ext,
                        scope,
                        imports,
                        target,
                        subpath,
                        pattern,
                        module_name,
                        key,
                        context,
                    )? {
                        return Ok(Some(result));
                    }
                }
                trace!(
                    self,
                    diagnostics::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                    &scope.directory,
                    module_name
                );
                Ok(None)
            }
            Value::Object(table) => {
                trace!(self, diagnostics::Entering_conditional_exports);
                for (condition, target) in table {
                    let matches = condition == "default"
                        || context
                            .conditions
                            .iter()
                            .any(|c| c.as_bytes() == condition.as_bytes())
                        || context.conditions.iter().any(|c| c.as_bytes() == b"types")
                            && is_applicable_versioned_types_key(condition.as_bytes());
                    if matches {
                        trace!(
                            self,
                            diagnostics::Matched_0_condition_1,
                            if imports { "imports" } else { "exports" },
                            condition.as_str()
                        );
                        if let Some(result) = self.map_target(
                            ext,
                            scope,
                            imports,
                            target,
                            subpath,
                            pattern,
                            module_name,
                            key,
                            context,
                        )? {
                            if result.is_resolved() {
                                trace!(
                                    self,
                                    diagnostics::Resolved_under_condition_0,
                                    condition.as_str()
                                );
                            }
                            trace!(self, diagnostics::Exiting_conditional_exports);
                            return Ok(Some(result));
                        }
                        trace!(
                            self,
                            diagnostics::Failed_to_resolve_under_condition_0,
                            condition.as_str()
                        );
                    } else {
                        trace!(
                            self,
                            diagnostics::Saw_non_matching_condition_0,
                            condition.as_str()
                        );
                    }
                }
                trace!(self, diagnostics::Exiting_conditional_exports);
                Ok(None)
            }
            Value::String(target) => {
                let target = target.as_bytes();
                if !pattern && !subpath.is_empty() && !target.ends_with(b"/") {
                    trace!(self,diagnostics::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,&scope.directory,module_name);
                    return Ok(None);
                }
                if !target.starts_with(b"./") {
                    if imports
                        && !target.starts_with(b"../")
                        && !target.starts_with(b"/")
                        && path::root_length(target) == 0
                    {
                        let combined = substitute(target, subpath, pattern);
                        let mut directory = scope.directory.as_bytes().to_vec();
                        directory.push(b'/');
                        trace!(
                            self,
                            diagnostics::Using_0_subpath_1_with_target_2,
                            "imports",
                            key,
                            &combined
                        );
                        trace!(
                            self,
                            diagnostics::Resolving_module_0_from_1,
                            &combined,
                            &directory
                        );
                        trace!(
                            self,
                            diagnostics::Resolving_in_0_mode_with_conditions_1,
                            if context.esm { b"ESM" } else { b"CJS" },
                            crate::trace::conditions(&context.conditions)
                        );
                        let result = self.resolve_worker(&combined, &directory, context)?;
                        return Ok(result.is_resolved().then_some(result));
                    }
                    trace!(self,diagnostics::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,&scope.directory,module_name);
                    return Ok(None);
                }
                if target[2..].split(|&c| c == b'/').any(invalid_part)
                    || subpath.split(|&c| c == b'/').any(invalid_part)
                {
                    trace!(self,diagnostics::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,&scope.directory,module_name);
                    return Ok(None);
                }
                trace!(
                    self,
                    diagnostics::Using_0_subpath_1_with_target_2,
                    if imports { "imports" } else { "exports" },
                    key,
                    substitute(target, subpath, pattern)
                );
                let resolved = path::combine(scope.directory.as_bytes(), &[target]);
                let final_path = path::absolute(
                    &substitute(&resolved, subpath, pattern),
                    self.cwd.as_bytes(),
                );
                let mut result =
                    self.input_file_for_path(&final_path, subpath, scope, imports, context)?;
                if result.is_none() {
                    result = self.package_field(ext, &final_path, target)?;
                }
                if let Some(found) = &mut result {
                    found.package_id =
                        self.package_identity(found.resolved_file_name.as_bytes(), scope)?;
                }
                Ok(result)
            }
            _ => {
                trace!(
                    self,
                    diagnostics::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                    &scope.directory,
                    module_name
                );
                Ok(None)
            }
        }
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadFileNameFromPackageJSONField
    pub(super) fn package_field(
        &mut self,
        ext: u8,
        candidate: &[u8],
        value: &[u8],
    ) -> Result<Option<ResolvedModule>, Error> {
        let suffix = extension(candidate);
        if ext & TS != 0 && matches!(suffix, b".ts" | b".tsx" | b".mts" | b".cts")
            || ext & DTS != 0 && path::is_declaration_file_name(candidate)
        {
            return Ok(self.lookup(candidate)?.map(|filename| {
                let suffix = JsString::from_bytes(extension(filename.as_bytes()));
                ResolvedModule {
                    resolved_file_name: filename,
                    resolved_using_ts_extension: value.ends_with(b"*") && !suffix.is_empty(),
                    extension: suffix,
                    ..Default::default()
                }
            }));
        }
        if self.config_lookup && ext & JSON != 0 && suffix == b".json" {
            if let Some(filename) = self.lookup(candidate)? {
                return Ok(Some(ResolvedModule {
                    resolved_file_name: filename,
                    extension: JsString::from_bytes(b".json".as_slice()),
                    ..Default::default()
                }));
            }
        }
        // NoImplicitExtensions is independent of the request's ESM mode.
        self.file(ext, candidate, true, false)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.tryLoadInputFileForPath
    fn input_file_for_path(
        &mut self,
        final_path: &[u8],
        entry: &[u8],
        scope: &PackageJson,
        imports: bool,
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        if self.config_lookup
            || self.options.declaration_dir.is_empty() && self.options.out_dir.is_empty()
            || final_path.windows(14).any(|w| w == b"/node_modules/")
            || !self.options.config_file_path.is_empty()
                && !self.contains_path(
                    scope.directory.as_bytes(),
                    self.options.config_file_path.as_bytes(),
                )
        {
            return Ok(None);
        }
        let root = if !self.options.root_dir.is_empty() {
            self.options.root_dir.as_bytes().to_vec()
        } else if !self.options.config_file_path.is_empty() {
            path::directory(self.options.config_file_path.as_bytes())
        } else {
            let message = if imports {
                ts_diagnostics::The_project_root_is_ambiguous_but_is_required_to_resolve_import_map_entry_0_in_file_1_Supply_the_rootDir_compiler_option_to_disambiguate
            } else {
                ts_diagnostics::The_project_root_is_ambiguous_but_is_required_to_resolve_export_map_entry_0_in_file_1_Supply_the_rootDir_compiler_option_to_disambiguate
            };
            return Ok(Some(ResolvedModule {
                resolution_diagnostics: vec![ts_ast::Diagnostic::new(
                    None,
                    TextRange::default(),
                    message,
                    vec![
                        JsString::from_bytes(if entry.is_empty() {
                            b".".as_slice()
                        } else {
                            entry
                        }),
                        JsString::from_bytes(path::combine(
                            scope.directory.as_bytes(),
                            &[b"package.json"],
                        )),
                    ],
                )],
                ..Default::default()
            }));
        };
        let base = if self.options.config_file_path.is_empty() {
            root.as_slice()
        } else {
            self.cwd.as_bytes()
        };
        let mut outputs = Vec::new();
        if !self.options.declaration_dir.is_empty() {
            outputs.push(path::absolute(
                &path::combine(base, &[self.options.declaration_dir.as_bytes()]),
                self.cwd.as_bytes(),
            ));
        }
        if !self.options.out_dir.is_empty() && self.options.out_dir != self.options.declaration_dir
        {
            outputs.push(path::absolute(
                &path::combine(base, &[self.options.out_dir.as_bytes()]),
                self.cwd.as_bytes(),
            ));
        }
        for output in outputs {
            if !self.contains_path(&output, final_path) {
                continue;
            }
            let fragment = if final_path.len() > output.len() {
                &final_path[output.len() + 1..]
            } else {
                b""
            };
            let input = path::combine(&root, &[fragment]);
            let suffix = extension(&input);
            if !matches!(
                suffix,
                b".mjs" | b".cjs" | b".js" | b".json" | b".d.mts" | b".d.cts" | b".d.ts"
            ) {
                continue;
            }
            let candidates: &[(&[u8], u8)] = match suffix {
                b".mjs" | b".d.mts" => &[(b".mts", TS), (b".mjs", JS)],
                b".cjs" | b".d.cts" => &[(b".cts", TS), (b".cjs", JS)],
                _ => &[(b".tsx", TS), (b".ts", TS), (b".jsx", JS), (b".js", JS)],
            };
            for &(candidate, mask) in candidates {
                if context.extensions & mask == 0 {
                    continue;
                }
                let mut filename = input[..input.len() - suffix.len()].to_vec();
                filename.extend_from_slice(candidate);
                if self.host.file_exists(&filename)? {
                    if let Some(result) = self.package_field(context.extensions, &filename, b"")? {
                        return Ok(Some(result));
                    }
                }
            }
        }
        Ok(None)
    }
    fn contains_path(&self, parent: &[u8], child: &[u8]) -> bool {
        let parent = path::to_path(
            parent,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        let child = path::to_path(
            child,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        child == parent
            || child
                .as_bytes()
                .strip_prefix(parent.as_bytes())
                .is_some_and(|rest| parent.as_bytes().ends_with(b"/") || rest.starts_with(b"/"))
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromSpecificNodeModulesDirectory
    pub(super) fn package(
        &mut self,
        ext: u8,
        name: &[u8],
        node_modules: &[u8],
        context: &Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        let candidate = path::absolute(&path::combine(node_modules, &[name]), b"");
        let candidate = candidate.strip_suffix(b"/").unwrap_or(&candidate);
        let (package_name, rest) = parse_package_name(name);
        let directory = if package_name.is_empty() {
            candidate.to_vec()
        } else {
            path::combine(node_modules, &[package_name])
        };
        if self.package_directory_only {
            return Ok(self
                .host
                .directory_exists(&directory)?
                .then(|| ResolvedModule {
                    resolved_file_name: JsString::from_bytes(directory),
                    ..Default::default()
                }));
        }
        let nested = self.package_json(candidate)?;
        let package = if rest.is_empty() {
            nested.clone()
        } else {
            self.package_json(&directory)?
        };
        if !rest.is_empty()
            && nested.is_some()
            && (!context.exports
                || package
                    .as_ref()
                    .is_none_or(|p| p.contents.get("exports").is_none()))
        {
            if let Some(result) = self.file(ext, candidate, context.esm, false)? {
                return Ok(Some(result));
            }
            let info = nested.as_deref();
            if let Some(mut result) = self.directory(ext, candidate, info, context.esm)? {
                if let Some(info) = info {
                    result.package_id =
                        self.package_identity(result.resolved_file_name.as_bytes(), info)?;
                }
                return Ok(Some(result));
            }
        }
        if let Some(info) = &package {
            if context.exports && info.contents.get("exports").is_some_and(truthy) {
                return self.exports(info, ext, &path::combine(b".", &[rest]), context);
            }
            if !rest.is_empty() {
                if let Some((version, paths)) = self.version_paths(info) {
                    trace!(self,diagnostics::X_package_json_has_a_typesVersions_entry_0_that_matches_compiler_version_1_looking_for_a_pattern_to_match_module_name_2,version,b"7.1.0-dev",rest);
                    if let Some(result) = self.paths_using(
                        rest,
                        &directory,
                        paths,
                        ext,
                        |resolver, ext, candidate, from_config| {
                            let mut result =
                                resolver.file(ext, candidate, context.esm, from_config)?;
                            if result.is_none() {
                                result =
                                    resolver.directory(ext, candidate, Some(info), context.esm)?;
                            }
                            if let Some(result) = &mut result {
                                result.package_id = resolver
                                    .package_identity(result.resolved_file_name.as_bytes(), info)?;
                            }
                            Ok(result)
                        },
                    )? {
                        return Ok(Some(result));
                    }
                }
            }
        }
        let mut result = if !rest.is_empty() || !context.esm {
            self.file(ext, candidate, context.esm, false)?
        } else {
            None
        };
        if result.is_none() {
            result = self.directory(ext, candidate, package.as_deref(), context.esm)?;
        }
        if result.is_none()
            && rest.is_empty()
            && context.esm
            && package
                .as_ref()
                .is_some_and(|p| p.contents.get("exports").is_none_or(Value::is_null))
        {
            result = self.file(
                ext,
                &path::combine(candidate, &[b"index.js"]),
                context.esm,
                false,
            )?;
        }
        if let (Some(result), Some(info)) = (&mut result, &package) {
            result.package_id =
                self.package_identity(result.resolved_file_name.as_bytes(), info)?;
        }
        Ok(result)
    }
}
fn invalid_part(part: &[u8]) -> bool {
    matches!(part, b"." | b".." | b"node_modules")
}
fn substitute(target: &[u8], subpath: &[u8], pattern: bool) -> Vec<u8> {
    if !pattern {
        let mut result = target.to_vec();
        result.extend_from_slice(subpath);
        return result;
    }
    let mut result = Vec::new();
    for (index, part) in target.split(|&b| b == b'*').enumerate() {
        if index != 0 {
            result.extend_from_slice(subpath);
        }
        result.extend_from_slice(part);
    }
    result
}
/// port: tsc/internal/module/util.go:ComparePatternKeys
fn compare_pattern_keys(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let star_a = a.iter().position(|&c| c == b'*');
    let star_b = b.iter().position(|&c| c == b'*');
    let base_a = star_a.map_or(a.len(), |i| i + 1);
    let base_b = star_b.map_or(b.len(), |i| i + 1);
    base_b
        .cmp(&base_a)
        .then_with(|| star_b.is_some().cmp(&star_a.is_some()))
        .then_with(|| b.len().cmp(&a.len()))
}

fn compiler_version() -> &'static ts_semver::Version {
    static VERSION: std::sync::OnceLock<ts_semver::Version> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| ts_semver::Version::must_parse(b"7.1.0-dev"))
}
/// port: tsc/internal/module/util.go:IsApplicableVersionedTypesKey
pub fn is_applicable_versioned_types_key(key: &[u8]) -> bool {
    key.strip_prefix(b"types@")
        .and_then(ts_semver::VersionRange::parse)
        .is_some_and(|range| range.test(Some(compiler_version())))
}
/// Source cache state is shared by every package-directory view of the package.
#[derive(Clone, Debug, Default)]
pub(super) struct VersionPaths {
    version: JsString,
    traces: Vec<crate::DiagAndArgs>,
    paths: std::sync::OnceLock<ts_core::PathMappings>,
}
impl PackageJson {
    /// The package and module-specifier consumers share the source first-use cache.
    /// Reading the mappings here does not emit resolution trace messages.
    pub fn version_paths(&self) -> Option<&ts_core::PathMappings> {
        self.selected_version_paths().map(|(_, paths)| paths)
    }
    // port: tsc/internal/packagejson/cache.go:PackageJson.GetVersionPaths
    fn selected_version_paths(&self) -> Option<(&JsString, &ts_core::PathMappings)> {
        let package = self;
        let selected=package.version_paths.get_or_init(|| {
            let mut result=VersionPaths::default();
            let mut emit=|message,args|result.traces.push(crate::DiagAndArgs{message,args});
            let Some(raw)=package.contents.get("typesVersions") else {
                emit(diagnostics::X_package_json_does_not_have_a_0_field,vec!["typesVersions".into()]);return result;
            };
            let Some(versions)=raw.as_object() else {
                emit(diagnostics::Expected_type_of_0_field_in_package_json_to_be_1_got_2,vec!["typesVersions".into(),"object".into(),json_type(raw).into()]);return result;
            };
            emit(diagnostics::X_package_json_has_a_typesVersions_field_with_version_specific_path_mappings,vec!["typesVersions".into()]);
            for (version,value) in versions {
                let Some(range)=ts_semver::VersionRange::parse(version.as_bytes()) else {
                    emit(diagnostics::X_package_json_has_a_typesVersions_entry_0_that_is_not_a_valid_semver_range,vec![version.as_str().into()]);continue;
                };
                if !range.test(Some(compiler_version())) {continue;}
                if !value.is_object(){
                    emit(diagnostics::Expected_type_of_0_field_in_package_json_to_be_1_got_2,vec![format!("typesVersions['{version}']").as_bytes().into(),"object".into(),json_type(value).into()]);return result;
                }
                result.version=JsString::from_bytes(version.as_bytes());return result;
            }
            emit(diagnostics::X_package_json_does_not_have_a_typesVersions_entry_that_matches_version_0,vec!["7.1".into()]);
            result
        });
        if selected.version.is_empty() {
            return None;
        }
        let paths = selected.paths.get_or_init(|| {
            package
                .contents
                .get("typesVersions")
                .and_then(Value::as_object)
                .and_then(|versions| {
                    versions.get(
                        std::str::from_utf8(selected.version.as_bytes()).expect("JSON version key"),
                    )
                })
                .and_then(Value::as_object)
                .expect("selected object version mapping")
                .iter()
                .filter_map(|(name, values)| {
                    Some((
                        JsString::from_bytes(name.as_bytes()),
                        Some(
                            values
                                .as_array()?
                                .iter()
                                .map(|value| {
                                    JsString::from_bytes(
                                        value.as_str().unwrap_or_default().as_bytes(),
                                    )
                                })
                                .collect(),
                        ),
                    ))
                })
                .collect()
        });
        Some((&selected.version, paths))
    }
}
impl Resolver {
    pub(super) fn version_paths<'a>(
        &mut self,
        package: &'a PackageJson,
    ) -> Option<(&'a JsString, &'a ts_core::PathMappings)> {
        let result = package.selected_version_paths();
        if self.tracer.active {
            for message in &package
                .version_paths
                .get()
                .expect("version selection initialized")
                .traces
            {
                self.tracer.write(message.message, message.args.clone());
            }
        }
        result
    }
}
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
