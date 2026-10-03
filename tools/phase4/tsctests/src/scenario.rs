//! The recorded scenarios (`data/phase4/scenarios.json.gz`, the format of
//! `scripts/phase4_scenarios.py`, version 1) and their replay as runner
//! inputs: each recorded edit becomes a closure that performs the recorded
//! operations on the fake system and requires the fake clock to advance by
//! the recorded number of readings, as the pin's replay
//! (`tools/phase4/recorder/replay_test.go`) does.
//!
//! The reader is strict: every documented key must be present with its type,
//! and no other key may appear. Each scenario's digest is recomputed from its
//! canonical JSON and must equal the one the file's provenance records.
use crate::runner::{TscEdit, TscInput};
use crate::sys::{new_test_sys, FileMap, TestSys, TSC_DEFAULT_LIB_CONTENT};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use tsr_jsstring::JsString;
use tsr_vfs::vfstest::{self, InputFile};

/// The format name and version this reader accepts.
pub const FORMAT: &str = "phase4-scenarios";
pub const VERSION: u64 = 1;

/// The four families, in the order the binary reports them.
pub const FAMILIES: [&str; 4] = ["tsc", "tsbuild", "tscWatch", "tsbuildWatch"];

/// A recorded operation of an edit's closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationKind {
    /// `sys.writeFileNoError(path, bytes)`.
    Write(Vec<u8>),
    /// `sys.removeNoError(path)`.
    Remove,
    /// `sys.FS().Chtimes(path, time.Time{}, sys.Now())`.
    Chtimes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub kind: OperationKind,
    pub path: Vec<u8>,
    /// The fake clock's readings the operation made.
    pub clock_readings: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEdit {
    pub caption: String,
    pub command_line_args: Option<Vec<JsString>>,
    pub expected_diff: String,
    /// Whether the pin's closure exists.
    pub edit: bool,
    pub operations: Vec<Operation>,
    /// The clean-build shadow's operations where they differ.
    pub shadow_operations: Option<Vec<Operation>>,
}

/// One recorded scenario: one `tscInput.run` of the pin.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// `<family>/<scenario>/<file>`, the reference path.
    pub id: String,
    pub family: String,
    pub scenario: String,
    pub file: String,
    pub sub_scenario: String,
    /// `None` is Go's nil slice; the runner treats it as no arguments.
    pub command_line_args: Option<Vec<JsString>>,
    pub cwd: Vec<u8>,
    pub env: BTreeMap<String, String>,
    pub output_is_tty: bool,
    pub ignore_case: bool,
    pub windows_style_root: Vec<u8>,
    pub files: FileMap,
    pub library_path: Vec<u8>,
    pub library_files: Vec<Vec<u8>>,
    pub initial_clock_readings: u64,
    pub edits: Vec<RecordedEdit>,
    /// The SHA-256 of the scenario's canonical JSON, checked against the
    /// file's provenance.
    pub sha256: String,
}

/// The whole recorded inventory.
#[derive(Clone, Debug)]
pub struct Inventory {
    pub library_text: Vec<u8>,
    pub orphan_references: Vec<String>,
    pub pin: String,
    pub scenarios: Vec<Scenario>,
}

type Fields<'a> = &'a Map<String, Value>;

fn object<'a>(
    value: &'a Value,
    what: &str,
    keys: &[&str],
    optional: &[&str],
) -> Result<Fields<'a>, String> {
    let Value::Object(fields) = value else {
        return Err(format!("{what} is not an object"));
    };
    for key in keys {
        if !fields.contains_key(*key) {
            return Err(format!("{what} lacks {key}"));
        }
    }
    if let Some(extra) = fields
        .keys()
        .find(|key| !keys.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(format!("{what} has the unknown key {extra}"));
    }
    Ok(fields)
}

fn string(fields: Fields<'_>, key: &str, what: &str) -> Result<String, String> {
    match &fields[key] {
        Value::String(text) => Ok(text.clone()),
        _ => Err(format!("{what}.{key} is not a string")),
    }
}

