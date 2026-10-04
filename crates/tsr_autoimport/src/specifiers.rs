use tsr_checker::{Error, ModuleSpecifierEnding as Preferred, Operation};
use tsr_module::{Ending, ResolvedEntrypoint};
use tsr_tspath as path;

pub fn for_package(
    export: &crate::Export,
    checker: &mut Operation<'_>,
    source: tsr_ast::NodeId,
    options: &tsr_core::CompilerOptions,
    preferences: &crate::Preferences,
) -> Result<Option<tsr_jsstring::JsString>, Error> {
    let mode = checker.import_file_module_formats(source)?.1;
    let conditions = tsr_module::get_conditions(options, mode);
    let ending = checker.import_ending_preferences(source, mode, preferences.ending.as_deref())?[0];
    for entry in export.entrypoints.iter() {
        if entry
            .include_conditions
            .as_ref()
            .is_some_and(|s| !s.iter().all(|c| conditions.contains(c)))
            || entry
                .exclude_conditions
                .as_ref()
                .is_some_and(|s| s.iter().any(|c| conditions.contains(c)))
        {
            continue;
        }
        let specifier = process_ending(entry, ending, options);
        if !preferences.excludes(&specifier) {
            return Ok(Some(tsr_jsstring::JsString::from_bytes(specifier)));
        }
    }
    Ok(None)
}
// port: tsc/internal/modulespecifiers/util.go:ProcessEntrypointEnding
pub fn process_ending(
    entry: &ResolvedEntrypoint,
    preferred: Preferred,
    options: &tsr_core::CompilerOptions,
) -> Vec<u8> {
    let specifier = entry.module_specifier.as_bytes();
    if entry.ending == Ending::Fixed {
        return specifier.to_vec();
    }
    let ext = path::try_get_extension_from_path(specifier);
    let minimal = matches!(preferred, Preferred::Minimal | Preferred::Index);
    let declaration = matches!(ext, b".d.ts" | b".d.mts" | b".d.cts");
    if declaration
        || matches!(
            ext,
            b".ts" | b".tsx" | b".mts" | b".cts" | b".js" | b".jsx" | b".mjs" | b".cjs"
        )
    {
        if minimal && entry.ending == Ending::Changeable && (!declaration || ext == b".d.ts") {
            let base = &specifier[..specifier.len() - ext.len()];
            return if preferred == Preferred::Minimal {
                base.strip_suffix(b"/index").unwrap_or(base).to_vec()
            } else {
                base.to_vec()
            };
        }
        if declaration
            || preferred != Preferred::Ts && matches!(ext, b".ts" | b".tsx" | b".mts" | b".cts")
        {
            let js = tsr_module::js_extension_for_file(specifier, options);
            if !js.is_empty() {
                return [&specifier[..specifier.len() - ext.len()], js].concat();
            }
        }
    }
    specifier.to_vec()
}
