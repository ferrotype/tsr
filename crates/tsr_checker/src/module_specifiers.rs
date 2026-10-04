//! Declaration-emit module specifiers. The checker supplies lexical module
//! identity; path ranking uses only the immutable program host's retained data.
use crate::{CheckerHost, Error, ModuleSpecifierPath};
use tsr_ast::{JsString, NodeId};
use tsr_core::{CompilerOptions, ResolutionMode as Mode};
use tsr_tspath as path;

#[path = "module_specifiers_packages.rs"]
mod packages;
#[path = "module_specifiers_paths.rs"]
mod paths;
pub use paths::Ending as ModuleSpecifierEnding;
use paths::{allowed_endings, ensure_non_module, same_volume_relative, Ending};
type ModulePath = ModuleSpecifierPath;

pub(super) struct Import {
    text: JsString,
    mode: Mode,
    resolved: Option<JsString>,
}
pub(super) struct Generation<'a> {
    host: &'a dyn CheckerHost,
    file: &'a [u8],
    imports: Vec<Import>,
    default_mode: Mode,
    mode: Mode,
    preference: Preferences<'a>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Relativity {
    Shortest,
    Relative,
    NonRelative,
    ProjectRelative,
}
struct Preferences<'a> {
    relative: Relativity,
    ending: Option<&'a str>,
    excluded: &'a dyn Fn(&[u8]) -> bool,
    old_specifier: &'a [u8],
}

// port: tsc/internal/modulespecifiers/specifiers.go:GetModuleSpecifiersForFileWithInfo
pub(crate) fn generate(
    host: &dyn CheckerHost,
    importer: NodeId,
    file: &[u8],
    target: &[u8],
    override_mode: Mode,
    request_js: bool,
) -> Result<JsString, Error> {
    generate_with_preferences(
        host,
        importer,
        file,
        target,
        override_mode,
        Preferences {
            relative: Relativity::ProjectRelative,
            ending: request_js.then_some("js"),
            excluded: &|_| false,
            old_specifier: b"",
        },
    )?
    .ok_or(Error::MissingLink("GetModuleSpecifiers returned no paths"))
}
fn generate_with_preferences(
    host: &dyn CheckerHost,
    importer: NodeId,
    file: &[u8],
    target: &[u8],
    override_mode: Mode,
    preference: Preferences<'_>,
) -> Result<Option<JsString>, Error> {
    let owner = host
        .get_source_file(file)
        .ok_or(Error::MissingLink("module specifier importing source"))?;
    let view = owner.view().ast();
    let source = view.source_file(importer)?;
    let default_mode = host.get_default_resolution_mode_for_file(file)?;
    let mode = if override_mode == Mode::NONE {
        default_mode
    } else {
        override_mode
    };
    let mut imports = Vec::new();
    for id in source.imports()?.iter().flatten() {
        let text = view.node_text(*id)?.into_js_string();
        let mode = host.get_mode_for_usage_location(file, *id)?;
        let resolved = host
            .get_resolved_module(file, text.as_bytes(), mode)?
            .filter(|module| module.is_resolved())
            .map(|module| module.resolved_file_name.clone());
        imports.push(Import {
            text,
            mode,
            resolved,
        });
    }
    let generation = Generation {
        host,
        file,
        imports,
        default_mode,
        mode,
        preference,
    };
    let module_paths = generation.sorted_paths(host.get_module_specifier_paths(file, target)?);
    let result = generation.compute(&module_paths)?;
    Ok(result.into_iter().next().map(JsString::from_bytes))
}

