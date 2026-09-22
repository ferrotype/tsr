//! Directory watches derived from validated include and exclude specifications.
use crate::{
    glob::{SpecMatcher, Usage},
    ParsedCommandLine,
};
use tsr_core::collections::OrderedMap;
use tsr_jsstring::{helpers::to_lower_go, JsString};
use tsr_tspath as path;
/// port: tsc/internal/tsoptions/wildcarddirectories.go:toCanonicalKey
pub fn canonical_key(value: &[u8], sensitive: bool) -> JsString {
    JsString::from_bytes(if sensitive {
        value.to_vec()
    } else {
        to_lower_go(value)
    })
}
/// Source type: tsc/internal/tsoptions/wildcarddirectories.go:wildcardDirectoryMatch
pub struct WildcardDirectory {
    pub key: JsString,
    pub path: JsString,
    pub recursive: bool,
}
/// port: tsc/internal/tsoptions/wildcarddirectories.go:getWildcardDirectoryFromSpec
pub fn wildcard_directory_from_spec(spec: &[u8], sensitive: bool) -> Option<WildcardDirectory> {
    if let Some(first) = spec.iter().position(|b| matches!(b, b'*' | b'?')) {
        if let Some(last) = spec[..first].iter().rposition(|b| *b == b'/') {
            let directory = &spec[..last];
            return Some(WildcardDirectory {
                key: canonical_key(directory, sensitive),
                path: JsString::from_bytes(directory),
                recursive: first < spec.iter().rposition(|b| *b == b'/').expect("separator"),
            });
        }
    }
    if let Some(last) = spec.iter().rposition(|b| *b == b'/') {
        if crate::glob::is_implicit_glob(&spec[last + 1..]) {
            let directory = path::remove_trailing_directory_separator(spec);
            return Some(WildcardDirectory {
                key: canonical_key(directory, sensitive),
                path: JsString::from_bytes(directory),
                recursive: true,
            });
        }
    }
    None
}
/// Preserves first-insertion spelling and order while promoting recursive watches.
/// Go returns an unordered map; retaining its include traversal order also serves
/// callers that render a deterministic test envelope.
/// port: tsc/internal/tsoptions/wildcarddirectories.go:getWildcardDirectories
pub fn wildcard_directories(
    include: &[JsString],
    exclude: &[JsString],
    cwd: &[u8],
    sensitive: bool,
) -> Option<OrderedMap<JsString, bool>> {
    if include.is_empty() {
        return None;
    }
    let excluded = SpecMatcher::new(exclude, cwd, Usage::Exclude, sensitive);
    let mut directories = OrderedMap::default();
    let mut paths = OrderedMap::<JsString, JsString>::default();
    let mut recursive = Vec::new();
    for spec in include {
        let spec = path::normalize(&path::combine(cwd, &[spec.as_bytes()])).into_owned();
        if excluded.as_ref().is_some_and(|m| m.matches(&spec)) {
            continue;
        }
        if let Some(found) = wildcard_directory_from_spec(&spec, sensitive) {
            let existing = paths.get(&found.key).cloned();
            if existing
                .as_ref()
                .is_none_or(|p| !directories.get(p).copied().unwrap_or(false) && found.recursive)
            {
                directories.insert(
                    existing.clone().unwrap_or_else(|| found.path.clone()),
                    found.recursive,
                );
                if existing.is_none() {
                    paths.insert(found.key.clone(), found.path);
                }
                if found.recursive {
                    recursive.push(found.key);
                }
            }
        }
        let remove: Vec<_> = directories
            .iter()
            .filter_map(|(p, _)| {
                let key = canonical_key(p.as_bytes(), sensitive);
                recursive
                    .iter()
                    .any(|r| {
                        *r != key
                            && path::contains_path(r.as_bytes(), key.as_bytes(), cwd, sensitive)
                    })
                    .then(|| p.clone())
            })
            .collect();
        for key in remove {
            directories.remove(&key);
        }
    }
    Some(directories)
}
impl ParsedCommandLine {
    /// Validated specifications are immutable between explicit replacements.
    pub fn config_specs(&self) -> Option<&crate::ConfigFileSpecs> {
        self.config_specs.as_ref()
    }
    /// Replace the complete matching context before publishing a snapshot.
    /// Exclusive access prevents a reader retaining a stale cache reference.
    pub fn set_config_specs(
        &mut self,
        specs: Option<crate::ConfigFileSpecs>,
        base: JsString,
        case_sensitive: bool,
    ) {
        self.config_specs = specs;
        self.config_base_path = base;
        self.config_case_sensitive = case_sensitive;
        self.wildcard_directories_cache.take();
        self.caches.globs.take();
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetCurrentDirectory
    pub fn current_directory(&self) -> &[u8] {
        self.config_base_path.as_bytes()
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.UseCaseSensitiveFileNames
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.config_case_sensitive
    }
    /// Cached once per immutable matching context. Clone owns its own cache;
    /// changing a clone's specs cannot change another published result.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WildcardDirectories
    pub fn wildcard_directories(&self) -> Option<&OrderedMap<JsString, bool>> {
        self.wildcard_directories_cache
            .get_or_init(|| {
                // Raw JSON also retains validated specs and needs no syntax hook.
                let specs = self.config_specs.as_ref()?;
                wildcard_directories(
                    &specs.validated_includes,
                    &specs.validated_excludes,
                    self.config_base_path.as_bytes(),
                    self.config_case_sensitive,
                )
            })
            .as_ref()
    }
}
