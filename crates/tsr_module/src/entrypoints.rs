//! Reverse package export discovery, retaining condition and symlink provenance.
use crate::{
    resolver::{DTS, TS},
    Error, PackageJson, Resolver,
};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};
use tsr_jsstring::{compare::has_prefix_and_suffix_without_overlap, JsString};
use tsr_tspath as path;

type Conditions = Option<Arc<BTreeSet<JsString>>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Ending {
    Fixed,
    ExtensionChangeable,
    Changeable,
}
#[derive(Clone, Debug)]
pub struct ResolvedEntrypoint {
    pub original_file_name: JsString,
    pub resolved_file_name: JsString,
    pub module_specifier: JsString,
    pub ending: Ending,
    pub include_conditions: Conditions,
    pub exclude_conditions: Conditions,
}
impl ResolvedEntrypoint {
    /// port: tsc/internal/module/resolver.go:ResolvedEntrypoint.SymlinkOrRealpath
    pub fn symlink_or_realpath(&self) -> &[u8] {
        if self.original_file_name.is_empty() {
            self.resolved_file_name.as_bytes()
        } else {
            self.original_file_name.as_bytes()
        }
    }
}
struct Target<'a> {
    subpath: &'a [u8],
    include: Conditions,
    exclude: Conditions,
    value: &'a Value,
}
impl Resolver {
    /// port: tsc/internal/module/resolver.go:Resolver.GetEntrypointsFromPackageJsonInfo
    pub fn entrypoints(
        &mut self,
        package: &PackageJson,
        name: &[u8],
        directory_search: bool,
    ) -> Result<Vec<ResolvedEntrypoint>, Error> {
        if let Some(exports) = package.contents.get("exports") {
            return self.export_entrypoints(package, name, exports);
        }
        let mut result = Vec::new();
        let main = self.directory(TS | DTS, package.directory.as_bytes(), Some(package), false)?;
        if let Some(main) = main.as_ref().filter(|main| main.is_resolved()) {
            result.push(self.entrypoint(
                main.resolved_file_name.as_bytes(),
                name,
                None,
                None,
                Ending::Fixed,
            )?);
        }
        if directory_search {
            for file in self.entrypoint_files(
                package,
                &[JsString::from_bytes(b"node_modules".as_slice())],
                b"**/*",
            )? {
                let sensitive = self.host.use_case_sensitive_file_names();
                if main.as_ref().is_some_and(|main| {
                    main.is_resolved()
                        && path::compare_paths(
                            file.as_bytes(),
                            main.resolved_file_name.as_bytes(),
                            b"",
                            sensitive,
                        )
                        .is_eq()
                }) {
                    continue;
                }
                let relative = path::relative_from_directory(
                    package.directory.as_bytes(),
                    file.as_bytes(),
                    b"",
                    sensitive,
                );
                result.push(self.entrypoint(
                    file.as_bytes(),
                    &path::resolve(name, &[&relative]),
                    None,
                    None,
                    Ending::Changeable,
                )?);
            }
        }
        Ok(result)
    }
    fn entrypoint_files(
        &self,
        package: &PackageJson,
        exclude: &[JsString],
        include: &[u8],
    ) -> Result<Vec<JsString>, Error> {
        let extensions: Vec<_> = path::SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS
            .iter()
            .chain(path::SUPPORTED_DECLARATION_EXTENSIONS)
            .map(|&ext| JsString::from_bytes(ext))
            .collect();
        Ok(tsr_tsoptions::glob::read_directory(
            self.host.as_ref(),
            self.cwd.as_bytes(),
            package.directory.as_bytes(),
            &extensions,
            exclude,
            &[JsString::from_bytes(include)],
            tsr_tsoptions::glob::UNLIMITED_DEPTH,
        )?)
    }
    /// port: tsc/internal/module/resolver.go:Resolver.createResolvedEntrypointHandlingSymlink
    fn entrypoint(
        &self,
        file: &[u8],
        specifier: &[u8],
        include: Conditions,
        exclude: Conditions,
        ending: Ending,
    ) -> Result<ResolvedEntrypoint, Error> {
        let real = self.host.realpath(file)?;
        Ok(ResolvedEntrypoint {
            original_file_name: if real.as_bytes() == file {
                JsString::default()
            } else {
                JsString::from_bytes(file)
            },
            resolved_file_name: real,
            module_specifier: JsString::from_bytes(specifier),
            include_conditions: include,
            exclude_conditions: exclude,
            ending,
        })
    }
    /// An explicit work stack preserves native depth-first order without
    /// consuming a Rust call frame for each user-controlled nested condition.
    /// port: tsc/internal/module/resolver.go:resolutionState.loadEntrypointsFromExportMap
    fn export_entrypoints(
        &mut self,
        package: &PackageJson,
        name: &[u8],
        exports: &Value,
    ) -> Result<Vec<ResolvedEntrypoint>, Error> {
        let mut pending = Vec::new();
        if crate::package_json::object_kind(exports)
            == Some(crate::package_json::ObjectKind::Subpaths)
        {
            for (subpath, value) in exports.as_object().expect("subpath object").iter().rev() {
                pending.push(Target {
                    subpath: subpath.as_bytes(),
                    include: None,
                    exclude: None,
                    value,
                });
            }
        } else {
            pending.push(Target {
                subpath: b".",
                include: None,
                exclude: None,
                value: exports,
            });
        }
        let mut result = Vec::new();
        while let Some(target) = pending.pop() {
            match target.value {
                Value::String(value) if value.starts_with("./") => {
                    let value = value.as_bytes();
                    let ending = if value.ends_with(b"*") {
                        Ending::ExtensionChangeable
                    } else {
                        Ending::Fixed
                    };
                    if let Some(star) = value.iter().position(|&b| b == b'*') {
                        if value[star + 1..].contains(&b'*') {
                            continue;
                        }
                        let pattern = path::resolve(package.directory.as_bytes(), &[value]);
                        // The original target has one star; normalization can
                        // remove its segment, just as strings.Cut can fail in Go.
                        let split = pattern.iter().position(|&b| b == b'*');
                        let (leading, trailing) = split
                            .map_or((pattern.as_slice(), b"".as_slice()), |i| {
                                (&pattern[..i], &pattern[i + 1..])
                            });
                        let include = path::change_full_extension(
                            &crate::paths::replace_first(value, b"**/*"),
                            b".*",
                        );
                        for file in self.entrypoint_files(package, &[], &include)? {
                            if let Some(matched) =
                                self.matched_entrypoint_star(file.as_bytes(), leading, trailing)
                            {
                                let subpath = crate::paths::replace_first(target.subpath, &matched);
                                result.push(self.entrypoint(
                                    file.as_bytes(),
                                    &path::resolve(name, &[&subpath]),
                                    target.include.clone(),
                                    target.exclude.clone(),
                                    ending,
                                )?);
                            }
                        }
                    } else {
                        let parts = path::path_components(value, b"");
                        if parts[2..]
                            .iter()
                            .any(|part| matches!(part.as_slice(), b".." | b"." | b"node_modules"))
                        {
                            continue;
                        }
                        let resolved = path::resolve(package.directory.as_bytes(), &[value]);
                        if let Some(file) = self
                            .package_field(TS | DTS, &resolved, value)?
                            .filter(crate::ResolvedModule::is_resolved)
                        {
                            result.push(self.entrypoint(
                                file.resolved_file_name.as_bytes(),
                                &path::resolve(name, &[target.subpath]),
                                target.include,
                                target.exclude,
                                ending,
                            )?);
                        }
                    }
                }
                Value::Array(values) => {
                    for value in values.iter().rev() {
                        pending.push(Target {
                            subpath: target.subpath,
                            include: target.include.clone(),
                            exclude: target.exclude.clone(),
                            value,
                        });
                    }
                }
                Value::Object(values) => {
                    let mut previous = Vec::new();
                    let mut exclude = target.exclude;
                    let mut children = Vec::new();
                    for (condition, value) in values {
                        let key = JsString::from_bytes(condition.as_bytes());
                        if exclude.as_ref().is_some_and(|set| set.contains(&key)) {
                            continue;
                        }
                        let always = condition == "default"
                            || condition == "types"
                            || crate::is_applicable_versioned_types_key(condition.as_bytes());
                        let mut include = target.include.clone();
                        if !always {
                            Arc::make_mut(include.get_or_insert_with(|| Arc::new(BTreeSet::new())))
                                .insert(key.clone());
                            if !previous.is_empty() {
                                Arc::make_mut(
                                    exclude.get_or_insert_with(|| Arc::new(BTreeSet::new())),
                                )
                                .extend(previous.iter().cloned());
                            }
                        }
                        previous.push(key);
                        children.push(Target {
                            subpath: target.subpath,
                            include,
                            exclude: exclude.clone(),
                            value,
                        });
                        if always {
                            break;
                        }
                    }
                    pending.extend(children.into_iter().rev());
                }
                _ => {}
            }
        }
        Ok(result)
    }
    /// port: tsc/internal/module/resolver.go:resolutionState.getMatchedStarForPatternEntrypoint
    fn matched_entrypoint_star(
        &self,
        file: &[u8],
        leading: &[u8],
        trailing: &[u8],
    ) -> Option<Vec<u8>> {
        let sensitive = self.host.use_case_sensitive_file_names();
        let matched = |file: &[u8]| {
            has_prefix_and_suffix_without_overlap(file, leading, trailing, sensitive)
                .then(|| file[leading.len()..file.len() - trailing.len()].to_vec())
        };
        matched(file).or_else(|| {
            let ext = crate::js_extension_for_file(file, &self.options);
            (!ext.is_empty())
                .then(|| matched(&path::change_full_extension(file, ext)))
                .flatten()
        })
    }
}
