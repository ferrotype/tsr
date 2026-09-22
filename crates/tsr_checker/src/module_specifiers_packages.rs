//! Reverse package exports/imports mappings for declaration type references.
//! Ordered JSON entries and source matching quirks are preserved at the pin.
use super::{
    paths::{has_extension, js_extension, replace_star, Ending},
    Generation, ModulePath,
};
use crate::Error;
use tsr_core::{ModuleResolutionKind, PathMappings, ResolutionMode as Mode};
use tsr_module::package_json::Value;
use tsr_tspath as path;

#[derive(Clone, Copy)]
struct Parts {
    top_node_modules: usize,
    top_package: usize,
    package_root: Option<usize>,
}
#[derive(Clone, Copy)]
enum Matching {
    Exact,
    Directory,
    Pattern,
}
struct DirectoryResult {
    file: Vec<u8>,
    root: Option<Vec<u8>>,
    blocked: bool,
    verbatim: bool,
}
impl DirectoryResult {
    fn file(file: &[u8]) -> Self {
        Self {
            file: file.to_vec(),
            root: None,
            blocked: false,
            verbatim: false,
        }
    }
}

// port: tsc/internal/modulespecifiers/util.go:GetNodeModulePathParts
fn node_module_parts(file: &[u8]) -> Option<Parts> {
    let mut result = Parts {
        top_node_modules: 0,
        top_package: 0,
        package_root: Some(0),
    };
    let mut state = 0;
    let mut end = Some(0);
    while let Some(start) = end {
        end = file
            .get(start + 1..)
            .and_then(|s| s.iter().position(|b| *b == b'/'))
            .map(|p| start + 1 + p);
        match state {
            0 if file[start..].starts_with(b"/node_modules/") => {
                result.top_node_modules = start;
                result.top_package = end?;
                state = 1;
            }
            1 | 2 => {
                if state == 1 && file.get(start + 1) == Some(&b'@') {
                    state = 2;
                } else {
                    result.package_root = end;
                    state = 3;
                }
            }
            3 if file[start..].starts_with(b"/node_modules/") => state = 1,
            _ => {}
        }
    }
    (state > 1).then_some(result)
}
use tsr_module::package_name_from_types_package_name as package_name;

fn matching(key: &[u8]) -> Matching {
    if key.ends_with(b"/") {
        Matching::Directory
    } else if key.contains(&b'*') {
        Matching::Pattern
    } else {
        Matching::Exact
    }
}
fn subpaths(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let mut dot = false;
    let mut other = false;
    for key in object.keys() {
        match key.as_bytes().first() {
            Some(b'.') => dot = true,
            Some(b'#') | None => {}
            Some(_) => other = true,
        }
    }
    // Once a dot key exists, any condition key makes the object invalid.
    dot && !other
}
// The shared path crate owns these; the names stay local so call sites read the same.
use path::{
    change_extension, change_full_extension,
    has_implementation_ts_file_extension as implementation_ts,
};
fn ts_file(file: &[u8]) -> bool {
    path::file_extension_is_one_of(file, path::SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS)
}

