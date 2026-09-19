//! Source config validation and manifest metadata; mapper execution is separate.
use crate::{ConfigValue, TsConfigSourceFile};
use std::collections::BTreeSet;
use ts_ast::{Diagnostic, NodeDataRead, NodeId, SyntaxKind as K};
use ts_diagnostics as diagnostics;
use ts_jsstring::JsString;

#[derive(Clone, Debug, Default)]
pub struct MapperManifest {
    pub name: JsString,
    pub version: JsString,
    pub exec: Option<Vec<JsString>>,
    pub compiler_options: Option<Vec<JsString>>,
    pub dynamic_config: bool,
}
#[derive(Clone, Debug, Default)]
pub struct ContentMapper {
    pub package: JsString,
    pub extensions: Vec<JsString>,
    pub options: Option<Vec<u8>>,
    pub manifest: MapperManifest,
    pub package_directory: JsString,
}
#[derive(Clone, Debug, Default)]
pub struct MapperResolution {
    pub manifest: MapperManifest,
    pub package_directory: JsString,
    pub diagnostic: Option<Diagnostic>,
}
#[derive(Clone, Debug, Default)]
pub struct ValidatedMappers {
    pub mappers: Option<Vec<ContentMapper>>,
    pub extensions: Option<Vec<JsString>>,
    pub diagnostics: Vec<Diagnostic>,
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:parseStringArrayStrict
pub fn parse_string_array_strict(value: &ConfigValue) -> Option<Vec<JsString>> {
    value
        .as_array()?
        .iter()
        .map(|value| value.as_string().cloned())
        .collect()
}
/// port: tsc/internal/tsoptions/parsinghelpers.go:parseContentMapper
pub fn parse_content_mapper(value: &ConfigValue) -> (Option<ContentMapper>, Vec<Diagnostic>) {
    if value.as_object().is_none() {
        return (None, Vec::new());
    }
    let mut mapper = ContentMapper::default();
    let mut errors = Vec::new();
    let mut invalid = |key: &[u8], kind: &[u8]| {
        errors.push(Diagnostic::compiler(
            diagnostics::Compiler_option_0_requires_a_value_of_type_1,
            vec![JsString::from_bytes(key), JsString::from_bytes(kind)],
        ));
    };
    if let Some(package) = value
        .get(b"package")
        .and_then(ConfigValue::as_string)
        .filter(|value| !value.is_empty())
    {
        mapper.package = package.clone();
    } else {
        invalid(b"contentMapper.package", b"string");
    }
    if let Some(extensions) = value.get(b"extensions").and_then(parse_string_array_strict) {
        mapper.extensions = extensions;
    } else {
        invalid(b"contentMapper.extensions", b"string[]");
    }
    if let Some(options) = value.get(b"options") {
        if options.as_object().is_some() {
            mapper.options = crate::config_json::stringify_json(options).ok();
        } else {
            invalid(b"contentMapper.options", b"object");
        }
    }
    (errors.is_empty().then_some(mapper), errors)
}
/// The input slice is the source getPropFromRaw result. Indices intentionally
/// refer to that returned slice, preserving the calling API's filtering rules.
pub fn validate_content_mappers<E>(
    values: &[ConfigValue],
    config: Option<&TsConfigSourceFile>,
    case_sensitive: bool,
    run_external_code: bool,
    containing_file: &[u8],
    resolve: &mut impl FnMut(&[u8], &[u8]) -> Result<MapperResolution, E>,
) -> Result<ValidatedMappers, E> {
    let mut result = ValidatedMappers {
        extensions: Some(Vec::new()),
        ..Default::default()
    };
    let mut indices = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let (mapper, errors) = parse_content_mapper(value);
        result.diagnostics.extend(errors.into_iter().map(|error| {
            set_diagnostic_location(
                error,
                config,
                config.and_then(|file| mapper_syntax(file, index as isize, b"")),
            )
        }));
        if let Some(mapper) = mapper {
            result.mappers.get_or_insert_with(Vec::new).push(mapper);
            indices.push(index);
        }
    }
    let mut seen = BTreeSet::new();
    const NATIVE: &[&[u8]] = &[
        b".ts", b".tsx", b".d.ts", b".cts", b".d.cts", b".mts", b".d.mts", b".js", b".jsx",
        b".mjs", b".cjs", b".json",
    ];
    for (mapper, &index) in result.mappers.iter_mut().flatten().zip(&indices) {
        let mut valid = Vec::new();
        for extension in std::mem::take(&mut mapper.extensions) {
            let message = if !extension.as_bytes().starts_with(b".") {
                Some(diagnostics::Content_mapper_file_extension_0_must_begin_with_a)
            } else if NATIVE
                .iter()
                .any(|native| ts_jsstring::equal_fold(native, extension.as_bytes()))
            {
                Some(diagnostics::Content_mapper_file_extension_0_is_a_built_in_extension_and_cannot_be_registered_by_a_content_mapper)
            } else if !seen
                .insert(ts_tspath::canonical(extension.as_bytes(), case_sensitive).into_owned())
            {
                Some(diagnostics::Content_mapper_file_extension_0_is_registered_by_more_than_one_content_mapper)
            } else {
                None
            };
            if let Some(message) = message {
                let node = config
                    .and_then(|file| extension_syntax(file, index as isize, extension.as_bytes()));
                result.diagnostics.push(set_diagnostic_location(
                    Diagnostic::compiler(message, vec![extension]),
                    config,
                    node,
                ));
            } else {
                result
                    .extensions
                    .as_mut()
                    .expect("extensions initialized")
                    .push(extension.clone());
                valid.push(extension);
            }
        }
        mapper.extensions = valid;
    }
    if result
        .mappers
        .as_ref()
        .is_some_and(|mappers| !mappers.is_empty())
    {
        if run_external_code {
            let mut resolved = Vec::new();
            for (mut mapper, &index) in result
                .mappers
                .take()
                .expect("nonempty mapper list")
                .into_iter()
                .zip(&indices)
            {
                let resolution = resolve(containing_file, mapper.package.as_bytes())?;
                mapper.package_directory = resolution.package_directory;
                if let Some(error) = resolution.diagnostic {
                    result.diagnostics.push(set_diagnostic_location(
                        error,
                        config,
                        config.and_then(|file| mapper_syntax(file, index as isize, b"package")),
                    ));
                } else {
                    mapper.manifest = resolution.manifest;
                    resolved.push(mapper);
                }
            }
            result.extensions = Some(
                resolved
                    .iter()
                    .flat_map(|mapper| mapper.extensions.iter().cloned())
                    .collect(),
            );
            result.mappers = Some(resolved);
        } else {
            result.diagnostics.push(set_diagnostic_location(Diagnostic::compiler(diagnostics::Content_mappers_require_the_runExternalCode_command_line_flag_to_be_enabled,Vec::new()),config,config.and_then(mappers_key_syntax)));
            result.mappers = None;
            result.extensions = None;
        }
    }
    Ok(result)
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getContentMapperSyntax
pub fn mapper_syntax(config: &TsConfigSourceFile, index: isize, subkey: &[u8]) -> Option<NodeId> {
    let property = crate::find_property(config, &[b"contentMappers"])?;
    let view = config.file.view();
    let node = view.node(property).expect("config property");
    let NodeDataRead::PropertyAssignment(data) = node.data() else {
        return None;
    };
    let initializer = data.initializer()?;
    let node = view.node(initializer).expect("config initializer");
    let NodeDataRead::ArrayLiteralExpression(data) = node.data() else {
        return Some(initializer);
    };
    let elements = view
        .node_slice(
            view.list(data.elements()?)
                .expect("mapper elements")
                .nodes(),
        )
        .expect("mapper element slice");
    let Some(element) = usize::try_from(index)
        .ok()
        .and_then(|index| elements.get(index))
        .flatten()
    else {
        return Some(initializer);
    };
    if !subkey.is_empty() {
        if let Some(property) = crate::find_property_in_object(config, element, &[subkey]) {
            let node = view.node(property).expect("mapper property");
            if let NodeDataRead::PropertyAssignment(data) = node.data() {
                if let Some(value) = data.initializer() {
                    return Some(value);
                }
            }
        }
    }
    Some(element)
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getContentMappersKeySyntax
pub fn mappers_key_syntax(config: &TsConfigSourceFile) -> Option<NodeId> {
    let property = crate::find_property(config, &[b"contentMappers"])?;
    config
        .file
        .view()
        .node(property)
        .expect("config property")
        .name()
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getContentMapperExtensionSyntax
pub fn extension_syntax(
    config: &TsConfigSourceFile,
    index: isize,
    extension: &[u8],
) -> Option<NodeId> {
    let id = mapper_syntax(config, index, b"extensions")?;
    let view = config.file.view();
    let node = view.node(id).expect("extension node");
    if let NodeDataRead::ArrayLiteralExpression(data) = node.data() {
        if let Some(list) = data.elements() {
            for element in view
                .node_slice(view.list(list).expect("extensions list").nodes())
                .expect("extension slice")
                .iter()
                .flatten()
            {
                if view.node(element).expect("extension element").kind() == K::StringLiteral
                    && view.node_text(element).expect("extension text").as_bytes() == extension
                {
                    return Some(element);
                }
            }
        }
    }
    Some(id)
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:setContentMapperDiagnosticLocation
pub fn set_diagnostic_location(
    mut diagnostic: Diagnostic,
    config: Option<&TsConfigSourceFile>,
    node: Option<NodeId>,
) -> Diagnostic {
    if let (Some(config), Some(node)) = (config, node) {
        let view = config.file.view();
        let node = view.node(node).expect("diagnostic node");
        let source = view.source_file(config.root).expect("diagnostic source");
        diagnostic.file = Some(config.root);
        diagnostic.loc = ts_core::TextRange::new(
            ts_scanner::skip_trivia(source.text().as_bytes(), i64::from(node.pos())),
            i64::from(node.end()),
        );
    }
    diagnostic
}
