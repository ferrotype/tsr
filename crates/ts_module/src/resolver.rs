use crate::trace::{extensions_text, trace};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use ts_core::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use ts_diagnostics as diagnostics;
use ts_jsstring::JsString;
use ts_tspath as path;
use ts_vfs::FileSystem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Host(ts_vfs::Error),
    MutableHost,
    Unsupported(&'static str),
    MalformedPackageJson(JsString),
}
impl From<ts_vfs::Error> for Error {
    fn from(value: ts_vfs::Error) -> Self {
        Self::Host(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageId {
    pub name: JsString,
    pub sub_module_name: JsString,
    pub version: JsString,
    pub peer_dependencies: JsString,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedModule {
    pub resolution_diagnostics: Vec<ts_ast::Diagnostic>,
    pub resolved_file_name: JsString,
    pub original_path: JsString,
    pub extension: JsString,
    pub resolved_using_ts_extension: bool,
    pub resolved_using_extra_extensions: bool,
    pub package_id: PackageId,
    pub is_external_library_import: bool,
    pub alternate_result: JsString,
}
impl ResolvedModule {
    pub fn is_resolved(&self) -> bool {
        !self.resolved_file_name.as_bytes().is_empty()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub path: JsString,
    pub exists: bool,
}
#[derive(Clone, Debug)]
pub struct PackageJson {
    pub directory: JsString,
    pub(super) shared: Arc<PackageContents>,
}
/// Parsed fields and first-use version mappings have source package identity;
/// caller-specific directory wrappers borrow these same shared contents.
#[derive(Debug)]
pub struct PackageContents {
    pub contents: crate::package_json::Fields,
    pub parseable: bool,
    pub(super) version_paths: std::sync::OnceLock<crate::package_maps::VersionPaths>,
}
impl std::ops::Deref for PackageJson {
    type Target = PackageContents;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}
impl PackageJson {
    pub fn string(&self, key: &str) -> Option<&[u8]> {
        self.contents.get(key)?.as_str().map(str::as_bytes)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    directory: JsString,
    name: JsString,
    mode: ModuleKind,
}
/// The host and options are immutable for this resolver's entire lifetime.
/// Cache hits borrow the entry; callers explicitly clone only escaping values.
/// Trace-enabled requests bypass the cache, as in resolver.go:ResolveModuleName.
pub struct Resolver {
    // True only in the fresh resolver owned by the standalone ResolveConfig API.
    // It is fixed before any lookup and never changes an ordinary resolver.
    pub(super) config_lookup: bool,
    pub(super) package_directory_only: bool,
    pub(super) host: Arc<dyn FileSystem>,
    pub(super) options: Arc<CompilerOptions>,
    pub(super) cwd: JsString,
    cache: BTreeMap<Key, ResolvedModule>,
    packages: BTreeMap<JsString, (bool, Option<Arc<PackageJson>>)>,
    pub(super) tracer: crate::trace::Tracer,
    probes: Vec<Probe>,
    pub(super) type_cache:
        BTreeMap<crate::type_references::TypeKey, crate::ResolvedTypeReferenceDirective>,
}
impl Resolver {
    pub fn new(
        host: Arc<dyn FileSystem>,
        options: Arc<CompilerOptions>,
        cwd: &[u8],
    ) -> Result<Self, Error> {
        if host.snapshot_id().is_none() {
            return Err(Error::MutableHost);
        }
        Ok(Self {
            config_lookup: false,
            package_directory_only: false,
            host,
            options,
            cwd: JsString::from_bytes(cwd),
            cache: BTreeMap::new(),
            packages: BTreeMap::new(),
            tracer: crate::trace::Tracer::default(),
            probes: Vec::new(),
            type_cache: BTreeMap::new(),
        })
    }
    pub fn take_trace(&mut self) -> Vec<crate::DiagAndArgs> {
        self.tracer.take()
    }
    pub fn probes(&self) -> &[Probe] {
        &self.probes
    }
    pub fn clear_probes(&mut self) {
        self.probes.clear();
    }
    pub fn host(&self) -> &dyn FileSystem {
        self.host.as_ref()
    }
    pub fn package_json(
        &mut self,
        directory: &[u8],
    ) -> Result<Option<Arc<PackageJson>>, ts_vfs::Error> {
        let file = path::combine(directory, &[b"package.json"]);
        let key = path::to_path(
            &file,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        if let Some((exists, cached)) = self.packages.get(&key) {
            if cached.is_some() {
                trace!(
                    self,
                    diagnostics::File_0_exists_according_to_earlier_cached_lookups,
                    &file
                );
            } else if *exists {
                trace!(
                    self,
                    diagnostics::File_0_does_not_exist_according_to_earlier_cached_lookups,
                    &file
                );
            }
            return Ok(cached.as_ref().map(|cached| {
                if cached.directory.as_bytes() == directory {
                    cached.clone()
                } else {
                    Arc::new(PackageJson {
                        directory: JsString::from_bytes(directory),
                        shared: Arc::clone(&cached.shared),
                    })
                }
            }));
        }
        let directory_exists = self.host.directory_exists(directory)?;
        let result = if directory_exists && self.host.file_exists(&file)? {
            let content = self.host.read_file(&file)?;
            let parsed = crate::package_json::parse(
                content
                    .as_ref()
                    .map_or(b"".as_slice(), |file| file.text.as_bytes()),
            );
            trace!(self, diagnostics::Found_package_json_at_0, &file);
            Some(Arc::new(PackageJson {
                directory: JsString::from_bytes(directory),
                shared: Arc::new(PackageContents {
                    contents: parsed.fields,
                    parseable: parsed.parseable,
                    version_paths: std::sync::OnceLock::new(),
                }),
            }))
        } else {
            if directory_exists {
                trace!(self, diagnostics::File_0_does_not_exist, &file);
            }
            None
        };
        self.packages
            .insert(key, (directory_exists, result.clone()));
        Ok(result)
    }

    pub fn package_scope(
        &mut self,
        directory: &[u8],
    ) -> Result<Option<Arc<PackageJson>>, ts_vfs::Error> {
        for dir in path::ancestors(directory) {
            if let Some(info) = self.package_json(&dir)? {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }
    /// Native direct package-scope callers have no resolution tracer.
    /// Keep the shared cache while preserving any enclosing resolver trace mode.
    pub fn package_scope_untraced(
        &mut self,
        directory: &[u8],
    ) -> Result<Option<Arc<PackageJson>>, ts_vfs::Error> {
        let previous = self.tracer.active;
        self.tracer.active = false;
        let result = self.package_scope(directory);
        self.tracer.active = previous;
        result
    }
    pub fn resolve(
        &mut self,
        name: &[u8],
        containing_file: &[u8],
        mode: ModuleKind,
    ) -> Result<&ResolvedModule, Error> {
        self.tracer.begin(self.options.trace_resolution.is_true());
        let directory = path::directory(&path::absolute(containing_file, self.cwd.as_bytes()));
        let key = Key {
            directory: JsString::from_bytes(directory.as_slice()),
            name: JsString::from_bytes(name),
            mode,
        };
        if !self.options.trace_resolution.is_true() && self.cache.contains_key(&key) {
            return Ok(&self.cache[&key]);
        }
        let result = self.trace_operation(|resolver| {
            trace!(
                resolver,
                diagnostics::Resolving_module_0_from_1,
                name,
                containing_file
            );
            let resolution = resolver.options.module_resolution_kind();
            let resolution_name = match resolution {
                ModuleResolutionKind::NODE16 => b"Node16".as_slice(),
                ModuleResolutionKind::NODE_NEXT => b"NodeNext",
                ModuleResolutionKind::BUNDLER => b"Bundler",
                _ => b"Unknown",
            };
            if resolver.options.module_resolution == resolution {
                trace!(
                    resolver,
                    diagnostics::Explicitly_specified_module_resolution_kind_Colon_0,
                    resolution_name
                );
            } else {
                trace!(
                    resolver,
                    diagnostics::Module_resolution_kind_is_not_specified_using_0,
                    resolution_name
                );
            }
            let context = crate::package_maps::Context::new(&resolver.options, mode);
            trace!(
                resolver,
                diagnostics::Resolving_in_0_mode_with_conditions_1,
                if context.esm { b"ESM" } else { b"CJS" },
                crate::trace::conditions(&context.conditions)
            );
            let outcome = resolver.resolve_worker(name, &directory, &context);
            if let Ok(result) = &outcome {
                if !result.is_resolved() {
                    trace!(resolver, diagnostics::Module_name_0_was_not_resolved, name);
                } else if result.package_id.name.is_empty() {
                    trace!(
                        resolver,
                        diagnostics::Module_name_0_was_successfully_resolved_to_1,
                        name,
                        &result.resolved_file_name
                    );
                } else {
                    trace!(
                        resolver,
                        diagnostics::Module_name_0_was_successfully_resolved_to_1_with_Package_ID_2,
                        name,
                        &result.resolved_file_name,
                        crate::trace::package_id(&result.package_id)
                    );
                }
            }
            outcome
        })?;
        self.cache.insert(key.clone(), result);
        Ok(&self.cache[&key])
    }
    pub(super) fn resolve_worker(
        &mut self,
        name: &[u8],
        directory: &[u8],
        context: &crate::package_maps::Context,
    ) -> Result<ResolvedModule, Error> {
        let resolution = self.options.module_resolution_kind();
        if !matches!(
            resolution,
            ModuleResolutionKind::NODE16
                | ModuleResolutionKind::NODE_NEXT
                | ModuleResolutionKind::BUNDLER
        ) {
            return Err(Error::Unsupported("module resolution kind"));
        }
        let esm = context.esm;
        let extensions = context.extensions;
        if let Some(result) = self.optional_paths(name, directory, extensions, esm)? {
            return self.finish_external(result);
        }
        if is_relative(name) {
            let mut candidate = path::resolve(directory, &[name]);
            if matches!(path::base_name(name), b"." | b"..") && !candidate.ends_with(b"/") {
                candidate.push(b'/');
            }
            return Ok(self
                .relative(extensions, &candidate, esm, true, false)?
                .map_or_else(ResolvedModule::default, |mut r| {
                    r.is_external_library_import =
                        contains(r.resolved_file_name.as_bytes(), b"/node_modules/");
                    r
                }));
        }
        if name.starts_with(b"#") && context.imports {
            if let Some(result) = self.package_imports(name, directory, context)? {
                return self.finish_external(result);
            }
        }
        if let Some(result) = self.self_name(name, directory, context)? {
            return self.finish_external(result);
        }
        if name.contains(&b':') {
            trace!(self,diagnostics::Skipping_module_0_that_looks_like_an_absolute_URI_target_file_types_Colon_1,name,extensions_text(extensions));
            return Ok(ResolvedModule::default());
        }
        trace!(
            self,
            diagnostics::Loading_module_0_from_node_modules_folder_target_file_types_Colon_1,
            name,
            extensions_text(extensions)
        );
        if let Some(result) = self.nearest_node_modules(name, directory, extensions, context)? {
            let mut result = self.finish_external(result)?;
            if context.exports
                && extensions & (TS | DTS) != 0
                && result.is_resolved()
                && result.is_external_library_import
                && !matches!(
                    result.extension.as_bytes(),
                    b".ts" | b".tsx" | b".mts" | b".cts" | b".d.ts" | b".d.mts" | b".d.cts"
                )
                && context.conditions.iter().any(|c| c.as_bytes() == b"import")
            {
                trace!(self,diagnostics::Resolution_of_non_relative_name_failed_trying_with_modern_Node_resolution_features_disabled_to_see_if_npm_library_needs_configuration_update);
                let mut alternate = context.clone();
                alternate.exports = false;
                alternate.extensions &= TS | DTS;
                let lookup = self.resolve_worker(name, directory, &alternate)?;
                if lookup.is_resolved() && lookup.is_external_library_import {
                    result.alternate_result = lookup.resolved_file_name;
                }
            }
            return Ok(result);
        }
        if extensions & DTS != 0 {
            if let Some(result) = self.resolve_from_type_roots(name, esm)? {
                return self.finish_external(result);
            }
        }
        Ok(ResolvedModule::default())
    }
    pub(super) fn finish_external(
        &mut self,
        mut result: ResolvedModule,
    ) -> Result<ResolvedModule, Error> {
        result.is_external_library_import =
            contains(result.resolved_file_name.as_bytes(), b"/node_modules/");
        if result.is_external_library_import && !self.options.preserve_symlinks.is_true() {
            let real = self.host.realpath(result.resolved_file_name.as_bytes())?;
            trace!(
                self,
                diagnostics::Resolving_real_path_for_0_result_1,
                &result.resolved_file_name,
                &real
            );
            if path::canonical(real.as_bytes(), self.host.use_case_sensitive_file_names())
                != path::canonical(
                    result.resolved_file_name.as_bytes(),
                    self.host.use_case_sensitive_file_names(),
                )
            {
                result.original_path = result.resolved_file_name;
                result.resolved_file_name = real;
            }
        }
        Ok(result)
    }
    // Source: resolver.go:nodeLoadModuleByRelativeName, including ESM directory suppression.
    pub(super) fn relative(
        &mut self,
        ext: u8,
        candidate: &[u8],
        esm: bool,
        consider_package: bool,
        from_config: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        trace!(self,diagnostics::Loading_module_as_file_Slash_folder_candidate_module_location_0_target_file_types_Colon_1,candidate,extensions_text(ext));
        if !candidate.ends_with(b"/") {
            let parent = path::directory(candidate);
            if !self.host.directory_exists(&parent)? {
                trace!(
                    self,
                    diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    &parent
                );
                return Ok(None);
            }
            if let Some(mut r) = self.file(ext, candidate, esm, from_config)? {
                if consider_package {
                    self.assign_node_package_identity(&mut r)?;
                }
                return Ok(Some(r));
            }
        }
        if !self.host.directory_exists(candidate)? {
            trace!(
                self,
                diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                candidate
            );
            return Ok(None);
        }
        if esm {
            return Ok(None);
        }
        let info = if consider_package {
            self.package_json(candidate)?
        } else {
            None
        };
        self.directory(ext, candidate, info.as_deref(), esm)
    }
    // Source: resolver.go:loadNodeModuleFromDirectoryWorker, excluding typesVersions (typed error).
    pub(super) fn directory(
        &mut self,
        ext: u8,
        candidate: &[u8],
        package: Option<&PackageJson>,
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        let candidate = if candidate.len() > path::root_length(candidate) {
            candidate.strip_suffix(b"/").unwrap_or(candidate)
        } else {
            candidate
        };
        let version_paths = package.and_then(|info| self.version_paths(info));
        let mut package_file = None;
        if let Some(info) = package.filter(|info| {
            path::to_path(
                candidate,
                self.cwd.as_bytes(),
                self.host.use_case_sensitive_file_names(),
            ) == path::to_path(
                info.directory.as_bytes(),
                self.cwd.as_bytes(),
                self.host.use_case_sensitive_file_names(),
            )
        }) {
            if self.config_lookup {
                package_file = self.package_json_path_field(info, "tsconfig");
            } else if ext & DTS != 0 {
                package_file = self
                    .package_json_path_field(info, "typings")
                    .or_else(|| self.package_json_path_field(info, "types"));
            }
            if !self.config_lookup && package_file.is_none() && ext & (TS | JS | DTS) != 0 {
                package_file = self.package_json_path_field(info, "main");
            }
        }
        if let Some((version, paths)) = version_paths {
            let selected = package_file.as_deref().unwrap_or_default();
            if selected.is_empty()
                || selected
                    .strip_prefix(candidate)
                    .is_some_and(|rest| rest.starts_with(b"/"))
            {
                let module_name = if selected.is_empty() {
                    if self.config_lookup {
                        b"tsconfig".as_slice()
                    } else {
                        b"index".as_slice()
                    }
                } else {
                    &selected[candidate.len() + 1..]
                };
                trace!(self,diagnostics::X_package_json_has_a_typesVersions_entry_0_that_matches_compiler_version_1_looking_for_a_pattern_to_match_module_name_2,version,b"7.1.0-dev",module_name);
                if let Some(result) = self.paths_using(
                    module_name,
                    candidate,
                    paths,
                    ext,
                    |resolver, ext, target, _| {
                        resolver.directory_field(
                            ext,
                            target,
                            package_file.as_deref().unwrap_or_default(),
                            package,
                            esm,
                        )
                    },
                )? {
                    return Ok(Some(result));
                }
            }
        }
        if let Some(package_file) = package_file {
            if let Some(result) =
                self.directory_field(ext, &package_file, &package_file, package, esm)?
            {
                return Ok(Some(result));
            }
        }
        if esm || !self.host.directory_exists(candidate)? {
            return Ok(None);
        }
        let index = if self.config_lookup {
            b"tsconfig".as_slice()
        } else {
            b"index".as_slice()
        };
        self.file(ext, &path::combine(candidate, &[index]), esm, false)
    }
    fn directory_field(
        &mut self,
        ext: u8,
        target: &[u8],
        field: &[u8],
        package: Option<&PackageJson>,
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        if let Some(result) = self.package_field(ext, target, field)? {
            return Ok(Some(result));
        }
        let expanded = if ext == DTS { TS | DTS } else { ext };
        self.relative(
            expanded,
            target,
            esm && package.is_none_or(|p| p.string("type") == Some(b"module")),
            false,
            true,
        )
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.loadModuleFromFile
    pub(super) fn file(
        &mut self,
        ext: u8,
        candidate: &[u8],
        esm: bool,
        from_config: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        if path::base_name(candidate).contains(&b'.') {
            let stripped = path::remove_file_extension(candidate);
            let stripped = if stripped.len() == candidate.len() {
                &candidate[..candidate
                    .iter()
                    .rposition(|&c| c == b'.')
                    .expect("basename contains dot")]
            } else {
                stripped
            };
            trace!(
                self,
                diagnostics::File_name_0_has_a_1_extension_stripping_it,
                candidate,
                &candidate[stripped.len()..]
            );
            if let Some(r) =
                self.add_extensions(ext, stripped, &candidate[stripped.len()..], from_config)?
            {
                return Ok(Some(r));
            }
        }
        if esm {
            Ok(None)
        } else {
            self.add_extensions(ext, candidate, b"", from_config)
        }
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.tryAddingExtensions
    fn add_extensions(
        &mut self,
        ext: u8,
        base: &[u8],
        original: &[u8],
        from_config: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        let directory = path::directory(base);
        if !directory.is_empty() && !self.host.directory_exists(&directory)? {
            return Ok(None);
        }
        let candidates: &[(&[u8], u8)] = match original {
            b".mjs" | b".mts" | b".d.mts" => &[(b".mts", TS), (b".d.mts", DTS), (b".mjs", JS)],
            b".cjs" | b".cts" | b".d.cts" => &[(b".cts", TS), (b".d.cts", DTS), (b".cjs", JS)],
            b".json" => &[(b".d.json.ts", DTS), (b".json", JSON)],
            b".tsx" | b".jsx" => &[
                (b".tsx", TS),
                (b".ts", TS),
                (b".d.ts", DTS),
                (b".jsx", JS),
                (b".js", JS),
            ],
            b".ts" | b".d.ts" | b".js" | b"" => &[
                (b".ts", TS),
                (b".tsx", TS),
                (b".d.ts", DTS),
                (b".js", JS),
                (b".jsx", JS),
            ],
            _ => &[],
        };
        for &(suffix, mask) in candidates {
            if ext & mask == 0 {
                continue;
            }
            let using_ts = !from_config
                && mask & (TS | DTS) != 0
                && matches!(
                    original,
                    b".ts" | b".tsx" | b".mts" | b".cts" | b".d.ts" | b".d.mts" | b".d.cts"
                );
            if let Some(r) = self.try_extension(base, suffix, using_ts)? {
                return Ok(Some(r));
            }
        }
        if self.config_lookup && matches!(original, b".ts" | b".d.ts" | b".js" | b"") {
            if let Some(result) = self.try_extension(base, b".json", false)? {
                return Ok(Some(result));
            }
        }
        if candidates.is_empty() && ext & DTS != 0 {
            let mut whole = base.to_vec();
            whole.extend_from_slice(original);
            if !path::is_declaration_file_name(&whole) {
                let mut suffix = b".d".to_vec();
                suffix.extend_from_slice(original);
                suffix.extend_from_slice(b".ts");
                return self.try_extension(base, &suffix, false);
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.tryExtension
    pub(super) fn try_extension(
        &mut self,
        base: &[u8],
        suffix: &[u8],
        using_ts: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        let mut filename = base.to_vec();
        filename.extend_from_slice(suffix);
        Ok(self.lookup(&filename)?.map(|name| ResolvedModule {
            resolved_file_name: name,
            extension: JsString::from_bytes(suffix),
            resolved_using_ts_extension: using_ts,
            ..Default::default()
        }))
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.tryFile
    pub(super) fn lookup(&mut self, filename: &[u8]) -> Result<Option<JsString>, Error> {
        if let Some(suffixes) = self
            .options
            .module_suffixes
            .as_ref()
            .filter(|s| !s.is_empty())
        {
            let ext = extension(filename);
            let base = &filename[..filename.len() - ext.len()];
            for suffix in suffixes {
                let mut candidate = base.to_vec();
                candidate.extend_from_slice(suffix.as_bytes());
                candidate.extend_from_slice(ext);
                let exists = self.host.file_exists(&candidate)?;
                if exists {
                    trace!(
                        self,
                        diagnostics::File_0_exists_use_it_as_a_name_resolution_result,
                        &candidate
                    );
                } else {
                    trace!(self, diagnostics::File_0_does_not_exist, &candidate);
                }
                let path = JsString::from_bytes(candidate);
                self.probes.push(Probe {
                    path: path.clone(),
                    exists,
                });
                if exists {
                    return Ok(Some(path));
                }
            }
            Ok(None)
        } else if self.probe(filename)? {
            Ok(Some(JsString::from_bytes(filename)))
        } else {
            Ok(None)
        }
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.tryFileLookup
    fn probe(&mut self, filename: &[u8]) -> Result<bool, Error> {
        let exists = self.host.file_exists(filename)?;
        if exists {
            trace!(
                self,
                diagnostics::File_0_exists_use_it_as_a_name_resolution_result,
                filename
            );
        } else {
            trace!(self, diagnostics::File_0_does_not_exist, filename);
        }
        self.probes.push(Probe {
            path: JsString::from_bytes(filename),
            exists,
        });
        Ok(exists)
    }
}

/// Resolves an `extends` package using an independent JSON-only source lookup.
/// port: tsc/internal/module/resolver.go:ResolveConfig
/// port: tsc/internal/module/resolver.go:Resolver.resolveConfig
pub fn resolve_config(
    name: &[u8],
    containing_file: &[u8],
    host: Arc<dyn FileSystem>,
    cwd: &[u8],
) -> Result<ResolvedModule, Error> {
    let options = Arc::new(CompilerOptions {
        module_resolution: ModuleResolutionKind::NODE_NEXT,
        ..Default::default()
    });
    let mut context = crate::package_maps::Context::new(&options, ModuleKind::COMMON_JS);
    context.extensions = JSON;
    let mut resolver = Resolver::new(host, options, cwd)?;
    resolver.config_lookup = true;
    resolver.resolve_worker(name, &path::directory(containing_file), &context)
}

/// Package-directory lookup used by config content-mapper manifest validation.
/// The operation does not load or execute package code.
/// port: tsc/internal/module/resolver.go:Resolver.ResolvePackageDirectory
pub fn resolve_package_directory(
    name: &[u8],
    containing_file: &[u8],
    host: Arc<dyn FileSystem>,
    cwd: &[u8],
) -> Result<Option<ResolvedModule>, Error> {
    let options = Arc::new(CompilerOptions {
        module_resolution: ModuleResolutionKind::BUNDLER,
        ..Default::default()
    });
    let mut resolver = Resolver::new(host, options, cwd)?;
    resolver.resolve_package_directory(name, containing_file, ModuleKind::NONE)
}
impl Resolver {
    /// Additional dependency discovery uses the already-retained package cache.
    /// Ordinary module resolutions are not recomputed by this operation.
    // port: tsc/internal/module/resolver.go:Resolver.ResolvePackageDirectory
    pub fn resolve_package_directory(
        &mut self,
        name: &[u8],
        containing_file: &[u8],
        mode: ModuleKind,
    ) -> Result<Option<ResolvedModule>, Error> {
        let context = crate::package_maps::Context::new(&self.options, mode);
        let previous = self.package_directory_only;
        let previous_trace = self.tracer.active;
        self.package_directory_only = true;
        self.tracer.active = false;
        let result = (|| {
            let Some(mut result) = self.nearest_node_modules(
                name,
                &path::directory(containing_file),
                context.extensions,
                &context,
            )?
            else {
                return Ok(None);
            };
            if is_relative(name) {
                result.is_external_library_import =
                    contains(result.resolved_file_name.as_bytes(), b"/node_modules/");
            } else {
                result = self.finish_external(result)?;
            }
            Ok(Some(result))
        })();
        self.package_directory_only = previous;
        self.tracer.active = previous_trace;
        result
    }
}

pub(super) const TS: u8 = 1;
pub(super) const JS: u8 = 2;
pub(super) const DTS: u8 = 4;
pub(super) const JSON: u8 = 8;
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        // Go JSON values carry float64, while IsFalsy compares its interface
        // value to int(0); even numeric zero is truthy at this pin.
        Value::Number(_) | Value::Array(_) | Value::Object(_) => true,
        Value::String(v) => !v.is_empty(),
    }
}
pub fn is_relative(name: &[u8]) -> bool {
    name == b"."
        || name == b".."
        || name.starts_with(b"./")
        || name.starts_with(b"../")
        || name.starts_with(b".\\")
        || name.starts_with(b"..\\")
        || path::encoded_root_length(name) > 0
}
pub(super) fn extension(path: &[u8]) -> &[u8] {
    let base = path::remove_file_extension(path);
    &path[base.len()..]
}
pub(super) fn parse_package_name(name: &[u8]) -> (&[u8], &[u8]) {
    let mut slash = name.iter().position(|&c| c == b'/');
    if name.starts_with(b"@") {
        slash = slash.and_then(|first| {
            name[first + 1..]
                .iter()
                .position(|&c| c == b'/')
                .map(|next| first + 1 + next)
        });
    }
    slash.map_or((name, b"".as_slice()), |n| (&name[..n], &name[n + 1..]))
}
/// port: tsc/internal/module/util.go:GetTypesPackageName
pub fn get_types_package_name(name: &[u8]) -> Vec<u8> {
    [b"@types/".as_slice(), &mangle_scoped(name)].concat()
}
// port: tsc/internal/module/util.go:MangleScopedPackageName
pub fn mangle_scoped_package_name(name: &[u8]) -> Vec<u8> {
    mangle_scoped(name)
}
pub fn mangle_scoped(name: &[u8]) -> Vec<u8> {
    if let Some(name) = name.strip_prefix(b"@") {
        if let Some(index) = name.iter().position(|&b| b == b'/') {
            let mut result = name[..index].to_vec();
            result.extend_from_slice(b"__");
            result.extend_from_slice(&name[index + 1..]);
            return result;
        }
    }
    name.to_vec()
}
/// port: tsc/internal/module/resolver.go:GetConditions
pub fn get_conditions(options: &CompilerOptions, mut mode: ModuleKind) -> Vec<JsString> {
    let resolution = options.module_resolution_kind();
    if mode == ModuleKind::NONE && resolution == ModuleResolutionKind::BUNDLER {
        mode = ModuleKind::ESNEXT;
    }
    let mut result = vec![JsString::from_bytes(if mode == ModuleKind::ESNEXT {
        b"import".as_slice()
    } else {
        b"require"
    })];
    if !options.no_dts_resolution.is_true() {
        result.push(JsString::from_bytes(b"types".as_slice()));
    }
    if resolution != ModuleResolutionKind::BUNDLER {
        result.push(JsString::from_bytes(b"node".as_slice()));
    }
    result.extend(options.custom_conditions.iter().flatten().cloned());
    result
}
