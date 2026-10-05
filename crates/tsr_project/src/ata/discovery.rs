use super::{display, js, CachedTyping, DisplayList, TypesRegistry, TypingsInfo};
use crate::logging::Logger;
use std::collections::{BTreeMap, BTreeSet};
use tsr_jsstring::JsString;
use tsr_module::package_json;
use tsr_semver::Version;
use tsr_tspath as path;
use tsr_vfs::{Error, FileSystem};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveredTypings {
    pub cached_typing_paths: Vec<JsString>,
    pub new_typing_names: Vec<JsString>,
    pub files_to_watch: Vec<JsString>,
}

pub(super) fn registry_version(versions: &BTreeMap<JsString, JsString>) -> Version {
    let tag = js(format!("ts{}", tsr_core::version_major_minor()));
    let version = versions.get(&tag).or_else(|| versions.get(&js("latest")));
    Version::must_parse(version.map_or(b"", JsString::as_bytes))
}

// port: tsc/internal/project/ata/discovertypings.go:isTypingUpToDate
pub fn is_typing_up_to_date(
    cached: &CachedTyping,
    versions: &BTreeMap<JsString, JsString>,
) -> bool {
    Version::compare(Some(&registry_version(versions)), cached.version.as_ref()).is_le()
}

// port: tsc/internal/project/ata/discovertypings.go:DiscoverTypings
pub fn discover_typings(
    fs: &dyn FileSystem,
    logger: &Logger,
    info: &TypingsInfo,
    file_names: &[JsString],
    project_root_path: &[u8],
    cached: &BTreeMap<JsString, CachedTyping>,
    registry: &TypesRegistry,
) -> Result<DiscoveredTypings, Error> {
    let mut inferred = BTreeMap::<JsString, JsString>::new();
    let files: Vec<_> = file_names
        .iter()
        .filter(|f| path::has_js_file_extension(f.as_bytes()))
        .collect();
    if let Some(include) = &info.type_acquisition.include {
        add_inferred_typings(
            logger,
            &mut inferred,
            include.iter().cloned(),
            "Explicitly included types",
        );
    }
    let mut result = DiscoveredTypings::default();
    if info.compiler_options.types.is_none() {
        let mut directories = BTreeSet::from([JsString::from_bytes(project_root_path)]);
        directories.extend(
            files
                .iter()
                .map(|f| JsString::from_bytes(path::directory(f.as_bytes()))),
        );
        for directory in directories {
            for (manifest, modules) in [
                (b"bower.json".as_slice(), b"bower_components".as_slice()),
                (b"package.json", b"node_modules"),
            ] {
                add_typing_names_and_get_files_to_watch(
                    fs,
                    logger,
                    &mut inferred,
                    &mut result.files_to_watch,
                    directory.as_bytes(),
                    manifest,
                    modules,
                )?;
            }
        }
    }
    if !info
        .type_acquisition
        .disable_filename_based_type_acquisition
        .is_true()
    {
        get_typing_names_from_source_file_names(logger, &mut inferred, &files);
    }
    let modules: BTreeSet<_> = info
        .unresolved_imports
        .iter()
        .map(|name| {
            JsString::from_bytes(
                tsr_core::node_modules::non_relative_module_name_for_typing_cache(name.as_bytes()),
            )
        })
        .collect();
    add_inferred_typings(
        logger,
        &mut inferred,
        modules,
        "Inferred typings from unresolved imports",
    );
    for name in info.type_acquisition.exclude.as_deref().unwrap_or_default() {
        inferred.remove(name);
        logger.log(format_args!(
            "ATA:: Typing for {} is in exclude list, will be ignored.",
            display(name)
        ));
    }
    for (name, cached) in cached {
        // The pin's empty-value lookup also admits cache entries absent from
        // inferredTypings, including ones removed by the exclude loop above.
        if inferred.get(name).is_none_or(JsString::is_empty)
            && registry
                .get(name)
                .is_some_and(|versions| is_typing_up_to_date(cached, versions))
        {
            inferred.insert(name.clone(), cached.typings_location.clone());
        }
    }
    for (name, location) in inferred {
        if location.is_empty() {
            result.new_typing_names.push(name);
        } else {
            result.cached_typing_paths.push(location);
        }
    }
    logger.log(format_args!("ATA:: Finished typings discovery: cachedTypingsPaths: {} newTypingNames: {}, filesToWatch {}", DisplayList(&result.cached_typing_paths), DisplayList(&result.new_typing_names), DisplayList(&result.files_to_watch)));
    Ok(result)
}

// port: tsc/internal/project/ata/discovertypings.go:addInferredTyping
fn add_inferred_typing(inferred: &mut BTreeMap<JsString, JsString>, name: JsString) {
    inferred.entry(name).or_default();
}

// port: tsc/internal/project/ata/discovertypings.go:addInferredTypings
fn add_inferred_typings(
    logger: &Logger,
    inferred: &mut BTreeMap<JsString, JsString>,
    names: impl IntoIterator<Item = JsString>,
    message: &str,
) {
    let names: Vec<_> = names.into_iter().collect();
    logger.log(format_args!("ATA:: {message}: {}", DisplayList(&names)));
    for name in names {
        add_inferred_typing(inferred, name);
    }
}