fn boolean(fields: Fields<'_>, key: &str, what: &str) -> Result<bool, String> {
    match &fields[key] {
        Value::Bool(value) => Ok(*value),
        _ => Err(format!("{what}.{key} is not a boolean")),
    }
}

fn count(fields: Fields<'_>, key: &str, what: &str) -> Result<u64, String> {
    fields[key]
        .as_u64()
        .ok_or_else(|| format!("{what}.{key} is not a non-negative integer"))
}

fn strings(value: &Value, what: &str) -> Result<Vec<String>, String> {
    let Value::Array(items) = value else {
        return Err(format!("{what} is not a list"));
    };
    items
        .iter()
        .map(|item| match item {
            Value::String(text) => Ok(text.clone()),
            _ => Err(format!("{what} holds a non-string")),
        })
        .collect()
}

/// `null | [string]`, the distinction kept.
fn arguments(value: &Value, what: &str) -> Result<Option<Vec<JsString>>, String> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(
        strings(value, what)?
            .into_iter()
            .map(|arg| JsString::from_bytes(arg.into_bytes()))
            .collect(),
    ))
}

fn hex_bytes(text: &str, what: &str) -> Result<Vec<u8>, String> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2)
        || !digits
            .iter()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!("{what} is not lowercase hex"));
    }
    Ok(digits
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b - b'a' + 10
                }
            };
            digit(pair[0]) << 4 | digit(pair[1])
        })
        .collect())
}

/// `{"text"}` or `{"text_hex"}`, exactly one; `text_hex` only for bytes that
/// are not UTF-8.
fn file_bytes(fields: Fields<'_>, what: &str) -> Result<Vec<u8>, String> {
    match (fields.get("text"), fields.get("text_hex")) {
        (Some(Value::String(text)), None) => Ok(text.clone().into_bytes()),
        (None, Some(Value::String(hex))) => {
            let bytes = hex_bytes(hex, what)?;
            if std::str::from_utf8(&bytes).is_ok() {
                return Err(format!("{what} stores UTF-8 bytes as text_hex"));
            }
            Ok(bytes)
        }
        _ => Err(format!("{what} needs exactly one of text and text_hex")),
    }
}

fn operation(value: &Value, what: &str) -> Result<Operation, String> {
    let Value::Object(fields) = value else {
        return Err(format!("{what} is not an object"));
    };
    let op = match fields.get("op") {
        Some(Value::String(op)) => op.as_str(),
        _ => return Err(format!("{what}.op is not a string")),
    };
    let (keys, optional): (&[&str], &[&str]) = match op {
        "write" => (
            &["op", "path", "clock_readings"],
            &["text", "text_hex", "via"],
        ),
        "remove" => (&["op", "path", "clock_readings"], &["via"]),
        "chtimes" => (&["op", "path", "clock_readings"], &[]),
        other => return Err(format!("{what} has the unknown operation {other}")),
    };
    let fields = object(value, what, keys, optional)?;
    if let Some(via) = fields.get("via") {
        if !via.is_string() {
            return Err(format!("{what}.via is not a string"));
        }
    }
    let kind = match op {
        "write" => OperationKind::Write(file_bytes(fields, what)?),
        "remove" => OperationKind::Remove,
        _ => OperationKind::Chtimes,
    };
    let clock_readings = count(fields, "clock_readings", what)?;
    let expected_readings = match kind {
        OperationKind::Write(_) => clock_readings >= 1,
        OperationKind::Remove => clock_readings == 0,
        OperationKind::Chtimes => clock_readings == 1,
    };
    if !expected_readings {
        return Err(format!(
            "{what} records {clock_readings} clock readings for a {op}"
        ));
    }
    Ok(Operation {
        kind,
        path: string(fields, "path", what)?.into_bytes(),
        clock_readings,
    })
}

fn operations(value: &Value, what: &str) -> Result<Vec<Operation>, String> {
    let Value::Array(items) = value else {
        return Err(format!("{what} is not a list"));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| operation(item, &format!("{what}[{index}]")))
        .collect()
}