impl Generation<'_> {
    pub(super) fn options(&self) -> &CompilerOptions {
        self.host.options()
    }
    pub(super) fn case_sensitive(&self) -> bool {
        self.host.use_case_sensitive_file_names()
    }
    fn endings(&self, syntax_mode: Mode) -> Vec<Ending> {
        allowed_endings(
            self.options(),
            self.file,
            &self.imports,
            self.default_mode,
            syntax_mode,
            self.preference.ending,
            self.preference.old_specifier,
        )
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:getAllModulePathsWorker
    #[allow(
        clippy::naive_bytecount,
        reason = "Count separators in module paths with the standard library; a byte-counting dependency is not justified here"
    )]
    fn sorted_paths(&self, paths: Vec<ModulePath>) -> Vec<ModulePath> {
        // Source first collects by exact spelling. An overwrite keeps the last
        // ModulePath for that spelling; tie ordering is explicit at this pin.
        let mut remaining = crate::types::Map::default();
        for path in paths {
            remaining.insert(path.file_name.clone(), path);
        }
        let compare = |a: &ModulePath, b: &ModulePath| {
            b.is_redirect
                .cmp(&a.is_redirect)
                .then_with(|| {
                    a.file_name
                        .as_bytes()
                        .iter()
                        .filter(|b| **b == b'/')
                        .count()
                        .cmp(
                            &b.file_name
                                .as_bytes()
                                .iter()
                                .filter(|b| **b == b'/')
                                .count(),
                        )
                })
                .then_with(|| {
                    path::compare_paths(
                        a.file_name.as_bytes(),
                        b.file_name.as_bytes(),
                        b"",
                        self.case_sensitive(),
                    )
                })
        };
        let mut result = Vec::with_capacity(remaining.len());
        let mut directory = path::directory(self.file);
        loop {
            let prefix = if directory.ends_with(b"/") {
                directory.clone()
            } else {
                [&directory, b"/".as_slice()].concat()
            };
            let keys: Vec<_> = remaining
                .keys()
                .filter(|name| name.as_bytes().starts_with(&prefix))
                .cloned()
                .collect();
            let mut in_directory: Vec<_> = keys
                .into_iter()
                .map(|key| remaining.remove(&key).expect("selected module path"))
                .collect();
            in_directory.sort_by(compare);
            result.extend(in_directory);
            let parent = path::directory(&directory);
            if parent == directory || remaining.is_empty() {
                break;
            }
            directory = parent;
        }
        let mut rest: Vec<_> = remaining.into_values().collect();
        rest.sort_by(compare);
        result.extend(rest);
        result
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:computeModuleSpecifiers
    fn compute(&self, paths: &[ModulePath]) -> Result<Vec<Vec<u8>>, Error> {
        for candidate in paths {
            let target = path::to_path(
                candidate.file_name.as_bytes(),
                self.host.get_current_directory(),
                self.case_sensitive(),
            );
            // Native considers the first matching import for each path. A
            // mode mismatch skips this path rather than searching later imports.
            let existing = self.imports.iter().find(|import| {
                import.resolved.as_ref().is_some_and(|resolved| {
                    path::to_path(
                        resolved.as_bytes(),
                        self.host.get_current_directory(),
                        self.case_sensitive(),
                    ) == target
                })
            });
            if let Some(existing) = existing {
                if self.preference.relative == Relativity::NonRelative
                    && path::is_relative(existing.text.as_bytes())
                {
                    continue;
                }
                if existing.mode != self.mode
                    && existing.mode != Mode::NONE
                    && self.mode != Mode::NONE
                {
                    continue;
                }
                if !existing.text.is_empty() {
                    return Ok(vec![existing.text.as_bytes().to_vec()]);
                }
            }
        }
        let in_node_modules = paths.iter().any(|path| path.is_in_node_modules);
        let mut mapped = vec![];
        let mut redirects = vec![];
        let mut packages = vec![];
        let mut relative = vec![];
        for candidate in paths {
            let package = if candidate.is_in_node_modules {
                self.node_module_specifier(candidate)?
            } else {
                vec![]
            };
            if !package.is_empty() && !(self.preference.excluded)(&package) {
                packages.push(package.clone());
                if candidate.is_redirect {
                    return Ok(packages);
                }
            }
            let local = self.local_specifier(
                candidate.file_name.as_bytes(),
                candidate.is_redirect || !package.is_empty(),
            )?;
            if local.is_empty() || (self.preference.excluded)(&local) {
                continue;
            }
            if candidate.is_redirect {
                redirects.push(local);
            } else if path::root_length(&local) == 0 && !path::is_relative(&local) {
                if contains_node_modules(&local) {
                    relative.push(local);
                } else {
                    mapped.push(local);
                }
            } else if !in_node_modules || candidate.is_in_node_modules {
                relative.push(local);
            }
        }
        if !mapped.is_empty() {
            Ok(mapped)
        } else if !redirects.is_empty() {
            Ok(redirects)
        } else if !packages.is_empty() {
            Ok(packages)
        } else {
            Ok(relative)
        }
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:getLocalModuleSpecifier
    #[allow(
        clippy::naive_bytecount,
        reason = "Count separators in module paths with the standard library; a byte-counting dependency is not justified here"
    )]
    fn local_specifier(&self, target: &[u8], paths_only: bool) -> Result<Vec<u8>, Error> {
        let options = self.options();
        if paths_only && options.paths.is_none() {
            return Ok(vec![]);
        }
        let directory = path::directory(self.file);
        let endings = self.endings(self.mode);
        let mut relative = self.root_dirs_path(target, &directory, &endings)?;
        if relative.is_empty() {
            relative = self.process_ending(
                &ensure_non_module(path::relative_from_directory(
                    &directory,
                    target,
                    self.host.get_current_directory(),
                    self.case_sensitive(),
                )),
                &endings,
            )?;
        }
        if options.paths.is_none() && !options.resolve_package_json_imports()
            || self.preference.relative == Relativity::Relative
        {
            return Ok(if paths_only { vec![] } else { relative });
        }
        let base = path::absolute(
            options.paths_base_path(self.host.get_current_directory()),
            self.host.get_current_directory(),
        );
        let relative_to_base = same_volume_relative(target, &base, self.case_sensitive());
        if relative_to_base.is_empty() {
            return Ok(if paths_only { vec![] } else { relative });
        }
        // Go compares IndexOf values, including -1 when a kind is absent.
        let priority = |ending| {
            endings
                .iter()
                .position(|e| *e == ending)
                .map_or(-1, |n| n as isize)
        };
        let prefer_ts = priority(Ending::Ts) > -1 && priority(Ending::Ts) < priority(Ending::Js);
        let mut non_relative = if paths_only {
            vec![]
        } else {
            self.package_imports(target, &directory, prefer_ts)?
        };
        if let Some(paths) = options
            .paths
            .as_ref()
            .filter(|_| paths_only || non_relative.is_empty())
        {
            non_relative =
                self.module_name_from_paths(&relative_to_base, paths, &endings, &base)?;
        }
        if paths_only {
            return Ok(non_relative);
        }
        if non_relative.is_empty() {
            return Ok(relative);
        }
        let relative_excluded = (self.preference.excluded)(&relative);
        let non_relative_excluded = (self.preference.excluded)(&non_relative);
        if !relative_excluded && non_relative_excluded {
            return Ok(relative);
        }
        if relative_excluded && !non_relative_excluded {
            return Ok(non_relative);
        }
        if self.preference.relative == Relativity::NonRelative && !path::is_relative(&non_relative)
        {
            return Ok(non_relative);
        }
        if self.preference.relative == Relativity::ProjectRelative
            && !path::is_relative(&non_relative)
        {
            let project = if options.config_file_path.is_empty() {
                self.host.get_current_directory().to_vec()
            } else {
                path::directory(options.config_file_path.as_bytes())
            };
            let project = path::to_path(
                &project,
                self.host.get_current_directory(),
                self.case_sensitive(),
            );
            let source = path::to_path(
                &directory,
                self.host.get_current_directory(),
                self.case_sensitive(),
            );
            let target_path = path::to_path(target, project.as_bytes(), self.case_sensitive());
            let contains = |file: &[u8]| {
                path::contains_path(project.as_bytes(), file, b"", self.case_sensitive())
            };
            if contains(source.as_bytes()) != contains(target_path.as_bytes()) {
                return Ok(non_relative);
            }
            let target_package = self
                .host
                .get_nearest_ancestor_directory_with_package_json(&path::directory(
                    target_path.as_bytes(),
                ))?
                .unwrap_or_default();
            let source_package = self
                .host
                .get_nearest_ancestor_directory_with_package_json(&directory)?
                .unwrap_or_default();
            if target_package != source_package
                && (target_package.is_empty()
                    || source_package.is_empty()
                    || !path::compare_paths(
                        target_package.as_bytes(),
                        source_package.as_bytes(),
                        self.host.get_current_directory(),
                        self.case_sensitive(),
                    )
                    .is_eq())
            {
                return Ok(non_relative);
            }
            return Ok(relative);
        }
        let count = |path: &[u8]| {
            path.strip_prefix(b"./")
                .unwrap_or(path)
                .iter()
                .filter(|c| **c == b'/')
                .count()
        };
        Ok(
            if non_relative.starts_with(b"..") || count(&relative) < count(&non_relative) {
                relative
            } else {
                non_relative
            },
        )
    }
}

