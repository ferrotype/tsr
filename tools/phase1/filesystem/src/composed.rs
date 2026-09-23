//! One real-OS rebuild through cached/tracking VFS and the production loader.
//! Mirrors the composition in execute/watcher.go; it does not run its scheduler.
use crate::api::{subject, Outcome};
use crate::fs_trace::{array, text};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tsr_arena::Counters;
use tsr_compiler::{FileCache, Program, ProgramFile, ProgramOptions};
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{ParseConfigHost, ParsedCommandLine};
use tsr_vfs::{cached::CachedFs, os, tracking::TrackingFs, FileSystem};

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == "composed.ProgramRebuild").then(|| match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error),
    })
}
fn string(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("UTF-8 composed fixture")
}
fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

// Access-only owner for the process-wide production OS implementation. Every
// read delegates directly; it deliberately advertises no snapshot identity.
struct LiveOs;
impl FileSystem for LiveOs {
    fn use_case_sensitive_file_names(&self) -> bool {
        os::fs().use_case_sensitive_file_names()
    }
    fn snapshot_id(&self) -> Option<tsr_vfs::SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<tsr_vfs::FileContent>, tsr_vfs::Error> {
        os::fs().read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<tsr_vfs::FileInfo>, tsr_vfs::Error> {
        os::fs().stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<tsr_vfs::Entries, tsr_vfs::Error> {
        os::fs().entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        os::fs().realpath(path)
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, tsr_vfs::Error> {
        os::fs().file_exists(path)
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, tsr_vfs::Error> {
        os::fs().directory_exists(path)
    }
}
struct Root {
    path: PathBuf,
    name: String,
    real: String,
}
impl Root {
    fn new() -> Result<Self, String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("tsr-composed-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        let name = string(&tsr_tspath::normalize(&os::native::bytes(&path)));
        let real = string(
            os::fs()
                .realpath(name.as_bytes())
                .map_err(|e| e.to_string())?
                .as_bytes(),
        );
        Ok(Self { path, name, real })
    }
    fn join(&self, relative: &str) -> Result<String, String> {
        if relative.is_empty()
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || relative.contains(['\\', ':'])
        {
            return Err(format!("invalid fixture-relative path {relative:?}"));
        }
        Ok(format!("{}/{relative}", self.name))
    }
    fn normalize(&self, name: &[u8]) -> String {
        let name = string(&tsr_tspath::normalize(name));
        // Prefer the physical spelling when /var and /private/var differ.
        if self.real != self.name
            && (name == self.real || name.starts_with(&format!("{}/", self.real)))
        {
            return format!("<realroot>{}", &name[self.real.len()..]);
        }
        if name == self.name || name.starts_with(&format!("{}/", self.name)) {
            return format!("<root>{}", &name[self.name.len()..]);
        }
        name
    }
    fn write(&self, relative: &str, content: &str) -> Result<(), String> {
        let path = os::native::path(self.join(relative)?.as_bytes());
        std::fs::create_dir_all(path.parent().ok_or("fixture parent")?)
            .map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
struct ConfigHost<'a> {
    fs: &'a dyn FileSystem,
    cwd: &'a [u8],
}
impl ParseConfigHost for ConfigHost<'_> {
    fn fs(&self) -> &dyn FileSystem {
        self.fs
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported(
            "composed fixture has no extends",
        ))
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        Err(tsr_vfs::Error::Unsupported(
            "composed fixture has no content mappers",
        ))
    }
}
fn config(root: &Root) -> Result<ParsedCommandLine, String> {
    let name = root.join("tsconfig.json")?;
    let content = os::fs()
        .read_file(name.as_bytes())
        .map_err(|e| e.to_string())?
        .ok_or("missing physical config")?;
    let source = tsr_tsoptions::TsConfigSourceFile::parse(
        js(&name),
        tsr_tspath::to_path(
            name.as_bytes(),
            root.name.as_bytes(),
            os::fs().use_case_sensitive_file_names(),
        ),
        SourceText::from_loaded_bytes(content.text.as_bytes()),
    );
    tsr_tsoptions::parse_json_source_file_config_file_content(
        source,
        &ConfigHost {
            fs: os::fs(),
            cwd: root.name.as_bytes(),
        },
        root.name.as_bytes(),
        &CompilerOptions::default(),
        &tsr_tsoptions::ConfigValue::Null,
        name.as_bytes(),
    )
    .map_err(|e| e.to_string())
}
fn bound(root: &Root, file: &ProgramFile) -> Result<Value, String> {
    let view = file.bound().view();
    let source = view.source_file().map_err(|e| format!("{e:?}"))?;
    let mut symbols = Vec::new();
    if let Some(locals) = view
        .node_binding(file.source())
        .map_err(|e| format!("{e:?}"))?
        .and_then(|binding| binding.locals)
    {
        for (name, symbol) in view
            .result()
            .tables()
            .get(locals)
            .map_err(|e| format!("{e:?}"))?
        {
            let symbol = view
                .symbol(symbol.ok_or("null local symbol")?)
                .map_err(|e| format!("{e:?}"))?;
            symbols.push(json!([
                string(name),
                string(symbol.name_bytes()),
                symbol.flags()
            ]));
        }
    }
    symbols.sort_by_key(Value::to_string);
    Ok(
        json!({"file": root.normalize(source.file_name()), "text": string(source.text().as_bytes()), "symbols": symbols}),
    )
}
fn build(
    root: &Root,
    request: &Value,
    cache: &mut FileCache,
) -> Result<(Program, Arc<CachedFs>, Value), String> {
    let parsed = config(root)?;
    let cached = Arc::new(CachedFs::new(Arc::new(LiveOs)));
    let tracking = Arc::new(TrackingFs::new(cached.clone()));
    let mut wildcard = Vec::new();
    if let Some(dirs) = parsed.wildcard_directories() {
        for (dir, _) in dirs {
            tracking.seen_files.insert(dir.clone());
            wildcard.push(root.normalize(dir.as_bytes()));
        }
    }
    wildcard.sort();
    tracking.seen_files.insert(js(&root.join("tsconfig.json")?));
    let program = Program::load_live(
        ProgramOptions {
            config: parsed,
            host: tracking.clone(),
            current_directory: js(&root.name),
            default_library_path: js(&root.join("lib")?),
            skip_module_resolution: false,
        },
        cache,
        &Counters::new(),
    )
    .map_err(|e| e.to_string())?;
    let mut files = program
        .files()
        .iter()
        .map(|file| bound(root, file))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by_key(|file| file["file"].as_str().unwrap().to_owned());
    let diagnostics = program.syntactic_diagnostics(None).map_err(|e| e.to_string())?.iter().map(|d| {
        let name = d.file.and_then(|id| program.files().iter().find(|f| f.source() == id)).map(|f| root.normalize(f.bound().view().source_file().unwrap().file_name())).unwrap_or_default();
        json!({"file": name, "pos": d.loc.pos(), "end": d.loc.end(), "code": d.code, "category": d.category, "key": string(d.message_key.as_bytes()), "args": d.message_args.iter().map(|s| string(s.as_bytes())).collect::<Vec<_>>()})
    }).collect::<Vec<_>>();
    let metadata = array(request, "metadata_paths")?
        .iter()
        .map(|path| {
            let name = root.join(path.as_str().ok_or("metadata path must be string")?)?;
            Ok(json!([
                root.normalize(name.as_bytes()),
                cached
                    .file_exists(name.as_bytes())
                    .map_err(|e| e.to_string())?
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut seen = tracking
        .seen_files
        .to_vec()
        .iter()
        .map(|path| root.normalize(path.as_bytes()))
        .collect::<Vec<_>>();
    seen.sort();
    cached.disable_and_clear_cache();
    Ok((
        program,
        cached,
        json!({"files": files, "syntactic": diagnostics, "seen": seen, "wildcard_directories": wildcard, "metadata": metadata}),
    ))
}
fn run(request: &Value) -> Result<Value, String> {
    let root = Root::new()?;
    root.write("tsconfig.json", text(request, "config")?)?;
    for (path, content) in request["initial_files"]
        .as_object()
        .ok_or("initial_files must be object")?
    {
        root.write(path, content.as_str().ok_or("file text must be string")?)?;
    }
    let mut cache = FileCache::new();
    let (first, first_cache, first_row) = build(&root, request, &mut cache)?;
    let retained_name = root.join(text(request, "retained_file")?)?;
    let unchanged_name = root.join(text(request, "unchanged_file")?)?;
    let retained_path = tsr_tspath::to_path(
        retained_name.as_bytes(),
        root.name.as_bytes(),
        os::fs().use_case_sensitive_file_names(),
    );
    let unchanged_path = tsr_tspath::to_path(
        unchanged_name.as_bytes(),
        root.name.as_bytes(),
        os::fs().use_case_sensitive_file_names(),
    );
    let retained = first
        .files()
        .iter()
        .find(|file| {
            file.bound().view().source_file().unwrap().file_name() == retained_name.as_bytes()
        })
        .cloned()
        .ok_or("missing retained file")?;
    let unchanged = first
        .file(unchanged_path.as_bytes())
        .ok_or("missing unchanged file")?
        .source();
    for mutation in array(request, "mutations")? {
        let path = text(mutation, "path")?;
        match text(mutation, "op")? {
            "write" => root.write(path, text(mutation, "text")?)?,
            "remove" => std::fs::remove_file(os::native::path(root.join(path)?.as_bytes()))
                .map_err(|e| e.to_string())?,
            other => return Err(format!("unknown composed mutation {other}")),
        }
    }
    let after_clear = array(request, "metadata_paths")?
        .iter()
        .map(|path| {
            let path = root.join(path.as_str().ok_or("metadata path must be string")?)?;
            Ok(json!([
                root.normalize(path.as_bytes()),
                first_cache
                    .file_exists(path.as_bytes())
                    .map_err(|e| e.to_string())?
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (second, _, second_row) = build(&root, request, &mut cache)?;
    let reused = second
        .file(unchanged_path.as_bytes())
        .ok_or("missing rebuilt unchanged file")?
        .source()
        == unchanged;
    let replaced = second
        .file(retained_path.as_bytes())
        .ok_or("missing rebuilt edited file")?
        .source()
        != retained.source();
    drop(first);
    drop(second);
    cache.prune();
    Ok(
        json!({"ordered": [first_row, second_row], "after_disable_and_clear": after_clear, "unchanged_reused": reused, "edited_replaced": replaced, "retained_after_programs_dropped": bound(&root, &retained)?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        let document: Value = serde_json::from_str(include_str!(
            "../../../../data/phase1/requests/filesystem-composed.json"
        ))
        .unwrap();
        document["requests"][0].clone()
    }

    #[test]
    fn immutable_entrypoint_still_rejects_live_os_host() {
        let root = Root::new().unwrap();
        let options = ProgramOptions {
            config: ParsedCommandLine::new(CompilerOptions::default(), Vec::new()),
            host: Arc::new(LiveOs),
            current_directory: js(&root.name),
            default_library_path: js(&root.join("lib").unwrap()),
            skip_module_resolution: false,
        };
        assert!(matches!(
            Program::load(options, &mut FileCache::new(), &Counters::new()),
            Err(tsr_compiler::Error::Resolution(
                tsr_module::Error::MutableHost
            ))
        ));
    }

    #[test]
    fn composed_rebuild_observes_mutations_and_retains_bound_source() {
        let observation = run(&request()).unwrap();
        let before = &observation["ordered"][0];
        let after = &observation["ordered"][1];
        let names = |row: &Value| {
            row["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| file["file"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert!(names(before).contains(&"<root>/src/deleted.ts".into()));
        assert!(!names(after).contains(&"<root>/src/deleted.ts".into()));
        assert!(!names(before).contains(&"<root>/src/created.ts".into()));
        assert!(names(after).contains(&"<root>/src/created.ts".into()));
        assert_eq!(before["syntactic"], json!([]));
        assert_eq!(after["syntactic"][0]["code"], 1109);
        assert_eq!(observation["unchanged_reused"], true);
        assert_eq!(observation["edited_replaced"], true);
        let retained = &observation["retained_after_programs_dropped"];
        assert_eq!(retained["text"], request()["initial_files"]["src/main.ts"]);
        assert_eq!(retained["symbols"][0][0], "oldName");
        for name in ["<root>/src", "<root>/src/missing.ts"] {
            assert!(after["seen"].as_array().unwrap().contains(&json!(name)));
        }
        assert_eq!(observation["after_disable_and_clear"], after["metadata"]);
    }
}