fn edit(value: &Value, what: &str) -> Result<RecordedEdit, String> {
    let fields = object(
        value,
        what,
        &[
            "caption",
            "command_line_args",
            "expected_diff",
            "edit",
            "operations",
        ],
        &["shadow_operations"],
    )?;
    let recorded = RecordedEdit {
        caption: string(fields, "caption", what)?,
        command_line_args: arguments(
            &fields["command_line_args"],
            &format!("{what}.command_line_args"),
        )?,
        expected_diff: string(fields, "expected_diff", what)?,
        edit: boolean(fields, "edit", what)?,
        operations: operations(&fields["operations"], &format!("{what}.operations"))?,
        shadow_operations: fields
            .get("shadow_operations")
            .map(|value| operations(value, &format!("{what}.shadow_operations")))
            .transpose()?,
    };
    if !recorded.edit && (!recorded.operations.is_empty() || recorded.shadow_operations.is_some()) {
        return Err(format!("{what} has operations without a closure"));
    }
    Ok(recorded)
}

fn scenario(value: &Value, digest: &str) -> Result<Scenario, String> {
    let fields = object(
        value,
        "scenario",
        &[
            "id",
            "family",
            "scenario",
            "file",
            "sub_scenario",
            "command_line_args",
            "cwd",
            "env",
            "output_is_tty",
            "ignore_case",
            "windows_style_root",
            "files",
            "files_from_build",
            "library",
            "initial_clock_readings",
            "edits",
        ],
        &[],
    )?;
    let id = string(fields, "id", "scenario")?;
    let what = id.clone();
    let what = what.as_str();
    let Value::Object(env) = &fields["env"] else {
        return Err(format!("{what}.env is not an object"));
    };
    let env = env
        .iter()
        .map(|(name, value)| match value {
            Value::String(value) => Ok((name.clone(), value.clone())),
            _ => Err(format!("{what}.env.{name} is not a string")),
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let Value::Object(entries) = &fields["files"] else {
        return Err(format!("{what}.files is not an object"));
    };
    let mut files = FileMap::new();
    for (path, entry) in entries {
        let entry_what = format!("{what}.files[{path}]");
        let entry = if entry.get("symlink").is_some() {
            let fields = object(entry, &entry_what, &["symlink"], &[])?;
            InputFile::File(vfstest::symlink(
                string(fields, "symlink", &entry_what)?.as_bytes(),
            ))
        } else {
            let fields = object(entry, &entry_what, &[], &["text", "text_hex"])?;
            InputFile::Text(file_bytes(fields, &entry_what)?)
        };
        files.insert(path.clone().into_bytes(), entry);
    }
    match &fields["files_from_build"] {
        Value::Null => {}
        build => {
            let build = object(
                build,
                &format!("{what}.files_from_build"),
                &["command_line_args", "paths"],
                &[],
            )?;
            strings(
                &build["command_line_args"],
                "files_from_build.command_line_args",
            )?;
            strings(&build["paths"], "files_from_build.paths")?;
        }
    }
    let library = object(
        &fields["library"],
        &format!("{what}.library"),
        &["path", "files"],
        &[],
    )?;
    let Value::Array(edits) = &fields["edits"] else {
        return Err(format!("{what}.edits is not a list"));
    };
    let edits = edits
        .iter()
        .enumerate()
        .map(|(index, value)| edit(value, &format!("{what}.edits[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let sha256 = hex_digest(&canonical_json(value));
    if sha256 != digest {
        return Err(format!(
            "{what}: the scenario's digest differs from the provenance's"
        ));
    }
    let scenario = Scenario {
        family: string(fields, "family", what)?,
        scenario: string(fields, "scenario", what)?,
        file: string(fields, "file", what)?,
        sub_scenario: string(fields, "sub_scenario", what)?,
        command_line_args: arguments(
            &fields["command_line_args"],
            &format!("{what}.command_line_args"),
        )?,
        cwd: string(fields, "cwd", what)?.into_bytes(),
        env,
        output_is_tty: boolean(fields, "output_is_tty", what)?,
        ignore_case: boolean(fields, "ignore_case", what)?,
        windows_style_root: string(fields, "windows_style_root", what)?.into_bytes(),
        files,
        library_path: string(library, "path", what)?.into_bytes(),
        library_files: strings(&library["files"], &format!("{what}.library.files"))?
            .into_iter()
            .map(String::into_bytes)
            .collect(),
        initial_clock_readings: count(fields, "initial_clock_readings", what)?,
        edits,
        sha256,
        id,
    };
    if !FAMILIES.contains(&scenario.family.as_str())
        || scenario.id
            != format!(
                "{}/{}/{}",
                scenario.family, scenario.scenario, scenario.file
            )
        || scenario.file != format!("{}.js", scenario.sub_scenario.replace(' ', "-"))
    {
        return Err(format!(
            "{what}: the id, family, scenario and file do not agree"
        ));
    }
    Ok(scenario)
}

/// The decompressed bytes of `path`: a `.gz` file through the host's `gzip`
/// (no decompressor is among the workspace's dependencies), anything else as
/// it is.
fn read_document(path: &Path) -> Result<Vec<u8>, String> {
    if path.extension().is_some_and(|extension| extension == "gz") {
        let output = std::process::Command::new("gzip")
            .arg("-dc")
            .arg(path)
            .output()
            .map_err(|error| format!("cannot run gzip for {}: {error}", path.display()))?;
        if !output.status.success() {
            return Err(format!(
                "gzip failed on {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        return Ok(output.stdout);
    }
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Reads and validates the recorded inventory at `path`.
pub fn read_inventory(path: &Path) -> Result<Inventory, String> {
    parse_inventory(&read_document(path)?).map_err(|error| format!("{}: {error}", path.display()))
}

/// Validates a recorded inventory: canonical JSON and a trailing newline.
pub fn parse_inventory(bytes: &[u8]) -> Result<Inventory, String> {
    let document: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if bytes.last() != Some(&b'\n') || canonical_json(&document) != bytes[..bytes.len() - 1] {
        return Err("the inventory is not canonical JSON".to_owned());
    }
    let fields = object(
        &document,
        "inventory",
        &[
            "format",
            "version",
            "library_text",
            "orphan_references",
            "provenance",
            "scenarios",
        ],
        &[],
    )?;
    if fields["format"] != FORMAT || fields["version"] != VERSION {
        return Err(format!("not a version-{VERSION} {FORMAT} file"));
    }
    let library_text = string(fields, "library_text", "inventory")?.into_bytes();
    if library_text != TSC_DEFAULT_LIB_CONTENT.as_bytes() {
        return Err("the recorded library text differs from the harness's".to_owned());
    }
    let provenance = object(
        &fields["provenance"],
        "provenance",
        &[
            "pin",
            "go",
            "goos",
            "goarch",
            "host",
            "sources",
            "pinned_sources",
            "scenario_count",
            "scenario_digests",
        ],
        &[],
    )?;
    let Value::Object(digests) = &provenance["scenario_digests"] else {
        return Err("provenance.scenario_digests is not an object".to_owned());
    };
    let Value::Array(items) = &fields["scenarios"] else {
        return Err("scenarios is not a list".to_owned());
    };
    let mut scenarios = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let Some(Value::String(digest)) = digests.get(id) else {
            return Err(format!("scenario {id:?} has no recorded digest"));
        };
        let scenario = scenario(item, digest)?;
        if scenarios
            .last()
            .is_some_and(|last: &Scenario| last.id >= scenario.id)
            || !seen.insert(scenario.id.clone())
        {
            return Err(format!(
                "scenarios are not sorted and unique at {}",
                scenario.id
            ));
        }
        scenarios.push(scenario);
    }
    if count(provenance, "scenario_count", "provenance")? != scenarios.len() as u64
        || digests.len() != scenarios.len()
    {
        return Err("the provenance counts other scenarios than the file holds".to_owned());
    }
    Ok(Inventory {
        library_text,
        orphan_references: strings(&fields["orphan_references"], "orphan_references")?,
        pin: string(provenance, "pin", "provenance")?,
        scenarios,
    })
}

/// Lowercase hex SHA-256.
pub fn hex_digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// Python's `json.dumps(value, sort_keys=True, separators=(",", ":"),
/// ensure_ascii=True)`: the canonical form the scenario digests are taken
/// over.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => out.extend_from_slice(number.to_string().as_bytes()),
        Value::String(text) => write_canonical_string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            out.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_canonical_string(key, out);
                out.push(b':');
                write_canonical(&fields[key], out);
            }
            out.push(b'}');
        }
    }
}

fn write_canonical_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(c as u8),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
                }
            }
        }
    }
    out.push(b'"');
}

