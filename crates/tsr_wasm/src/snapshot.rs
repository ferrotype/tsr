//! Owned observations of a retained production session. No diagnostic/emit
//! schedule, acceptance walker or external protocol is defined in this module.
use std::collections::HashMap;
use tsr_ast::{AstView, Diagnostic, NodeId};
use tsr_embed::{EmitOutput, Program, Session};

#[derive(serde::Serialize)]
pub(super) struct Snapshot {
    outcome: &'static str,
    roots: Vec<Vec<u8>>,
    options_json: Vec<u8>,
    #[serde(skip_serializing)]
    sources: Vec<NodeId>,
    diagnostics: Vec<DiagnosticRecord>,
    checkers: String,
    types: String,
    nodes: String,
    identifiers: String,
    #[serde(skip_serializing)]
    write_count: usize,
    emitted_files: Vec<Vec<u8>>,
    emit_skipped: Option<bool>,
    emit_diagnostics: Vec<DiagnosticRecord>,
}
#[derive(serde::Serialize)]
struct Source {
    name: Vec<u8>,
    path: Vec<u8>,
    text: Vec<u8>,
    default_library: bool,
    node_count: String,
    identifier_count: String,
}
#[derive(serde::Serialize)]
struct Write {
    name: Vec<u8>,
    text: Vec<u8>,
}
#[derive(serde::Serialize)]
struct DiagnosticRecord {
    file: Option<usize>,
    pos: String,
    end: String,
    code: i32,
    category: i32,
    message_key: Vec<u8>,
    message_text: Vec<u8>,
    message_args: Vec<Vec<u8>>,
    localized: Vec<u8>,
    chain: Vec<DiagnosticRecord>,
    related: Vec<DiagnosticRecord>,
    source: Vec<u8>,
    unnecessary: bool,
    deprecated: bool,
    skipped_on_no_emit: bool,
    repopulate: Option<Repopulate>,
}
#[derive(serde::Serialize)]
struct Repopulate {
    kind: i32,
    module_reference: Vec<u8>,
    mode: i32,
    package_name: Vec<u8>,
}

pub(super) fn encode(
    session: &Session,
    diagnostics: &[Diagnostic],
    emit: Option<&EmitOutput>,
) -> Result<Vec<u8>, String> {
    let snapshot = collect(session, diagnostics, emit)?;
    let sources = snapshot
        .sources
        .iter()
        .map(|&node| source(session.program(), node))
        .collect::<Result<Vec<_>, _>>()?;
    let writes = (0..snapshot.write_count)
        .map(|index| write(emit, index))
        .collect::<Result<Vec<_>, _>>()?;
    #[derive(serde::Serialize)]
    struct Rows<'a> {
        #[serde(flatten)]
        header: &'a Snapshot,
        sources: Vec<Source>,
        writes: Vec<Write>,
    }
    serde_json::to_vec(&Rows {
        header: &snapshot,
        sources,
        writes,
    })
    .map_err(display)
}

pub(super) fn collect(
    session: &Session,
    diagnostics: &[Diagnostic],
    emit: Option<&EmitOutput>,
) -> Result<Snapshot, String> {
    let program = session.program();
    let mut observer = Observer {
        program,
        indices: HashMap::new(),
        sources: Vec::new(),
    };
    let (checkers, types) = session
        .with_compiler_operations(|operations| {
            Ok((
                operations.len(),
                operations.iter().fold(0u32, |count, operation| {
                    count.wrapping_add(operation.type_count() as u32)
                }),
            ))
        })
        .map_err(display)?;
    let mut nodes = 0i64;
    let mut identifiers = 0i64;
    for file in program.files() {
        observer.add_source(file.source())?;
        let source = file.bound().view().source_file().map_err(display)?;
        nodes = nodes.wrapping_add(source.node_count);
        identifiers = identifiers.wrapping_add(source.identifier_count);
    }
    let diagnostics = diagnostics
        .iter()
        .map(|value| observer.diagnostic(value))
        .collect::<Result<_, _>>()?;
    let mut write_count = 0;
    let mut emitted_files = Vec::new();
    let mut emit_diagnostics = Vec::new();
    if let Some(emit) = emit {
        for write in &emit.writes {
            emit.written_file(write)
                .ok_or("retained emit write is unavailable")?;
            write_count += 1;
        }
        emitted_files = emit
            .files
            .iter()
            .map(|file| file.name.as_bytes().to_vec())
            .collect();
        emit_diagnostics = emit
            .diagnostics
            .iter()
            .map(|value| observer.diagnostic(value))
            .collect::<Result<_, _>>()?;
    }
    Ok(Snapshot {
        outcome: "compiled",
        roots: program
            .config()
            .root_file_names
            .iter()
            .map(|name| name.as_bytes().to_vec())
            .collect(),
        options_json: tsr_tsoptions::options_json::marshal_compiler_options(program.options())
            .map_err(display)?,
        sources: observer.sources,
        diagnostics,
        checkers: checkers.to_string(),
        types: types.to_string(),
        nodes: nodes.to_string(),
        identifiers: identifiers.to_string(),
        write_count,
        emitted_files,
        emit_skipped: emit.map(|value| value.emit_skipped),
        emit_diagnostics,
    })
}

