//! Package discovery uses the production resolver's export/condition and symlink
//! rules. Auxiliary programs are operation-owned; only extracted export metadata
//! enters the published index.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tsr_compiler::{Error, Program};
use tsr_jsstring::JsString;
use tsr_module::{ResolvedEntrypoint, Resolver};
use tsr_tspath as path;
use tsr_vfs::FileSystem;

pub struct Package {
    pub name: JsString,
    pub entrypoints: Vec<ResolvedEntrypoint>,
    /// The extracted files as (symlink or realpath, realpath). A referenced
    /// project's declaration output is replaced by the source it was built from.
    roots: Vec<(JsString, JsString)>,
    paths: Arc<crate::realpaths::PackagePaths>,
}
fn directories(mut path: Vec<u8>) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    loop {
        let parent = path::directory(&path);
        result.push(path.clone());
        if parent == path {
            return result;
        }
        path = parent;
    }
}
/// Discovery is scoped to the requesting file. A lower node_modules directory
/// shadows the same name in its ancestors even if it has no usable entrypoint.
pub fn discover(
    program: &Program,
    checker: &mut tsr_checker::Operation<'_>,
    file: &[u8],
    host: &Arc<dyn FileSystem>,
    preferences: &crate::Preferences,
    canceled: impl Fn() -> bool,
) -> Result<Option<Vec<Package>>, Error> {
    let ancestors = directories(path::directory(file));
    let mut resolver = Resolver::with_options(
        host.clone(),
        Arc::new(tsr_core::CompilerOptions::default()),
        program.current_directory(),
        tsr_module::ResolverOptions {
            allow_live_host: true,
            ..Default::default()
        },
    )?;
    let mut allowed = None::<BTreeSet<JsString>>;
    for directory in &ancestors {
        if canceled() {
            return Ok(None);
        }
        if let Some(package) = resolver.package_json(directory)?.filter(|p| p.parseable) {
            let allowed = allowed.get_or_insert_with(BTreeSet::new);
            for field in ["dependencies", "peerDependencies"] {
                if let Some(deps) = package
                    .contents
                    .get(field)
                    .and_then(tsr_module::package_json::Value::as_object)
                {
                    allowed.extend(
                        deps.keys()
                            .filter(|s| {
                                !s.is_empty() && s.as_str() != "@types/" && !s.starts_with('.')
                            })
                            .map(|s| {
                                JsString::from_bytes(
                                    tsr_module::package_name_from_types_package_name(s.as_bytes()),
                                )
                            }),
                    );
                }
            }
        }
    }
    let names = crate::package_names::collect(program, checker, &mut resolver)?;
    if let Some(allowed) = &mut allowed {
        allowed.extend(names.resolved);
    }
    let deep = names.deep;
    let excludes = preferences.file_matcher(host.use_case_sensitive_file_names());
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for directory in ancestors {
        let modules = path::resolve(&directory, &[b"node_modules"]);
        if !host.directory_exists(&modules)? {
            continue;
        }
        let mut names = BTreeSet::new();
        for name in host.entries(&modules)?.directories.into_iter().flatten() {
            if name.is_empty() || name.as_bytes().starts_with(b".") {
                continue;
            }
            if name.as_bytes().starts_with(b"@") {
                for child in host
                    .entries(&path::resolve(&modules, &[name.as_bytes()]))?
                    .directories
                    .into_iter()
                    .flatten()
                {
                    let full = path::combine(name.as_bytes(), &[child.as_bytes()]);
                    names.insert(JsString::from_bytes(
                        tsr_module::package_name_from_types_package_name(&full),
                    ));
                }
            } else {
                names.insert(name);
            }
        }
        for name in names {
            if canceled() {
                return Ok(None);
            }
            if !seen.insert(name.clone()) || allowed.as_ref().is_some_and(|a| !a.contains(&name)) {
                continue;
            }
            let mut entrypoints = Vec::new();
            let mut paths = None;
            // The native registry tries @types only when the package has no
            // extractable TypeScript entrypoints (including file exclusions).
            for package_name in [
                name.as_bytes().to_vec(),
                tsr_module::get_types_package_name(name.as_bytes()),
            ] {
                let location = path::resolve(&modules, &[&package_name]);
                if host.directory_exists(&location)? {
                    let package = resolver.package_json(&location)?.unwrap_or_else(|| {
                        Arc::new(tsr_module::PackageJson::parse(&location, b"{}"))
                    });
                    let package_paths = Arc::new(crate::realpaths::PackagePaths::new(
                        host.clone(),
                        &location,
                    )?);
                    let mut package_resolver = Resolver::with_options(
                        package_paths.wrapped(),
                        Arc::new(tsr_core::CompilerOptions::default()),
                        program.current_directory(),
                        tsr_module::ResolverOptions {
                            allow_live_host: true,
                            ..Default::default()
                        },
                    )?;
                    entrypoints.extend(package_resolver.entrypoints(
                        &package,
                        name.as_bytes(),
                        package_name.starts_with(b"@types/")
                            || preferences.directory_search == Some(true)
                            || recursive_package(name.as_bytes())
                            || deep.contains(&name),
                    )?);
                    if let Some(excludes) = &excludes {
                        entrypoints
                            .retain(|entry| !excludes.matches(entry.resolved_file_name.as_bytes()));
                    }
                    if !entrypoints.is_empty() {
                        paths = Some(package_paths);
                        break;
                    }
                }
            }
            if let Some(paths) = paths {
                // port: tsc/internal/ls/autoimport/registry.go:registryBuilder.extractPackage
                let roots = entrypoints
                    .iter()
                    .map(|entry| {
                        let output = path::to_path(
                            entry.resolved_file_name.as_bytes(),
                            program.current_directory(),
                            program.use_case_sensitive_file_names(),
                        );
                        match program.project_reference_source_of_output_dts(output.as_bytes()) {
                            Some(source) => (paths.to_symlink(source.as_bytes()), source.clone()),
                            None => (
                                JsString::from_bytes(entry.symlink_or_realpath()),
                                entry.resolved_file_name.clone(),
                            ),
                        }
                    })
                    .collect();
                result.push(Package {
                    name,
                    entrypoints,
                    roots,
                    paths,
                });
            }
        }
    }
    Ok(Some(result))
}

