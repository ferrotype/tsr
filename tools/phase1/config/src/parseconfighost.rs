//! Test-only composition of the pinned tsoptionstest host helpers. Production
//! parsing and filesystem behavior are supplied by their respective crates.

use crate::api::{subject, Outcome};
use serde_json::{json, Map, Value};
use tsr_vfs::{
    vfstest::{self, InputFile},
    FileSystem,
};

fn files_of(request: &Value, key: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    request
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(path, value)| {
                    (
                        path.as_bytes().to_vec(),
                        value.as_str().unwrap_or_default().as_bytes().to_vec(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(value).into_owned())
}

/// Build the filesystem the pinned factory would build, and report what the
/// host exposes for each probed path, in request order.
fn describe(request: &Value, with_symlinks: bool) -> Outcome {
    let current = request
        .get("currentDirectory")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .as_bytes()
        .to_vec();
    let case_sensitive = request
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut files: std::collections::BTreeMap<_, _> = files_of(request, "files")
        .into_iter()
        .map(|(name, content)| (name, InputFile::Text(content)))
        .collect();
    let mut links: Vec<Value> = Vec::new();
    if with_symlinks {
        // The pinned factory normalizes BOTH sides against the current
        // directory before building the entry (vfsparseconfighost.go:52-53),
        // and delegates to the plain factory when the map is empty (:47-49).
        let mut declared = files_of(request, "symlinks");
        declared.sort();
        for (link, target) in declared {
            // `tsr_tspath::absolute` is the production counterpart of the
            // pinned `tspath.GetNormalizedAbsolutePath`. F2a recorded a known
            // divergence between them on a bare root; using the real
            // counterpart is the point, so a case that hits it reports
            // `different` rather than being quietly routed around.
            let normalized_link = tsr_tspath::absolute(&link, &current);
            let normalized_target = tsr_tspath::absolute(&target, &current);
            files.insert(
                normalized_link.clone(),
                InputFile::File(vfstest::symlink(&normalized_target)),
            );
            links.push(json!({
                "declared_link": text(&link),
                "declared_target": text(&target),
                "normalized_link": text(&normalized_link),
                "normalized_target": text(&normalized_target),
            }));
        }
    }
    let storage = vfstest::from_map(&files, case_sensitive).into_vfs();
    let snapshot: &dyn FileSystem = &storage;
    let mut rows = Vec::new();
    for path in request
        .get("probe")
        .and_then(Value::as_array)
        .map_or(&[][..], |items| items.as_slice())
    {
        let path = path.as_str().unwrap_or_default().as_bytes();
        let refused =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| snapshot.file_exists(path)));
        if let Err(payload) = refused {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("");
            if !message.contains("is not absolute") {
                return Outcome::Failed(format!("unexpected fixture path panic: {message}"));
            }
            rows.push(json!({"path":text(path),"refused":true}));
            continue;
        }
        let content = match snapshot.read_file(path) {
            Ok(Some(found)) => text(&found.raw),
            Ok(None) => Value::Null,
            Err(error) => return Outcome::Failed(format!("read_file: {error}")),
        };
        let realpath = match snapshot.realpath(path) {
            Ok(resolved) => text(resolved.as_bytes()),
            Err(error) => return Outcome::Failed(format!("realpath: {error}")),
        };
        let (Ok(file_exists), Ok(directory_exists)) =
            (snapshot.file_exists(path), snapshot.directory_exists(path))
        else {
            return Outcome::Failed("stat failed".into());
        };
        rows.push(json!({
            "path": text(path),
            "refused": false,
            "file_exists": file_exists,
            "directory_exists": directory_exists,
            "content": content,
            "realpath": realpath,
        }));
    }
    let mut observation = Map::new();
    observation.insert("current_directory".into(), text(&current));
    observation.insert(
        "use_case_sensitive_file_names".into(),
        Value::Bool(snapshot.use_case_sensitive_file_names()),
    );
    observation.insert("ordered".into(), Value::Array(rows));
    if with_symlinks {
        observation.insert("symlinks".into(), Value::Array(links));
    }
    Outcome::Observed(Value::Object(observation))
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "parseConfigHost" {
        return None;
    }
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(match action {
        "from_map" => describe(request, false),
        "from_map_with_symlinks" => describe(request, true),
        "get_parsed_command_line" => {
            let current = request["currentDirectory"]
                .as_str()
                .unwrap_or_default()
                .as_bytes();
            let name = tsr_tspath::combine(current, &[b"tsconfig.json"]);
            let host = super::configparse::build_host(request);
            let mut config_request = request.clone();
            config_request["basePath"] = text(current);
            let result = super::configparse::parse_source(
                &config_request,
                &host,
                &name,
                request["jsonText"].as_str().unwrap_or_default().as_bytes(),
                None,
            );
            match result {
                Ok(parsed) => Outcome::Observed(
                    json!({"config_file_name":text(&name),"file_names":parsed.root_file_names.iter().map(|s|text(s.as_bytes())).collect::<Vec<_>>(),"error_codes":parsed.errors.iter().map(|d|d.code).collect::<Vec<_>>(),"has_config_file":parsed.config_file.is_some()}),
                ),
                Err(error) => Outcome::Failed(format!("config parse: {error}")),
            }
        }
        other => Outcome::Failed(format!("unknown parse-config host action {other:?}")),
    })
}
