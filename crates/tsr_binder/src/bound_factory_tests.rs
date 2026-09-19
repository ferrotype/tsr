use ts_arena::Counters;
use ts_ast::{
    AstBuilder, Factory, FactoryMethods, JsString, RuntimeFactory, SourceFileParseOptions,
};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

#[test]
fn clone_and_update_bound_js_match_go_without_rebinding() {
    let parsed = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(
            b"exports.x = this; async function f() { return this; }".as_slice(),
        ),
        ScriptKind::JS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/bound.js".as_slice()),
            path: JsString::from_bytes(b"/bound.js".as_slice()),
            ..Default::default()
        },
    );
    let original = parsed.root();
    let file = parsed.publish_unbound();
    let (statements, eof) = {
        let node = file.view().node(original).unwrap();
        let data = node.data_source().as_source_file().unwrap();
        (data.statements().unwrap(), data.end_of_file_token())
    };
    let children = file.view().list(statements).unwrap().nodes();
    let function = file.view().node_slice(children).unwrap().at(1).unwrap();
    let first_child = file.view().node_slice(children).unwrap().at(0);
    let parsed_flags = file.view().node(original).unwrap().flags();
    let parsed_function_flags = file.view().node(function).unwrap().flags();
    assert!(file
        .view()
        .source_file(original)
        .unwrap()
        .common_js_module_indicator()
        .is_none());
    let bound = crate::bind_source_file(&file, original).unwrap();
    let common_js = bound
        .view()
        .source_file()
        .unwrap()
        .common_js_module_indicator();
    let bound_flags = bound.view().node(original).unwrap().flags();
    let bound_function_flags = bound.view().node(function).unwrap().flags();
    assert_ne!(parsed_flags, bound_flags);
    assert_ne!(parsed_function_flags, bound_function_flags);

    let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
    factory.retain_file(file.clone());
    let cloned = factory.clone_source_file(original);
    let changed_statements = factory.clone_list_header(statements);
    let updated = factory.update_source_file(original, Some(changed_statements), eof);
    let cloned_function = factory.clone_function_declaration(function);
    let unchanged = factory.update_source_file(original, Some(statements), eof);
    let observation = serde_json::json!({
        "parsedFlags": parsed_flags,
        "boundFlags": bound_flags,
        "cloneFlags": factory.node(cloned).flags(),
        "updateFlags": factory.node(updated).flags(),
        "parsedFunctionFlags": parsed_function_flags,
        "boundFunctionFlags": bound_function_flags,
        "cloneFunctionFlags": factory.node(cloned_function).flags(),
        "commonJS": common_js.is_some(),
        "cloneCommonJS": factory.read_source_file(cloned).unwrap().common_js_module_indicator() == common_js,
        "updateCommonJS": factory.read_source_file(updated).unwrap().common_js_module_indicator() == common_js,
        "sameCloneStatements": factory.node(cloned).data_source().as_source_file().unwrap().statements() == Some(statements),
        "sameUpdateChild": factory.read_nodes(factory.read_list(changed_statements).nodes()).at(0) == first_child,
        "sameCloneEOF": factory.node(cloned).data_source().as_source_file().unwrap().end_of_file_token() == eof,
        "sameUpdateEOF": factory.node(updated).data_source().as_source_file().unwrap().end_of_file_token() == eof,
        "sameUnchangedUpdate": unchanged == original,
    });
    assert_eq!(factory.view().node(original).unwrap().flags(), parsed_flags);
    assert_eq!(
        file.view().node(function).unwrap().flags(),
        parsed_function_flags
    );
    assert!(factory
        .view()
        .source_file(original)
        .unwrap()
        .common_js_module_indicator()
        .is_none());
    let result = factory.complete(cloned).unwrap().publish_unbound();
    let mut observation = observation;
    observation["cloneBound"] = result.is_bound(cloned).unwrap().into();
    observation["updateBound"] = result.is_bound(updated).unwrap().into();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../data/s07/bound-clone.json")).unwrap();
    assert_eq!(observation, expected);
    drop(bound);
    drop(file);
    assert_eq!(
        result
            .view()
            .source_file(cloned)
            .unwrap()
            .common_js_module_indicator(),
        common_js
    );
    assert!(result.view().node(common_js.unwrap()).is_ok());
    assert!(result.is_bound(original).unwrap());
    assert!(!result.is_bound(updated).unwrap());
}
