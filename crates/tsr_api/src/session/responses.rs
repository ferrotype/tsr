//! Response constructors over projects, configurations and diagnostics, and
//! the snapshot difference the client uses to keep its source-file cache.
//! port: tsc/internal/api/proto.go
use crate::proto::{
    Category, CompilerOptionsValue, ConfigFileResponse, DiagnosticPositionResponse,
    DiagnosticResponse, DiagnosticSourceLineResponse, Path, ProjectFileChanges, ProjectId,
    ProjectReferenceValue, ProjectResponse, SnapshotChanges, TypeAcquisitionValue,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_ast::{AstView, Diagnostic, NodeId};
use tsr_jsstring::JsString;
use tsr_project::Project;
use tsr_tsoptions::{ConfigValue, ParsedCommandLine};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// port: tsc/internal/api/proto.go:ProjectHandle
pub fn project_handle(project: &Project) -> ProjectId {
    ProjectId(text(
        project.data().expect("loaded project").path.as_bytes(),
    ))
}

/// The pin panics on an unloaded project; callers skip those.
/// port: tsc/internal/api/proto.go:NewProjectResponse
pub fn project_response(project: &Project) -> ProjectResponse {
    let data = project
        .data()
        .expect("NewProjectResponse called with unloaded project");
    ProjectResponse {
        id: project_handle(project),
        config_file_name: text(data.name.as_bytes()),
        current_directory: text(data.current_directory.as_bytes()),
        parsed_command_line: Some(Box::new(config_file_response(&data.command_line))),
        root_files: data
            .command_line
            .root_file_names
            .iter()
            .map(|name| text(name.as_bytes()))
            .collect(),
        compiler_options: Some(Box::new(CompilerOptionsValue(
            data.command_line.options.clone(),
        ))),
    }
}

/// port: tsc/internal/api/proto.go:NewConfigFileResponse
pub fn config_file_response(command_line: &ParsedCommandLine) -> ConfigFileResponse {
    let compile_on_save =
        command_line
            .compile_on_save
            .or_else(|| match command_line.raw.get(b"compileOnSave") {
                Some(ConfigValue::Boolean(value)) => Some(*value),
                _ => None,
            });
    ConfigFileResponse {
        file_names: command_line
            .root_file_names
            .iter()
            .map(|name| text(name.as_bytes()))
            .collect(),
        options: Some(Box::new(CompilerOptionsValue(command_line.options.clone()))),
        project_references: command_line
            .project_references
            .iter()
            .flatten()
            .map(|reference| Some(Box::new(ProjectReferenceValue(reference.clone()))))
            .collect(),
        // The pin's accessor is nil unless the configuration sets type
        // acquisition; the Rust command line carries the default instead.
        type_acquisition: command_line
            .type_acquisition
            .as_ref()
            .filter(|value| **value != tsr_tsoptions::TypeAcquisition::default())
            .map(|value| Box::new(TypeAcquisitionValue(value.clone()))),
        compile_on_save: compile_on_save.map(Box::new),
        raw: raw_config_json(&command_line.raw),
        errors: diagnostic_responses(&command_line.errors, config_view(command_line)),
    }
}

/// The view that owns a command line's diagnostics: its config source file.
fn config_view(command_line: &ParsedCommandLine) -> Option<AstView<'_>> {
    command_line
        .config_file
        .as_ref()
        .map(|config| config.file.view())
}

/// The raw configuration as protocol JSON: typed watch-option enums become
/// their zero-based protocol numbers.
/// port: tsc/internal/api/proto.go:toProtocolJSONValue
pub fn raw_config_json(raw: &ConfigValue) -> Option<tsr_json::RawValue> {
    if raw.is_null() {
        return None;
    }
    let value = to_protocol_json_value(raw);
    tsr_json::marshal(&value, tsr_json::Options::default())
        .ok()
        .map(tsr_json::RawValue)
}

fn to_protocol_json_value(value: &ConfigValue) -> ConfigValue {
    match value {
        // Only the watch kinds keep a typed enum in the raw object; the pin
        // subtracts one from each of them.
        ConfigValue::Enum(number) => ConfigValue::Integer(i64::from(*number) - 1),
        ConfigValue::Object(entries) => ConfigValue::Object(
            entries
                .entries()
                .map(|(key, child)| (key.clone(), to_protocol_json_value(child)))
                .collect(),
        ),
        ConfigValue::Array(Some(values)) => {
            ConfigValue::Array(Some(values.iter().map(to_protocol_json_value).collect()))
        }
        other => other.clone(),
    }
}

