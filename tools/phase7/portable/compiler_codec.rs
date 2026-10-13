//! Shared byte-preserving codecs; these perform no compiler/checker queries.
use crate::compiler_protocol as wire;
use crate::protocol::{DiagnosticRecord, RepopulateRecord};
use std::collections::HashMap;
use std::sync::Arc;
use tsr_ast::{Diagnostic, NodeId};
use tsr_core::{CompilerOptions, TextRange};
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{ConfigValue, ParsedCommandLine, TsConfigSourceFile};

pub fn freeze_options(options: &CompilerOptions) -> Result<Vec<u8>, String> {
    tsr_tsoptions::options_json::marshal_compiler_options(options)
        .map_err(|error| format!("compiler options: {error}"))
}

pub fn thaw_options(bytes: &[u8]) -> Result<CompilerOptions, String> {
    tsr_json::unmarshal(
        bytes,
        &mut tsr_json::RawValue::default(),
        tsr_json::Options::default(),
    )
    .map_err(|error| format!("compiler options JSON: {error}"))?;
    let raw =
        serde_json::from_slice(bytes).map_err(|error| format!("compiler options JSON: {error}"))?;
    tsr_tsoptions::raw::compiler_options(&raw)
        .map_err(|error| format!("compiler options: {error:?}"))
}

pub fn bytes(values: &[JsString]) -> Vec<Vec<u8>> {
    values
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect()
}
pub fn strings(values: Vec<Vec<u8>>) -> Vec<JsString> {
    values.into_iter().map(JsString::from_bytes).collect()
}

pub fn freeze_diagnostic(
    diagnostic: &Diagnostic,
    index: &impl Fn(NodeId) -> Result<usize, String>,
    display_args: &impl Fn(&Diagnostic) -> Result<Vec<JsString>, String>,
) -> Result<DiagnosticRecord, String> {
    if let Some(message) = diagnostic.message {
        if tsr_diagnostics::by_key_bytes(diagnostic.message_key.as_bytes()) != Some(message) {
            return Err("diagnostic message cannot be reconstructed from the pinned key".into());
        }
    }
    Ok(DiagnosticRecord {
        file: diagnostic.file.map(index).transpose()?,
        pos: diagnostic.loc.pos(),
        end: diagnostic.loc.end(),
        code: diagnostic.code,
        category: diagnostic.category,
        source: diagnostic.source.as_bytes().to_vec(),
        key: diagnostic.message_key.as_bytes().to_vec(),
        text: diagnostic.message_text.as_bytes().to_vec(),
        args: bytes(&diagnostic.message_args),
        display_args: bytes(&display_args(diagnostic)?),
        has_message: diagnostic.message.is_some(),
        ad_hoc_message: diagnostic
            .ad_hoc_message
            .as_ref()
            .map(|message| message.text().to_vec()),
        chain: diagnostic
            .message_chain
            .iter()
            .map(|value| freeze_diagnostic(value, index, display_args))
            .collect::<Result<_, _>>()?,
        related: diagnostic
            .related_information
            .iter()
            .map(|value| freeze_diagnostic(value, index, display_args))
            .collect::<Result<_, _>>()?,
        reports_unnecessary: diagnostic.reports_unnecessary,
        reports_deprecated: diagnostic.reports_deprecated,
        skipped_on_no_emit: diagnostic.skipped_on_no_emit,
        repopulate: diagnostic
            .repopulate_info
            .as_ref()
            .map(|info| RepopulateRecord {
                kind: info.kind,
                module_reference: info.module_reference.as_bytes().to_vec(),
                mode: info.mode,
                package_name: info.package_name.as_bytes().to_vec(),
            }),
    })
}

