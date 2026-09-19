//! Module-specifier host data derived from the retained program and snapshot.
use crate::checker_host::ProgramCheckerHost;
use std::collections::{BTreeMap, BTreeSet};
use ts_checker::{CheckerHost, Error, ModuleSpecifierPath};
use ts_core::ModuleKind;
use ts_jsstring::JsString;
use ts_tspath as path;

#[derive(Default)]
pub(crate) struct KnownSymlinks {
    directories: BTreeSet<JsString>,
    by_realpath: BTreeMap<JsString, BTreeSet<JsString>>,
}
fn trailing(bytes: &[u8]) -> Vec<u8> {
    if bytes.ends_with(b"/") {
        bytes.to_vec()
    } else {
        [bytes, b"/"].concat()
    }
}
fn contains(bytes: &[u8], part: &[u8]) -> bool {
    bytes.windows(part.len()).any(|window| window == part)
}
// port: tsc/internal/tspath/ignoredpaths.go:ContainsIgnoredPath
fn ignored(bytes: &[u8]) -> bool {
    [b"/node_modules/.".as_slice(), b"/.git", b".#"]
        .iter()
        .any(|part| contains(bytes, part))
}
// port: tsc/internal/tspath/path.go:StartsWithDirectory
fn starts_with_directory(file: &[u8], directory: &[u8], case_sensitive: bool) -> bool {
    if directory.is_empty() {
        return false;
    }
    let file = path::canonical(file, case_sensitive);
    let directory = path::canonical(directory, case_sensitive);
    let directory = directory.strip_suffix(b"/").unwrap_or(&directory);
    let directory = directory.strip_suffix(b"\\").unwrap_or(directory);
    file.starts_with(&[directory, b"/"].concat()) || file.starts_with(&[directory, b"\\"].concat())
}
impl KnownSymlinks {
    // port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution
    fn process(&mut self, original: &[u8], resolved: &[u8], cwd: &[u8], case_sensitive: bool) {
        if original.is_empty() || resolved.is_empty() {
            return;
        }
        let mut real = path::normalized_components(resolved, cwd);
        let mut link = path::normalized_components(original, cwd);
        let is_package = |part: &[u8]| {
            path::canonical(part, case_sensitive).as_ref() == b"node_modules"
                || part.starts_with(b"@")
        };
        let mut directory = false;
        while real.len() >= 2
            && link.len() >= 2
            && !is_package(&real[real.len() - 2])
            && !is_package(&link[link.len() - 2])
            && path::canonical(real.last().expect("two components"), case_sensitive)
                == path::canonical(link.last().expect("two components"), case_sensitive)
        {
            real.pop();
            link.pop();
            directory = true;
        }
        if !directory {
            return;
        }
        let real = path::path_from_components(&real);
        let link = path::path_from_components(&link);
        let key = path::to_path(&link, cwd, case_sensitive);
        if ignored(key.as_bytes()) {
            return;
        }
        let key = JsString::from_bytes(trailing(key.as_bytes()));
        if self.directories.insert(key) {
            let real_key = path::to_path(&real, cwd, case_sensitive);
            self.by_realpath
                .entry(JsString::from_bytes(trailing(real_key.as_bytes())))
                .or_default()
                .insert(JsString::from_bytes(link));
        }
    }
    fn has_directory(&self, directory: &[u8], cwd: &[u8], case_sensitive: bool) -> bool {
        let key = path::to_path(directory, cwd, case_sensitive);
        self.directories
            .contains(trailing(key.as_bytes()).as_slice())
    }
}

