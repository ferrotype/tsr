use crate::resolver::{is_relative, mangle_scoped, DTS, JS, JSON, TS};
use crate::trace::{extensions_text, joined, trace};
use crate::{Error, PackageId, PackageJson, ResolvedModule, Resolver};
use std::collections::BTreeSet;
use ts_core::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use ts_diagnostics as diagnostics;
use ts_jsstring::JsString;
use ts_tspath as path;
pub const INFERRED_TYPES_CONTAINING_FILE: &[u8] = b"__inferred type names__.ts";
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedTypeReferenceDirective {
    pub resolution_diagnostics: Vec<ts_ast::Diagnostic>,
    pub primary: bool,
    pub resolved_file_name: JsString,
    pub original_path: JsString,
    pub package_id: PackageId,
    pub is_external_library_import: bool,
}
impl ResolvedTypeReferenceDirective {
    pub fn is_resolved(&self) -> bool {
        !self.resolved_file_name.is_empty()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TypeKey {
    directory: JsString,
    name: JsString,
    mode: ModuleKind,
    inferred: bool,
}
/// port: tsc/internal/core/compileroptions.go:CompilerOptions.GetEffectiveTypeRoots
pub fn effective_type_roots(options: &CompilerOptions, cwd: &[u8]) -> (Vec<JsString>, bool) {
    if let Some(roots) = &options.type_roots {
        return (roots.clone(), true);
    }
    let base = if options.config_file_path.is_empty() {
        assert!(
            !cwd.is_empty(),
            "cannot get effective type roots without a config file path or current directory"
        );
        cwd.to_vec()
    } else {
        path::directory(options.config_file_path.as_bytes())
    };
    (
        path::ancestors(&base)
            .into_iter()
            .map(|dir| JsString::from_bytes(path::combine(&dir, &[b"node_modules/@types"])))
            .collect(),
        false,
    )
}
impl Resolver {
    pub(super) fn nearest_node_modules(
        &mut self,
        name: &[u8],
        directory: &[u8],
        extensions: u8,
        context: &crate::package_maps::Context,
    ) -> Result<Option<ResolvedModule>, Error> {
        for ext in [extensions & (TS | DTS), extensions & (JS | JSON)] {
            if ext == 0 {
                continue;
            }
            if ext & (TS | DTS) != 0 {
                trace!(self,diagnostics::Searching_all_ancestor_node_modules_directories_for_preferred_extensions_Colon_0,extensions_text(ext));
            } else {
                trace!(self,diagnostics::Searching_all_ancestor_node_modules_directories_for_fallback_extensions_Colon_0,extensions_text(ext));
            }
            for dir in path::ancestors(directory) {
                if path::base_name(&dir) == b"node_modules" {
                    continue;
                }
                let node_modules = path::combine(&dir, &[b"node_modules"]);
                if !self.host.directory_exists(&node_modules)? {
                    trace!(
                        self,
                        diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                        &node_modules
                    );
                    continue;
                }
                if let Some(r) = self.package(ext, name, &node_modules, context)? {
                    return Ok(Some(r));
                }
                if ext & DTS != 0 {
                    let types = path::combine(&node_modules, &[b"@types"]);
                    if self.host.directory_exists(&types)? {
                        let mangled = self.trace_mangle_scoped(name);
                        if let Some(r) = self.package(DTS, &mangled, &types, context)? {
                            return Ok(Some(r));
                        }
                    } else {
                        trace!(
                            self,
                            diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                            &types
                        );
                    }
                }
            }
        }
        Ok(None)
    }
    pub fn resolve_type_reference(
        &mut self,
        name: &[u8],
        containing_file: &[u8],
        mode: ModuleKind,
    ) -> Result<&ResolvedTypeReferenceDirective, Error> {
        self.tracer.begin(self.options.trace_resolution.is_true());
        let directory = path::directory(containing_file);
        let inferred = containing_file.ends_with(INFERRED_TYPES_CONTAINING_FILE);
        let key = TypeKey {
            directory: JsString::from_bytes(directory.as_slice()),
            name: JsString::from_bytes(name),
            mode,
            inferred,
        };
        if !self.options.trace_resolution.is_true() && self.type_cache.contains_key(&key) {
            return Ok(&self.type_cache[&key]);
        }
        let result = self.trace_operation(|resolver| {
        let (roots, from_config) = effective_type_roots(&resolver.options, resolver.cwd.as_bytes());
        trace!(
            resolver,
            diagnostics::Resolving_type_reference_directive_0_containing_file_1_root_directory_2,
            name,
            containing_file,
            joined(&roots, b",")
        );
        let outcome = resolver.resolve_type_reference_worker(
            name,
            &directory,
            mode,
            inferred,
            &roots,
            from_config,
        );
        if let Ok(result) = &outcome {
            if !result.is_resolved() {
                trace!(
                    resolver,
                    diagnostics::Type_reference_directive_0_was_not_resolved,
                    name
                );
            } else if result.package_id.name.is_empty() {
                trace!(resolver,diagnostics::Type_reference_directive_0_was_successfully_resolved_to_1_primary_Colon_2,name,&result.resolved_file_name,result.primary);
            } else {
                trace!(resolver,diagnostics::Type_reference_directive_0_was_successfully_resolved_to_1_with_Package_ID_2_primary_Colon_3,name,&result.resolved_file_name,crate::trace::package_id(&result.package_id),result.primary);
            }
        }
            outcome
        })?;
        self.type_cache.insert(key.clone(), result);
        Ok(&self.type_cache[&key])
    }
    fn resolve_type_reference_worker(
        &mut self,
        name: &[u8],
        directory: &[u8],
        mode: ModuleKind,
        inferred: bool,
        roots: &[JsString],
        from_config: bool,
    ) -> Result<ResolvedTypeReferenceDirective, Error> {
        if roots.is_empty() {
            trace!(
                self,
                diagnostics::Root_directory_cannot_be_determined_skipping_primary_search_paths
            );
        } else {
            trace!(
                self,
                diagnostics::Resolving_with_primary_search_path_0,
                joined(roots, b", ")
            );
        }
        let esm = self.options.module_resolution_kind() != ModuleResolutionKind::BUNDLER
            && mode == ModuleKind::ESNEXT;
        let mut primary = false;
        let mut resolved = None;
        for root in roots {
            let candidate = self.type_candidate(root.as_bytes(), name);
            if !self.host.directory_exists(root.as_bytes())? {
                trace!(
                    self,
                    diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    root
                );
                continue;
            }
            if from_config {
                resolved = self.file(DTS, &candidate, esm, false)?;
                if let Some(result) = &mut resolved {
                    self.assign_node_package_identity(result)?;
                }
            }
            if resolved.is_none() {
                let package = self.package_json(&candidate)?;
                resolved = self.directory(DTS, &candidate, package.as_deref(), esm)?;
            }
            if resolved.is_some() {
                primary = true;
                break;
            }
        }
        if resolved.is_none() && (!from_config || !inferred) {
            trace!(
                self,
                diagnostics::Looking_up_in_node_modules_folder_initial_location_0,
                directory
            );
            resolved = if is_relative(name) {
                let candidate = path::resolve(directory, &[name]);
                self.relative(DTS, &candidate, esm, true, false)?
            } else {
                self.nearest_node_modules(
                    name,
                    directory,
                    DTS,
                    &crate::package_maps::Context::new(&self.options, mode),
                )?
            };
        }
        if resolved.is_none() && from_config && inferred {
            trace!(self,diagnostics::Resolving_type_reference_directive_for_program_that_specifies_custom_typeRoots_skipping_lookup_in_node_modules_folder);
        }
        let mut result = ResolvedTypeReferenceDirective::default();
        if let Some(found) = resolved {
            result
                .resolution_diagnostics
                .clone_from(&found.resolution_diagnostics);
            if !found.is_resolved() {
                return Ok(result);
            }
            let filename = found.resolved_file_name;
            let external = filename
                .as_bytes()
                .windows(14)
                .any(|w| w == b"/node_modules/");
            let real = if self.options.preserve_symlinks.is_true() {
                filename.clone()
            } else {
                let real = self.host.realpath(filename.as_bytes())?;
                trace!(
                    self,
                    diagnostics::Resolving_real_path_for_0_result_1,
                    &filename,
                    &real
                );
                real
            };
            let (original_path, resolved_file_name) =
                if path::canonical(real.as_bytes(), self.host.use_case_sensitive_file_names())
                    == path::canonical(
                        filename.as_bytes(),
                        self.host.use_case_sensitive_file_names(),
                    )
                {
                    (JsString::default(), filename)
                } else {
                    (filename, real)
                };
            result = ResolvedTypeReferenceDirective {
                resolution_diagnostics: found.resolution_diagnostics,
                primary,
                resolved_file_name,
                original_path,
                package_id: found.package_id,
                is_external_library_import: external,
            };
        }
        Ok(result)
    }
    pub(super) fn resolve_from_type_roots(
        &mut self,
        name: &[u8],
        esm: bool,
    ) -> Result<Option<ResolvedModule>, Error> {
        let options = self.options.clone();
        for root in options.type_roots.iter().flatten() {
            let candidate = self.type_candidate(root.as_bytes(), name);
            if !self.host.directory_exists(root.as_bytes())? {
                trace!(
                    self,
                    diagnostics::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    root
                );
                continue;
            }
            if let Some(mut result) = self.file(DTS, &candidate, esm, false)? {
                self.assign_node_package_identity(&mut result)?;
                return Ok(Some(result));
            }
            let info = self.package_json(&candidate)?;
            if let Some(result) = self.directory(DTS, &candidate, info.as_deref(), esm)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
    pub(super) fn assign_node_package_identity(
        &mut self,
        result: &mut ResolvedModule,
    ) -> Result<(), Error> {
        if let Some(directory) = node_module_directory(result.resolved_file_name.as_bytes()) {
            if let Some(info) = self.package_json(&directory)? {
                result.package_id =
                    self.package_identity(result.resolved_file_name.as_bytes(), &info)?;
            }
        }
        Ok(())
    }
    pub(super) fn package_identity(
        &mut self,
        resolved_file: &[u8],
        package: &PackageJson,
    ) -> Result<PackageId, Error> {
        let (Some(name), Some(version)) = (package.string("name"), package.string("version"))
        else {
            return Ok(PackageId::default());
        };
        let mut peers = Vec::new();
        let peers_valid = self.validate_package_field(package, "peerDependencies", "object");
        if let Some(values) = package
            .contents
            .get("peerDependencies")
            .and_then(serde_json::Value::as_object)
            .filter(|m| peers_valid && !m.is_empty())
        {
            trace!(
                self,
                diagnostics::X_package_json_has_a_peerDependencies_field
            );
            let real = self.host.realpath(package.directory.as_bytes())?;
            trace!(
                self,
                diagnostics::Resolving_real_path_for_0_result_1,
                &package.directory,
                &real
            );
            if let Some(index) = real
                .as_bytes()
                .windows(13)
                .rposition(|w| w == b"/node_modules")
            {
                let node_modules = &real.as_bytes()[..index + 13];
                let mut names: Vec<_> = values.keys().collect();
                names.sort();
                for name in names {
                    if let Some(info) =
                        self.package_json(&path::combine(node_modules, &[name.as_bytes()]))?
                    {
                        peers.push(b'+');
                        peers.extend_from_slice(name.as_bytes());
                        peers.push(b'@');
                        peers.extend_from_slice(info.string("version").unwrap_or_default());
                        trace!(
                            self,
                            diagnostics::Found_peerDependency_0_with_1_version,
                            name.as_str(),
                            info.string("version").unwrap_or_default()
                        );
                    } else {
                        trace!(
                            self,
                            diagnostics::Failed_to_find_peerDependency_0,
                            name.as_str()
                        );
                    }
                }
            }
        }
        Ok(PackageId {
            name: JsString::from_bytes(name),
            version: JsString::from_bytes(version),
            sub_module_name: JsString::from_bytes(
                resolved_file
                    .get(package.directory.as_bytes().len() + 1..)
                    .unwrap_or_default(),
            ),
            peer_dependencies: JsString::from_bytes(peers),
        })
    }
    /// port: tsc/internal/module/resolver.go:GetAutomaticTypeDirectiveNames
    pub fn automatic_type_directive_names(&mut self) -> Result<Vec<JsString>, Error> {
        let options = self.options.clone();
        if !options.uses_wildcard_types() {
            return Ok(options.types.clone().unwrap_or_default());
        }
        let (roots, _) = effective_type_roots(&options, self.cwd.as_bytes());
        let mut wildcard = Vec::new();
        for root in roots {
            if !self.host.directory_exists(root.as_bytes())? {
                continue;
            }
            for entry in self.host.entries(root.as_bytes())?.directories {
                let normalized = path::normalize(entry.as_bytes());
                // GetAutomaticTypeDirectiveNames reads the package directly;
                // it does not populate the resolver's package information cache.
                let package_path = path::combine(root.as_bytes(), &[&normalized, b"package.json"]);
                let not_needed = if self.host.file_exists(&package_path)? {
                    let content = self.host.read_file(&package_path)?;
                    let parsed = crate::package_json::parse(
                        content
                            .as_ref()
                            .map_or(b"".as_slice(), |file| file.text.as_bytes()),
                    );
                    parsed
                        .fields
                        .field("typings")
                        .is_some_and(|field| field.state.null)
                } else {
                    false
                };
                if not_needed {
                    continue;
                }
                let name = path::base_name(&normalized);
                if !name.starts_with(b".") {
                    wildcard.push(JsString::from_bytes(name));
                }
            }
        }
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        for name in options.types.iter().flatten() {
            let names = if name.as_bytes() == b"*" {
                wildcard.as_slice()
            } else {
                std::slice::from_ref(name)
            };
            for name in names {
                if seen.insert(name.clone()) {
                    result.push(name.clone());
                }
            }
        }
        Ok(result)
    }
}
impl Resolver {
    fn type_candidate(&mut self, root: &[u8], name: &[u8]) -> Vec<u8> {
        let name = if root.ends_with(b"/node_modules/@types")
            || root.ends_with(b"/node_modules/@types/")
        {
            self.trace_mangle_scoped(name)
        } else {
            name.to_vec()
        };
        path::combine(root, &[&name])
    }
    fn trace_mangle_scoped(&mut self, name: &[u8]) -> Vec<u8> {
        let mangled = mangle_scoped(name);
        if mangled != name {
            trace!(
                self,
                diagnostics::Scoped_package_detected_looking_in_0,
                &mangled
            );
        }
        mangled
    }
}

fn node_module_directory(file: &[u8]) -> Option<Vec<u8>> {
    let start = file.windows(14).rposition(|w| w == b"/node_modules/")? + 14;
    let rest = &file[start..];
    let (package, _) = crate::resolver::parse_package_name(rest);
    Some(file[..start + package.len()].to_vec())
}
