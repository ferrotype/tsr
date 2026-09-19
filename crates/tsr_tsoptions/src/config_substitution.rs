use crate::ConfigValue;
use ts_core::CompilerOptions;
use ts_jsstring::JsString;
const TEMPLATE: &[u8] = b"${configDir}";
/// port: tsc/internal/tsoptions/tsconfigparsing.go:startsWithConfigDirTemplate
pub fn starts_with_config_dir(value: &[u8]) -> bool {
    ts_jsstring::helpers::to_lower_go(value).starts_with(b"${configdir}")
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getSubstitutedPathWithConfigDirTemplate
pub fn substitute_path(value: &[u8], base: &[u8]) -> JsString {
    // The predicate ignores case, but strings.Replace is case sensitive.
    let mut result = value.to_vec();
    if let Some(index) = value
        .windows(TEMPLATE.len())
        .position(|part| part == TEMPLATE)
    {
        result.splice(index..index + TEMPLATE.len(), b"./".iter().copied());
    }
    JsString::from_bytes(ts_tspath::absolute(&result, base))
}
pub fn substitute_strings(values: &mut [JsString], base: &[u8]) {
    for value in values {
        if starts_with_config_dir(value.as_bytes()) {
            *value = substitute_path(value.as_bytes(), base);
        }
    }
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:handleOptionConfigDirTemplateSubstitution
pub fn substitute_options(options: &mut CompilerOptions, base: &[u8]) {
    if let Some(paths) = &mut options.paths {
        for (_, values) in paths {
            if let Some(values) = values {
                substitute_strings(values, base);
            }
        }
    }
    for values in [&mut options.root_dirs, &mut options.type_roots]
        .into_iter()
        .flatten()
    {
        substitute_strings(values, base);
    }
    for value in [
        &mut options.generate_cpu_profile,
        &mut options.generate_trace,
        &mut options.out_file,
        &mut options.out_dir,
        &mut options.root_dir,
        &mut options.ts_build_info_file,
        &mut options.base_url,
        &mut options.declaration_dir,
    ] {
        if starts_with_config_dir(value.as_bytes()) {
            *value = substitute_path(value.as_bytes(), base);
        }
    }
}
pub(crate) fn inherited_specs(
    value: &ConfigValue,
    extended: &[u8],
    base: &[u8],
    case_sensitive: bool,
) -> ConfigValue {
    let ConfigValue::Array(Some(values)) = value else {
        return value.clone();
    };
    let relative = ts_tspath::relative_from_directory(
        base,
        &ts_tspath::directory(extended),
        b"",
        case_sensitive,
    );
    ConfigValue::Array(Some(
        values
            .iter()
            .map(|value| {
                let ConfigValue::String(value) = value else {
                    return value.clone();
                };
                if starts_with_config_dir(value.as_bytes())
                    || ts_tspath::encoded_root_length(value.as_bytes()) > 0
                {
                    ConfigValue::String(value.clone())
                } else {
                    ConfigValue::String(JsString::from_bytes(ts_tspath::combine(
                        &relative,
                        &[value.as_bytes()],
                    )))
                }
            })
            .collect(),
    ))
}
