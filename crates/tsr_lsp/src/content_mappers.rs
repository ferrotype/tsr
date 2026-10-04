//! Validation of editor-contributed mapper declarations. Execution and project
//! installation belong to L6; parsing never launches an external process.
use std::collections::HashSet;
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;
use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};
#[derive(Default, Debug)]
pub struct Contributions {
    pub mappers: Vec<ContentMapper>,
    pub extensions: Vec<String>,
}
// port: tsc/internal/lsp/server.go:parseContentMapperContributions
pub fn parse(
    values: &[Option<Box<lsp::ContentMapperContribution>>],
) -> Result<Contributions, String> {
    let mut result = Contributions::default();
    let mut claimed = HashSet::new();
    for (index, value) in values.iter().enumerate() {
        let value = value
            .as_deref()
            .filter(|v| !v.contributor_id.is_empty())
            .ok_or("content mapper contribution requires a contributorId")?;
        let identity = format!("{}[{index}]", value.contributor_id);
        for extension in &value.extensions {
            if !valid_extension(extension) {
                return Err(format!(
                    "content mapper contribution {identity:?} has invalid extension {extension:?}"
                ));
            }
        }
        let Some(inferred) = value.inferred_project_contribution.as_deref() else {
            continue;
        };
        let manifest = inferred
            .manifest
            .as_deref()
            .filter(|m| !m.name.is_empty() && !m.exec.is_empty())
            .ok_or_else(|| {
                format!(
                    "content mapper contribution {identity:?} requires a manifest name and exec"
                )
            })?;
        for option in manifest
            .compiler_options
            .iter()
            .flat_map(|options| options.iter())
        {
            if tsr_tsoptions::compiler_option_name_map()
                .get(option.as_bytes())
                .is_none()
            {
                return Err(format!("content mapper contribution {identity:?} requests unknown compiler option {option:?}"));
            }
        }
        for extension in &value.extensions {
            if !claimed.insert(extension.to_lowercase()) {
                return Err(format!(
                    "content mapper contributions both claim extension {extension:?}"
                ));
            }
            result.extensions.push(extension.clone());
        }
        if manifest
            .cwd
            .as_deref()
            .is_some_and(|cwd| !tsr_tspath::path_is_absolute(cwd.as_bytes()))
        {
            return Err(format!(
                "content mapper contribution {identity:?} has non-absolute cwd"
            ));
        }
        let options = inferred.options.as_deref().map_or_else(
            || Ok(b"{}".to_vec()),
            |value| {
                tsr_json::marshal(value, tsr_json::Options::default()).map_err(|_| {
                    format!("content mapper contribution {identity:?} has invalid options")
                })
            },
        )?;
        let bytes = |value: &String| JsString::from_bytes(value.as_bytes());
        result.mappers.push(ContentMapper {
            package: JsString::from_bytes(identity.as_bytes()),
            contribution_id: JsString::from_bytes(identity.as_bytes()),
            extensions: value.extensions.iter().map(bytes).collect(),
            options: Some(options),
            package_directory: manifest.cwd.as_deref().map(bytes).unwrap_or_default(),
            manifest: MapperManifest {
                name: bytes(&manifest.name),
                version: manifest.version.as_deref().map(bytes).unwrap_or_default(),
                exec: Some(manifest.exec.iter().map(bytes).collect()),
                compiler_options: manifest
                    .compiler_options
                    .as_deref()
                    .map(|v| v.iter().map(bytes).collect()),
                dynamic_config: manifest.dynamic_config.as_deref().copied().unwrap_or(false),
            },
        });
    }
    result.extensions.sort();
    Ok(result)
}
// port: tsc/internal/lsp/server.go:isValidContributedContentMapperExtension
fn valid_extension(extension: &str) -> bool {
    // GetAnyExtensionFromPath with a nil extension list selects the final dot.
    extension.len() > 1
        && extension.starts_with('.')
        && !extension[1..].contains(['.', '/', '\\'])
        && ![
            ".ts", ".tsx", ".cts", ".mts", ".js", ".jsx", ".mjs", ".cjs", ".json",
        ]
        .iter()
        .any(|native| extension.eq_ignore_ascii_case(native))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(text: &str) -> Result<Contributions, String> {
        let mut values = Vec::<Option<Box<lsp::ContentMapperContribution>>>::new();
        tsr_json::unmarshal(text.as_bytes(), &mut values, tsr_json::Options::default()).unwrap();
        parse(&values)
    }
    #[test]
    fn pinned_contribution_validation_and_identity() {
        let result = parsed(r#"[{"contributorId":"publisher.extension","extensions":[".vue"],"inferredProjectContribution":{"options":{"mode":"embedded"},"manifest":{"name":"Vue mapper","version":"2.3.4","cwd":"/workspace/mapper","compilerOptions":["strict"],"exec":["node","mapper.js"]}}},{"contributorId":"publisher.extension","extensions":[".svelte"]}]"#).unwrap();
        assert_eq!(result.extensions, [".vue"]);
        assert_eq!(
            tsr_contentmapper::identity(&result.mappers[0]).as_bytes(),
            b"publisher.extension[0] (Vue mapper@2.3.4)"
        );
        assert_eq!(
            result.mappers[0].options.as_deref().unwrap(),
            br#"{"mode":"embedded"}"#
        );
        let template = r#"{"contributorId":"first","extensions":[".vue"],"inferredProjectContribution":{"manifest":{"name":"mapper","exec":["mapper"]}}}"#;
        assert_eq!(
            parsed(&format!("[{template}]")).unwrap().mappers[0]
                .options
                .as_deref()
                .unwrap(),
            b"{}"
        );
        assert!(parsed(&format!(
            "[{template},{}]",
            template.replace(".vue", ".VUE")
        ))
        .unwrap_err()
        .contains("both claim"));
        for invalid in [".TS", "vue", ".", ".d.vue", ".foo/bar"] {
            assert!(!valid_extension(invalid));
        }
        assert!(parsed("[null]").is_err());
        assert!(parsed(&format!(
            "[{}]",
            template.replace(
                "\"name\":\"mapper\"",
                "\"name\":\"mapper\",\"cwd\":\"relative\""
            )
        ))
        .unwrap_err()
        .contains("non-absolute"));
        assert!(parsed(&format!(
            "[{}]",
            template.replace(
                "\"name\":\"mapper\"",
                "\"name\":\"mapper\",\"compilerOptions\":[\"missingOption\"]"
            )
        ))
        .unwrap_err()
        .contains("unknown compiler option"));
    }
}
