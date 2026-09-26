//! Module-specifier host data derived from the retained program and snapshot.
use crate::checker_host::ProgramCheckerHost;
use std::collections::BTreeSet;
use tsr_checker::{CheckerHost, Error, ModuleSpecifierPath};
use tsr_core::ModuleKind;
use tsr_jsstring::JsString;
use tsr_module::symlinks::KnownSymlinks;
use tsr_tspath::{self as path, contains_ignored_path as ignored, starts_with_directory};

fn contains(bytes: &[u8], part: &[u8]) -> bool {
    bytes.windows(part.len()).any(|window| window == part)
}
impl ProgramCheckerHost {
    // port: tsc/internal/compiler/program.go:Program.GetSymlinkCache
    fn compute_known_symlinks(&self) -> Result<KnownSymlinks, Error> {
        let program = self.program();
        let cwd = program.current_directory();
        let case_sensitive = self.use_case_sensitive_file_names();
        let result = KnownSymlinks::new(cwd, case_sensitive);
        for resolution in program.resolutions() {
            result.process_resolution(
                resolution.result.original_path.as_bytes(),
                resolution.result.resolved_file_name.as_bytes(),
            );
        }
        for resolution in program.type_resolutions() {
            result.process_resolution(
                resolution.result.original_path.as_bytes(),
                resolution.result.resolved_file_name.as_bytes(),
            );
        }
        let mut seen = BTreeSet::new();
        for (file_path, metadata) in &program.metadata {
            let file = program
                .file(file_path.as_bytes())
                .ok_or(tsr_arena::Error::InvalidGraph)?;
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
                    .and_then(tsr_module::package_json::Value::as_object)
                {
                    dependencies.extend(values.keys().map(String::as_bytes));
                }
            }
            for dependency in dependencies {
                let possible = path::combine(directory, &[b"node_modules", dependency]);
                if result.has_directory(&path::to_path(&possible, cwd, case_sensitive).into()) {
                    continue;
                }
                if !dependency.starts_with(b"@types") {
                    let types_name = tsr_module::get_types_package_name(dependency);
                    let possible_types = path::combine(directory, &[b"node_modules", &types_name]);
                    if result
                        .has_directory(&path::to_path(&possible_types, cwd, case_sensitive).into())
                    {
                        continue;
                    }
                }
                let resolution = program
                    .package_resolver
                    .lock()
                    .expect("retained package resolver poisoned")
                    .resolve_package_directory(dependency, &json_name, ModuleKind::COMMON_JS)
                    .map_err(|error| match error {
                        tsr_module::Error::Host(error) => Error::Host(error),
                        tsr_module::Error::Unsupported(context) => Error::Unsupported(context),
                        // These constructors are not reachable from this operation:
                        // the retained resolver has a snapshot, and its JSON parser
                        // keeps malformed contents instead of constructing an error.
                        tsr_module::Error::MutableHost => {
                            Error::Unsupported("GetSymlinkCache: mutable resolver host")
                        }
                        tsr_module::Error::MalformedPackageJson(_) => {
                            Error::Unsupported("GetSymlinkCache: package parser failure")
                        }
                    })?;
                if let Some(resolution) =
                    resolution.filter(|r| r.is_resolved() && !r.original_path.is_empty())
                {
                    result.process_resolution(
                        &path::combine(resolution.original_path.as_bytes(), &[b"package.json"]),
                        &path::combine(
                            resolution.resolved_file_name.as_bytes(),
                            &[b"package.json"],
                        ),
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
        // The only caller, tsr_checker's GetModuleSpecifiersForFileWithInfo port,
        // passes the module's own file and the importer's own name. The pin names
        // both by their source when they are included project-reference outputs
        // (GetModuleSpecifiersWithInfo and GetModuleSpecifiersForFileWithInfo).
        // The module is renamed here. The importer's source also chooses the
        // directory that tsr_checker's proximity sort starts from, and that sort
        // reads the importer's own name, so an output importer is refused.
        let importer_path = path::to_path(importer, cwd, case_sensitive);
        if program
            .source_of_project_reference_if_output_included(importer_path.as_bytes(), importer)
            != importer
        {
            return Err(Error::Unsupported(
                "module specifiers for an importer that is a project-reference output",
            ));
        }
        let target_path = path::to_path(target, cwd, case_sensitive);
        let target =
            program.source_of_project_reference_if_output_included(target_path.as_bytes(), target);
        let imported = path::to_path(target, cwd, case_sensitive);
        // A referenced project's source is named by its declaration output
        // first, then as itself and through package-identity redirects.
        let reference_redirect = program
            .references
            .project_reference_from_source(imported.as_bytes())
            .filter(|output| !output.output_dts.is_empty())
            .map(|output| path::absolute(output.output_dts.as_bytes(), cwd));
        let mut targets: Vec<Vec<u8>> = reference_redirect.iter().cloned().collect();
        targets.push(path::absolute(target, cwd));
        // port: tsc/internal/compiler/program.go:Program.GetRedirectTargets
        for (alias, destination) in &program.redirect_paths {
            if destination == &imported {
                let name = program
                    .redirect_file_names
                    .get(alias)
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                targets.push(path::absolute(name.as_bytes(), cwd));
            }
        }
        let mut filter_ignored = targets.iter().any(|name| !ignored(name));
        let symlinks = match self
            .known_symlinks
            .get_or_init(|| self.compute_known_symlinks())
        {
            Ok(value) => value,
            Err(error) => return Err(error.clone()),
        };
        let mut result = vec![];
        for directory in path::ancestors(&path::directory(&path::absolute(target, cwd))) {
            let key = path::to_path(&directory, cwd, case_sensitive);
            let key = path::Path::from(key).ensure_trailing_directory_separator();
            let Some(links) = symlinks.directories_by_realpath().load(&key) else {
                continue;
            };
            // The pinned set is unordered; sorting keeps the result deterministic.
            let mut links = links.to_vec();
            links.sort();
            if starts_with_directory(importer, &directory, case_sensitive) {
                break;
            }
            for target in &targets {
                if !starts_with_directory(target, &directory, case_sensitive) {
                    continue;
                }
                let relative =
                    path::relative_from_directory(&directory, target, cwd, case_sensitive);
                for link in &links {
                    let option = path::resolve(link, &[&relative]);
                    result.push(ModuleSpecifierPath {
                        is_in_node_modules: contains(&option, b"/node_modules/"),
                        file_name: JsString::from_bytes(option),
                        is_redirect: reference_redirect.as_ref() == Some(target),
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
                is_redirect: reference_redirect.as_ref() == Some(&target),
                file_name: JsString::from_bytes(target),
            });
        }
        Ok(result)
    }
}