impl Generation<'_> {
    fn package_paths_equal(&self, left: &[u8], right: &[u8]) -> bool {
        path::compare_paths(
            left,
            right,
            self.host.get_current_directory(),
            self.case_sensitive(),
        )
        .is_eq()
    }
    fn package_contains(&self, parent: &[u8], child: &[u8]) -> bool {
        path::contains_path(
            parent,
            child,
            self.host.get_current_directory(),
            self.case_sensitive(),
        )
    }
    fn package_relative(&self, parent: &[u8], child: &[u8]) -> Vec<u8> {
        path::relative_from_directory(
            parent,
            child,
            self.host.get_current_directory(),
            self.case_sensitive(),
        )
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameAsNodeModule
    pub(super) fn node_module_specifier(&self, candidate: &ModulePath) -> Result<Vec<u8>, Error> {
        let file = candidate.file_name.as_bytes();
        let Some(parts) = node_module_parts(file) else {
            return Ok(vec![]);
        };
        let endings = self.endings(Mode::NONE);
        let mut root_cursor = parts.package_root;
        let mut first_file = Vec::new();
        let (specifier, is_root) = loop {
            // The pinned loop advances its cursor but passes the original parts
            // to each directory attempt. Preserve that observable search rule.
            let attempt = self.package_directory(parts, candidate, &endings)?;
            if attempt.blocked {
                return Ok(vec![]);
            }
            if attempt.verbatim {
                return Ok(attempt.file);
            }
            if let Some(root) = attempt.root {
                break (root, true);
            }
            if first_file.is_empty() {
                first_file = attempt.file;
            }
            root_cursor = match root_cursor {
                Some(index) => file
                    .get(index + 1..)
                    .and_then(|s| s.iter().position(|b| *b == b'/'))
                    .map(|p| index + 1 + p),
                None => file.iter().position(|b| *b == b'/'),
            };
            if root_cursor.is_none() {
                break (self.process_ending(&first_file, &endings)?, false);
            }
        };
        if candidate.is_redirect && !is_root {
            return Ok(vec![]);
        }
        let top = &specifier[..parts.top_node_modules];
        let source_directory = path::directory(self.file);
        let global = self.host.get_global_typings_cache_location()?;
        if !self.prefix(&source_directory, top)
            || !global.is_empty() && self.prefix(global.as_bytes(), top)
        {
            return Ok(vec![]);
        }
        Ok(package_name(&specifier[parts.top_package + 1..]))
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryDirectoryWithPackageJson
    fn package_directory(
        &self,
        parts: Parts,
        candidate: &ModulePath,
        endings: &[Ending],
    ) -> Result<DirectoryResult, Error> {
        let file = candidate.file_name.as_bytes();
        let root = &file[..parts.package_root.unwrap_or(file.len())];
        let json_path = path::combine(root, &[b"package.json"]);
        let Some(package) = self.host.get_package_json_info(&json_path)? else {
            let mut result = DirectoryResult::file(file);
            let tail = &file[parts.package_root.map_or(0, |index| index + 1)..];
            if [
                b"index.d.ts".as_slice(),
                b"index.js",
                b"index.ts",
                b"index.tsx",
            ]
            .contains(&tail)
            {
                result.root = Some(root.to_vec());
            }
            return Ok(result);
        };
        let mut import_mode = self.mode;
        if self.options().resolve_package_json_exports() {
            let name = package_name(&root[parts.top_package + 1..]);
            // This Go pin intentionally selects exports conditions from the
            // target extension before the importing file's default mode.
            if has_extension(file, &[b".cjs", b".cts", b".d.cts"]) {
                import_mode = Mode::COMMON_JS;
            } else if has_extension(file, &[b".mjs", b".mts", b".d.mts"]) {
                import_mode = Mode::ESNEXT;
            }
            let conditions = tsr_module::get_conditions(self.options(), import_mode);
            if let Some(exports) = package.contents.get("exports") {
                let found = self.package_exports(file, root, &name, exports, &conditions)?;
                if !found.is_empty() {
                    return Ok(DirectoryResult {
                        file: found,
                        root: None,
                        blocked: false,
                        verbatim: true,
                    });
                }
                return Ok(DirectoryResult {
                    file: file.to_vec(),
                    root: None,
                    blocked: true,
                    verbatim: false,
                });
            }
        }
        let mut result = DirectoryResult::file(file);
        let mut maybe_blocked = false;
        let version_paths = package.version_paths();
        let versions = version_paths.paths();
        if let Some(paths) = versions {
            let submodule = &file[root.len() + 1..];
            let from = self.module_name_from_paths(submodule, paths, endings, root)?;
            if from.is_empty() {
                maybe_blocked = true;
            } else {
                result.file = path::combine(root, &[&from]);
            }
        }
        let main = package
            .string("typings")
            .or_else(|| package.string("types"))
            .or_else(|| package.string("main"))
            .unwrap_or(b"index.js");
        if !(main.is_empty()
            || maybe_blocked
                && versions.is_some_and(|paths| Self::package_matches_paths(main, paths)))
        {
            let main_path = path::to_path(main, root, self.case_sensitive());
            if self.package_paths_equal(
                path::remove_file_extension(main_path.as_bytes()),
                path::remove_file_extension(&result.file),
            ) || package.string("type") != Some(b"module")
                && !has_extension(
                    &result.file,
                    &[b".mts", b".d.mts", b".mjs", b".cts", b".d.cts", b".cjs"],
                )
                && self.prefix(&result.file, main_path.as_bytes())
                && self.package_paths_equal(
                    &path::directory(&result.file),
                    main_path
                        .as_bytes()
                        .strip_suffix(b"/")
                        .unwrap_or(main_path.as_bytes()),
                )
                && path::remove_file_extension(path::base_name(&result.file)) == b"index"
            {
                result.root = Some(root.to_vec());
            }
        }
        Ok(result)
    }

    // MatchPatternOrExact's only observed output here is whether a key matched.
    fn package_matches_paths(value: &[u8], paths: &PathMappings) -> bool {
        paths.iter().any(|(key, _)| {
            let key = key.as_bytes();
            if key == value {
                return true;
            }
            let Some(star) = key.iter().position(|b| *b == b'*') else {
                return false;
            };
            !key[star + 1..].contains(&b'*')
                && value.len() >= key.len() - 1
                && value.starts_with(&key[..star])
                && value.ends_with(&key[star + 1..])
        })
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameFromExports
    fn package_exports(
        &self,
        target: &[u8],
        directory: &[u8],
        name: &[u8],
        exports: &Value,
        conditions: &[tsr_ast::JsString],
    ) -> Result<Vec<u8>, Error> {
        if subpaths(exports) {
            for (key, value) in exports.as_object().expect("subpaths object") {
                let subname = path::absolute(&path::combine(name, &[key.as_bytes()]), b"");
                let found = self.package_mapping(
                    target,
                    directory,
                    &subname,
                    value,
                    conditions,
                    matching(key.as_bytes()),
                    false,
                    false,
                )?;
                if !found.is_empty() {
                    return Ok(found);
                }
            }
        }
        self.package_mapping(
            target,
            directory,
            name,
            exports,
            conditions,
            Matching::Exact,
            false,
            false,
        )
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameFromPackageJsonImports
    pub(super) fn package_imports(
        &self,
        target: &[u8],
        source_directory: &[u8],
        prefer_ts: bool,
    ) -> Result<Vec<u8>, Error> {
        if !self.options().resolve_package_json_imports() {
            return Ok(vec![]);
        }
        let Some(directory) = self
            .host
            .get_nearest_ancestor_directory_with_package_json(source_directory)?
        else {
            return Ok(vec![]);
        };
        let Some(package) = self
            .host
            .get_package_json_info(&path::combine(directory.as_bytes(), &[b"package.json"]))?
        else {
            return Ok(vec![]);
        };
        let Some(imports) = package.contents.get("imports").and_then(Value::as_object) else {
            return Ok(vec![]);
        };
        let conditions = tsr_module::get_conditions(self.options(), self.mode);
        for (key, value) in imports {
            let key = key.as_bytes();
            if key == b"#" || key == b"#/" || !key.starts_with(b"#") {
                continue;
            }
            if key.starts_with(b"#/")
                && ![
                    ModuleResolutionKind::NODE_NEXT,
                    ModuleResolutionKind::BUNDLER,
                ]
                .contains(&self.options().module_resolution_kind())
            {
                continue;
            }
            let found = self.package_mapping(
                target,
                directory.as_bytes(),
                key,
                value,
                &conditions,
                matching(key),
                true,
                prefer_ts,
            )?;
            if !found.is_empty() {
                return Ok(found);
            }
        }
        Ok(vec![])
    }

    // port: tsc/internal/modulespecifiers/specifiers.go:tryGetModuleNameFromExportsOrImports
    #[allow(
        clippy::too_many_arguments,
        reason = "Native recursive export/import inversion preserves independent matching and extension modes"
    )]
    fn package_mapping(
        &self,
        target: &[u8],
        directory: &[u8],
        name: &[u8],
        value: &Value,
        conditions: &[tsr_ast::JsString],
        mode: Matching,
        is_imports: bool,
        prefer_ts: bool,
    ) -> Result<Vec<u8>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.package_mapping_worker(
                target, directory, name, value, conditions, mode, is_imports, prefer_ts,
            )
        })
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Same parameters as the guarded native mapping operation"
    )]
    fn package_mapping_worker(
        &self,
        target: &[u8],
        directory: &[u8],
        name: &[u8],
        value: &Value,
        conditions: &[tsr_ast::JsString],
        mode: Matching,
        is_imports: bool,
        prefer_ts: bool,
    ) -> Result<Vec<u8>, Error> {
        match value {
            Value::String(text) => {
                let output = if is_imports {
                    self.host.get_output_js_file_name(target)?
                } else {
                    tsr_ast::JsString::default()
                };
                let declaration = if is_imports {
                    self.host.get_output_declaration_file_name(target)?
                } else {
                    tsr_ast::JsString::default()
                };
                let pattern = path::absolute(&path::combine(directory, &[text.as_bytes()]), b"");
                let swapped = if ts_file(target) {
                    change_extension(
                        target,
                        js_extension(target, self.options()).unwrap_or_default(),
                    )
                } else {
                    vec![]
                };
                let can_ts = prefer_ts && implementation_ts(target);
                match mode {
                    Matching::Exact => {
                        if !swapped.is_empty() && self.package_paths_equal(&swapped, &pattern)
                            || self.package_paths_equal(target, &pattern)
                            || !output.is_empty()
                                && self.package_paths_equal(output.as_bytes(), &pattern)
                            || !declaration.is_empty()
                                && self.package_paths_equal(declaration.as_bytes(), &pattern)
                        {
                            return Ok(name.to_vec());
                        }
                    }
                    Matching::Directory => {
                        // Keep the pin's reversed containment check only for the
                        // preferred implementation-TS arm; the other arms differ.
                        if can_ts && self.package_contains(target, &pattern) {
                            let fragment = self.package_relative(&pattern, target);
                            return Ok(path::absolute(
                                &path::combine(name, &[text.as_bytes(), &fragment]),
                                b"",
                            ));
                        }
                        if !swapped.is_empty() && self.package_contains(&pattern, &swapped) {
                            let fragment = self.package_relative(&pattern, &swapped);
                            return Ok(path::absolute(
                                &path::combine(name, &[text.as_bytes(), &fragment]),
                                b"",
                            ));
                        }
                        if !can_ts && self.package_contains(&pattern, target) {
                            let fragment = self.package_relative(&pattern, target);
                            return Ok(path::absolute(
                                &path::combine(name, &[text.as_bytes(), &fragment]),
                                b"",
                            ));
                        }
                        if !output.is_empty() && self.package_contains(&pattern, output.as_bytes())
                        {
                            return Ok(path::combine(
                                name,
                                &[&self.package_relative(&pattern, output.as_bytes())],
                            ));
                        }
                        if !declaration.is_empty()
                            && self.package_contains(&pattern, declaration.as_bytes())
                        {
                            let fragment = self.package_relative(&pattern, declaration.as_bytes());
                            let extension = js_extension(declaration.as_bytes(), self.options())
                                .unwrap_or(b".js");
                            return Ok(path::combine(
                                name,
                                &[&change_extension(&fragment, extension)],
                            ));
                        }
                    }
                    Matching::Pattern => {
                        let star = pattern
                            .iter()
                            .position(|b| *b == b'*')
                            .unwrap_or(pattern.len());
                        let (prefix, suffix) = (
                            &pattern[..star],
                            pattern.get(star + 1..).unwrap_or_default(),
                        );
                        let matches = |file: &[u8]| {
                            file.len() >= prefix.len() + suffix.len()
                                && self.prefix(file, prefix)
                                && self.suffix(file, suffix)
                        };
                        let substitute = |file: &[u8]| {
                            replace_star(name, &file[prefix.len()..file.len() - suffix.len()])
                        };
                        if can_ts && matches(target) {
                            return Ok(substitute(target));
                        }
                        if !swapped.is_empty() && matches(&swapped) {
                            return Ok(substitute(&swapped));
                        }
                        if !can_ts && matches(target) {
                            return Ok(substitute(target));
                        }
                        if !output.is_empty() && matches(output.as_bytes()) {
                            return Ok(substitute(output.as_bytes()));
                        }
                        if !declaration.is_empty() && matches(declaration.as_bytes()) {
                            if let Some(extension) =
                                js_extension(declaration.as_bytes(), self.options())
                            {
                                return Ok(change_full_extension(
                                    &substitute(declaration.as_bytes()),
                                    extension,
                                ));
                            }
                        }
                    }
                }
                Ok(vec![])
            }
            Value::Array(values) => {
                for value in values {
                    let found = self.package_mapping(
                        target, directory, name, value, conditions, mode, is_imports, prefer_ts,
                    )?;
                    if !found.is_empty() {
                        return Ok(found);
                    }
                }
                Ok(vec![])
            }
            Value::Object(object) => {
                let types = conditions.iter().any(|value| value.as_bytes() == b"types");
                for (key, value) in object {
                    if key == "default"
                        || conditions
                            .iter()
                            .any(|value| value.as_bytes() == key.as_bytes())
                        || types && tsr_module::is_applicable_versioned_types_key(key.as_bytes())
                    {
                        let found = self.package_mapping(
                            target, directory, name, value, conditions, mode, is_imports, prefer_ts,
                        )?;
                        if !found.is_empty() {
                            return Ok(found);
                        }
                    }
                }
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}