/// Performs one recorded operation as the pin's closure did.
///
/// # Panics
/// When the fake clock does not advance by the recorded readings, or the
/// operation fails, as the pin's `NoError` helpers do.
pub fn apply(sys: &TestSys, op: &Operation) {
    let before = sys.clock.readings();
    match &op.kind {
        OperationKind::Write(bytes) => sys.write_file_no_error(&op.path, bytes),
        OperationKind::Remove => sys.remove_no_error(&op.path),
        OperationKind::Chtimes => sys.touch(&op.path),
    }
    let readings = sys.clock.readings() - before;
    assert!(
        readings == op.clock_readings,
        "replay: {:?} {} read the clock {readings} times, recorded {}",
        op.kind_name(),
        String::from_utf8_lossy(&op.path),
        op.clock_readings
    );
}

impl Operation {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            OperationKind::Write(_) => "write",
            OperationKind::Remove => "remove",
            OperationKind::Chtimes => "chtimes",
        }
    }
}

impl Scenario {
    /// The runner input the scenario was recorded from: the recorded edits
    /// replay their operations (the shadow's own where they differ).
    pub fn to_tsc_input(&self) -> TscInput {
        let edits = self
            .edits
            .iter()
            .map(|recorded| TscEdit {
                caption: recorded.caption.clone(),
                command_line_args: recorded.command_line_args.clone(),
                expected_diff: recorded.expected_diff.clone(),
                edit: recorded.edit.then(|| {
                    let operations = recorded.operations.clone();
                    let shadow_operations = recorded.shadow_operations.clone();
                    Arc::new(move |sys: &TestSys| {
                        let operations = match &shadow_operations {
                            Some(shadow) if sys.for_incremental_correctness => shadow,
                            _ => &operations,
                        };
                        for op in operations {
                            apply(sys, op);
                        }
                    }) as crate::runner::EditFn
                }),
            })
            .collect();
        TscInput {
            sub_scenario: self.sub_scenario.clone(),
            command_line_args: self.command_line_args.clone().unwrap_or_default(),
            files: self.files.clone(),
            cwd: self.cwd.clone(),
            edits,
            env: self.env.clone(),
            output_is_tty: Some(self.output_is_tty),
            ignore_case: self.ignore_case,
            windows_style_root: self.windows_style_root.clone(),
        }
    }