/// Restore raw arguments. The retained source's mapping computes display
/// arguments at rendering time; callers can compare those with `display_args`.
pub fn thaw_diagnostic(
    value: DiagnosticRecord,
    source: &impl Fn(usize) -> Result<NodeId, String>,
) -> Result<Diagnostic, String> {
    let message = if value.has_message {
        Some(
            tsr_diagnostics::by_key_bytes(&value.key)
                .ok_or("portable diagnostic names an unknown pinned message")?,
        )
    } else {
        None
    };
    let mut result = Diagnostic::external(
        value.file.map(source).transpose()?,
        TextRange::new(value.pos, value.end),
        JsString::from_bytes(value.source),
        value.category,
        value.code,
        JsString::from_bytes(value.text),
    );
    result.message = message;
    result.message_key = JsString::from_bytes(value.key);
    result.message_args = strings(value.args);
    result.message_chain = value
        .chain
        .into_iter()
        .map(|value| thaw_diagnostic(value, source).map(Arc::new))
        .collect::<Result<_, _>>()?;
    result.related_information = value
        .related
        .into_iter()
        .map(|value| thaw_diagnostic(value, source).map(Arc::new))
        .collect::<Result<_, _>>()?;
    result.reports_unnecessary = value.reports_unnecessary;
    result.reports_deprecated = value.reports_deprecated;
    result.skipped_on_no_emit = value.skipped_on_no_emit;
    result.ad_hoc_message = value
        .ad_hoc_message
        .map(tsr_diagnostics::AdHocMessage::new)
        .map(Arc::new);
    result.repopulate_info = value.repopulate.map(|info| {
        Arc::new(tsr_ast::diagnostic_api::RepopulateDiagnosticInfo {
            kind: info.kind,
            module_reference: JsString::from_bytes(info.module_reference),
            mode: info.mode,
            package_name: JsString::from_bytes(info.package_name),
        })
    });
    Ok(result)
}

fn freeze_value(value: &ConfigValue) -> wire::ConfigValueSnapshot {
    use wire::ConfigValueSnapshot as W;
    match value {
        ConfigValue::Null => W::Null,
        ConfigValue::EmptyStruct => W::EmptyStruct,
        ConfigValue::Boolean(value) => W::Boolean(*value),
        ConfigValue::Number(value) => W::Number(*value),
        ConfigValue::Integer(value) => W::Integer(*value),
        ConfigValue::Enum(value) => W::Enum(*value),
        ConfigValue::String(value) => W::String(value.as_bytes().to_vec()),
        ConfigValue::Array(value) => W::Array(
            value
                .as_ref()
                .map(|values| values.iter().map(freeze_value).collect()),
        ),
        ConfigValue::Object(value) => W::Object(
            value
                .iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), freeze_value(value)))
                .collect(),
        ),
        ConfigValue::StringArray(value) => {
            W::StringArray(value.as_ref().map(|values| bytes(values)))
        }
        ConfigValue::UnorderedObject(value) => {
            let mut pairs: Vec<_> = value
                .iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), freeze_value(value)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            W::UnorderedObject(pairs)
        }
    }
}
fn thaw_value(value: wire::ConfigValueSnapshot) -> ConfigValue {
    use wire::ConfigValueSnapshot as W;
    match value {
        W::Null => ConfigValue::Null,
        W::EmptyStruct => ConfigValue::EmptyStruct,
        W::Boolean(value) => ConfigValue::Boolean(value),
        W::Number(value) => ConfigValue::Number(value),
        W::Integer(value) => ConfigValue::Integer(value),
        W::Enum(value) => ConfigValue::Enum(value),
        W::String(value) => ConfigValue::String(JsString::from_bytes(value)),
        W::Array(value) => {
            ConfigValue::Array(value.map(|values| values.into_iter().map(thaw_value).collect()))
        }
        W::Object(value) => ConfigValue::Object(
            value
                .into_iter()
                .map(|(key, value)| (JsString::from_bytes(key), thaw_value(value)))
                .collect(),
        ),
        W::StringArray(value) => ConfigValue::StringArray(value.map(strings)),
        W::UnorderedObject(value) => ConfigValue::UnorderedObject(
            value
                .into_iter()
                .map(|(key, value)| (JsString::from_bytes(key), thaw_value(value)))
                .collect(),
        ),
    }
}