/// port: tsc/internal/api/proto.go:NewDiagnosticResponses
pub fn diagnostic_responses(
    diagnostics: &[Diagnostic],
    view: Option<AstView<'_>>,
) -> Vec<Option<Box<DiagnosticResponse>>> {
    diagnostics
        .iter()
        .map(|diagnostic| Some(Box::new(diagnostic_response(diagnostic, view))))
        .collect()
}

/// Positions are UTF-16 offsets, as the pin's are; the source lines are the
/// first two and last two of the diagnostic's span when it covers more than
/// four lines.
/// port: tsc/internal/api/proto.go:newDiagnosticResponse
pub fn diagnostic_response(
    diagnostic: &Diagnostic,
    view: Option<AstView<'_>>,
) -> DiagnosticResponse {
    let message = tsr_compiler::diagnostic_writer::localized(diagnostic)
        .unwrap_or_else(|_| diagnostic.message_text.as_bytes().to_vec());
    let mut response = DiagnosticResponse {
        file_name: String::new(),
        pos: diagnostic.loc.pos(),
        end: diagnostic.loc.end(),
        start_position: None,
        end_position: None,
        source_lines: Vec::new(),
        code: diagnostic.code,
        category: Category(diagnostic.category),
        source: text(diagnostic.source.as_bytes()),
        text: text(&message),
        reports_unnecessary: diagnostic.reports_unnecessary,
        reports_deprecated: diagnostic.reports_deprecated,
        message_chain: Vec::new(),
        related_information: Vec::new(),
    };
    let file = diagnostic
        .file
        .and_then(|file| view.and_then(|view| view.source_file(file).ok()));
    if let Some(file) = file {
        let source = file.text();
        let length = isize::try_from(source.len()).unwrap_or(isize::MAX);
        let pos = isize::try_from(diagnostic.loc.pos())
            .unwrap_or(0)
            .clamp(0, length);
        let end = isize::try_from(diagnostic.loc.end())
            .unwrap_or(0)
            .clamp(pos, length);
        response.file_name = text(file.file_name());
        let positions = file.position_map();
        response.pos = positions.utf8_to_utf16(pos) as i64;
        response.end = positions.utf8_to_utf16(end) as i64;
        let (start_line, start_character) =
            tsr_jsstring::scanner_positions::get_ecma_line_and_utf16_character_of_position(
                source.as_bytes(),
                pos,
            );
        let (end_line, end_character) =
            tsr_jsstring::scanner_positions::get_ecma_line_and_utf16_character_of_position(
                source.as_bytes(),
                end,
            );
        response.start_position = Some(Box::new(DiagnosticPositionResponse {
            line: start_line as i64,
            character: crate::proto::Utf16Offset(start_character as i64),
        }));
        response.end_position = Some(Box::new(DiagnosticPositionResponse {
            line: end_line as i64,
            character: crate::proto::Utf16Offset(end_character as i64),
        }));
        response.source_lines = source_lines(
            source.as_bytes(),
            file.ecma_line_map(),
            start_line,
            end_line,
        );
    }
    response.message_chain = nested_responses(&diagnostic.message_chain, view);
    response.related_information = nested_responses(&diagnostic.related_information, view);
    response
}

fn nested_responses(
    diagnostics: &[Arc<Diagnostic>],
    view: Option<AstView<'_>>,
) -> Vec<Option<Box<DiagnosticResponse>>> {
    diagnostics
        .iter()
        .map(|diagnostic| Some(Box::new(diagnostic_response(diagnostic, view))))
        .collect()
}

/// port: tsc/internal/api/proto.go:diagnosticSourceLines
fn source_lines(
    text_bytes: &[u8],
    line_map: &[i32],
    first_line: isize,
    last_line: isize,
) -> Vec<Option<Box<DiagnosticSourceLineResponse>>> {
    if line_map.is_empty() {
        return Vec::new();
    }
    let lines: Vec<isize> = if last_line - first_line >= 4 {
        vec![first_line, first_line + 1, last_line - 1, last_line]
    } else {
        (first_line..=last_line).collect()
    };
    lines
        .into_iter()
        .map(|line| {
            let index = line as usize;
            let start = line_map.get(index).map_or(0, |start| *start as usize);
            let end = line_map
                .get(index + 1)
                .map_or(text_bytes.len(), |end| *end as usize);
            Some(Box::new(DiagnosticSourceLineResponse {
                line: line as i64,
                text: text(&text_bytes[start.min(text_bytes.len())..end.min(text_bytes.len())]),
            }))
        })
        .collect()
}