// port: tsc/internal/project/ata/discovertypings.go:getTypingNamesFromSourceFileNames
fn get_typing_names_from_source_file_names(
    logger: &Logger,
    inferred: &mut BTreeMap<JsString, JsString>,
    files: &[&JsString],
) {
    let mut has_jsx = false;
    let mut names = Vec::new();
    for file in files {
        has_jsx |= path::file_extension_is(file.as_bytes(), b".jsx");
        let lower = path::file_name_lower_case(path::base_name(file.as_bytes()));
        let name = remove_min_and_version_numbers(path::remove_any_file_extension(&lower));
        if let Some(typing) = super::types_map::typing_for_file_name(name) {
            names.push(JsString::from_bytes(typing));
        }
    }
    if !names.is_empty() {
        add_inferred_typings(logger, inferred, names, "Inferred typings from file names");
    }
    if has_jsx {
        logger.log(format_args!(
            "ATA:: Inferred 'react' typings due to presence of '.jsx' extension"
        ));
        add_inferred_typing(inferred, js("react"));
    }
}

// port: tsc/internal/project/ata/discovertypings.go:addTypingNamesAndGetFilesToWatch
fn add_typing_names_and_get_files_to_watch(
    fs: &dyn FileSystem,
    logger: &Logger,
    inferred: &mut BTreeMap<JsString, JsString>,
    watched: &mut Vec<JsString>,
    root: &[u8],
    manifest_name: &[u8],
    modules_name: &[u8],
) -> Result<(), Error> {
    let manifest_path = path::combine(root, &[manifest_name]);
    let mut names = Vec::new();
    if let Some(content) = fs.read_file(&manifest_path)? {
        watched.push(JsString::from_bytes(manifest_path.as_slice()));
        let manifest = package_json::parse(content.text.as_bytes());
        // DependencyFields uses the strict JSON decoder at the pin, whereas
        // packagejson.Parse (used for each dependency below) accepts duplicate
        // document keys. Keep the distinction at this manifest boundary.
        let mut validated = tsr_json::RawValue::default();
        if manifest.parseable
            && tsr_json::unmarshal(
                content.text.as_bytes(),
                &mut validated,
                tsr_json::Options::default(),
            )
            .is_ok()
        {
            for field in [
                "dependencies",
                "devDependencies",
                "optionalDependencies",
                "peerDependencies",
            ] {
                if let Some(entries) = manifest
                    .fields
                    .get(field)
                    .and_then(|value| value.as_object())
                {
                    names.extend(entries.keys().map(|key| js(key.as_str())));
                }
            }
            add_inferred_typings(
                logger,
                inferred,
                names.iter().cloned(),
                &format!(
                    "Typing names in '{}' dependencies",
                    String::from_utf8_lossy(&manifest_path)
                ),
            );
        }
    }
    let folder = path::combine(root, &[modules_name]);
    watched.push(JsString::from_bytes(folder.as_slice()));
    if !fs.directory_exists(&folder)? {
        return Ok(());
    }
    let mut manifests = Vec::new();
    if names.is_empty() {
        // Only top-level packages and one additional scope component qualify.
        // Avoid descending into ordinary packages' nested node_modules.
        for name in fs.entries(&folder)?.directories.unwrap_or_default() {
            let directory = path::combine(&folder, &[name.as_bytes()]);
            let direct = path::combine(&directory, &[manifest_name]);
            if fs.file_exists(&direct)? {
                manifests.push(direct);
            }
            if name.as_bytes().starts_with(b"@") {
                for child in fs.entries(&directory)?.directories.unwrap_or_default() {
                    let manifest = path::combine(&directory, &[child.as_bytes(), manifest_name]);
                    if fs.file_exists(&manifest)? {
                        manifests.push(manifest);
                    }
                }
            }
        }
    } else {
        manifests.extend(
            names
                .iter()
                .map(|name| path::combine(&folder, &[name.as_bytes(), manifest_name])),
        );
    }
    logger.log(format_args!(
        "ATA:: Searching for typing names in {}; all files: {}",
        String::from_utf8_lossy(&folder),
        DisplayList(
            &manifests
                .iter()
                .map(|p| JsString::from_bytes(p.as_slice()))
                .collect::<Vec<_>>()
        )
    ));
    let mut package_names = Vec::new();
    for manifest_path in manifests {
        let Some(content) = fs.read_file(&manifest_path)? else {
            continue;
        };
        let manifest = package_json::parse(content.text.as_bytes());
        let Some(name) = manifest
            .fields
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !manifest.parseable {
            continue;
        }
        let own_types = ["types", "typings"].into_iter().find_map(|key| {
            manifest
                .fields
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        });
        if let Some(own_types) = own_types {
            let absolute = path::absolute(own_types.as_bytes(), &path::directory(&manifest_path));
            if fs.file_exists(&absolute)? {
                logger.log(format_args!(
                    "ATA::     Package '{name}' provides its own types."
                ));
                inferred.insert(js(name), JsString::from_bytes(absolute));
            } else {
                logger.log(format_args!(
                    "ATA::     Package '{name}' provides its own types but they are missing."
                ));
            }
        } else {
            package_names.push(js(name));
        }
    }
    add_inferred_typings(logger, inferred, package_names, "    Found package names");
    Ok(())
}

// port: tsc/internal/project/ata/discovertypings.go:removeMinAndVersionNumbers
pub fn remove_min_and_version_numbers(name: &[u8]) -> &[u8] {
    let mut end = name.len();
    let mut position = end;
    while position > 0 {
        if name[position - 1].is_ascii_digit() {
            while position > 0 && name[position - 1].is_ascii_digit() {
                position -= 1;
            }
        } else if position > 4 && name[position - 3..position].eq_ignore_ascii_case(b"min") {
            position -= 3;
        } else {
            break;
        }
        if position == 0 || !matches!(name[position - 1], b'.' | b'-') {
            break;
        }
        position -= 1;
        end = position;
    }
    &name[..end]
}