pub fn freeze_config(config: &ParsedCommandLine) -> Result<wire::ConfigSnapshot, String> {
    let mut sources = Vec::new();
    let mut indices = HashMap::new();
    let mut add = |config: &Arc<TsConfigSourceFile>| -> Result<usize, String> {
        if let Some(&index) = indices.get(&config.root) {
            return Ok(index);
        }
        let source = config
            .file
            .view()
            .source_file(config.root)
            .map_err(|error| format!("config source: {error:?}"))?;
        let index = sources.len();
        indices.insert(config.root, index);
        sources.push(wire::ConfigSource {
            name: source.file_name().to_vec(),
            path: source.path().to_vec(),
            text: source.text().as_bytes().to_vec(),
            extended_source_files: bytes(&config.extended_source_files),
        });
        Ok(index)
    };
    let main = config.config_file.as_ref().map(&mut add).transpose()?;
    let dependencies = config
        .config_dependencies
        .iter()
        .map(&mut add)
        .collect::<Result<_, _>>()?;
    let errors = config
        .errors
        .iter()
        .map(|diagnostic| {
            freeze_diagnostic(
                diagnostic,
                &|id| {
                    indices
                        .get(&id)
                        .copied()
                        .ok_or("config diagnostic names an unretained source".into())
                },
                &|diagnostic| Ok(diagnostic.message_args.clone()),
            )
        })
        .collect::<Result<_, _>>()?;
    let specs = config.config_specs().map(|specs| wire::ConfigSpecs {
        files_specs: freeze_value(&specs.files_specs),
        include_specs: freeze_value(&specs.include_specs),
        exclude_specs: freeze_value(&specs.exclude_specs),
        validated_files: bytes(&specs.validated_files),
        validated_includes: bytes(&specs.validated_includes),
        validated_excludes: bytes(&specs.validated_excludes),
        files_before_substitution: bytes(&specs.files_before_substitution),
        includes_before_substitution: bytes(&specs.includes_before_substitution),
        is_default_include: specs.is_default_include,
    });
    let mappers = config
        .content_mappers
        .as_ref()
        .map(|mappers| {
            mappers
                .iter()
                .map(crate::mapper_codec::freeze_mapper)
                .collect::<Result<_, _>>()
        })
        .transpose()?;
    Ok(wire::ConfigSnapshot {
        sources,
        main,
        dependencies,
        errors,
        specs,
        mappers,
    })
}

pub fn thaw_config(
    snapshot: wire::ConfigSnapshot,
    options: CompilerOptions,
    roots: Vec<JsString>,
) -> Result<ParsedCommandLine, String> {
    let sources: Vec<_> = snapshot
        .sources
        .into_iter()
        .map(|source| {
            let mut config = TsConfigSourceFile::parse(
                JsString::from_bytes(source.name),
                JsString::from_bytes(source.path),
                SourceText::from_loaded_bytes(source.text),
            );
            config.extended_source_files = strings(source.extended_source_files);
            Arc::new(config)
        })
        .collect();
    let get = |index: usize| {
        sources
            .get(index)
            .cloned()
            .ok_or_else(|| "config source index outside snapshots".to_owned())
    };
    let mut config = ParsedCommandLine::new(options, roots);
    config.config_file = snapshot.main.map(get).transpose()?;
    config.config_dependencies = snapshot
        .dependencies
        .into_iter()
        .map(get)
        .collect::<Result<_, _>>()?;
    config.errors = snapshot
        .errors
        .into_iter()
        .map(|value| thaw_diagnostic(value, &|index| get(index).map(|source| source.root)))
        .collect::<Result<_, _>>()?;
    config.content_mappers = snapshot
        .mappers
        .map(|mappers| {
            mappers
                .iter()
                .map(crate::mapper_codec::thaw_mapper)
                .collect::<Result<_, _>>()
        })
        .transpose()?;
    let specs = snapshot.specs.map(|specs| tsr_tsoptions::ConfigFileSpecs {
        files_specs: thaw_value(specs.files_specs),
        include_specs: thaw_value(specs.include_specs),
        exclude_specs: thaw_value(specs.exclude_specs),
        validated_files: strings(specs.validated_files),
        validated_includes: strings(specs.validated_includes),
        validated_excludes: strings(specs.validated_excludes),
        files_before_substitution: strings(specs.files_before_substitution),
        includes_before_substitution: strings(specs.includes_before_substitution),
        is_default_include: specs.is_default_include,
    });
    config.set_config_specs(specs, JsString::default(), false);
    Ok(config)
}