/// A diagnostic the client sent back, for example a config-file parsing
/// diagnostic of `createProgram`.
/// port: tsc/internal/api/proto.go:DiagnosticResponse.ToDiagnostic
pub fn to_diagnostic(response: &DiagnosticResponse) -> Diagnostic {
    let nested = |items: &[Option<Box<DiagnosticResponse>>]| -> Vec<Arc<Diagnostic>> {
        items
            .iter()
            .flatten()
            .map(|item| Arc::new(to_diagnostic(item)))
            .collect()
    };
    Diagnostic::from_text(
        None,
        tsr_core::TextRange::new(response.pos, response.end),
        response.code,
        response.category.0,
        response.text.as_bytes(),
        nested(&response.message_chain),
        nested(&response.related_information),
        response.reports_unnecessary,
        response.reports_deprecated,
    )
}

/// Projects removed since `previous`, and the files of every project whose
/// program changed, deleted or changed by identity.
/// port: tsc/internal/api/session.go:computeSnapshotChanges
pub fn compute_snapshot_changes(
    previous: &tsr_project::Snapshot,
    next: &tsr_project::Snapshot,
) -> SnapshotChanges {
    let previous_projects: BTreeMap<&JsString, &Project> = previous.projects_by_path().collect();
    let next_projects: BTreeMap<&JsString, &Project> = next.projects_by_path().collect();
    let mut changes = SnapshotChanges::default();
    for (path, old) in &previous_projects {
        let Some(new) = next_projects.get(path) else {
            changes.removed_projects.push(project_handle(old));
            continue;
        };
        let (Some(old_program), Some(new_program)) = (old.program(), new.program()) else {
            continue;
        };
        if Arc::ptr_eq(old_program, new_program) {
            continue;
        }
        let old_files: BTreeMap<Vec<u8>, &Arc<tsr_compiler::ProgramFile>> = old_program
            .files()
            .iter()
            .map(|file| (file_path(file), file))
            .collect();
        let mut project_changes = ProjectFileChanges::default();
        let mut seen = std::collections::BTreeSet::new();
        for file in new_program.files() {
            let path = file_path(file);
            if let Some(old_file) = old_files.get(&path) {
                if !Arc::ptr_eq(old_file, file) {
                    project_changes.changed_files.push(Path(text(&path)));
                }
            }
            seen.insert(path);
        }
        for path in old_files.keys() {
            if !seen.contains(path) {
                project_changes.deleted_files.push(Path(text(path)));
            }
        }
        if !project_changes.changed_files.is_empty() || !project_changes.deleted_files.is_empty() {
            changes
                .changed_projects
                .0
                .insert(project_handle(new), Some(Box::new(project_changes)));
        }
    }
    changes
}

/// A program file's path (the pin's `SourceFile.Path()`).
pub fn file_path(file: &tsr_compiler::ProgramFile) -> Vec<u8> {
    file.bound()
        .view()
        .source_file()
        .map(|source| source.path().to_vec())
        .unwrap_or_default()
}

/// A program file's name as loaded.
pub fn file_name(file: &tsr_compiler::ProgramFile) -> Vec<u8> {
    file.bound()
        .view()
        .source_file()
        .map(|source| source.file_name().to_vec())
        .unwrap_or_default()
}

/// Standard base64 with padding, as `encoding/base64.StdEncoding` writes it.
pub fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut word = [0u8; 3];
        word[..chunk.len()].copy_from_slice(chunk);
        let value = (u32::from(word[0]) << 16) | (u32::from(word[1]) << 8) | u32::from(word[2]);
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[value as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The file node of a diagnostic, for callers that own the view.
pub fn diagnostic_file(diagnostic: &Diagnostic) -> Option<NodeId> {
    diagnostic.file
}

/// Standard base64 with padding to bytes; `None` for malformed input.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 {
            return None;
        }
        let mut word = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            let digit = if *byte == b'=' && index >= 4 - padding {
                0
            } else {
                value(*byte)?
            };
            word = (word << 6) | digit;
        }
        out.push((word >> 16) as u8);
        if padding < 2 {
            out.push((word >> 8) as u8);
        }
        if padding < 1 {
            out.push(word as u8);
        }
    }
    Some(out)
}