    /// Compares the recorded initial facts with a system the harness builds
    /// from `input`: the working directory, the library path and files, the
    /// TTY flag, the clock readings and the baseline path.
    pub fn check_initial_state(&self, input: &TscInput) -> Result<(), String> {
        let sys = new_test_sys(input, false);
        let what = &self.id;
        if crate::execute::tsc::System::get_current_directory(&*sys) != self.cwd.as_slice()
            || crate::execute::tsc::System::default_library_path(&*sys)
                != self.library_path.as_slice()
            || crate::execute::tsc::System::write_output_is_tty(&*sys) != self.output_is_tty
        {
            return Err(format!(
                "{what}: the recorded working directory, library path or TTY flag differs"
            ));
        }
        let prefix = [self.library_path.as_slice(), b"/"].concat();
        let names: Vec<Vec<u8>> = sys
            .fs
            .default_libs
            .snapshot()
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                path.strip_prefix(prefix.as_slice())
                    .map_or(path.clone(), <[u8]>::to_vec)
            })
            .collect();
        if names != self.library_files {
            return Err(format!(
                "{what}: the recorded library files differ from the harness's"
            ));
        }
        let readings = sys.clock.readings();
        if readings != self.initial_clock_readings {
            return Err(format!(
                "{what}: the harness's system read the clock {readings} times, recorded {}",
                self.initial_clock_readings
            ));
        }
        if input.get_baseline_sub_folder() != self.family {
            return Err(format!(
                "{what}: the runner's family differs from the recorded one"
            ));
        }
        Ok(())
    }
}
