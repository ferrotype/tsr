//! Directory watches derived from validated include and exclude specifications.
use crate::{
    glob::{SpecMatcher, Usage},
    ParsedCommandLine,
};
use tsr_core::collections::OrderedMap;
use tsr_jsstring::{helpers::to_lower_go, JsString};
use tsr_tspath as path;
fn canonical(value: &[u8], sensitive: bool) -> JsString {
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
                key: canonical(directory, sensitive),
                path: JsString::from_bytes(directory),
                recursive: first < spec.iter().rposition(|b| *b == b'/').expect("separator"),
            });
        }
    }
    if let Some(last) = spec.iter().rposition(|b| *b == b'/') {
        if crate::glob::is_implicit_glob(&spec[last + 1..]) {
            let directory = path::remove_trailing_directory_separator(spec);
            return Some(WildcardDirectory {
                key: canonical(directory, sensitive),
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
                let key = canonical(p.as_bytes(), sensitive);
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
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WildcardDirectories
    pub fn wildcard_directories(&self) -> Option<OrderedMap<JsString, bool>> {
        // The raw-value entry point also retains validated specs, so it does
        // not require the native test hook that recovers them without syntax.
        let specs = self.config_specs.as_ref()?;
        wildcard_directories(
            &specs.validated_includes,
            &specs.validated_excludes,
            self.config_base_path.as_bytes(),
            self.config_case_sensitive,
        )
    }
}
