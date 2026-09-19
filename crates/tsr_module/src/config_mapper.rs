//! Content mapper package metadata lookup never starts external code.
use std::sync::Arc;
use ts_ast::Diagnostic;
use ts_diagnostics as diagnostics;
use ts_jsstring::JsString;
use ts_tsoptions::config_mappers::{MapperManifest, MapperResolution};
use ts_vfs::FileSystem;

/// port: tsc/internal/tsoptions/contentmappers.go:resolveContentMapperManifest
pub fn resolve_content_mapper_manifest(
    host: &Arc<dyn FileSystem>,
    cwd: &[u8],
    containing_file: &[u8],
    name: &[u8],
) -> Result<MapperResolution, crate::Error> {
    let Some(resolved) =
        crate::resolve_package_directory(name, containing_file, host.clone(), cwd)?
    else {
        return Ok(failure(
            name,
            JsString::default(),
            diagnostics::The_content_mapper_package_0_could_not_be_resolved,
        ));
    };
    let directory = resolved.resolved_file_name;
    let filename = ts_tspath::combine(directory.as_bytes(), &[b"package.json"]);
    let Some(content) = host.read_file(&filename)? else {
        return Ok(failure(
            name,
            directory,
            diagnostics::The_content_mapper_package_0_could_not_be_resolved,
        ));
    };
    let parsed = crate::package_json::parse(content.text.as_bytes());
    if !parsed.parseable {
        return Ok(failure(
            name,
            directory,
            diagnostics::The_package_json_of_the_content_mapper_package_0_could_not_be_parsed,
        ));
    }
    let string = |key| {
        parsed
            .fields
            .field(key)
            .map(|field| &field.value)
            .and_then(serde_json::Value::as_str)
            .map_or_else(JsString::default, |value| {
                JsString::from_bytes(value.as_bytes())
            })
    };
    let manifest_name = string("name");
    if manifest_name.is_empty() {
        return Ok(failure(
            name,
            directory,
            diagnostics::The_package_json_of_the_content_mapper_package_0_does_not_specify_a_name,
        ));
    }
    let Some(mapper) = parsed.fields.content_mapper.get_value() else {
        return Ok(failure(name,directory,diagnostics::The_package_json_of_the_content_mapper_package_0_does_not_declare_a_typescript_contentMapper_object));
    };
    let array = |key, require_valid| {
        mapper
            .field(key)
            .and_then(|field| {
                if require_valid {
                    field.get_value()
                } else {
                    Some(&field.value)
                }
            })
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        JsString::from_bytes(
                            value
                                .as_str()
                                .expect("validated package string array")
                                .as_bytes(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
    };
    let exec = array("exec", true);
    if exec.as_ref().is_none_or(Vec::is_empty) {
        return Ok(failure(name,directory,diagnostics::The_typescript_contentMapper_exec_of_the_content_mapper_package_0_must_be_a_non_empty_array_of_strings));
    }
    Ok(MapperResolution {
        package_directory: directory,
        diagnostic: None,
        manifest: MapperManifest {
            name: manifest_name,
            version: string("version"),
            exec,
            compiler_options: array("compilerOptions", false),
            dynamic_config: mapper
                .field("dynamicConfig")
                .map(|field| &field.value)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or_default(),
        },
    })
}
fn failure(
    name: &[u8],
    package_directory: JsString,
    message: &'static diagnostics::Message,
) -> MapperResolution {
    MapperResolution {
        package_directory,
        diagnostic: Some(Diagnostic::compiler(
            message,
            vec![JsString::from_bytes(name)],
        )),
        ..Default::default()
    }
}
