//! Shared path, exports, imports and typesVersions completion rules.
use super::{
    directory_fragment, file_name, path, Ending, Entries, Kind, LanguageService, Operation,
    PathOptions, ResolutionMode, Result,
};
use tsr_jsstring::JsString;
use tsr_module::package_json::Value;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MappingKind {
    Paths,
    Exports,
    Imports,
}
fn priority(a: &[u8], b: &[u8], kind: MappingKind) -> std::cmp::Ordering {
    if kind == MappingKind::Paths {
        b.iter()
            .position(|c| *c == b'*')
            .unwrap_or(b.len())
            .cmp(&a.iter().position(|c| *c == b'*').unwrap_or(a.len()))
    } else {
        tsr_module::compare_pattern_keys(a, b)
    }
}
fn extension(name: &[u8]) -> Vec<u8> {
    let known = path::try_get_extension_from_path(name);
    if known.is_empty() {
        path::any_extension_from_path(name, &[] as &[&[u8]], false).to_vec()
    } else {
        known.to_vec()
    }
}
fn first_condition<'a>(value: &'a Value, conditions: &[JsString]) -> Option<&'a str> {
    match value {
        Value::String(s) => Some(s),
        Value::Object(entries) => entries
            .iter()
            .filter(|(key, _)| {
                key.as_str() == "default"
                    || conditions.iter().any(|c| c.as_bytes() == key.as_bytes())
                    || tsr_module::is_applicable_versioned_types_key(key.as_bytes())
            })
            .find_map(|(_, v)| first_condition(v, conditions)),
        _ => None,
    }
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/string_completions.go:LanguageService.addCompletionEntriesFromPathsOrExportsOrImports
    pub(super) fn mapping_entries(
        &self,
        paths: &tsr_core::PathMappings,
        fragment: &[u8],
        base: &[u8],
        options: &PathOptions,
        kind: MappingKind,
        result: &mut Entries,
    ) -> Result<bool> {
        let mut matched = None::<Vec<u8>>;
        let mut rows: Vec<(bool, Entries)> = Vec::new();
        for (key, targets) in paths {
            if key.as_bytes() == b"." {
                continue;
            }
            let mut key = key
                .as_bytes()
                .strip_prefix(b"./")
                .unwrap_or(key.as_bytes())
                .to_vec();
            if kind != MappingKind::Paths && key.ends_with(b"/") {
                key.push(b'*');
            }
            let Some(targets) = targets.as_ref().filter(|v| !v.is_empty()) else {
                continue;
            };
            let pattern = tsr_core::pattern::Pattern::parse(&key);
            if !pattern.is_valid() {
                continue;
            }
            let is_match = pattern.matches(fragment);
            if is_match
                && matched
                    .as_ref()
                    .is_none_or(|old| priority(&key, old, kind).is_lt())
            {
                matched = Some(key.clone());
                rows.retain(|(m, _)| !m);
            }
            if pattern.star_index == -1
                || matched
                    .as_ref()
                    .is_none_or(|old| !priority(&key, old, kind).is_gt())
            {
                rows.push((
                    is_match,
                    self.path_mapping(&key, targets, fragment, base, options, kind)?,
                ));
            }
        }
        for (_, entries) in rows {
            for (_, e) in entries.0 {
                result.add(e.name, e.kind, e.extension);
            }
        }
        Ok(matched.is_some())
    }
    // port: tsc/internal/ls/string_completions.go:LanguageService.getCompletionsForPathMapping
    fn path_mapping(
        &self,
        key: &[u8],
        targets: &[JsString],
        fragment: &[u8],
        base: &[u8],
        options: &PathOptions,
        kind: MappingKind,
    ) -> Result<Entries> {
        let mut directory = directory_fragment(fragment);
        if !directory.is_empty() && !directory.ends_with(b"/") {
            directory.push(b'/');
        }
        let mut result = Entries::default();
        let mut name_only = |name: &[u8], kind, extension| {
            if name.starts_with(fragment) {
                let name = name.strip_suffix(b"/").unwrap_or(name);
                result.add(
                    name.strip_prefix(directory.as_slice())
                        .unwrap_or(name)
                        .to_vec(),
                    kind,
                    extension,
                );
            }
        };
        let Some(star) = key.iter().position(|&c| c == b'*') else {
            name_only(
                key,
                Kind::File,
                targets
                    .first()
                    .map_or_else(Vec::new, |t| extension(t.as_bytes())),
            );
            return Ok(result);
        };
        let (prefix, suffix) = (&key[..star], &key[star + 1..]);
        let remaining = if let Some(rest) = fragment.strip_prefix(prefix) {
            rest
        } else {
            if !prefix.starts_with(fragment) {
                return Ok(result);
            }
            if key.ends_with(b"/*") {
                name_only(prefix, Kind::Directory, Vec::new());
                return Ok(result);
            }
            b""
        };
        let directory_prefix = if directory.starts_with(prefix) {
            b"".as_slice()
        } else {
            &prefix[directory.len()..]
        };
        for target in targets {
            for (_, mut entry) in self
                .pattern_modules(remaining, base, target.as_bytes(), options, kind)?
                .0
            {
                entry.name = [
                    directory_prefix,
                    &entry.name,
                    if entry.kind == Kind::File {
                        suffix
                    } else {
                        b""
                    },
                ]
                .concat();
                result.add(entry.name, entry.kind, entry.extension);
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/string_completions.go:LanguageService.getModulesForPathsPattern
    fn pattern_modules(
        &self,
        fragment: &[u8],
        base: &[u8],
        target: &[u8],
        options: &PathOptions,
        kind: MappingKind,
    ) -> Result<Entries> {
        let pattern = tsr_core::pattern::Pattern::parse(target);
        let mut result = Entries::default();
        if !pattern.is_valid() || pattern.star_index < 0 {
            return Ok(result);
        }
        let star = pattern.star_index as usize;
        let prefix = path::normalize(&target[..star]).into_owned();
        let suffix = path::normalize(&target[star + 1..]).into_owned();
        let (directory, basename) = if target[..star].ends_with(b"/") {
            (prefix.clone(), Vec::new())
        } else {
            (path::directory(&prefix), path::base_name(&prefix).to_vec())
        };
        let has_path = fragment.contains(&b'/');
        let expanded = if has_path {
            path::resolve(
                &directory,
                &[&[basename.as_slice(), &directory_fragment(fragment)].concat()],
            )
        } else {
            directory
        };
        let mut bases = vec![path::resolve(base, &[&expanded])];
        if kind == MappingKind::Imports {
            let compiler = self.program.options();
            for output in [&compiler.out_dir, &compiler.declaration_dir] {
                if !output.is_empty() {
                    bases.push(path::resolve(
                        &self.program.common_source_directory()?,
                        &[&path::relative_from_directory(
                            output.as_bytes(),
                            &bases[0],
                            self.program.current_directory(),
                            self.program.use_case_sensitive_file_names(),
                        )],
                    ));
                }
            }
        }
        let mut suffixes = Vec::new();
        if !suffix.is_empty() {
            let ext =
                path::declaration_emit_extension_for_path(&[b"_", suffix.as_slice()].concat());
            if !ext.is_empty() {
                suffixes.push(path::change_extension(&suffix, &ext));
            }
            for ext in path::possible_original_input_extensions(&[b"_", suffix.as_slice()].concat())
            {
                suffixes.push(path::change_extension(&suffix, &ext));
            }
        }
        suffixes.push(suffix.clone());
        let includes: Vec<_> = if suffix.is_empty() {
            vec![JsString::from_bytes(b"./*".as_slice())]
        } else {
            suffixes
                .iter()
                .map(|s| JsString::from_bytes([b"**/*", s.as_slice()].concat()))
                .collect()
        };
        let extensions: Vec<_> = options
            .extensions
            .iter()
            .map(|s| JsString::from_bytes(s.as_slice()))
            .collect();
        let mut filenames = options.clone();
        if kind != MappingKind::Paths && target.ends_with(b"/*") {
            filenames
                .endings
                .retain(|e| !matches!(e, Ending::Minimal | Ending::Index));
        }
        for directory in bases {
            self.check_canceled()?;
            let complete_prefix = if has_path {
                directory.clone()
            } else {
                [
                    directory.as_slice(),
                    if directory.ends_with(b"/") { b"" } else { b"/" },
                    basename.as_slice(),
                ]
                .concat()
            };
            let files = tsr_tsoptions::glob::read_directory(
                self.completion_file_system(),
                self.program.current_directory(),
                &directory,
                &extensions,
                &[],
                &includes,
                0,
            )
            .map_err(tsr_compiler::Error::from)?;
            for file in files {
                let file = path::normalize(file.as_bytes());
                let Some(trimmed) = suffixes.iter().find_map(|suffix| {
                    file.strip_prefix(complete_prefix.as_slice())?
                        .strip_suffix(suffix.as_slice())
                }) else {
                    continue;
                };
                let trimmed = trimmed.strip_prefix(b"/").unwrap_or(trimmed);
                if trimmed.is_empty() {
                    continue;
                }
                if let Some(slash) = trimmed.iter().position(|c| *c == b'/') {
                    result.add(trimmed[..slash].to_vec(), Kind::Directory, Vec::new());
                } else {
                    let (name, mut ext) = file_name(trimmed, self.program.options(), &filenames);
                    if ext.is_empty() {
                        ext = extension(&file);
                    }
                    result.add(name, Kind::File, ext);
                }
            }
            if suffix.is_empty()
                && self
                    .completion_file_system()
                    .directory_exists(&directory)
                    .map_err(tsr_compiler::Error::from)?
            {
                for name in self
                    .completion_file_system()
                    .entries(&directory)
                    .map_err(tsr_compiler::Error::from)?
                    .directories
                    .into_iter()
                    .flatten()
                {
                    if name.as_bytes() != b"node_modules" {
                        result.add(name.as_bytes().to_vec(), Kind::Directory, Vec::new());
                    }
                }
            }
        }
        Ok(result)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Carries the pinned exports/imports table, conditions and mapping context together"
    )]
    fn exports_entries(
        &self,
        table: Option<&Value>,
        conditions: &[JsString],
        fragment: &[u8],
        base: &[u8],
        options: &PathOptions,
        kind: MappingKind,
        result: &mut Entries,
    ) -> Result<bool> {
        let Some(Value::Object(table)) = table else {
            return Ok(table.is_some());
        };
        let paths = table
            .iter()
            .map(|(key, value)| {
                let values = first_condition(value, conditions).map(|target| {
                    let mut target = target.as_bytes().to_vec();
                    if key.ends_with('/') && target.ends_with(b"/") {
                        target.push(b'*');
                    }
                    vec![JsString::from_bytes(target)]
                });
                (JsString::from_bytes(key.as_bytes()), values)
            })
            .collect();
        self.mapping_entries(&paths, fragment, base, options, kind, result)?;
        Ok(true)
    }
    pub(super) fn package_path_entries(
        &self,
        checker: &mut Operation<'_>,
        fragment: &[u8],
        directory: &[u8],
        options: &PathOptions,
        mode: ResolutionMode,
        result: &mut Entries,
    ) -> Result<()> {
        let compiler = self.program.options();
        let conditions = tsr_module::get_conditions(compiler, mode);
        let mut seen_scope = false;
        let mut ancestor = directory.to_vec();
        loop {
            let mut parts = fragment.split(|&c| c == b'/');
            let first = parts.next().unwrap_or_default();
            let package = if first.starts_with(b"@") {
                parts.next().map(|p| [first, b"/", p].concat())
            } else {
                Some(first.to_vec())
            };
            let mut handled = false;
            if !directory_fragment(fragment).is_empty() && compiler.resolve_package_json_exports() {
                if let Some(package) = &package {
                    if compiler.resolve_package_json_imports() && package.starts_with(b"#") {
                        handled = true;
                    } else {
                        let base = path::resolve(&ancestor, &[b"node_modules", package]);
                        if let Some(json) = checker
                            .import_package_json(&path::resolve(&base, &[b"package.json"]))?
                        {
                            let subpath = fragment
                                .strip_prefix(package.as_slice())
                                .unwrap_or_default()
                                .strip_prefix(b"/")
                                .unwrap_or_default();
                            handled = self.exports_entries(
                                json.contents.get("exports"),
                                &conditions,
                                subpath,
                                &base,
                                options,
                                MappingKind::Exports,
                                result,
                            )?;
                        }
                    }
                }
            }
            if !handled {
                let base = path::resolve(&ancestor, &[b"node_modules"]);
                let resolved = path::resolve(&base, &[&directory_fragment(fragment)]);
                let mut package_dir = resolved.clone();
                let mut redirected = false;
                loop {
                    if let Some(json) = checker
                        .import_package_json(&path::resolve(&package_dir, &[b"package.json"]))?
                    {
                        let versions = json.version_paths();
                        if let Some(paths) = versions.paths() {
                            let prefix = [package_dir.as_slice(), b"/"].concat();
                            let relative =
                                resolved.strip_prefix(prefix.as_slice()).unwrap_or_default();
                            let relative = if relative.is_empty() {
                                Vec::new()
                            } else {
                                [relative, b"/"].concat()
                            };
                            redirected = self.mapping_entries(
                                paths,
                                &relative,
                                &package_dir,
                                options,
                                MappingKind::Paths,
                                result,
                            )?;
                        }
                        break;
                    }
                    let parent = path::directory(&package_dir);
                    if parent == package_dir {
                        break;
                    }
                    package_dir = parent;
                }
                if !redirected {
                    self.directory_path_entries(fragment, &base, b"", options, result)?;
                }
            }
            if compiler.resolve_package_json_imports() && !seen_scope {
                if let Some(json) =
                    checker.import_package_json(&path::resolve(&ancestor, &[b"package.json"]))?
                {
                    seen_scope = true;
                    self.exports_entries(
                        json.contents.get("imports"),
                        &conditions,
                        fragment,
                        &ancestor,
                        options,
                        MappingKind::Imports,
                        result,
                    )?;
                }
            }
            let parent = path::directory(&ancestor);
            if parent == ancestor {
                break;
            }
            ancestor = parent;
        }
        Ok(())
    }
}
