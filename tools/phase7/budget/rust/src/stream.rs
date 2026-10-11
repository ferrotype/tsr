//! Bounded, untimed snapshot preparation. Only the final immutable filesystem
//! survives preparation; metadata codecs and content staging are dropped.
use crate::{discovery, protocol as p};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tsr_jsstring::JsString;
use tsr_vfs::{FileSystem, MemoryBuilder};

pub const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_METADATA: usize = 1024 * 1024;
const MAX_CHUNK: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    PrepareBegin {
        version: u8,
        id: String,
        input: p::Input,
        expected_entries: String,
        expected_files: String,
        expected_contents: String,
    },
    PrepareMetadata {
        version: u8,
        id: String,
        entries: Vec<discovery::Entry>,
    },
    ContentBegin {
        version: u8,
        id: String,
        path_b64: String,
        raw_size: String,
        sha256: String,
    },
    ContentChunk {
        version: u8,
        id: String,
        offset: String,
        bytes_b64: String,
    },
    ContentEnd {
        version: u8,
        id: String,
    },
    PrepareEnd {
        version: u8,
        id: String,
    },
}
impl Command {
    pub fn identity(&self) -> (u8, &str) {
        match self {
            Self::PrepareBegin { version, id, .. }
            | Self::PrepareMetadata { version, id, .. }
            | Self::ContentBegin { version, id, .. }
            | Self::ContentChunk { version, id, .. }
            | Self::ContentEnd { version, id }
            | Self::PrepareEnd { version, id } => (*version, id),
        }
    }
}
pub fn decode(bytes: &[u8]) -> Result<Command, String> {
    if bytes.len() > MAX_FRAME {
        return Err("v2 frame exceeds 2 MiB".into());
    }
    let command: Command = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let (version, id) = command.identity();
    if version != 2 {
        return Err("stream preparation requires version 2".into());
    }
    decimal(id)?;
    if matches!(&command, Command::PrepareMetadata { .. }) {
        #[derive(Deserialize)]
        struct Metadata<'a> {
            #[serde(borrow)]
            entries: &'a serde_json::value::RawValue,
        }
        let metadata: Metadata<'_> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if metadata.entries.get().len() > MAX_METADATA {
            return Err("encoded metadata array exceeds 1 MiB".into());
        }
    }
    Ok(command)
}
pub fn decimal(text: &str) -> Result<u64, String> {
    let value = text
        .parse::<u64>()
        .map_err(|_| "invalid canonical u64 decimal")?;
    if value.to_string() != text {
        return Err("invalid canonical u64 decimal".into());
    }
    Ok(value)
}
struct FileSpec {
    path: Vec<u8>,
    size: u64,
    provided: bool,
}
struct Pending {
    key: JsString,
    bytes: Vec<u8>,
    hash: Sha256,
    expected: String,
}
pub struct Preparation {
    pub id: String,
    input: p::Input,
    builder: MemoryBuilder,
    expected_entries: u64,
    expected_files: u64,
    expected_contents: u64,
    entries: u64,
    contents: u64,
    paths: HashSet<JsString>,
    files: HashMap<JsString, FileSpec>,
    pending: Option<Pending>,
    content_phase: bool,
}
impl Preparation {
    pub fn begin(
        id: String,
        input: p::Input,
        entries: &str,
        files: &str,
        contents: &str,
    ) -> Result<Self, String> {
        if !input.entries.is_empty() {
            return Err("stream input entries must be empty".into());
        }
        crate::engine::validate_input(&input)?;
        let expected_entries = decimal(entries)?;
        let expected_files = decimal(files)?;
        let expected_contents = decimal(contents)?;
        if expected_files > expected_entries || expected_contents > expected_files {
            return Err("invalid snapshot counts".into());
        }
        let mut builder = MemoryBuilder::new(&input.current_directory, input.case_sensitive);
        builder.insert_directory(&input.current_directory);
        Ok(Self {
            id,
            input,
            builder,
            expected_entries,
            expected_files,
            expected_contents,
            entries: 0,
            contents: 0,
            paths: HashSet::new(),
            files: HashMap::new(),
            pending: None,
            content_phase: false,
        })
    }
    fn key(&self, path: &[u8]) -> JsString {
        tsr_tspath::to_path(path, b"", self.input.case_sensitive)
    }
    pub fn metadata(&mut self, entries: Vec<discovery::Entry>) -> Result<(), String> {
        if self.content_phase {
            return Err("metadata cannot follow content".into());
        }
        for entry in entries {
            let encoded = match &entry {
                discovery::Entry::Directory { path_b64 }
                | discovery::Entry::PhysicalFile { path_b64, .. }
                | discovery::Entry::Symlink { path_b64, .. } => path_b64,
            };
            let path = discovery::decode(encoded)?;
            crate::engine::validate_path(&path)?;
            let key = self.key(&path);
            if !self.paths.insert(key.clone()) {
                return Err("duplicate canonical metadata path".into());
            }
            self.entries = self
                .entries
                .checked_add(1)
                .ok_or("metadata count overflow")?;
            if self.entries > self.expected_entries {
                return Err("too many metadata entries".into());
            }
            match entry {
                discovery::Entry::Directory { .. } => self.builder.insert_directory(&path),
                discovery::Entry::PhysicalFile { raw_size, .. } => {
                    let size = decimal(&raw_size)?;
                    self.builder.insert_unavailable_file(&path, size);
                    self.files.insert(
                        key,
                        FileSpec {
                            path,
                            size,
                            provided: false,
                        },
                    );
                    if self.files.len() as u64 > self.expected_files {
                        return Err("too many physical files".into());
                    }
                }
                discovery::Entry::Symlink { target_b64, .. } => self
                    .builder
                    .insert_symlink(&path, &discovery::decode(&target_b64)?),
            }
        }
        Ok(())
    }
    fn complete_metadata(&self) -> Result<(), String> {
        if self.entries != self.expected_entries || self.files.len() as u64 != self.expected_files {
            return Err("snapshot metadata counts mismatch".into());
        }
        Ok(())
    }
    pub fn content_begin(
        &mut self,
        path_b64: &str,
        size: &str,
        hash: String,
    ) -> Result<(), String> {
        self.complete_metadata()?;
        if self.pending.is_some() {
            return Err("content already active".into());
        }
        if self.contents >= self.expected_contents {
            return Err("too many content paths".into());
        }
        let path = discovery::decode(path_b64)?;
        crate::engine::validate_path(&path)?;
        let key = self.key(&path);
        let spec = self
            .files
            .get(&key)
            .ok_or("content requires physical metadata file")?;
        if spec.provided {
            return Err("duplicate canonical content path".into());
        }
        if decimal(size)? != spec.size {
            return Err("content size differs from metadata".into());
        }
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("invalid sha256".into());
        }
        self.content_phase = true;
        self.pending = Some(Pending {
            key,
            bytes: Vec::new(),
            hash: Sha256::new(),
            expected: hash,
        });
        Ok(())
    }
    pub fn chunk(&mut self, offset: &str, encoded: &str) -> Result<(), String> {
        let pending = self
            .pending
            .as_mut()
            .ok_or("content chunk requires active file")?;
        if decimal(offset)? != pending.bytes.len() as u64 {
            return Err("noncontiguous content offset".into());
        }
        let bytes = discovery::decode(encoded)?;
        if bytes.len() > MAX_CHUNK {
            return Err("content chunk exceeds 1 MiB".into());
        }
        let total = (pending.bytes.len() as u64)
            .checked_add(bytes.len() as u64)
            .ok_or("content length overflow")?;
        if total > self.files[&pending.key].size {
            return Err("content exceeds declared size".into());
        }
        pending.hash.update(&bytes);
        pending.bytes.extend_from_slice(&bytes);
        Ok(())
    }
    pub fn content_end(&mut self) -> Result<(), String> {
        let pending = self
            .pending
            .take()
            .ok_or("content end requires active file")?;
        let spec = self
            .files
            .get_mut(&pending.key)
            .expect("pending metadata file");
        if pending.bytes.len() as u64 != spec.size
            || format!("{:x}", pending.hash.finalize()) != pending.expected
        {
            return Err("content size or sha256 mismatch".into());
        }
        self.builder.insert_physical(&spec.path, pending.bytes);
        spec.provided = true;
        self.contents += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<(p::Input, Arc<dyn FileSystem>), String> {
        self.complete_metadata()?;
        if self.pending.is_some() || self.contents != self.expected_contents {
            return Err("snapshot content counts mismatch".into());
        }
        let fs = Arc::new(tsr_embed::BundledFs::new(Arc::new(self.builder.finish())));
        Ok((self.input, fs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::{json, Value};
    pub(super) fn send(
        engine: &mut crate::engine::Engine,
        command: &str,
        fields: Value,
    ) -> Result<p::Response<p::ResponseValue>, String> {
        let mut frame = fields.as_object().unwrap().clone();
        frame.insert("version".into(), json!(2));
        frame.insert("command".into(), json!(command));
        frame.insert("id".into(), json!("1"));
        engine.execute_command(&serde_json::to_vec(&frame).unwrap())
    }
    pub(super) fn begin(
        engine: &mut crate::engine::Engine,
        entries: u64,
        files: u64,
        contents: u64,
    ) {
        assert_eq!(send(engine,"prepare_begin",json!({
            "input":{"current_directory":b"/project","case_sensitive":false,"entries":[],"configuration":{"config_path":b"/project/tsconfig.json","arguments":[]},"operation":"check_emit"},
            "expected_entries":entries.to_string(),"expected_files":files.to_string(),"expected_contents":contents.to_string()
        })).unwrap().kind,"preparing");
    }
    fn metadata(path: &[u8], size: usize) -> Value {
        json!({"kind":"physical_file","path_b64":STANDARD.encode(path),"raw_size":size.to_string()})
    }
    fn start(
        engine: &mut crate::engine::Engine,
        path: &[u8],
        bytes: &[u8],
    ) -> Result<p::Response<p::ResponseValue>, String> {
        send(
            engine,
            "content_begin",
            json!({"path_b64":STANDARD.encode(path),"raw_size":bytes.len().to_string(),"sha256":format!("{:x}",Sha256::digest(bytes))}),
        )
    }
    fn provide(engine: &mut crate::engine::Engine, path: &[u8], bytes: &[u8]) {
        start(engine, path, bytes).unwrap();
        let split = bytes.len() / 2;
        for (offset, part) in [(0, &bytes[..split]), (split, &bytes[split..])] {
            send(
                engine,
                "content_chunk",
                json!({"offset":offset.to_string(),"bytes_b64":STANDARD.encode(part)}),
            )
            .unwrap();
        }
        send(engine, "content_end", json!({})).unwrap();
    }
    #[test]
    fn streamed_snapshot_retains_real_outputs_and_unread_metadata() {
        let config = br#"{"compilerOptions":{"noLib":true},"files":["alias.ts"]}"#;
        let source = b"\xef\xbb\xbfexport const value = 1;";
        let mut engine = crate::engine::Engine::default();
        begin(&mut engine, 6, 3, 2);
        send(&mut engine,"prepare_metadata",json!({"entries":[
            {"kind":"directory","path_b64":STANDARD.encode(b"/project")},
            {"kind":"directory","path_b64":STANDARD.encode(b"/project/empty")},
            metadata(b"/project/tsconfig.json",config.len()),metadata(b"/project/Main.ts",source.len()),metadata(b"/project/unread.bin",u32::MAX as usize),
            {"kind":"symlink","path_b64":STANDARD.encode(b"/project/alias.ts"),"target_b64":STANDARD.encode(b"MAIN.ts")}
        ]})).unwrap();
        provide(&mut engine, b"/project/tsconfig.json", config);
        provide(&mut engine, b"/project/MAIN.ts", source);
        assert_eq!(
            send(&mut engine, "prepare_end", json!({})).unwrap().kind,
            "prepared"
        );
        send(&mut engine, "execute", json!({})).unwrap();
        let checkpoint = send(&mut engine, "checkpoint", json!({})).unwrap();
        let p::ResponseValue::Checkpoint(metrics) = checkpoint.value else {
            panic!("checkpoint metrics");
        };
        assert!(
            metrics["process_peak_rss_before_observation_bytes"]
                .parse::<u64>()
                .unwrap()
                > 0
        );
        let p::ResponseValue::Observation(observation) =
            send(&mut engine, "observe", json!({})).unwrap().value
        else {
            panic!("expected observation")
        };
        assert_eq!(observation.checkers, "1");
        assert_eq!(observation.sources.len(), 1);
        assert_eq!(observation.sources[0].text, b"export const value = 1;");
        assert_eq!(observation.writes.len(), 1);
        let original = serde_json::to_value(&observation).unwrap();
        // Row commands may initialize the lazy cache directly after checkpoint.
        let first = send(&mut engine, "observe_source", json!({"index":"0"})).unwrap();
        assert_eq!(first.kind, "observation_source");
        let begin_response = send(&mut engine, "observe_begin", json!({})).unwrap();
        assert_eq!(begin_response.kind, "observation_begin");
        let mut header = serde_json::to_value(&begin_response.value).unwrap();
        assert!(header.get("sources").is_none() && header.get("writes").is_none());
        assert_eq!(header["source_count"], "1");
        assert_eq!(header["write_count"], "1");
        let repeated = send(&mut engine, "observe_begin", json!({})).unwrap();
        assert_eq!(serde_json::to_value(&repeated.value).unwrap(), header);
        let write = send(&mut engine, "observe_write", json!({"index":"0"})).unwrap();
        assert_eq!(write.kind, "observation_write");
        let object = header.as_object_mut().unwrap();
        object.remove("source_count");
        object.remove("write_count");
        object.insert(
            "sources".into(),
            json!([serde_json::to_value(&first.value).unwrap()]),
        );
        object.insert(
            "writes".into(),
            json!([serde_json::to_value(&write.value).unwrap()]),
        );
        assert_eq!(header, original); // Every owned field and byte survives bounded reconstruction.
        send(&mut engine, "release", json!({})).unwrap();
        begin(&mut engine, 0, 0, 0);
    }
    #[test]
    fn omitted_known_content_is_terminal_when_read() {
        let mut engine = crate::engine::Engine::default();
        begin(&mut engine, 1, 1, 0);
        send(
            &mut engine,
            "prepare_metadata",
            json!({"entries":[metadata(b"/project/tsconfig.json",0)]}),
        )
        .unwrap();
        send(&mut engine, "prepare_end", json!({})).unwrap();
        assert!(send(&mut engine, "execute", json!({})).is_err());
        assert!(send(&mut engine, "checkpoint", json!({}))
            .err()
            .unwrap()
            .contains("terminal"));
    }
    #[test]
    fn preparation_rejects_integrity_order_identity_and_count_failures() {
        for failure in [
            "duplicate",
            "offset",
            "hash",
            "counts",
            "ordering",
            "version",
            "base64",
            "unknown",
        ] {
            let mut engine = crate::engine::Engine::default();
            begin(&mut engine, 1, 1, 1);
            send(
                &mut engine,
                "prepare_metadata",
                json!({"entries":[metadata(b"/project/tsconfig.json",1)]}),
            )
            .unwrap();
            let result = match failure {
                "duplicate" => send(
                    &mut engine,
                    "prepare_metadata",
                    json!({"entries":[metadata(b"/project/TSCONFIG.json",1)]}),
                ),
                "counts" => send(&mut engine, "prepare_end", json!({})),
                "version" => {
                    engine.execute_command(br#"{"version":1,"command":"execute","id":"1"}"#)
                }
                "unknown" => start(&mut engine, b"/project/missing", b"x"),
                _ => {
                    start(&mut engine, b"/project/tsconfig.json", b"x").unwrap();
                    match failure {
                        "offset" => send(
                            &mut engine,
                            "content_chunk",
                            json!({"offset":"1","bytes_b64":"eA=="}),
                        ),
                        "hash" => {
                            send(
                                &mut engine,
                                "content_chunk",
                                json!({"offset":"0","bytes_b64":"eQ=="}),
                            )
                            .unwrap();
                            send(&mut engine, "content_end", json!({}))
                        }
                        "ordering" => send(&mut engine, "prepare_metadata", json!({"entries":[]})),
                        "base64" => send(
                            &mut engine,
                            "content_chunk",
                            json!({"offset":"0","bytes_b64":"eA"}),
                        ),
                        _ => unreachable!(),
                    }
                }
            };
            assert!(result.is_err(), "accepted {failure}");
            assert!(send(&mut engine, "prepare_end", json!({}))
                .err()
                .unwrap()
                .contains("terminal"));
        }
    }
    #[test]
    fn zero_length_content_and_strict_frame_limits() {
        let mut engine = crate::engine::Engine::default();
        begin(&mut engine, 1, 1, 1);
        send(
            &mut engine,
            "prepare_metadata",
            json!({"entries":[metadata(b"/project/empty",0)]}),
        )
        .unwrap();
        start(&mut engine, b"/project/empty", b"").unwrap();
        send(&mut engine, "content_end", json!({})).unwrap();
        send(&mut engine, "prepare_end", json!({})).unwrap();
        for raw in [
            r#"{"version":2,"command":"content_end","id":"1","offset":"0"}"#,
            r#"{"version":2,"command":"prepare_metadata","id":"1","entries":null}"#,
            r#"{"version":2,"command":"prepare_end","id":"01"}"#,
        ] {
            assert!(decode(raw.as_bytes()).is_err());
        }
        let mut frame =
            br#"{"version":2,"command":"prepare_metadata","id":"1","entries":[]}"#.to_vec();
        frame.resize(MAX_METADATA + 1, b' ');
        assert!(decode(&frame).is_ok()); // Whitespace outside the array uses only the frame bound.
        let mut oversized_array =
            br#"{"version":2,"command":"prepare_metadata","id":"1","entries":["#.to_vec();
        oversized_array.resize(MAX_METADATA + 64, b' ');
        oversized_array.extend_from_slice(b"]}");
        assert!(decode(&oversized_array).err().unwrap().contains("1 MiB"));
        frame.resize(MAX_FRAME + 1, b' ');
        assert!(decode(&frame).err().unwrap().contains("2 MiB"));
    }
}

#[cfg(test)]
mod observation_failures {
    use crate::{engine::Engine, protocol};
    #[test]
    fn row_requests_reject_invalid_fields_versions_and_ranges() {
        for raw in [
            r#"{"version":1,"command":"observe_begin","id":"1"}"#,
            r#"{"version":1,"command":"observe_source","id":"1","index":"0"}"#,
            r#"{"version":2,"command":"observe_source","id":"1"}"#,
            r#"{"version":2,"command":"observe_source","id":"1","index":null}"#,
            r#"{"version":2,"command":"observe_source","id":"1","index":"01"}"#,
            r#"{"version":2,"command":"observe_write","id":"1","index":"-1"}"#,
            r#"{"version":2,"command":"observe_begin","id":"1","index":"0"}"#,
            r#"{"version":2,"command":"execute","id":"1","input":null}"#,
        ] {
            assert!(protocol::decode(raw.as_bytes()).is_err(), "accepted {raw}");
        }
        for before_checkpoint in [false, true] {
            let mut engine = Engine::default();
            super::tests::begin(&mut engine, 0, 0, 0);
            super::tests::send(&mut engine, "prepare_end", serde_json::json!({})).unwrap();
            super::tests::send(&mut engine, "execute", serde_json::json!({})).unwrap();
            if !before_checkpoint {
                super::tests::send(&mut engine, "checkpoint", serde_json::json!({})).unwrap();
            }
            let error = super::tests::send(
                &mut engine,
                "observe_source",
                serde_json::json!({"index":"0"}),
            )
            .err()
            .unwrap();
            assert!(error.contains(if before_checkpoint {
                "checkpoint"
            } else {
                "range"
            }));
            assert!(
                super::tests::send(&mut engine, "release", serde_json::json!({}))
                    .err()
                    .unwrap()
                    .contains("terminal")
            );
        }
    }
}
