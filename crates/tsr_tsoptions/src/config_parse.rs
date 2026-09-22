use crate::{
    convert_config_file_to_object, convert_json_option, default_compiler_options,
    diagnostic_for_node, find_declaration, find_property, find_property_in_object,
    merge_compiler_options, parse_compiler_options, parse_string_array, parse_tristate,
    substitute_options, ConfigFileSpecs, ConfigValue, OptionDeclaration, OptionSyntax,
    ParseConfigHost, ParsedCommandLine, TsConfigSourceFile, COMPILER_OPTIONS, ROOT_OPTIONS,
    TYPE_ACQUISITION_OPTIONS,
};
use std::sync::Arc;
use tsr_ast::{Diagnostic, NodeDataRead, NodeId, SyntaxKind as K};
use tsr_core::{CompilerOptions, Tristate};
use tsr_diagnostics::{self as d, Message};
use tsr_jsstring::JsString;
use tsr_vfs::Error;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeAcquisition {
    pub enable: Tristate,
    pub include: Option<Vec<JsString>>,
    pub exclude: Option<Vec<JsString>>,
    pub disable_filename_based_type_acquisition: Tristate,
}
impl TypeAcquisition {
    /// port: tsc/internal/tsoptions/parsinghelpers.go:ParseTypeAcquisition
    pub fn parse_option(&mut self, key: &[u8], value: &ConfigValue) {
        if value.is_null() {
            return;
        }
        match key {
            b"enable" => self.enable = parse_tristate(value),
            b"include" => self.include = parse_string_array(value),
            b"exclude" => self.exclude = parse_string_array(value),
            b"disableFilenameBasedTypeAcquisition" => {
                self.disable_filename_based_type_acquisition = parse_tristate(value);
            }
            _ => {}
        }
    }
    fn for_config(name: &[u8]) -> Self {
        Self {
            enable: if tsr_tspath::base_name(name) == b"jsconfig.json" {
                Tristate::TRUE
            } else {
                Tristate::UNKNOWN
            },
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectReference {
    pub path: JsString,
    pub original_path: JsString,
    pub circular: bool,
}
struct Parsed {
    raw: ConfigValue,
    options: Option<CompilerOptions>,
    types: TypeAcquisition,
    source: Option<TsConfigSourceFile>,
    dependencies: Vec<Arc<TsConfigSourceFile>>,
    errors: Vec<Diagnostic>,
}
pub(crate) fn diagnostic(
    config: Option<&TsConfigSourceFile>,
    node: Option<NodeId>,
    message: &'static Message,
    args: Vec<JsString>,
) -> Diagnostic {
    match (config, node) {
        (Some(config), Some(node)) => diagnostic_for_node(config, node, message, args),
        _ => Diagnostic::compiler(message, args),
    }
}
pub(crate) fn initializer(config: &TsConfigSourceFile, property: NodeId) -> Option<NodeId> {
    let read = config.file.view().node(property).expect("config property");
    let NodeDataRead::PropertyAssignment(data) = read.data() else {
        return None;
    };
    data.initializer()
}
pub(crate) fn array_elements(config: &TsConfigSourceFile, node: NodeId) -> Vec<NodeId> {
    let view = config.file.view();
    let read = view.node(node).expect("config array");
    let NodeDataRead::ArrayLiteralExpression(data) = read.data() else {
        return vec![];
    };
    data.elements().map_or_else(Vec::new, |list| {
        view.node_slice(view.list(list).expect("array list").nodes())
            .expect("array elements")
            .iter()
            .flatten()
            .collect()
    })
}
pub(crate) fn array_element(
    config: &TsConfigSourceFile,
    key: &[u8],
    index: usize,
) -> Option<NodeId> {
    array_elements(config, initializer(config, find_property(config, &[key])?)?)
        .get(index)
        .copied()
}
fn array_string(config: &TsConfigSourceFile, key: &[u8], value: &[u8]) -> Option<NodeId> {
    let view = config.file.view();
    // ForEachPropertyAssignment continues after a callback returns nil.
    let root = config.object()?;
    let read = view.node(root).expect("config object");
    let NodeDataRead::ObjectLiteralExpression(data) = read.data() else {
        return None;
    };
    for node in view
        .node_slice(view.list(data.properties()?).ok()?.nodes())
        .ok()?
        .iter()
        .flatten()
    {
        let read = view.node(node).ok()?;
        let NodeDataRead::PropertyAssignment(data) = read.data() else {
            continue;
        };
        if data
            .name()
            .and_then(|name| crate::property_name(config, name))
            .is_none_or(|name| name.as_bytes() != key)
        {
            continue;
        }
        for element in data
            .initializer()
            .into_iter()
            .flat_map(|node| array_elements(config, node))
        {
            if view.node(element).expect("array element").kind() == K::StringLiteral
                && view.node_text(element).expect("string element").as_bytes() == value
            {
                return Some(element);
            }
        }
    }
    None
}
fn unknown(config: &TsConfigSourceFile, node: NodeId, key: &[u8], parent: &str) -> Diagnostic {
    let options = if parent == "compilerOptions" {
        COMPILER_OPTIONS
    } else {
        TYPE_ACQUISITION_OPTIONS
    };
    let (unknown, suggested) =
        crate::extra_key_diagnostics(parent.as_bytes()).expect("known config option parent");
    let suggestion = find_declaration(options, key, false)
        .map(|option| option.name.as_bytes())
        .or_else(|| {
            tsr_scanner::get_spelling_suggestion_for_strings(
                key,
                options.iter().map(|option| option.name.as_bytes()),
            )
        });
    if let Some(suggestion) = suggestion.filter(|suggestion| *suggestion != key) {
        diagnostic_for_node(
            config,
            node,
            suggested,
            vec![JsString::from_bytes(key), JsString::from_bytes(suggestion)],
        )
    } else {
        diagnostic_for_node(config, node, unknown, vec![JsString::from_bytes(key)])
    }
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getExtendsConfigPath
fn extends_path(
    value: &[u8],
    host: &dyn ParseConfigHost,
    base: &[u8],
    config: Option<&TsConfigSourceFile>,
    node: Option<NodeId>,
) -> Result<(Option<JsString>, Vec<Diagnostic>), Error> {
    let value = tsr_tspath::normalize_slashes(value);
    if tsr_tspath::encoded_root_length(&value) > 0
        || value.starts_with(b"./")
        || value.starts_with(b"../")
    {
        let mut path = tsr_tspath::absolute(&value, base);
        if !host.fs().file_exists(&path)? && !path.ends_with(b".json") {
            path.extend_from_slice(b".json");
            if !host.fs().file_exists(&path)? {
                return Ok((
                    None,
                    vec![diagnostic(
                        config,
                        node,
                        d::File_0_not_found,
                        vec![JsString::from_bytes(value.into_owned())],
                    )],
                ));
            }
        }
        return Ok((Some(JsString::from_bytes(path)), vec![]));
    }
    if let Some(path) =
        host.resolve_config(&value, &tsr_tspath::combine(base, &[b"tsconfig.json"]))?
    {
        return Ok((Some(path), vec![]));
    }
    let (message, arg) = if value.is_empty() {
        (
            d::Compiler_option_0_cannot_be_given_an_empty_string,
            JsString::from_bytes(b"extends".as_slice()),
        )
    } else {
        (
            d::File_0_not_found,
            JsString::from_bytes(value.into_owned()),
        )
    };
    Ok((None, vec![diagnostic(config, node, message, vec![arg])]))
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:getExtendsConfigPathOrArray
fn extends_paths(
    value: &ConfigValue,
    host: &dyn ParseConfigHost,
    base: &[u8],
    name: &[u8],
    config: Option<&TsConfigSourceFile>,
    property: Option<NodeId>,
) -> Result<(Vec<JsString>, Vec<Diagnostic>), Error> {
    let new_base = if name.is_empty() {
        base.to_vec()
    } else {
        tsr_tspath::directory(&tsr_tspath::combine(base, &[name]))
    };
    let expression = config
        .zip(property)
        .and_then(|(config, property)| initializer(config, property));
    let option = find_declaration(ROOT_OPTIONS, b"extends", false).expect("extends declaration");
    let syntax = OptionSyntax {
        config,
        property,
        value: expression,
    };
    if let ConfigValue::String(value) = value {
        let (path, errors) = extends_path(value.as_bytes(), host, &new_base, config, expression)?;
        return Ok((path.into_iter().collect(), errors));
    }
    let mut paths = Vec::new();
    let mut errors = Vec::new();
    if let ConfigValue::Array(values) = value {
        let expressions = expression
            .zip(config)
            .map_or_else(Vec::new, |(node, config)| array_elements(config, node));
        for (index, value_entry) in values.as_deref().unwrap_or_default().iter().enumerate() {
            let expression = expression.map(|_| expressions[index]);
            if let ConfigValue::String(value) = value_entry {
                let (path, mut diagnostics) =
                    extends_path(value.as_bytes(), host, &new_base, config, expression)?;
                paths.extend(path);
                errors.append(&mut diagnostics);
            } else {
                // Source passes the entire array to Elements() on this branch.
                let (_, mut diagnostics) = convert_json_option(
                    option.element.expect("extends element"),
                    value,
                    base,
                    OptionSyntax {
                        value: expression,
                        ..syntax
                    },
                );
                errors.append(&mut diagnostics);
            }
        }
    } else {
        errors = convert_json_option(option, value, base, syntax).1;
    }
    Ok((paths, errors))
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:parseOwnConfigOfJsonSourceFile
fn own_config(
    source: TsConfigSourceFile,
    host: &dyn ParseConfigHost,
    base: &[u8],
    name: &[u8],
) -> Result<(Parsed, Vec<JsString>), Error> {
    let mut options = default_compiler_options(name);
    let mut types = TypeAcquisition::for_config(name);
    let mut extended = Vec::new();
    let mut root_options = Vec::new();
    let mut host_error = None;
    let mut notifier = |key: &JsString,
                        value: &ConfigValue,
                        property: NodeId,
                        parent: Option<&'static OptionDeclaration>,
                        option: Option<&'static OptionDeclaration>| {
        let mut errors = Vec::new();
        let mut converted = std::borrow::Cow::Borrowed(value);
        if let Some(option) = option.filter(|option| option.name != "extends") {
            (converted, errors) = convert_json_option(
                option,
                value,
                base,
                OptionSyntax {
                    config: Some(&source),
                    property: Some(property),
                    value: initializer(&source, property),
                },
            );
        }
        let parent = parent.map(|option| option.name);
        if parent.is_some_and(|parent| parent != "undefined") && !converted.is_null() {
            if let Some(option) = option.filter(|option| !option.name.is_empty()) {
                match parent {
                    Some("compilerOptions") => {
                        parse_compiler_options(option.name.as_bytes(), &converted, &mut options);
                    }
                    Some("typeAcquisition") => {
                        types.parse_option(option.name.as_bytes(), &converted);
                    }
                    _ => {}
                }
            } else if !key.is_empty()
                && matches!(parent, Some("compilerOptions" | "typeAcquisition"))
            {
                let node = source
                    .file
                    .view()
                    .node(property)
                    .expect("config property")
                    .name()
                    .expect("property name");
                errors.push(unknown(
                    &source,
                    node,
                    key.as_bytes(),
                    parent.expect("known parent"),
                ));
            }
        } else if parent == Some("undefined") {
            if option.is_some_and(|option| option.name == "extends") {
                match extends_paths(&converted, host, base, name, Some(&source), Some(property)) {
                    Ok((paths, mut diagnostics)) => {
                        extended = paths;
                        errors.append(&mut diagnostics);
                    }
                    Err(error) => host_error = Some(error),
                }
            } else if option.is_none() {
                let node = source
                    .file
                    .view()
                    .node(property)
                    .expect("config property")
                    .name()
                    .expect("property name");
                if key.as_bytes() == b"excludes" {
                    errors.push(diagnostic_for_node(
                        &source,
                        node,
                        d::Unknown_option_excludes_Did_you_mean_exclude,
                        vec![],
                    ));
                }
                if COMPILER_OPTIONS
                    .iter()
                    .any(|option| option.name.as_bytes() == key.as_bytes())
                {
                    root_options.push(node);
                }
            }
        }
        errors
    };
    let (raw, mut errors) = convert_config_file_to_object(&source, Some(&mut notifier));
    if let Some(error) = host_error {
        return Err(error);
    }
    if let Some(node) = root_options
        .first()
        .filter(|_| raw.as_object().is_some() && raw.get(b"compilerOptions").is_none())
    {
        errors.push(diagnostic_for_node(
            &source,
            *node,
            d::X_0_should_be_set_inside_the_compilerOptions_object_of_the_config_json_file,
            vec![crate::property_name(&source, *node).expect("root option name")],
        ));
    }
    if options.paths.is_some() {
        options.paths_base_path = JsString::from_bytes(base);
    }
    Ok((
        Parsed {
            raw,
            options: Some(options),
            types,
            source: Some(source),
            dependencies: Vec::new(),
            errors,
        },
        extended,
    ))
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:parseConfig
fn parse_config(
    source: Option<TsConfigSourceFile>,
    raw: Option<ConfigValue>,
    host: &dyn ParseConfigHost,
    base: &[u8],
    name: &[u8],
    stack: &[JsString],
) -> Result<Parsed, Error> {
    let base = tsr_tspath::normalize_slashes(base);
    let resolved = tsr_tspath::to_path(name, &base, host.fs().use_case_sensitive_file_names());
    if stack.contains(&resolved) {
        return Ok(Parsed {
            raw: ConfigValue::Null,
            options: None,
            types: TypeAcquisition::default(),
            source,
            dependencies: vec![],
            errors: vec![Diagnostic::compiler(
                d::Circularity_detected_while_resolving_configuration_Colon_0,
                vec![],
            )],
        });
    }
    let (mut own, extended) = match (source, raw) {
        (Some(source), None) => own_config(source, host, &base, name)?,
        (None, Some(raw)) => own_json_config(raw, host, &base, name)?,
        _ => unreachable!("one configuration representation"),
    };
    if extended.is_empty() {
        return Ok(own);
    }
    let mut stack = stack.to_vec();
    stack.push(resolved);
    let mut options = CompilerOptions::default();
    let mut inherited = ConfigValue::Object(tsr_core::collections::OrderedMap::default());
    let mut compile_on_save = false;
    for path in extended {
        // ParseExtendedConfig returns a source-file record even when ReadFile
        // fails; getExtendedConfig records its name before returning errors.
        if let Some(source) = &mut own.source {
            source.extended_source_files.push(path.clone());
        }
        let resolved = tsr_tspath::to_path(
            path.as_bytes(),
            host.current_directory(),
            host.fs().use_case_sensitive_file_names(),
        );
        let Some(content) = host.fs().read_file(path.as_bytes())? else {
            own.errors.push(Diagnostic::compiler(
                d::Cannot_read_file_0,
                vec![path.clone()],
            ));
            continue;
        };
        let source = TsConfigSourceFile::parse(path.clone(), resolved, content.text);
        let diagnostics = source
            .file
            .view()
            .source_file(source.root)
            .expect("extended source")
            .diagnostics
            .clone();
        if !diagnostics.is_empty() {
            own.errors.extend(diagnostics);
            own.dependencies.push(Arc::new(source));
            continue;
        }
        let mut parsed = parse_config(
            Some(source),
            None,
            host,
            &tsr_tspath::directory(path.as_bytes()),
            tsr_tspath::base_name(path.as_bytes()),
            &stack,
        )?;
        own.errors.append(&mut parsed.errors);
        if let Some(extended_options) = parsed.options {
            for key in [b"include".as_slice(), b"exclude", b"files"] {
                if own.raw.get(key).is_none() {
                    if let Some(value @ ConfigValue::Array(Some(_))) = parsed.raw.get(key) {
                        inherited.set(
                            JsString::from_bytes(key),
                            crate::config_substitution::inherited_specs(
                                value,
                                path.as_bytes(),
                                &base,
                                host.fs().use_case_sensitive_file_names(),
                            ),
                        );
                    }
                }
            }
            if let Some(value @ ConfigValue::Array(Some(_))) = parsed.raw.get(b"contentMappers") {
                inherited.set(
                    JsString::from_bytes(b"contentMappers".as_slice()),
                    value.clone(),
                );
            }
            if let Some(ConfigValue::Boolean(value)) = parsed.raw.get(b"compileOnSave") {
                compile_on_save = *value;
            }
            merge_compiler_options(&mut options, &extended_options, &parsed.raw);
        }
        if let Some(source) = parsed.source {
            if let Some(own_source) = &mut own.source {
                own_source
                    .extended_source_files
                    .extend(source.extended_source_files.iter().cloned());
            }
            own.dependencies.append(&mut parsed.dependencies);
            own.dependencies.push(Arc::new(source));
        }
    }
    for (key, value) in inherited.as_object().expect("inherited property map") {
        if key.as_bytes() != b"contentMappers" || own.raw.get(key.as_bytes()).is_none() {
            own.raw.set(key.clone(), value.clone());
        }
    }
    if compile_on_save && own.raw.get(b"compileOnSave").is_none() {
        own.raw.set(
            JsString::from_bytes(b"compileOnSave".as_slice()),
            ConfigValue::Boolean(true),
        );
    }
    if let Some(source) = &mut own.source {
        source.extended_source_files.sort();
        source.extended_source_files.dedup();
    }
    if let Some(own_options) = &own.options {
        merge_compiler_options(&mut options, own_options, &own.raw);
    }
    own.options = Some(options);
    Ok(own)
}

fn raw_array(raw: &ConfigValue, key: &[u8]) -> Option<Vec<ConfigValue>> {
    match raw.get(key) {
        Some(ConfigValue::Array(values)) => values.clone(),
        _ => None,
    }
}
fn specs(parsed: &mut Parsed, base: &[u8], name: &[u8]) -> ConfigFileSpecs {
    validated_raw_array(parsed, b"references", "object");
    let files = validated_raw_array(parsed, b"files", "string");
    let mut includes = raw_array(&parsed.raw, b"include");
    let mut excludes = raw_array(&parsed.raw, b"exclude");
    let mut default_include = false;
    if files.as_ref().is_some_and(Vec::is_empty)
        && raw_array(&parsed.raw, b"references").is_none_or(|references| references.is_empty())
        && parsed.raw.get(b"extends").is_none_or(ConfigValue::is_null)
    {
        let node = parsed
            .source
            .as_ref()
            .and_then(|s| find_property(s, &[b"files"]).and_then(|p| initializer(s, p)));
        parsed.errors.push(diagnostic(
            parsed.source.as_ref(),
            node,
            d::The_files_list_in_config_file_0_is_empty,
            vec![JsString::from_bytes(
                if name.is_empty() && parsed.source.is_some() {
                    b"tsconfig.json"
                } else {
                    name
                },
            )],
        ));
    }
    validated_raw_array(parsed, b"include", "string");
    validated_raw_array(parsed, b"exclude", "string");
    let invalid_excludes = parsed.source.is_none()
        && parsed
            .raw
            .get(b"exclude")
            .is_some_and(|v| !v.is_null() && v.as_array().is_none());
    if excludes.is_none() && !invalid_excludes {
        if let Some(options) = &parsed.options {
            let mut values = Vec::new();
            for value in [&options.out_dir, &options.declaration_dir] {
                if !value.is_empty() {
                    values.push(ConfigValue::String(value.clone()));
                }
            }
            if !values.is_empty() {
                excludes = Some(values);
            }
        }
    }
    if files.is_none() && includes.is_none() {
        includes = Some(vec![ConfigValue::String(JsString::from_bytes(
            b"**/*".as_slice(),
        ))]);
        default_include = true;
    }
    let mut validate = |values: &Option<Vec<ConfigValue>>, key: &[u8], disallow: bool| {
        let mut result = Vec::new();
        for value in values
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(ConfigValue::as_string)
        {
            if let Some(message) = crate::spec_diagnostic(value.as_bytes(), disallow) {
                let node = parsed
                    .source
                    .as_ref()
                    .and_then(|s| array_string(s, key, value.as_bytes()));
                parsed.errors.push(diagnostic(
                    parsed.source.as_ref(),
                    node,
                    message,
                    vec![value.clone()],
                ));
            } else {
                result.push(value.clone());
            }
        }
        result
    };
    let includes_before_substitution = validate(&includes, b"include", true);
    let mut validated_excludes = validate(&excludes, b"exclude", false);
    let files_before_substitution: Vec<_> = files
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(ConfigValue::as_string)
        .cloned()
        .collect();
    let mut validated_includes = includes_before_substitution.clone();
    let mut validated_files = files_before_substitution.clone();
    for values in [
        &mut validated_includes,
        &mut validated_excludes,
        &mut validated_files,
    ] {
        crate::config_substitution::substitute_strings(values, base);
    }
    ConfigFileSpecs {
        files_specs: ConfigValue::Array(files),
        include_specs: ConfigValue::Array(includes),
        exclude_specs: ConfigValue::Array(excludes),
        validated_files,
        validated_includes,
        validated_excludes,
        files_before_substitution,
        includes_before_substitution,
        is_default_include: default_include,
    }
}
fn references(parsed: &mut Parsed, base: &[u8]) -> Option<Vec<ProjectReference>> {
    // The raw API validates twice at the pin: once in specs, then here.
    // Preserve both diagnostics (the source-file API validates its AST instead).
    let values = validated_raw_array(parsed, b"references", "object")?;
    let mut result = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if value.as_object().is_none() {
            continue;
        }
        let node_for = |key: &[u8]| {
            parsed.source.as_ref().and_then(|s| {
                array_element(s, b"references", index).map(|element| {
                    find_property_in_object(s, element, &[key])
                        .and_then(|p| initializer(s, p))
                        .unwrap_or(element)
                })
            })
        };
        let Some(path) = value.get(b"path").and_then(ConfigValue::as_string) else {
            parsed.errors.push(diagnostic(
                parsed.source.as_ref(),
                node_for(b"path"),
                d::Compiler_option_0_requires_a_value_of_type_1,
                vec![
                    JsString::from_bytes(b"reference.path".as_slice()),
                    JsString::from_bytes(b"string".as_slice()),
                ],
            ));
            continue;
        };
        if path.is_empty() {
            parsed.errors.push(diagnostic(
                parsed.source.as_ref(),
                node_for(b"path"),
                d::Compiler_option_0_cannot_be_given_an_empty_string,
                vec![JsString::from_bytes(b"reference.path".as_slice())],
            ));
            continue;
        }
        let circular = match value.get(b"circular") {
            Some(ConfigValue::Boolean(value)) => *value,
            Some(_) => {
                parsed.errors.push(diagnostic(
                    parsed.source.as_ref(),
                    node_for(b"circular"),
                    d::Compiler_option_0_requires_a_value_of_type_1,
                    vec![
                        JsString::from_bytes(b"reference.circular".as_slice()),
                        JsString::from_bytes(b"boolean".as_slice()),
                    ],
                ));
                false
            }
            None => false,
        };
        result.push(ProjectReference {
            path: JsString::from_bytes(tsr_tspath::absolute(path.as_bytes(), base)),
            original_path: path.clone(),
            circular,
        });
    }
    Some(result)
}
/// Parse a JSON-mode source file, preserving raw property order and source ranges.
/// Extended syntax owners live with the returned command line and its diagnostics.
/// port: tsc/internal/tsoptions/tsconfigparsing.go:ParseJsonSourceFileConfigFileContent
pub fn parse_json_source_file_config_file_content(
    source: TsConfigSourceFile,
    host: &dyn ParseConfigHost,
    base: &[u8],
    existing: &CompilerOptions,
    existing_raw: &ConfigValue,
    name: &[u8],
) -> Result<ParsedCommandLine, Error> {
    tsr_parser::on_parser_worker(|| {
        let parsed = parse_config(Some(source), None, host, base, name, &[])?;
        finish_config(parsed, host, base, existing, existing_raw, name)
    })
}
fn finish_config(
    mut parsed: Parsed,
    host: &dyn ParseConfigHost,
    base: &[u8],
    existing: &CompilerOptions,
    existing_raw: &ConfigValue,
    name: &[u8],
) -> Result<ParsedCommandLine, Error> {
    let base_files = if name.is_empty() {
        tsr_tspath::normalize(base).into_owned()
    } else {
        tsr_tspath::normalize(&tsr_tspath::directory(&tsr_tspath::combine(base, &[name])))
            .into_owned()
    };
    let options = parsed.options.get_or_insert_default();
    merge_compiler_options(options, existing, existing_raw);
    substitute_options(options, &base_files);
    if !name.is_empty() {
        options.config_file_path =
            JsString::from_bytes(tsr_tspath::normalize_slashes(name).into_owned());
    }
    let config_specs = specs(&mut parsed, &base_files, name);
    let containing = if name.is_empty() {
        tsr_tspath::combine(&base_files, &[b"tsconfig.json"])
    } else {
        name.to_vec()
    };
    let mapper_values = validated_raw_array(&mut parsed, b"contentMappers", "object");
    let options = parsed.options.as_ref().expect("root config options");
    let mut mappers = crate::config_mappers::validate_content_mappers(
        mapper_values.as_deref().unwrap_or_default(),
        parsed.source.as_ref(),
        host.fs().use_case_sensitive_file_names(),
        options.run_external_code.is_true(),
        &containing,
        &mut |containing, package| host.resolve_content_mapper(containing, package),
    )?;
    parsed.errors.append(&mut mappers.diagnostics);
    let (file_names, literal_file_names_len) = crate::file_names_from_specs(
        &config_specs,
        &base_files,
        options,
        host.fs(),
        mappers.extensions.as_deref().unwrap_or_default(),
    )?;
    if file_names.is_empty()
        && parsed.raw.get(b"files").is_none()
        && parsed.raw.get(b"references").is_none()
    {
        parsed.errors.push(Diagnostic::compiler(d::No_inputs_were_found_in_config_file_0_Specified_include_paths_were_1_and_exclude_paths_were_2,vec![JsString::from_bytes(name),json_specs(&config_specs.include_specs),json_specs(&config_specs.exclude_specs)]));
    }
    let project_references = references(&mut parsed, &base_files);
    let compile_on_save = Some(matches!(
        parsed.raw.get(b"compileOnSave"),
        Some(ConfigValue::Boolean(true))
    ));
    Ok(ParsedCommandLine {
        watch_options: None,
        options: parsed.options.expect("root config options"),
        root_file_names: file_names,
        config_file: parsed.source.map(Arc::new),
        config_dependencies: parsed.dependencies,
        caches: crate::parsed_accessors::ParsedCaches::default(),
        errors: parsed.errors,
        raw: parsed.raw,
        compile_on_save,
        wildcard_directories_cache: std::sync::OnceLock::new(),
        config_specs: Some(config_specs),
        config_base_path: JsString::from_bytes(base_files),
        config_case_sensitive: host.fs().use_case_sensitive_file_names(),
        type_acquisition: Some(parsed.types),
        project_references,
        literal_file_names_len,
        content_mappers: mappers.mappers,
    })
}
fn json_specs(value: &ConfigValue) -> JsString {
    JsString::from_bytes(crate::config_json::stringify_json(value).expect("config spec JSON"))
}

/// Parse an already-decoded value without inventing source ranges. Config values
/// carry ordered object members, so no reflection/normalization bridge is needed.
/// port: tsc/internal/tsoptions/tsconfigparsing.go:ParseJsonConfigFileContent
pub fn parse_json_config_file_content(
    raw: ConfigValue,
    host: &dyn ParseConfigHost,
    base: &[u8],
    existing: &CompilerOptions,
    name: &[u8],
    resolution_stack: &[JsString],
) -> Result<ParsedCommandLine, Error> {
    let raw = if raw.as_object().is_some() {
        raw
    } else {
        ConfigValue::Object(tsr_core::collections::OrderedMap::default())
    };
    tsr_parser::on_parser_worker(|| {
        let parsed = parse_config(None, Some(raw), host, base, name, resolution_stack)?;
        finish_config(parsed, host, base, existing, &ConfigValue::Null, name)
    })
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:parseOwnConfigOfJson
fn own_json_config(
    mut raw: ConfigValue,
    host: &dyn ParseConfigHost,
    base: &[u8],
    name: &[u8],
) -> Result<(Parsed, Vec<JsString>), Error> {
    let mut errors = Vec::new();
    if raw.get(b"excludes").is_some() {
        errors.push(Diagnostic::compiler(
            d::Unknown_option_excludes_Did_you_mean_exclude,
            vec![],
        ));
    }
    let (mut options, mut option_errors) = crate::compiler_options_from_json(
        raw.get(b"compilerOptions").unwrap_or(&ConfigValue::Null),
        base,
        name,
    );
    errors.append(&mut option_errors);
    let (types, mut type_errors) = type_acquisition_from_json(
        raw.get(b"typeAcquisition").unwrap_or(&ConfigValue::Null),
        base,
        name,
    );
    errors.append(&mut type_errors);
    if let Some(value) = raw.get(b"compileOnSave") {
        let declaration = find_declaration(ROOT_OPTIONS, b"compileOnSave", false)
            .expect("compileOnSave declaration");
        let (converted, mut diagnostics) =
            convert_json_option(declaration, value, base, OptionSyntax::default());
        let converted = converted.into_owned();
        errors.append(&mut diagnostics);
        raw.set(JsString::from_bytes(b"compileOnSave".as_slice()), converted);
    }
    let mut extended = Vec::new();
    if let Some(value) = raw
        .get(b"extends")
        .filter(|value| !value.is_null() && !value.as_string().is_some_and(JsString::is_empty))
    {
        let (paths, mut diagnostics) = extends_paths(value, host, base, name, None, None)?;
        extended = paths;
        errors.append(&mut diagnostics);
    }
    if options.paths.is_some() {
        options.paths_base_path = JsString::from_bytes(base);
    }
    Ok((
        Parsed {
            raw,
            options: Some(options),
            types,
            source: None,
            dependencies: Vec::new(),
            errors,
        },
        extended,
    ))
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:convertTypeAcquisitionFromJsonWorker
pub fn type_acquisition_from_json(
    value: &ConfigValue,
    base: &[u8],
    name: &[u8],
) -> (TypeAcquisition, Vec<Diagnostic>) {
    let mut result = TypeAcquisition::for_config(name);
    let mut errors = Vec::new();
    if let Some(values) = value.as_object() {
        for (key, value) in values {
            if let Some(declaration) =
                find_declaration(TYPE_ACQUISITION_OPTIONS, key.as_bytes(), false)
                    .filter(|d| d.name.as_bytes() == key.as_bytes())
            {
                let (value, mut diagnostics) =
                    convert_json_option(declaration, value, base, OptionSyntax::default());
                errors.append(&mut diagnostics);
                result.parse_option(key.as_bytes(), &value);
            } else {
                let suggestion = tsr_scanner::get_spelling_suggestion_for_strings(
                    key.as_bytes(),
                    TYPE_ACQUISITION_OPTIONS.iter().map(|d| d.name.as_bytes()),
                );
                errors.push(match suggestion {
                    Some(suggestion) => Diagnostic::compiler(
                        d::Unknown_type_acquisition_option_0_Did_you_mean_1,
                        vec![key.clone(), JsString::from_bytes(suggestion)],
                    ),
                    None => Diagnostic::compiler(
                        d::Unknown_type_acquisition_option_0,
                        vec![key.clone()],
                    ),
                });
            }
        }
    }
    (result, errors)
}

fn validated_raw_array(parsed: &mut Parsed, key: &[u8], element: &str) -> Option<Vec<ConfigValue>> {
    let value = parsed.raw.get(key)?;
    if parsed.source.is_none() && !value.is_null() {
        let invalid = match value.as_array() {
            Some(values)
                if values.iter().all(|v| {
                    if element == "string" {
                        v.as_string().is_some()
                    } else {
                        v.as_object().is_some()
                    }
                }) =>
            {
                None
            }
            Some(_) => Some(element),
            None => Some("Array"),
        };
        if let Some(expected) = invalid {
            parsed.errors.push(Diagnostic::compiler(
                d::Compiler_option_0_requires_a_value_of_type_1,
                vec![
                    JsString::from_bytes(key),
                    JsString::from_bytes(expected.as_bytes()),
                ],
            ));
        }
    }
    raw_array(&parsed.raw, key)
}
