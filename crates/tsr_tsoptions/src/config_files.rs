use crate::{
    glob::{SpecMatcher, Usage},
    ConfigFileSpecs,
};
use tsr_core::{collections::OrderedMap, CompilerOptions};
use tsr_jsstring::JsString;
use tsr_vfs::{Error, FileSystem};
fn key(value: &[u8], case_sensitive: bool) -> JsString {
    if case_sensitive {
        JsString::from_bytes(value)
    } else {
        JsString::from_bytes(tsr_tspath::file_name_lower_case(value).into_owned())
    }
}
type FileMap = OrderedMap<JsString, JsString>;
fn extension_is(file: &[u8], extension: &[u8]) -> bool {
    file.len() > extension.len() && file.ends_with(extension)
}
fn changed_extension(file: &[u8], extension: &[u8]) -> Vec<u8> {
    let base = tsr_tspath::remove_file_extension(file);
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
            map.remove(&key);
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
    let base = tsr_tspath::normalize(base);
    let case_sensitive = host.use_case_sensitive_file_names();
    let (mut literal, mut wildcard, mut json) =
        (FileMap::default(), FileMap::default(), FileMap::default());
    let supported = crate::supported_extensions(options, extra);
    for file in &specs.validated_files {
        literal.insert(
            key(file.as_bytes(), case_sensitive),
            JsString::from_bytes(tsr_tspath::absolute(file.as_bytes(), &base)),
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
                    if !literal.contains_key(&key) && !json.contains_key(&key) {
                        json.insert(key, file);
                    }
                }
                continue;
            }
            if has_higher(bytes, &supported, |name| {
                let key = key(name, case_sensitive);
                literal.contains_key(&key) || wildcard.contains_key(&key)
            }) {
                continue;
            }
            remove_lower(bytes, &mut wildcard, &supported, case_sensitive);
            let key = key(bytes, case_sensitive);
            if !literal.contains_key(&key) && !wildcard.contains_key(&key) {
                wildcard.insert(key, file);
            }
        }
    }
    let count = literal.len();
    Ok((
        literal
            .into_values()
            .chain(wildcard.into_values())
            .chain(json.into_values())
            .collect(),
        count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_core::Tristate;
    use tsr_vfs::MemoryBuilder;

    #[test]
    fn file_groups_keep_literal_position_casing_and_extension_priority() {
        let mut host = MemoryBuilder::new(b"/project", false);
        for name in ["a.ts", "z.ts", "main.js", "main.ts", "data.json"] {
            host.insert_loaded(name.as_bytes(), b"".as_slice());
        }
        let specs = ConfigFileSpecs {
            // Overwrite a case-insensitive key without moving its position.
            validated_files: ["/project/A.ts", "/project/z.ts", "/project/a.ts"]
                .map(|s| JsString::from_bytes(s.as_bytes()))
                .to_vec(),
            // Encounter JS first, then remove it when the TS wildcard arrives.
            validated_includes: ["*.js", "*.ts", "*.json"]
                .map(|s| JsString::from_bytes(s.as_bytes()))
                .to_vec(),
            ..ConfigFileSpecs::default()
        };
        let options = CompilerOptions {
            allow_js: Tristate::TRUE,
            resolve_json_module: Tristate::TRUE,
            ..CompilerOptions::default()
        };
        let (files, literal_count) =
            file_names_from_specs(&specs, b"/project", &options, &host.finish(), &[]).unwrap();
        assert_eq!(literal_count, 2);
        assert_eq!(
            files.iter().map(JsString::as_bytes).collect::<Vec<_>>(),
            [
                b"/project/a.ts".as_slice(),
                b"/project/z.ts",
                b"/project/main.ts",
                b"/project/data.json",
            ]
        );
    }
}
