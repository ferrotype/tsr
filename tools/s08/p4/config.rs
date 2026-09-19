//! Reproduce the native test runner's config parse before CompileFilesEx.
//! The frozen loading request already contains the final options and roots;
//! this phase supplies diagnostics and their owning source files separately.
use serde_json::Value;
use std::sync::Arc;
use ts_core::{CompilerOptions, Tristate};
use ts_jsstring::{JsString, SourceText};
use ts_tsoptions::{ConfigValue, ParsedCommandLine, TsConfigSourceFile};

#[path = "../../s07/config/host.rs"]
mod host;

pub(super) fn parse(
    request: &Value,
) -> Result<Option<ParsedCommandLine>, Box<dyn std::error::Error>> {
    let loading = &request["loading"];
    let Some(name) = loading["options"]["configFilePath"].as_str() else {
        return Ok(None);
    };
    // The earlier P4 inventory does not carry baseline input text. E2 and P5
    // provide these original inputs, including the config removed by the native
    // runner before it constructs the compiler's filesystem.
    let Some(inputs) = request.get("error_inputs").and_then(Value::as_array) else {
        return Ok(None);
    };
    let cwd = loading["cwd"]
        .as_str()
        .ok_or("missing config parse directory")?;
    // parseTestCaseContentWithSettings always uses a case-sensitive config VFS,
    // independently of the compiler host's useCaseSensitiveFileNames option.
    let mut fs = ts_vfs::MemoryBuilder::new(cwd.as_bytes(), true);
    let mut config_text = None;
    for input in inputs {
        let path = super::observation::bytes(
            input["name_hex"]
                .as_str()
                .ok_or("missing config input path")?,
        );
        let text = super::observation::bytes(
            input["content_hex"]
                .as_str()
                .ok_or("missing config input bytes")?,
        );
        if path == name.as_bytes() {
            config_text = Some(text.clone());
        }
        fs.insert_physical(&path, text);
    }
    let config_text = config_text.ok_or("config source missing from original baseline inputs")?;
    if let Some(links) = loading["symlinks"].as_object() {
        for (path, target) in links {
            fs.insert_symlink(
                path.as_bytes(),
                target.as_str().ok_or("invalid config symlink")?.as_bytes(),
            );
        }
    }
    let host = host::Host {
        fs: Arc::new(fs.finish()),
        cwd: JsString::from_bytes(cwd.as_bytes()),
    };
    // Native parses the root directly from its test unit, not through ReadFile's
    // BOM/encoding conversion. Inherited configs still load through the VFS.
    let source = TsConfigSourceFile::parse(
        JsString::from_bytes(name.as_bytes()),
        ts_tspath::to_path(name.as_bytes(), cwd.as_bytes(), true),
        SourceText::from_loaded_bytes(config_text),
    );
    Ok(Some(
        ts_tsoptions::parse_json_source_file_config_file_content(
            source,
            &host,
            &ts_tspath::directory(name.as_bytes()),
            &CompilerOptions {
                run_external_code: if loading["options"]["runExternalCode"] == true {
                    Tristate::TRUE
                } else {
                    Tristate::UNKNOWN
                },
                ..Default::default()
            },
            &ConfigValue::Null,
            name.as_bytes(),
        )?,
    ))
}
