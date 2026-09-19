use serde_json::{json, Value};
use std::fmt::Write;
use std::sync::Arc;
use ts_jsstring::{JsString, SourceText};
use ts_tsoptions::{config_mappers::validate_content_mappers, ConfigValue};
use ts_vfs::MemoryBuilder;
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn config_value(row: &Value) -> ConfigValue {
    match row["kind"].as_str().unwrap() {
        "null" => ConfigValue::Null,
        "empty" => ConfigValue::EmptyStruct,
        "nil_array" => ConfigValue::Array(None),
        "boolean" => ConfigValue::Boolean(row["boolean"].as_bool().unwrap()),
        "number" => ConfigValue::Number(f64::from_bits(
            u64::from_str_radix(row["bits"].as_str().unwrap(), 16).unwrap(),
        )),
        "string" => ConfigValue::String(JsString::from_bytes(unhex(row["hex"].as_str().unwrap()))),
        "array" => ConfigValue::Array(Some(
            row["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(config_value)
                .collect(),
        )),
        "object" => ConfigValue::Object(
            row["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        JsString::from_bytes(unhex(entry["key"].as_str().unwrap())),
                        config_value(&entry["value"]),
                    )
                })
                .collect(),
        ),
        other => panic!("unknown value {other}"),
    }
}
#[test]
fn original_go_compact_json() {
    let requests: Value = serde_json::from_str(include_str!(
        "../../../data/s07/config-mapper-requests.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../data/s07/config-mapper-observations.json"
    ))
    .unwrap();
    let requests = requests["json"].as_array().unwrap();
    let expected = expected["json"].as_array().unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        let observed =
            match ts_tsoptions::config_json::stringify_json(&config_value(&request["value"])) {
                Ok(text) => json!({"id":request["id"],"error":false,"text_hex":hex(&text)}),
                Err(_) => json!({"id":request["id"],"error":true}),
            };
        assert_eq!(&observed, expected, "{}", request["id"]);
    }
}
#[test]
fn original_go_mapper_validation_and_manifests() {
    let requests: Value = serde_json::from_str(include_str!(
        "../../../data/s07/config-mapper-requests.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../data/s07/config-mapper-observations.json"
    ))
    .unwrap();
    let requests = requests["mappers"].as_array().unwrap();
    let expected = expected["mappers"].as_array().unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        let mut host = MemoryBuilder::new(
            b"/home/project",
            request["CaseSensitive"].as_bool().unwrap(),
        );
        for (path, text) in request["files"].as_object().unwrap() {
            host.insert_physical(path.as_bytes(), text.as_str().unwrap().as_bytes());
        }
        let host: Arc<dyn ts_vfs::FileSystem> = Arc::new(host.finish());
        let text = SourceText::from_loaded_bytes(request["text"].as_str().unwrap().as_bytes());
        let name = JsString::from_bytes(b"/home/project/tsconfig.json".as_slice());
        let parsed = ts_tsoptions::parse_config_file_text_to_json(name.clone(), name.clone(), text);
        let values = parsed
            .value
            .get(b"contentMappers")
            .and_then(ConfigValue::as_array)
            .unwrap_or_default();
        let property = ts_tsoptions::find_property(&parsed.source, &[b"contentMappers"]).unwrap();
        let node = parsed.source.file.view().node(property).unwrap();
        let ts_ast::NodeDataRead::PropertyAssignment(data) = node.data() else {
            panic!("config property")
        };
        let option = ts_tsoptions::ROOT_OPTIONS
            .iter()
            .find(|option| option.name == "contentMappers")
            .unwrap();
        let (_, mut prelude) = ts_tsoptions::convert_json_option(
            option,
            parsed.value.get(b"contentMappers").unwrap(),
            b"/home/project",
            ts_tsoptions::OptionSyntax {
                config: Some(&parsed.source),
                property: Some(property),
                value: data.initializer(),
            },
        );
        let result = validate_content_mappers(
            values,
            Some(&parsed.source),
            request["CaseSensitive"].as_bool().unwrap(),
            request["RunExternalCode"].as_bool().unwrap(),
            name.as_bytes(),
            &mut |containing, package| {
                ts_module::resolve_content_mapper_manifest(
                    &host,
                    b"/home/project",
                    containing,
                    package,
                )
            },
        )
        .unwrap();
        let mappers=result.mappers.map(|mappers|mappers.iter().map(|mapper|{
            let strings=|values:&[JsString]|values.iter().map(|value|value.as_str().unwrap().to_owned()).collect::<Vec<_>>();
            json!({"package":mapper.package.as_str().unwrap(),"extensions":strings(&mapper.extensions),"options_hex":hex(mapper.options.as_deref().unwrap_or_default()),"directory":mapper.package_directory.as_str().unwrap(),"manifest":{"Name":mapper.manifest.name.as_str().unwrap(),"Version":mapper.manifest.version.as_str().unwrap(),"Exec":mapper.manifest.exec.as_ref().map(|v|strings(v)),"CompilerOptions":mapper.manifest.compiler_options.as_ref().map(|v|strings(v)),"DynamicConfig":mapper.manifest.dynamic_config}})
        }).collect::<Vec<_>>()).filter(|mappers|!mappers.is_empty());
        prelude.extend(result.diagnostics);
        let diagnostics=prelude.iter().map(|d|json!({"code":d.code,"pos":d.loc.pos(),"end":d.loc.end(),"args_hex":d.message_args.iter().map(|arg|hex(arg.as_bytes())).collect::<Vec<_>>()})).collect::<Vec<_>>();
        let observed = json!({"id":request["id"],"mappers":mappers,"diagnostics":diagnostics});
        assert_eq!(&observed, expected, "{}", request["id"]);
    }
}
