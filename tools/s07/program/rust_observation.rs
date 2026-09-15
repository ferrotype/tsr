use super::ts_compiler_error;
use super::{FileCache, Program, ProgramOptions};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use ts_arena::Counters;
use ts_jsstring::JsString;
use ts_vfs::MemoryBuilder;
pub(super) fn bytes(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn text(value: &[u8]) -> &str {
    std::str::from_utf8(value).expect("fixture output is valid UTF-8")
}
pub(super) fn try_load(
    request: &Value,
    cache: &mut FileCache,
    counters: &Counters,
    parsed_config: Option<ts_tsoptions::ParsedCommandLine>,
) -> Result<Program, ts_compiler_error::Error> {
    let mut fs = MemoryBuilder::new(
        request["cwd"].as_str().unwrap().as_bytes(),
        request["case_sensitive"].as_bool().unwrap(),
    );
    for (name, value) in request["files"].as_object().unwrap() {
        fs.insert_physical(name.as_bytes(), bytes(value.as_str().unwrap()));
    }
    if let Some(links) = request["symlinks"].as_object() {
        for (name, target) in links {
            fs.insert_symlink(name.as_bytes(), target.as_str().unwrap().as_bytes());
        }
    }
    let options = ts_tsoptions::raw::compiler_options(&request["options"]).unwrap();
    let roots = request["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| JsString::from_bytes(v.as_str().unwrap().as_bytes()))
        .collect();
    let host = Arc::new(ts_bundled::BundledFs::new(Arc::new(fs.finish())));
    let mut config = ts_tsoptions::ParsedCommandLine::new(options, roots);
    if let Some(parsed) = parsed_config {
        // CompileFilesEx carries config diagnostics and their syntax owners
        // beside the already finalized fixture options and source-file list.
        config.config_file = parsed.config_file;
        config.config_dependencies = parsed.config_dependencies;
        config.errors = parsed.errors;
        config.content_mappers = parsed.content_mappers;
        // Go stores these include/exclude specifications on TsConfigSourceFile.
        // Rust keeps them beside that source in ParsedCommandLine.
        config.config_specs = parsed.config_specs;
        // CompileFilesEx constructs a fresh ParsedCommandLine without copying
        // comparePathsOptions. Preserve its zero-valued matching context even
        // though the earlier config parse had an explicit directory and casing.
        config.config_case_sensitive = false;
    } else if let Some(name) = request["config_name"].as_str() {
        let text = bytes(request["config_text"].as_str().expect("config text hex"));
        config.config_file = Some(Arc::new(ts_tsoptions::TsConfigSourceFile::parse(
            JsString::from_bytes(name.as_bytes()),
            ts_tspath::to_path(
                name.as_bytes(),
                request["cwd"].as_str().unwrap().as_bytes(),
                request["case_sensitive"].as_bool().unwrap(),
            ),
            ts_jsstring::SourceText::from_loaded_bytes(text),
        )));
    }
    Program::load(
        ProgramOptions {
            config,
            host,
            current_directory: JsString::from_bytes(request["cwd"].as_str().unwrap().as_bytes()),
            default_library_path: JsString::from_bytes(ts_bundled::LIB_PATH),
            skip_module_resolution: request["skip_module_resolution"].as_bool().unwrap_or(false),
        },
        cache,
        counters,
    )
}
fn nullable<T>(values: Vec<T>) -> Option<Vec<T>> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}
pub(super) fn diagnostic(d: &ts_ast::Diagnostic, program: &Program) -> Value {
    let name = d
        .file
        .map(|id| {
            if let Some(config) = program
                .config()
                .config_file
                .iter()
                .chain(&program.config().config_dependencies)
                .find(|config| config.root == id)
            {
                return config
                    .file
                    .view()
                    .source_file(config.root)
                    .unwrap()
                    .parse_options()
                    .file_name
                    .clone();
            }
            program
                .files()
                .iter()
                .find(|f| f.source() == id)
                .expect("diagnostic source retained by program")
                .bound()
                .view()
                .source_file()
                .unwrap()
                .parse_options()
                .file_name
                .clone()
        })
        .unwrap_or_default();
    json!({"File":text(name.as_bytes()),"Pos":d.loc.pos(),"End":d.loc.end(),"Code":d.code,"Category":d.category,"Key":text(d.message_key.as_bytes()),"Args":nullable(d.message_args.iter().map(|a|text(a.as_bytes())).collect()),"Text":text(d.message_text.as_bytes()),"Chain":nullable(d.message_chain.iter().map(|d|diagnostic(d, program)).collect()),"Related":nullable(d.related_information.iter().map(|d|diagnostic(d, program)).collect())})
}
pub(super) fn observe(id: &str, program: &Program) -> Value {
    let files:Vec<_>=program.files().iter().map(|file|{let view=file.bound().view();let source=view.source_file().unwrap();let options=source.parse_options();let name=options.file_name.as_bytes();let path=options.path.as_bytes();let meta=program.metadata(path).unwrap();json!({"Name":text(name),"Path":text(path),"SHA256":format!("{:x}",Sha256::digest(source.text().as_bytes())),"Bytes":source.text().as_bytes().len(),"Meta":{"PackageJsonType":text(meta.package_json_type.as_bytes()),"PackageJsonDirectory":text(meta.package_json_directory.as_bytes()),"ImpliedNodeFormat":meta.implied_node_format.0},"Lib":program.is_lib(path),"Imports":source.imports().unwrap().iter().map(|id|text(view.ast().node_text(id.unwrap()).unwrap().as_bytes()).to_owned()).collect::<Vec<_>>(),"External":source.external_module_indicator.is_some()})}).collect();
    let resolutions:Vec<_>=program.resolutions().iter().map(|r|{let result=&r.result;let p=&result.package_id;json!({"File":text(r.file.as_bytes()),"Name":text(r.name.as_bytes()),"Mode":r.mode.0,"Result":{"ResolutionDiagnostics":nullable(result.resolution_diagnostics.iter().map(|d|diagnostic(d, program)).collect()),"ResolvedFileName":text(result.resolved_file_name.as_bytes()),"OriginalPath":text(result.original_path.as_bytes()),"Extension":text(result.extension.as_bytes()),"ResolvedUsingTsExtension":result.resolved_using_ts_extension,"ResolvedUsingExtraExtensions":result.resolved_using_extra_extensions,"PackageId":{"Name":text(p.name.as_bytes()),"SubModuleName":text(p.sub_module_name.as_bytes()),"Version":text(p.version.as_bytes()),"PeerDependencies":text(p.peer_dependencies.as_bytes())},"IsExternalLibraryImport":result.is_external_library_import,"AlternateResult":text(result.alternate_result.as_bytes())}})}).collect();
    let types:Vec<_> = program.type_resolutions().iter().map(|r| { let result=&r.result; let p=&result.package_id; json!({"File":text(r.file.as_bytes()),"Name":text(r.name.as_bytes()),"Mode":r.mode.0,"Result":{"ResolutionDiagnostics":nullable(result.resolution_diagnostics.iter().map(|d|diagnostic(d, program)).collect()),"Primary":result.primary,"ResolvedFileName":text(result.resolved_file_name.as_bytes()),"OriginalPath":text(result.original_path.as_bytes()),"PackageId":{"Name":text(p.name.as_bytes()),"SubModuleName":text(p.sub_module_name.as_bytes()),"Version":text(p.version.as_bytes()),"PeerDependencies":text(p.peer_dependencies.as_bytes())},"IsExternalLibraryImport":result.is_external_library_import}}) }).collect();
    json!({"ID":id,"Files":files,"Missing":nullable(program.missing_files().iter().map(|p|text(p.as_bytes())).collect()),"Resolutions":resolutions,"TypeResolutions":types,"Diagnostics":nullable(program.include_diagnostics().iter().map(|d| diagnostic(d, program)).collect()),"Trace":nullable(program.trace().iter().map(|entry| {let args:Vec<_>=entry.args.iter().map(|arg| match arg {ts_module::TraceArg::Text(value)=>text(value.as_bytes()).to_owned(),ts_module::TraceArg::Bool(value)=>value.to_string()}).collect();format!("{}:[{}]",entry.message.code,args.join(" "))}).collect())})
}

/// Observe option verification separately from loader graph construction.
#[allow(dead_code)] // This shared probe module also compiles in loader-only test targets.
pub(super) fn verify_options(id: &str, program: &Program) -> Value {
    let report = program.option_verification();
    json!({"id":id,"diagnostics":report.diagnostics.iter().map(|d|diagnostic(d,program)).collect::<Vec<_>>(),"includes":report.include_diagnostics.iter().map(|d|json!({"file":text(d.file.as_bytes()),"code":d.message.code,"args":d.args.iter().map(|arg|text(arg.as_bytes())).collect::<Vec<_>>()})).collect::<Vec<_>>(),"blocked":report.blocked_output_paths.iter().map(|path|text(path.as_bytes())).collect::<Vec<_>>()} )
}
