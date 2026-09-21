//! The parse-config host group: `internal/tsoptions/tsoptionstest`.
//!
//! The pinned package is a host *factory*: it turns a `{path -> content}` map
//! plus a case-sensitivity flag into something satisfying
//! `tsoptions.ParseConfigHost`. Rust has the same two pieces --
//! `tsr_vfs::MemoryBuilder` builds the filesystem and
//! `tsr_tsoptions::ParseConfigHost` is the trait the parse consumes -- but no
//! factory joining them, so each caller assembles its own (for instance
//! `tools/s07/config/host.rs:27`).
//!
//! That is a real difference and this group reports it as one rather than
//! papering over it: the two host-construction cases assemble the pieces here
//! and compare what the built host exposes, because the pieces exist; the
//! one-shot `GetParsedCommandLine` case reports the gap, because the pinned
//! helper's whole job -- derive the config file name, build the source file,
//! parse -- has no Rust counterpart to compare against.

use crate::api::{subject, Outcome};
use serde_json::{json, Map, Value};
use tsr_vfs::{FileSystem, MemoryBuilder};

const GET_PARSED_COMMAND_LINE: (&str, &str, &str) = (
    "tsc/internal/tsoptions/tsoptionstest/parsedcommandline.go:GetParsedCommandLine, which \
     combines the current directory with \"tsconfig.json\", builds a TsConfigSourceFile through \
     the pinned NewTsconfigSourceFileFromFilePath and returns \
     ParseJsonSourceFileConfigFileContent over it",
    "pub fn parsed_command_line_from_map(json_text: &[u8], files: &BTreeMap<Vec<u8>, Vec<u8>>, \
     current_directory: &[u8], case_sensitive: bool) -> ParsedCommandLine -- the one-shot helper; \
     its three steps all exist separately in tsr_tsoptions, the composition does not",
    "no Rust home: crates/tsr_tsoptions has the parse (config_parse.rs:691) and the source-file \
     constructor, and crates/tsr_vfs has MemoryBuilder, but nothing joins them the way the pinned \
     tsoptionstest package does; every caller assembles its own host \
     (for example tools/s07/config/host.rs:27)",
);

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
    let mut builder = MemoryBuilder::new(&current, case_sensitive);
    for (path, content) in files_of(request, "files") {
        builder.insert_physical(&path, content);
    }
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
            builder.insert_symlink(&normalized_link, &normalized_target);
            links.push(json!({
                "declared_link": text(&link),
                "declared_target": text(&target),
                "normalized_link": text(&normalized_link),
                "normalized_target": text(&normalized_target),
            }));
        }
    }
    let snapshot = builder.finish();
    let mut rows = Vec::new();
    for path in request
        .get("probe")
        .and_then(Value::as_array)
        .map_or(&[][..], |items| items.as_slice())
    {
        let path = path.as_str().unwrap_or_default().as_bytes();
        // The pinned filesystem refuses a non-absolute path by panicking, and
        // the probe records that as `refused`. `tsr_vfs` has no such refusal:
        // it answers a relative path like any other. Reporting the Rust answer
        // here, rather than manufacturing a matching refusal, is what makes the
        // difference visible instead of hidden.
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
            let (authority, signature, home) = GET_PARSED_COMMAND_LINE;
            Outcome::missing(
                "tsc/internal/tsoptions/tsoptionstest/parsedcommandline.go:GetParsedCommandLine",
                authority,
                signature,
                home,
            )
        }
        other => Outcome::Failed(format!("unknown parse-config host action {other:?}")),
    })
}
