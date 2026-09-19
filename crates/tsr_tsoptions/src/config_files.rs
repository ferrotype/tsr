use crate::{
    glob::{SpecMatcher, Usage},
    ConfigFileSpecs,
};
use ts_core::CompilerOptions;
use ts_jsstring::JsString;
use ts_vfs::{Error, FileSystem};
fn key(value: &[u8], case_sensitive: bool) -> JsString {
    if case_sensitive {
        JsString::from_bytes(value)
    } else {
        JsString::from_bytes(ts_tspath::file_name_lower_case(value).into_owned())
    }
}
type FileMap = Vec<(JsString, JsString)>;
fn has(map: &FileMap, key: &JsString) -> bool {
    map.iter().any(|(k, _)| k == key)
}
fn set(map: &mut FileMap, key: JsString, value: JsString) {
    if let Some((_, v)) = map.iter_mut().find(|(k, _)| *k == key) {
        *v = value;
    } else {
        map.push((key, value));
    }
}
fn extension_is(file: &[u8], extension: &[u8]) -> bool {
    file.len() > extension.len() && file.ends_with(extension)
}
fn changed_extension(file: &[u8], extension: &[u8]) -> Vec<u8> {
    let base = ts_tspath::remove_file_extension(file);
    if base.len() == file.len() {
        return file.to_vec();
    }
    let mut result = base.to_vec();
    result.extend_from_slice(extension);
    result
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:hasFileWithHigherPriorityExtension
fn has_higher(file: &[u8], extensions: &[Vec<JsString>], has_file: impl Fn(&[u8]) -> bool) -> bool {
    for group in extensions
        .iter()
        .filter(|group| group.iter().any(|ext| extension_is(file, ext.as_bytes())))
    {
        for ext in group {
            let ext = ext.as_bytes();
            if extension_is(file, ext) && (ext != b".ts" || !extension_is(file, b".d.ts")) {
                return false;
            }
            if has_file(&changed_extension(file, ext)) {
                if ext == b".d.ts" && (extension_is(file, b".js") || extension_is(file, b".jsx")) {
                    continue;
                }
                return true;
            }
        }
    }
    false
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:removeWildcardFilesWithLowerPriorityExtension
fn remove_lower(
    file: &[u8],
    map: &mut FileMap,
    extensions: &[Vec<JsString>],
    case_sensitive: bool,
) {
    for group in extensions
        .iter()
        .rev()
        .filter(|group| group.iter().any(|ext| extension_is(file, ext.as_bytes())))
    {
        for ext in group.iter().rev() {
            if extension_is(file, ext.as_bytes()) {
                return;
            }
            let key = key(&changed_extension(file, ext.as_bytes()), case_sensitive);
            map.retain(|(k, _)| *k != key);
        }
    }
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getFileNamesFromConfigSpecs
pub fn file_names_from_specs(
    specs: &ConfigFileSpecs,
    base: &[u8],
    options: &CompilerOptions,
    host: &dyn FileSystem,
    extra: &[JsString],
) -> Result<(Vec<JsString>, usize), Error> {
    let base = ts_tspath::normalize(base);
    let case_sensitive = host.use_case_sensitive_file_names();
    let (mut literal, mut wildcard, mut json) = (FileMap::new(), FileMap::new(), FileMap::new());
    let supported = crate::supported_extensions(options, extra);
    for file in &specs.validated_files {
        set(
            &mut literal,
            key(file.as_bytes(), case_sensitive),
            JsString::from_bytes(ts_tspath::absolute(file.as_bytes(), &base)),
        );
    }
    let json_specs: Vec<_> = specs
        .validated_includes
        .iter()
        .filter(|spec| spec.as_bytes().ends_with(b".json"))
        .cloned()
        .collect();
    let mut json_matcher = None;
    if !specs.validated_includes.is_empty() {
        let extensions: Vec<_> = crate::supported_extensions_with_json(options, extra)
            .into_iter()
            .flatten()
            .collect();
        for file in crate::glob::read_directory(
            host,
            &base,
            &base,
            &extensions,
            &specs.validated_excludes,
            &specs.validated_includes,
            crate::glob::UNLIMITED_DEPTH,
        )? {
            let bytes = file.as_bytes();
            if extension_is(bytes, b".json") {
                if json_matcher.is_none() {
                    json_matcher =
                        SpecMatcher::new(&json_specs, &base, Usage::Files, case_sensitive);
                }
                if json_matcher
                    .as_ref()
                    .is_some_and(|matcher| matcher.matches(bytes))
                {
                    let key = key(bytes, case_sensitive);
                    if !has(&literal, &key) && !has(&json, &key) {
                        set(&mut json, key, file);
                    }
                }
                continue;
            }
            if has_higher(bytes, &supported, |name| {
                let key = key(name, case_sensitive);
                has(&literal, &key) || has(&wildcard, &key)
            }) {
                continue;
            }
            remove_lower(bytes, &mut wildcard, &supported, case_sensitive);
            let key = key(bytes, case_sensitive);
            if !has(&literal, &key) && !has(&wildcard, &key) {
                set(&mut wildcard, key, file);
            }
        }
    }
    let count = literal.len();
    Ok((
        literal
            .into_iter()
            .chain(wildcard)
            .chain(json)
            .map(|(_, v)| v)
            .collect(),
        count,
    ))
}