// port: tsc/internal/modulespecifiers/specifiers.go:ContainsNodeModules
pub(super) fn contains_node_modules(path: &[u8]) -> bool {
    path.windows(14).any(|part| part == b"/node_modules/")
}

impl crate::Operation<'_> {
    /// Recompute a specifier after a file move using the original source's
    /// resolution mode and preferences, but the importing file's new location.
    // port: tsc/internal/modulespecifiers/specifiers.go:UpdateModuleSpecifier
    pub fn update_module_specifier(
        &self,
        source: NodeId,
        importing_file: &[u8],
        old_specifier: NodeId,
        target: &[u8],
        ending: Option<&str>,
    ) -> Result<JsString, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let source_file = view.source_file(source)?;
        let host = state.program()?.host.as_ref();
        let old = view.node_text(old_specifier)?.into_js_string();
        let default_mode = host.get_default_resolution_mode_for_file(source_file.file_name())?;
        let override_mode =
            host.get_mode_for_usage_location(source_file.file_name(), old_specifier)?;
        let mut imports = Vec::new();
        for &id in source_file.imports()?.iter().flatten() {
            imports.push(Import {
                text: view.node_text(id)?.into_js_string(),
                mode: host.get_mode_for_usage_location(source_file.file_name(), id)?,
                resolved: None,
            });
        }
        let generation = Generation {
            host,
            file: importing_file,
            imports,
            default_mode,
            mode: if override_mode == Mode::NONE {
                default_mode
            } else {
                override_mode
            },
            preference: Preferences {
                relative: if path::is_relative(old.as_bytes()) {
                    Relativity::Relative
                } else {
                    Relativity::NonRelative
                },
                ending,
                excluded: &|_| false,
                old_specifier: old.as_bytes(),
            },
        };
        let paths =
            generation.sorted_paths(host.get_module_specifier_paths(importing_file, target)?);
        for candidate in &paths {
            let name = generation.node_module_specifier(candidate)?;
            if !name.is_empty() {
                return Ok(JsString::from_bytes(name));
            }
        }
        Ok(JsString::from_bytes(
            generation.local_specifier(target, false)?,
        ))
    }

    pub fn resolved_import_file(
        &self,
        source: NodeId,
        specifier: NodeId,
    ) -> Result<Option<JsString>, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let file = view.source_file(source)?;
        let host = state.program()?.host.as_ref();
        let mode = host.get_mode_for_usage_location(file.file_name(), specifier)?;
        Ok(host
            .get_resolved_module(
                file.file_name(),
                view.node_text(specifier)?.as_bytes(),
                mode,
            )?
            .filter(|module| module.is_resolved())
            .map(|module| module.resolved_file_name.clone()))
    }

    /// The existing pinned module-specifier generator, shared by declaration
    /// display and the language service. The source is checked against this
    /// operation's immutable program; no path-only owner bypass is introduced.
    pub fn module_specifier_for_file(
        &self,
        source: NodeId,
        target: &[u8],
    ) -> Result<JsString, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let file = view.source_file(source)?;
        generate(
            state.program()?.host.as_ref(),
            source,
            file.file_name(),
            target,
            Mode::NONE,
            false,
        )
    }
    /// Auto-import ranking shares the declaration path generator, with the
    /// request's relative/ending preferences and specifier exclusion predicate.
    pub fn module_specifier_for_auto_import(
        &self,
        source: NodeId,
        target: &[u8],
        relative: Option<&str>,
        ending: Option<&str>,
        excluded: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Option<JsString>, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let file = view.source_file(source)?;
        let relative = match relative {
            Some("relative") => Relativity::Relative,
            Some("non-relative") => Relativity::NonRelative,
            Some("project-relative") => Relativity::ProjectRelative,
            _ => Relativity::Shortest,
        };
        generate_with_preferences(
            state.program()?.host.as_ref(),
            source,
            file.file_name(),
            target,
            Mode::NONE,
            Preferences {
                relative,
                ending,
                excluded,
                old_specifier: b"",
            },
        )
    }
    pub fn import_file_module_formats(
        &self,
        source: NodeId,
    ) -> Result<(tsr_core::ModuleKind, tsr_core::ModuleKind), Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let file = view.source_file(source)?;
        let host = state.program()?.host.as_ref();
        Ok((
            host.get_emit_module_format_of_file(file.file_name())?,
            host.get_implied_node_format_for_emit(file.file_name())?,
        ))
    }
}