impl Package {
    /// The extracted files' names, symlinked and real.
    pub fn root_file_names(&self) -> impl Iterator<Item = &JsString> {
        self.roots.iter().flat_map(|(file, real)| [file, real])
    }
    /// Root files that re-export from a bare module name the package program
    /// could not resolve, by path, with those names. The pin's alias resolver
    /// records these as failed ambient module lookups.
    // port: tsc/internal/ls/autoimport/aliasresolver.go:aliasResolver.GetResolvedModule
    pub fn failed_ambient_lookups(
        &self,
        program: &Program,
    ) -> Result<Vec<(JsString, Vec<JsString>)>, Error> {
        let mut result = Vec::new();
        for file in self.loaded_roots() {
            let Some(file) = program.source_file(file.as_bytes()) else {
                continue;
            };
            let view = file.bound().view().ast();
            let mut names = Vec::new();
            for statement in view
                .node_slice(view.node(file.source())?.statements(view)?)?
                .iter()
                .flatten()
            {
                let read = view.node(statement)?;
                if read.kind() != tsr_ast::SyntaxKind::ExportDeclaration {
                    continue;
                }
                let Some(specifier) = read.module_specifier() else {
                    continue;
                };
                let name = view.node_text(specifier)?.into_js_string();
                if path::is_relative(name.as_bytes()) || names.contains(&name) {
                    continue;
                }
                if !program
                    .resolved_module_from_specifier(file, specifier)?
                    .is_some_and(tsr_module::ResolvedModule::is_resolved)
                {
                    names.push(name);
                }
            }
            if !names.is_empty() {
                let source = view.source_file(file.source())?;
                result.push((JsString::from_bytes(source.path()), names));
            }
        }
        Ok(result)
    }
    /// The ambient module names the root files declare, with each file name.
    pub fn ambient_modules(&self, program: &Program) -> Result<Vec<(JsString, JsString)>, Error> {
        let mut result = Vec::new();
        for file in self.loaded_roots() {
            let Some(file) = program.source_file(file.as_bytes()) else {
                continue;
            };
            let view = file.bound().view().ast();
            let source = view.source_file(file.source())?;
            for name in source.ambient_module_names()?.iter() {
                result.push((name.clone(), JsString::from_bytes(source.file_name())));
            }
        }
        Ok(result)
    }
    fn loaded_roots(&self) -> BTreeSet<JsString> {
        self.roots
            .iter()
            .map(|(file, _)| self.paths.to_symlink(file.as_bytes()))
            .collect()
    }
    pub fn load(
        &self,
        parent: &Program,
        counters: &tsr_arena::Counters,
    ) -> Result<Arc<Program>, Error> {
        self.load_with(parent, counters, &[])
    }
    /// The package program with `extra` roots, such as files declaring ambient
    /// modules the package re-exports.
    pub fn load_with(
        &self,
        parent: &Program,
        counters: &tsr_arena::Counters,
        extra: &[JsString],
    ) -> Result<Arc<Program>, Error> {
        let mut roots = self.loaded_roots();
        roots.extend(extra.iter().cloned());
        // The pin's aliasResolver has empty compiler options and no default
        // library. Root entrypoints and their imported aliases supply its graph.
        let options = tsr_core::CompilerOptions {
            no_lib: tsr_core::Tristate::TRUE,
            types: Some(Vec::new()),
            ..Default::default()
        };
        Ok(Arc::new(Program::load_live(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, roots.into_iter().collect()),
                host: self.paths.wrapped(),
                current_directory: JsString::from_bytes(parent.current_directory()),
                default_library_path: JsString::from_bytes(parent.default_library_path()),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            counters,
        )?))
    }
    pub fn retain_entrypoints(&self, registry: &mut crate::Registry) {
        let mut paths: BTreeMap<&[u8], Vec<ResolvedEntrypoint>> = BTreeMap::new();
        for entry in &self.entrypoints {
            paths
                .entry(entry.symlink_or_realpath())
                .or_default()
                .push(entry.clone());
            if entry.symlink_or_realpath() != entry.resolved_file_name.as_bytes() {
                paths
                    .entry(entry.resolved_file_name.as_bytes())
                    .or_default()
                    .push(entry.clone());
            }
        }
        let roots: BTreeSet<&[u8]> = self
            .roots
            .iter()
            .flat_map(|(file, real)| [file.as_bytes(), real.as_bytes()])
            .collect();
        registry.index = registry
            .index
            .filtered(|export| roots.contains(export.path.as_bytes()));
        // Entrypoints are keyed by their own files. A source standing in for a
        // referenced project's output has none unless another entrypoint names it.
        for export in registry.index.entries_mut() {
            export.package_name = self.name.clone();
            export.entrypoints = paths
                .get(export.path.as_bytes())
                .cloned()
                .unwrap_or_default()
                .into();
        }
    }
}
// Source: knownRecursiveSearchPackages, internal/ls/autoimport/registry.go.
fn recursive_package(name: &[u8]) -> bool {
    matches!(
        name,
        b"@material-ui/core"
            | b"@material-ui/icons"
            | b"@sap/cds"
            | b"@testing-library/react-native"
            | b"ajv"
            | b"asap"
            | b"async"
            | b"aws-sdk"
            | b"braintree-web"
            | b"core-js"
            | b"core-js-pure"
            | b"crypto-js"
            | b"cypress-mochawesome-reporter"
            | b"dd-trace"
            | b"dumi"
            | b"dva"
            | b"egg-mock"
            | b"electron-log"
            | b"es-abstract"
            | b"es6-promise"
            | b"eslint-config-taro"
            | b"expo"
            | b"expo-router"
            | b"flow-remove-types"
            | b"gatsby"
            | b"glamor"
            | b"gluegun"
            | b"graphology-indices"
            | b"graphology-traversal"
            | b"graphology-utils"
            | b"jest-expo"
            | b"lodash"
            | b"lodash-es"
            | b"moment"
            | b"mz"
            | b"next"
            | b"pdfjs-dist"
            | b"protobufjs"
            | b"react-app-polyfill"
            | b"react-dev-utils"
            | b"react-devtools-inline"
            | b"recast"
            | b"semver"
            | b"stylelint-config-html"
            | b"umi"
            | b"web3-provider-engine"
            | b"webpack"
    )
}