pub(super) fn header(snapshot: &Snapshot) -> Result<Vec<u8>, String> {
    #[derive(serde::Serialize)]
    struct Header<'a> {
        #[serde(flatten)]
        snapshot: &'a Snapshot,
        source_count: String,
        write_count: String,
    }
    serde_json::to_vec(&Header {
        snapshot,
        source_count: snapshot.sources.len().to_string(),
        write_count: snapshot.write_count.to_string(),
    })
    .map_err(display)
}
pub(super) fn source_row(
    snapshot: &Snapshot,
    program: &Program,
    index: usize,
) -> Result<Vec<u8>, String> {
    let node = *snapshot
        .sources
        .get(index)
        .ok_or("retained source index out of range")?;
    serde_json::to_vec(&source(program, node)?).map_err(display)
}
pub(super) fn write_row(
    snapshot: &Snapshot,
    emit: Option<&EmitOutput>,
    index: usize,
) -> Result<Vec<u8>, String> {
    if index >= snapshot.write_count {
        return Err("retained write index out of range".to_owned());
    }
    serde_json::to_vec(&write(emit, index)?).map_err(display)
}
fn source(program: &Program, node: NodeId) -> Result<Source, String> {
    let observer = Observer {
        program,
        indices: HashMap::new(),
        sources: Vec::new(),
    };
    let source = observer.view(node)?.source_file(node).map_err(display)?;
    let options = source.parse_options();
    Ok(Source {
        name: options.file_name.as_bytes().to_vec(),
        path: options.path.as_bytes().to_vec(),
        text: source.text().as_bytes().to_vec(),
        default_library: program.is_lib(options.path.as_bytes()),
        node_count: source.node_count.to_string(),
        identifier_count: source.identifier_count.to_string(),
    })
}
fn write(emit: Option<&EmitOutput>, index: usize) -> Result<Write, String> {
    let emit = emit.ok_or("retained emit is unavailable")?;
    let item = emit
        .writes
        .get(index)
        .ok_or("retained write index out of range")?;
    let file = emit
        .written_file(item)
        .ok_or("retained emit write is unavailable")?;
    Ok(Write {
        name: file.name.as_bytes().to_vec(),
        text: file.text.clone(),
    })
}

struct Observer<'a> {
    program: &'a Program,
    indices: HashMap<NodeId, usize>,
    sources: Vec<NodeId>,
}
impl<'a> Observer<'a> {
    fn view(&self, node: NodeId) -> Result<AstView<'a>, String> {
        if let Some(file) = self.program.file_of_node(node) {
            return Ok(file.bound().view().ast());
        }
        self.program
            .config_source(node)
            .map(|source| source.file.view())
            .ok_or_else(|| "diagnostic source owner is unavailable".to_owned())
    }
    fn add_source(&mut self, node: NodeId) -> Result<usize, String> {
        if let Some(&index) = self.indices.get(&node) {
            return Ok(index);
        }
        let source = self.view(node)?.source_file(node).map_err(display)?;
        if source.content_mapper_info().is_some() {
            return Err("retained snapshot does not support mapped sources".to_owned());
        }
        let index = self.sources.len();
        self.sources.push(node);
        self.indices.insert(node, index);
        Ok(index)
    }
    fn diagnostic(&mut self, value: &Diagnostic) -> Result<DiagnosticRecord, String> {
        let file = value.file.map(|id| self.add_source(id)).transpose()?;
        let view = value.file.map(|id| self.view(id)).transpose()?;
        Ok(DiagnosticRecord {
            file,
            pos: value.loc.pos().to_string(),
            end: value.loc.end().to_string(),
            code: value.code,
            category: value.category,
            message_key: value.message_key.as_bytes().to_vec(),
            message_text: value.message_text.as_bytes().to_vec(),
            message_args: value
                .message_args
                .iter()
                .map(|arg| arg.as_bytes().to_vec())
                .collect(),
            localized: value.string(view).map_err(display)?,
            chain: value
                .message_chain
                .iter()
                .map(|item| self.diagnostic(item))
                .collect::<Result<_, _>>()?,
            related: value
                .related_information
                .iter()
                .map(|item| self.diagnostic(item))
                .collect::<Result<_, _>>()?,
            source: value.source.as_bytes().to_vec(),
            unnecessary: value.reports_unnecessary,
            deprecated: value.reports_deprecated,
            skipped_on_no_emit: value.skipped_on_no_emit,
            repopulate: value.repopulate_info.as_ref().map(|info| Repopulate {
                kind: info.kind,
                module_reference: info.module_reference.as_bytes().to_vec(),
                mode: info.mode,
                package_name: info.package_name.as_bytes().to_vec(),
            }),
        })
    }
}
fn display(value: impl std::fmt::Display) -> String {
    value.to_string()
}
