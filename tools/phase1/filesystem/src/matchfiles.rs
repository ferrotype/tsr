//! Carried test envelope only. Config parsing, wildcard discovery, JSON and
//! diagnostic formatting all call production code. Only the historical input
//! prefix is decoded; the expected Result and Errors sections are discarded.
use crate::{api::Outcome, fs_trace as t};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use tsr_core::{collections::OrderedMap, CompilerOptions};
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{
    self as options, ConfigValue as V, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile,
};
use tsr_vfs::{self as vfs, Error, FileSystem, MemoryBuilder, WalkControl};
fn js(s: &[u8]) -> JsString {
    JsString::from_bytes(s)
}
fn text(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())
}
struct Host {
    fs: Arc<dyn FileSystem>,
    resolution: Arc<dyn FileSystem>,
    cwd: Vec<u8>,
}
fn module_error(e: tsr_module::Error) -> Error {
    match e {
        tsr_module::Error::Host(e) => e,
        tsr_module::Error::MutableHost => {
            Error::Unsupported("config resolver requires immutable host")
        }
        tsr_module::Error::Unsupported(e) => Error::Unsupported(e),
        tsr_module::Error::MalformedPackageJson(_) => Error::Unsupported("malformed package JSON"),
    }
}
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        &self.cwd
    }
    fn resolve_config(&self, name: &[u8], containing: &[u8]) -> Result<Option<JsString>, Error> {
        let r = tsr_module::resolve_config(name, containing, self.resolution.clone(), &self.cwd)
            .map_err(module_error)?;
        Ok((!r.resolved_file_name.is_empty()).then_some(r.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<options::config_mappers::MapperResolution, Error> {
        tsr_module::resolve_content_mapper_manifest(
            &self.resolution,
            &self.cwd,
            containing,
            package,
        )
        .map_err(module_error)
    }
}
struct Input {
    config: String,
    name: String,
    entries: Vec<(String, String, bool)>,
    root: String,
    sensitive: bool,
}
fn inputs(raw: &str) -> Result<Input, String> {
    let (config, rest) = raw
        .strip_prefix("config:\n")
        .ok_or("missing config header")?
        .split_once("\nFs::\n")
        .ok_or("missing Fs header")?;
    let (section, rest) = rest
        .split_once("\nconfigFileName:: ")
        .ok_or("missing config name")?;
    let (name, _expected) = rest
        .split_once("\nResult\n")
        .ok_or("missing Result boundary")?;
    if name.contains('\n') {
        return Err("multi-line config name".into());
    }
    let starts: Vec<_> = section
        .match_indices("//// [")
        .filter(|(i, _)| *i == 0 || section.as_bytes()[i - 1] == b'\n')
        .map(|(i, _)| i)
        .collect();
    let mut entries: Vec<(String, String, bool)> = Vec::new();
    for (i, &start) in starts.iter().enumerate() {
        let block = &section[start..starts.get(i + 1).copied().unwrap_or(section.len())];
        let (header, body) = block
            .split_once("\r\n")
            .ok_or("missing CRLF entry header")?;
        let (path, trailer) = header
            .strip_prefix("//// [")
            .unwrap()
            .split_once(']')
            .ok_or("unclosed file header")?;
        if let Some(target) = trailer.strip_prefix(" symlink(") {
            if !body.is_empty() {
                return Err("symlink has body".into());
            }
            entries.push((
                path.into(),
                target.strip_suffix(')').ok_or("unclosed symlink")?.into(),
                true,
            ));
        } else {
            if !trailer.is_empty() {
                return Err("invalid file trailer".into());
            }
            entries.push((
                path.into(),
                body.strip_suffix("\r\n\r\n")
                    .ok_or("file missing trailing CRLFs")?
                    .into(),
                false,
            ));
        }
    }
    let first: &str = &entries.first().ok_or("empty FS")?.0;
    let sensitive = first.starts_with('/');
    if entries
        .iter()
        .any(|(p, _, _)| p.starts_with('/') != sensitive)
    {
        return Err("mixed path styles".into());
    }
    let root = first[..tsr_tspath::root_length(first.as_bytes())].to_owned();
    if root.is_empty() {
        return Err("unrooted FS".into());
    }
    Ok(Input {
        config: config.into(),
        name: name.into(),
        entries,
        root,
        sensitive,
    })
}
fn ordered(pairs: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (js(k.as_bytes()), v))
            .collect(),
    )
}
fn strings(values: &[JsString]) -> V {
    V::Array(Some(values.iter().cloned().map(V::String).collect()))
}
fn result(parsed: &ParsedCommandLine) -> V {
    let encoded = options::compiler_options_value(&parsed.options);
    let members = encoded.as_object().expect("compiler option object");
    let mut ordered_options = OrderedMap::default();
    if let Some(declared) = parsed.raw.get(b"compilerOptions").and_then(V::as_object) {
        for (key, _) in declared {
            if let Some(value) = members.get(key) {
                ordered_options.insert(key.clone(), value.clone());
            }
        }
    }
    for (key, value) in members {
        if !ordered_options.contains_key(key) {
            ordered_options.insert(key.clone(), value.clone());
        }
    }
    let acquisition = parsed.type_acquisition.as_ref();
    let types = ordered([
        (
            "enable",
            V::Boolean(acquisition.is_some_and(|a| a.enable.is_true())),
        ),
        (
            "include",
            strings(
                acquisition
                    .and_then(|a| a.include.as_deref())
                    .unwrap_or_default(),
            ),
        ),
        (
            "exclude",
            strings(
                acquisition
                    .and_then(|a| a.exclude.as_deref())
                    .unwrap_or_default(),
            ),
        ),
    ]);
    let watches = V::Object(
        parsed
            .wildcard_directories()
            .into_iter()
            .flat_map(|map| map.iter())
            .map(|(path, recursive)| {
                (
                    path.clone(),
                    V::String(js(if *recursive {
                        b"WatchDirectoryFlags.Recursive"
                    } else {
                        b"WatchDirectoryFlags.None"
                    })),
                )
            })
            .collect(),
    );
    ordered([
        ("options", V::Object(ordered_options)),
        ("fileNames", strings(&parsed.root_file_names)),
        ("typeAcquisition", types),
        ("raw", parsed.raw.clone()),
        ("wildcardDirectories", watches),
        (
            "compileOnSave",
            V::Boolean(parsed.compile_on_save.unwrap_or(false)),
        ),
    ])
}
fn render(request: &Value) -> Result<String, String> {
    let baseline = t::text(request, "baseline")?;
    let path = std::path::Path::new(baseline);
    if !baseline.starts_with("config/matchFiles/")
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("invalid baseline input path".into());
    }
    let input = inputs(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../upstream/tsc/testdata/baselines/reference")
                .join(path),
        )
        .map_err(|e| e.to_string())?,
    )?;
    let base = tsr_tspath::absolute(&tsr_tspath::directory(input.name.as_bytes()), b"");
    let mut files = BTreeMap::new();
    let mut resolver = MemoryBuilder::new(&base, input.sensitive);
    let mut links = BTreeMap::new();
    for (path, value, link) in &input.entries {
        let file = if *link {
            links.insert(path.clone(), value.clone());
            resolver.insert_symlink(path.as_bytes(), value.as_bytes());
            vfs::vfstest::symlink(value.as_bytes())
        } else {
            resolver.insert_loaded(path.as_bytes(), value.as_bytes());
            vfs::iofs::MapFile {
                data: value.as_bytes().into(),
                ..Default::default()
            }
        };
        files.insert(
            path.as_bytes().to_vec(),
            vfs::vfstest::InputFile::File(file),
        );
    }
    let host = Host {
        fs: Arc::new(
            vfs::vfstest::from_map_with_clock(
                &files,
                input.sensitive,
                Arc::new(t::FixedClock(vfs::iofs::Time::ZERO)),
            )
            .into_vfs(),
        ),
        resolution: Arc::new(resolver.finish()),
        cwd: base.clone(),
    };
    let name = tsr_tspath::absolute(input.name.as_bytes(), &base);
    let path = tsr_tspath::to_path(input.name.as_bytes(), &base, input.sensitive);
    let source = SourceText::from_loaded_bytes(input.config.as_bytes());
    let parsed = match t::text(request, "api")? {
        "json" => {
            let raw = options::parse_config_file_text_to_json(js(&name), path, source);
            options::parse_json_config_file_content(
                raw.value,
                &host,
                &base,
                &CompilerOptions::default(),
                &name,
                &[],
            )
        }
        "jsonSourceFile" => options::parse_json_source_file_config_file_content(
            TsConfigSourceFile::parse(js(&name), path, source),
            &host,
            &base,
            &CompilerOptions::default(),
            &V::Null,
            &name,
        ),
        api => return Err(format!("unknown config API {api}")),
    }
    .map_err(|e| e.to_string())?;
    let mut output = format!("config:\n{}\nFs::\n", input.config);
    let mut failure = None;
    host.fs
        .walk_dir(input.root.as_bytes(), &mut |path, entry, error| {
            if let Some(error) = error {
                return Err(error);
            }
            let entry = entry.ok_or(Error::Unsupported("missing fixture entry"))?;
            let name = match text(path) {
                Ok(s) => s,
                Err(e) => {
                    failure = Some(e);
                    return Ok(WalkControl::SkipAll);
                }
            };
            if entry.symlink {
                if let Some(target) = links.get(&name) {
                    output.push_str(&format!("//// [{name}] symlink({target})\r\n"));
                } else {
                    failure = Some("undeclared symlink".into());
                    return Ok(WalkControl::SkipAll);
                }
            } else if entry.info.mode.is_regular() {
                let content = host
                    .fs
                    .read_file(path)?
                    .ok_or(Error::Unsupported("missing fixture file"))?;
                match text(&content.raw) {
                    Ok(content) => output.push_str(&format!("//// [{name}]\r\n{content}\r\n\r\n")),
                    Err(e) => {
                        failure = Some(e);
                        return Ok(WalkControl::SkipAll);
                    }
                }
            }
            Ok(WalkControl::Continue)
        })
        .map_err(|e| e.to_string())?;
    if let Some(error) = failure {
        return Err(error);
    }
    output.push_str(&format!("\nconfigFileName:: {}\nResult\n", input.name));
    output.push_str(&text(
        &options::config_json::stringify_json_indent(&result(&parsed), "", "  ")
            .map_err(|e| e.to_string())?,
    )?);
    output.push_str("\nErrors::\n");
    let mut writer = tsr_compiler::diagnostic_writer::DiagnosticWriter::from_sources(
        &parsed,
        tsr_compiler::diagnostic_writer::FormattingOptions {
            new_line: b"\r\n".to_vec(),
            current_directory: base,
            case_sensitive: input.sensitive,
            ..tsr_compiler::diagnostic_writer::FormattingOptions::default()
        },
    );
    for diagnostic in &parsed.errors {
        output.push_str(&text(
            &writer
                .format(&[diagnostic], true)
                .map_err(|e| e.to_string())?,
        )?);
        if diagnostic.file.is_none()
            || diagnostic.code == tsr_diagnostics::File_appears_to_be_binary.code
        {
            output.push_str("\r\n");
        }
    }
    output.push('\n');
    Ok(output)
}
pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "matchFilesBaseline" {
        return None;
    }
    Some(match render(request) {
        Ok(rendered) => Outcome::Observed(json!({"rendered":rendered})),
        Err(e) => Outcome::Failed(e),
    })
}
