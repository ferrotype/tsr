use super::*;
use crate::{FileCache, ProgramOptions};
use serde_json::{json, Value};
use ts_arena::Counters;
use ts_tsoptions::{ConfigFileSpecs, ParsedCommandLine, TsConfigSourceFile};

fn bytes(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn hex(value: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}
fn strings(value: &Value) -> Vec<JsString> {
    value
        .as_array()
        .map(|array| {
            array
                .iter()
                .map(|value| JsString::from_bytes(value.as_str().unwrap().as_bytes()))
                .collect()
        })
        .unwrap_or_default()
}
fn load(request: &Value) -> Program {
    let cwd = request["cwd"].as_str().unwrap().as_bytes();
    let case_sensitive = request["case_sensitive"].as_bool().unwrap();
    let mut fs = ts_vfs::MemoryBuilder::new(cwd, case_sensitive);
    for (name, value) in request["files"].as_object().unwrap() {
        fs.insert_physical(name.as_bytes(), bytes(value.as_str().unwrap()));
    }
    if let Some(links) = request["symlinks"].as_object() {
        for (name, target) in links {
            fs.insert_symlink(name.as_bytes(), target.as_str().unwrap().as_bytes());
        }
    }
    let options = ts_tsoptions::raw::compiler_options(&request["options"]).unwrap();
    let mut config = ParsedCommandLine::new(options, strings(&request["roots"]));
    if let Some(name) = request["config_name"].as_str() {
        config.config_file = Some(Arc::new(TsConfigSourceFile::parse(
            JsString::from_bytes(name.as_bytes()),
            path::to_path(name.as_bytes(), cwd, case_sensitive),
            ts_jsstring::SourceText::from_loaded_bytes(bytes(
                request["config_text"].as_str().unwrap(),
            )),
        )));
        let specs = &request["specs"];
        config.config_specs = Some(ConfigFileSpecs {
            validated_files: strings(&specs["Files"]),
            files_before_substitution: strings(&specs["BeforeFiles"]),
            validated_includes: strings(&specs["Includes"]),
            includes_before_substitution: strings(&specs["BeforeIncludes"]),
            is_default_include: specs["Default"].as_bool().unwrap_or(false),
            ..ConfigFileSpecs::default()
        });
        config.config_base_path = JsString::from_bytes(cwd);
        config.config_case_sensitive = case_sensitive;
    }
    Program::load(
        ProgramOptions {
            config,
            host: Arc::new(ts_bundled::BundledFs::new(Arc::new(fs.finish()))),
            current_directory: JsString::from_bytes(cwd),
            default_library_path: JsString::from_bytes(ts_bundled::LIB_PATH),
            skip_module_resolution: request["skip_module_resolution"].as_bool().unwrap_or(false),
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap()
}
fn diagnostic(diagnostic: &Diagnostic, program: &Program) -> Value {
    let file = diagnostic.file.map_or_else(String::new, |source| {
        if let Some(config) = &program.config().config_file {
            if config.root == source {
                return hex(config.file.view().source_file(source).unwrap().file_name());
            }
        }
        let file = program
            .files()
            .iter()
            .find(|file| file.source() == source)
            .expect("diagnostic source retained by program");
        hex(file.bound().view().source_file().unwrap().file_name())
    });
    json!({"File":file,"Pos":diagnostic.loc.pos(),"End":diagnostic.loc.end(),"Code":diagnostic.code,"Args":diagnostic.message_args.iter().map(|arg| hex(arg.as_bytes())).collect::<Vec<_>>(),"Chain":diagnostic.message_chain.iter().map(|d| self::diagnostic(d,program)).collect::<Vec<_>>(),"Related":diagnostic.related_information.iter().map(|d| self::diagnostic(d,program)).collect::<Vec<_>>()})
}

#[test]
fn independently_observed_go_include_explanations() {
    let requests: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/include-reason-requests.json"
    ))
    .unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/include-reason-observations.json"
    ))
    .unwrap();
    assert_eq!(requests.len(), expected.len());
    for (request, expected) in requests.iter().zip(expected) {
        assert_eq!(request["id"], expected["id"]);
        let mut program = load(request);
        let mut observed = Vec::new();
        for query in request["queries"].as_array().unwrap() {
            let file_path = query["Path"].as_str().unwrap();
            let original = program.include_reasons.get(file_path.as_bytes()).cloned();
            if let Some(source_path) = query["AugmentationFrom"].as_str() {
                let source = program
                    .file(source_path.as_bytes())
                    .unwrap()
                    .bound()
                    .view()
                    .source_file()
                    .unwrap();
                let reason = Arc::new(IncludeReason::new(IncludeReasonData::Import {
                    file: JsString::from_bytes(source_path.as_bytes()),
                    index: isize::try_from(source.imports().unwrap().len()).unwrap(),
                    synthetic: None,
                    package_id: PackageId::default(),
                }));
                program
                    .include_reasons
                    .entry(JsString::from_bytes(file_path.as_bytes()))
                    .or_default()
                    .push(reason);
            }
            if let Some(identity) = query["Identity"].as_str() {
                let reasons = program
                    .include_reasons
                    .get_mut(file_path.as_bytes())
                    .unwrap();
                let first = &reasons[0];
                let duplicate = if identity == "distinct" {
                    Arc::new(IncludeReason::new(first.data.clone()))
                } else {
                    Arc::clone(first)
                };
                reasons.push(duplicate);
            }
            let reasons = program
                .include_reasons
                .get(file_path.as_bytes())
                .map_or(&[][..], Vec::as_slice);
            assert!(
                !reasons.is_empty() || matches!(file_path, "" | "/missing.ts"),
                "{}: missing reason for {file_path}",
                request["id"]
            );
            let preferred = query["Preferred"]
                .as_u64()
                .map(|i| reasons[usize::try_from(i).unwrap()].as_ref());
            let result = program.explain_file_include_with_reason(file_path.as_bytes(), preferred, d::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files, vec![JsString::from_bytes(file_path.as_bytes()), JsString::from_bytes(b"/root".as_slice())]).unwrap();
            let entries: Vec<_> = reasons.iter().map(|reason| {
                let absolute = reason.diagnostic(&program, false).unwrap();
                let relative = reason.diagnostic(&program, true).unwrap();
                let related = reason.related_info(&program).unwrap();
                let stable = Arc::ptr_eq(absolute, reason.diagnostic(&program, false).unwrap()) && Arc::ptr_eq(relative, reason.diagnostic(&program, true).unwrap()) && related.map(Arc::as_ptr) == reason.related_info(&program).unwrap().map(Arc::as_ptr);
                json!({"absolute":diagnostic(absolute,&program),"relative":diagnostic(relative,&program),"related":related.map(|d|diagnostic(d,&program)),"stable":stable})
            }).collect();
            observed.push(json!({"path":file_path,"diagnostic":diagnostic(&result,&program),"reasons":entries}));
            if let Some(original) = original {
                program
                    .include_reasons
                    .insert(JsString::from_bytes(file_path.as_bytes()), original);
            } else {
                program.include_reasons.remove(file_path.as_bytes());
            }
        }
        let globals: Vec<_> = program
            .program_diagnostics()
            .unwrap()
            .iter()
            .map(|d| diagnostic(d, &program))
            .collect();
        let include_globals: Vec<_> = program
            .global_include_diagnostics()
            .unwrap()
            .iter()
            .map(|d| diagnostic(d, &program))
            .collect();
        let mut include_files = serde_json::Map::new();
        for file in program.files() {
            let state = file.bound().view().source_file().unwrap();
            let key = state.parse_options().path.as_bytes();
            let values: Vec<_> = program
                .include_diagnostics_for_file(key)
                .unwrap()
                .iter()
                .map(|d| diagnostic(d, &program))
                .collect();
            if !values.is_empty() {
                include_files.insert(std::str::from_utf8(key).unwrap().to_owned(), json!(values));
            }
        }
        assert_eq!(
            json!({"id":request["id"],"queries":observed,"program":globals,"include_globals":include_globals,"include_files":include_files}),
            expected,
            "{}",
            request["id"]
        );
    }
}

