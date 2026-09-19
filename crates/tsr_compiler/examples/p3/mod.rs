//! Shared program loading for P3 contract drivers; all use the production
//! loader and the actual bundled library declarations.
use serde_json::Value;
use std::sync::Arc;
use ts_arena::Counters;
use ts_compiler::{FileCache, Program, ProgramOptions};
use ts_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use ts_jsstring::JsString;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| "expected string".into())
}
pub fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "expected array".into())
}
fn unhex(value: &Value) -> Result<Vec<u8>> {
    let s = text(value)?;
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&s[i..i + 2], 16)?))
        .collect()
}
pub fn load(request: &Value, counters: &Counters) -> Result<Arc<Program>> {
    let mut fs = ts_vfs::MemoryBuilder::new(b"/", false);
    fs.insert_loaded(b"/fixture.ts", unhex(&request["source_hex"])?);
    for (name, bytes) in request["files"].as_object().ok_or("expected files")? {
        fs.insert_loaded(name.as_bytes(), unhex(bytes)?);
    }
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        skip_lib_check: Tristate::TRUE,
        allow_js: if request["allow_js"] == true {
            Tristate::TRUE
        } else {
            Tristate::FALSE
        },
        ..Default::default()
    };
    let config = ts_tsoptions::ParsedCommandLine::new(
        options,
        vec![JsString::from_bytes(b"/fixture.ts".as_slice())],
    );
    Ok(Arc::new(Program::load(
        ProgramOptions {
            config,
            host: Arc::new(ts_bundled::BundledFs::new(Arc::new(fs.finish()))),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(ts_bundled::LIB_PATH),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        counters,
    )?))
}
