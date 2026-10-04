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
    if let Some(allowed) = &mut allowed {
        for resolution in program.resolutions() {
            let name = &resolution.result.package_id.name;
            if !name.is_empty() {
                allowed.insert(JsString::from_bytes(
                    tsr_module::package_name_from_types_package_name(name.as_bytes()),
                ));
            }
        }
        for name in program.options().types.iter().flatten() {
            if name.as_bytes() != b"*" {
                allowed.insert(JsString::from_bytes(
                    tsr_module::package_name_from_types_package_name(name.as_bytes()),
                ));
            }
        }
    }
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
                            || program.resolutions().iter().any(|r| {
                                r.result.package_id.name == name
                                    && r.name.as_bytes().starts_with(name.as_bytes())
                                    && r.name.as_bytes().get(name.len()) == Some(&b'/')
                            }),
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
                result.push(Package {
                    name,
                    entrypoints,
                    paths,
                });
            }
        }
    }
    Ok(Some(result))
}

impl Package {
    pub fn load(
        &self,
        parent: &Program,
        counters: &tsr_arena::Counters,
    ) -> Result<Arc<Program>, Error> {
        let roots: BTreeSet<_> = self
            .entrypoints
            .iter()
            .map(|e| self.paths.to_symlink(e.symlink_or_realpath()))
            .collect();
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
        registry.index = registry
            .index
            .filtered(|export| paths.contains_key(export.path.as_bytes()));
        for export in registry.index.entries_mut() {
            export.package_name = self.name.clone();
            export.entrypoints = paths.get(export.path.as_bytes()).unwrap().clone().into();
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
