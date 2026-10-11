use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub version: u8,
    pub command: String,
    pub id: String,
    #[serde(default, deserialize_with = "some_nonnull")]
    pub input: Option<Input>,
    #[serde(default, deserialize_with = "some_nonnull")]
    pub index: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub current_directory: Vec<u8>,
    pub case_sensitive: bool,
    pub entries: Vec<Entry>,
    pub configuration: Configuration,
    pub operation: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub config_path: Vec<u8>,
    pub arguments: Vec<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Entry {
    Directory { path: Vec<u8> },
    PhysicalFile { path: Vec<u8>, bytes: Vec<u8> },
    Symlink { path: Vec<u8>, target: Vec<u8> },
}

#[derive(Serialize)]
pub struct Response<T> {
    pub version: u8,
    pub id: String,
    pub kind: &'static str,
    pub value: T,
}

#[derive(Default, Serialize)]
pub struct Observation {
    pub outcome: &'static str,
    pub roots: Vec<Vec<u8>>,
    pub options_json: Vec<u8>,
    pub sources: Vec<Source>,
    pub diagnostics: Vec<Diagnostic>,
    pub checkers: String,
    pub types: String,
    pub nodes: String,
    pub identifiers: String,
    pub writes: Vec<Write>,
    pub emitted_files: Vec<Vec<u8>>,
    pub emit_skipped: Option<bool>,
    pub emit_diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Serialize)]
pub struct Source {
    pub name: Vec<u8>,
    pub path: Vec<u8>,
    pub text: Vec<u8>,
    pub default_library: bool,
    pub node_count: String,
    pub identifier_count: String,
}

#[derive(Clone, Serialize)]
pub struct Write {
    pub name: Vec<u8>,
    pub text: Vec<u8>,
}

#[derive(Clone, Serialize)]
pub struct Diagnostic {
    pub file: Option<usize>,
    pub pos: String,
    pub end: String,
    pub code: i32,
    pub category: i32,
    pub message_key: Vec<u8>,
    pub message_text: Vec<u8>,
    pub message_args: Vec<Vec<u8>>,
    pub localized: Vec<u8>,
    pub chain: Vec<Self>,
    pub related: Vec<Self>,
    pub source: Vec<u8>,
    pub unnecessary: bool,
    pub deprecated: bool,
    pub skipped_on_no_emit: bool,
    pub repopulate: Option<Repopulate>,
}

#[derive(Clone, Serialize)]
pub struct Repopulate {
    pub kind: i32,
    pub module_reference: Vec<u8>,
    pub mode: i32,
    pub package_name: Vec<u8>,
}

pub fn decode(bytes: &[u8]) -> Result<Command, String> {
    let command: Command = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !matches!(command.version, 1 | 2) {
        return Err("unsupported budget protocol version".into());
    }
    if command
        .id
        .parse::<u64>()
        .map(|id| id.to_string())
        .as_deref()
        != Ok(command.id.as_str())
    {
        return Err("invalid operation id".into());
    }
    if command.version == 2 && command.command == "prepare" {
        return Err("version 2 requires streamed preparation".into());
    }
    if command.command != "prepare" && command.input.is_some() {
        return Err("input only valid for prepare".into());
    }
    if matches!(command.command.as_str(), "observe_source" | "observe_write") {
        if command.version != 2 {
            return Err("row observation requires version 2".into());
        }
        crate::stream::decimal(
            command
                .index
                .as_deref()
                .ok_or("row observation requires index")?,
        )?;
    } else if command.index.is_some() {
        return Err("index only valid for row observation".into());
    }
    if command.command == "observe_begin" && command.version != 2 {
        return Err("bounded observation requires version 2".into());
    }
    Ok(command)
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ResponseValue {
    Empty,
    Checkpoint(std::collections::BTreeMap<&'static str, String>),
    Observation(Box<Observation>),
    ObservationBegin(Box<ObservationHeader>),
    ObservationSource(Source),
    ObservationWrite(Write),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_commands_reject_ambiguous_or_incomplete_json() {
        for raw in [
            r#"{"version":1,"command":"execute","id":"01"}"#,
            r#"{"version":1,"command":"execute","id":"1","version":1}"#,
            r#"{"version":1,"command":"execute","id":"1","extra":0}"#,
            r#"{"version":1,"command":"prepare","id":"1","input":{"current_directory":[],"entries":[],"configuration":{"config_path":[],"arguments":[]},"operation":"check"}}"#,
            r#"{"version":1,"command":"prepare","id":"1","input":{"current_directory":[],"case_sensitive":true,"entries":null,"configuration":{"config_path":[],"arguments":[]},"operation":"check"}}"#,
            r#"{"version":1,"command":"prepare","id":"1","input":{"current_directory":[],"case_sensitive":true,"entries":[{"kind":"physical_file","path":[]}],"configuration":{"config_path":[],"arguments":[]},"operation":"check"}}"#,
        ] {
            assert!(decode(raw.as_bytes()).is_err(), "accepted {raw}");
        }
        assert!(
            decode(br#"{"version":1,"command":"execute","id":"18446744073709551615"}"#).is_ok()
        );
    }
}

#[derive(Deserialize)]
pub struct Envelope<'a> {
    pub version: u8,
    pub command: &'a str,
    pub id: &'a str,
}
pub fn envelope(bytes: &[u8]) -> Result<Envelope<'_>, String> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

fn some_nonnull<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
pub struct ObservationHeader {
    pub outcome: &'static str,
    pub roots: Vec<Vec<u8>>,
    pub options_json: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    pub checkers: String,
    pub types: String,
    pub nodes: String,
    pub identifiers: String,
    pub emitted_files: Vec<Vec<u8>>,
    pub emit_skipped: Option<bool>,
    pub emit_diagnostics: Vec<Diagnostic>,
    pub source_count: String,
    pub write_count: String,
}
impl From<&Observation> for ObservationHeader {
    fn from(value: &Observation) -> Self {
        Self {
            outcome: value.outcome,
            roots: value.roots.clone(),
            options_json: value.options_json.clone(),
            diagnostics: value.diagnostics.clone(),
            checkers: value.checkers.clone(),
            types: value.types.clone(),
            nodes: value.nodes.clone(),
            identifiers: value.identifiers.clone(),
            emitted_files: value.emitted_files.clone(),
            emit_skipped: value.emit_skipped,
            emit_diagnostics: value.emit_diagnostics.clone(),
            source_count: value.sources.len().to_string(),
            write_count: value.writes.len().to_string(),
        }
    }
}