#[test]
fn inclusion_caches_are_lazy_and_empty_results_stable() {
    let requests: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/include-reason-requests.json"
    ))
    .unwrap();
    let program = load(&requests[0]);
    assert!(!program.diagnostic_snapshot.initialized());
    assert!(program
        .include_explanations
        .redirects
        .read()
        .unwrap()
        .is_empty());
    assert!(program
        .include_explanations
        .compiler_options
        .get()
        .is_none());
    for reason in program.include_reasons.values().flatten() {
        assert!(reason.diagnostic.get().is_none());
        assert!(reason.relative_diagnostic.get().is_none());
        assert!(reason.location.get().is_none());
        assert!(reason.related.get().is_none());
    }
    let before = program
        .include_explanations
        .redirects(&program, b"/src/main.ts", false)
        .unwrap();
    assert!(before.is_empty());
    let after = program
        .include_explanations
        .redirects(&program, b"/src/main.ts", true)
        .unwrap();
    assert!(Arc::ptr_eq(&before, &after));
    assert!(program
        .include_explanations
        .compiler_options(&program)
        .is_none());
    assert_eq!(
        program.include_explanations.compiler_options.get(),
        Some(&None)
    );
    let root = &program.include_reasons[b"/src/main.ts".as_slice()][0];
    assert!(root.related_info(&program).unwrap().is_none());
    assert!(matches!(root.related.get(), Some(Ok(None))));
    let before = program.program_diagnostics().unwrap();
    assert!(program.diagnostic_snapshot.initialized());
    assert!(std::ptr::eq(before, program.program_diagnostics().unwrap()));
}