impl ProgramCheckerHost {
    // port: tsc/internal/compiler/program.go:Program.GetSymlinkCache
    fn compute_known_symlinks(&self) -> Result<KnownSymlinks, Error> {
        let program = self.program();
        let cwd = program.current_directory();
        let case_sensitive = self.use_case_sensitive_file_names();
        let mut result = KnownSymlinks::default();
        for resolution in program.resolutions() {
            result.process(
                resolution.result.original_path.as_bytes(),
                resolution.result.resolved_file_name.as_bytes(),
                cwd,
                case_sensitive,
            );
        }
        for resolution in program.type_resolutions() {
            result.process(
                resolution.result.original_path.as_bytes(),
                resolution.result.resolved_file_name.as_bytes(),
                cwd,
                case_sensitive,
            );
        }
        let mut seen = BTreeSet::new();
        for (file_path, metadata) in &program.metadata {
            let file = program
                .file(file_path.as_bytes())
                .ok_or(ts_arena::Error::InvalidGraph)?;
            let directory = metadata.package_json_directory.as_bytes();
            if directory.is_empty()
                || !self.source_file_may_be_emitted(file.bound(), false)?
                || !seen.insert(path::to_path(directory, cwd, case_sensitive))
            {
                continue;
            }
            let json_name = path::combine(directory, &[b"package.json"]);
            let Some(package) = self.get_package_json_info(&json_name)? else {
                continue;
            };
            let mut dependencies = BTreeSet::new();
            for field in ["dependencies", "peerDependencies", "optionalDependencies"] {
                if let Some(values) = package
                    .contents
                    .get(field)
                    .and_then(ts_module::package_json::Value::as_object)
                {
                    dependencies.extend(values.keys().map(String::as_bytes));
                }
            }
            for dependency in dependencies {
                let possible = path::combine(directory, &[b"node_modules", dependency]);
                if result.has_directory(&possible, cwd, case_sensitive) {
                    continue;
                }
                if !dependency.starts_with(b"@types") {
                    let types_name = ts_module::get_types_package_name(dependency);
                    let possible_types = path::combine(directory, &[b"node_modules", &types_name]);
                    if result.has_directory(&possible_types, cwd, case_sensitive) {
                        continue;
                    }
                }
                let resolution = program
                    .package_resolver
                    .lock()
                    .expect("retained package resolver poisoned")
                    .resolve_package_directory(dependency, &json_name, ModuleKind::COMMON_JS)
                    .map_err(|error| match error {
                        ts_module::Error::Host(error) => Error::Host(error),
                        ts_module::Error::Unsupported(context) => Error::Unsupported(context),
                        // These constructors are not reachable from this operation:
                        // the retained resolver has a snapshot, and its JSON parser
                        // keeps malformed contents instead of constructing an error.
                        ts_module::Error::MutableHost => {
                            Error::Unsupported("GetSymlinkCache: mutable resolver host")
                        }
                        ts_module::Error::MalformedPackageJson(_) => {
                            Error::Unsupported("GetSymlinkCache: package parser failure")
                        }
                    })?;
                if let Some(resolution) =
                    resolution.filter(|r| r.is_resolved() && !r.original_path.is_empty())
                {
                    result.process(
                        &path::combine(resolution.original_path.as_bytes(), &[b"package.json"]),
                        &path::combine(
                            resolution.resolved_file_name.as_bytes(),
                            &[b"package.json"],
                        ),
                        cwd,
                        case_sensitive,
                    );
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:GetEachFileNameOfModule
    pub(crate) fn module_specifier_paths(
        &self,
        importer: &[u8],
        target: &[u8],
    ) -> Result<Vec<ModuleSpecifierPath>, Error> {
        let program = self.program();
        let cwd = program.current_directory();
        let case_sensitive = self.use_case_sensitive_file_names();
        let imported = path::to_path(target, cwd, case_sensitive);
        // Nonempty project references are rejected by Program::load, so there
        // is no project-reference redirect. Package-identity redirects remain.
        let mut targets = vec![path::absolute(target, cwd)];
        for (alias, destination) in &program.redirect_paths {
            if destination == &imported {
                let name = program
                    .redirect_file_names
                    .get(alias)
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                targets.push(path::absolute(name.as_bytes(), cwd));
            }
        }
        let mut filter_ignored = targets.iter().any(|name| !ignored(name));
        let symlinks = match self
            .known_symlinks
            .get_or_init(|| self.compute_known_symlinks())
        {
            Ok(value) => value,
            Err(error) => return Err(*error),
        };
        let mut result = vec![];
        for directory in path::ancestors(&path::directory(&path::absolute(target, cwd))) {
            let key = path::to_path(&directory, cwd, case_sensitive);
            let Some(links) = symlinks
                .by_realpath
                .get(trailing(key.as_bytes()).as_slice())
            else {
                continue;
            };
            if starts_with_directory(importer, &directory, case_sensitive) {
                break;
            }
            for target in &targets {
                if !starts_with_directory(target, &directory, case_sensitive) {
                    continue;
                }
                let relative =
                    path::relative_from_directory(&directory, target, cwd, case_sensitive);
                for link in links {
                    let option = path::resolve(link.as_bytes(), &[&relative]);
                    result.push(ModuleSpecifierPath {
                        is_in_node_modules: contains(&option, b"/node_modules/"),
                        file_name: JsString::from_bytes(option),
                        is_redirect: false,
                    });
                    filter_ignored = true;
                }
            }
        }
        for target in targets {
            if filter_ignored && ignored(&target) {
                continue;
            }
            result.push(ModuleSpecifierPath {
                is_in_node_modules: contains(&target, b"/node_modules/"),
                file_name: JsString::from_bytes(target),
                is_redirect: false,
            });
        }
        Ok(result)
    }
}