impl crate::Operation<'_> {
    pub fn import_ending_preferences(
        &self,
        source: NodeId,
        syntax_mode: Mode,
        preference: Option<&str>,
    ) -> Result<Vec<ModuleSpecifierEnding>, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        let file = view.source_file(source)?;
        let host = state.program()?.host.as_ref();
        let mut imports = Vec::new();
        for &id in file.imports()?.iter().flatten() {
            imports.push(Import {
                text: view.node_text(id)?.into_js_string(),
                mode: host.get_mode_for_usage_location(file.file_name(), id)?,
                resolved: None,
            });
        }
        Ok(allowed_endings(
            host.options(),
            file.file_name(),
            &imports,
            host.get_default_resolution_mode_for_file(file.file_name())?,
            syntax_mode,
            preference,
            b"",
        ))
    }
    pub fn import_usage_resolution_mode(
        &self,
        source: NodeId,
        specifier: NodeId,
    ) -> Result<Mode, Error> {
        let state = self.state();
        let view = state.ast(source)?;
        view.node(specifier)?;
        state
            .program()?
            .host
            .get_mode_for_usage_location(view.source_file(source)?.file_name(), specifier)
    }
    pub fn import_package_json(
        &self,
        path: &[u8],
    ) -> Result<Option<std::sync::Arc<tsr_module::PackageJson>>, Error> {
        self.state().program()?.host.get_package_json_info(path)
    }
}
