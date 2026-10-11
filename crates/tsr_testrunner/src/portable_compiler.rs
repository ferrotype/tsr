//! Persistent compiler consumer transport. The host prepares bytes and renders
//! observations; compiler work runs exclusively in the selected child.
use crate::compile::{
    self, Compilation, CompilationBackend, CompilationEmit, CompileError, Inputs,
    PreparedCompilation,
};
pub use crate::compiler_protocol as protocol;
use crate::harness::baselines::{Failure, SourceMapEmitResult};
use crate::harness::errors::ContentMapperSources;
use crate::harness::program_view::{FileFacts, ProgramFacts, ProgramSnapshot};
use crate::{compiler_codec as codec, mapper_codec};
use protocol::{
    CallbackCall, CompilerCallbackReply, CompilerCommand, CompilerOutputFrame, CompilerReply,
    CompilerRequest,
};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tsr_ast::{AstBuilder, ContentMapperSourceFileInfo, Diagnostic, NodeId, SourceFileRead};
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_jsstring::{JsString, SourceText};

const RESPONSE_TIMEOUT: Duration = Duration::from_mins(10);
const STDERR_LIMIT: usize = 32 * 1024;
const FRAME_LIMIT: u64 = 256 * 1024 * 1024;
type Batch = protocol::ObservationRow;

struct Client {
    child: Child,
    input: Option<mpsc::Sender<Vec<u8>>>,
    output: mpsc::Receiver<Result<CompilerOutputFrame, String>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next_id: u64,
    failed: bool,
}
impl Client {
    fn start(path: &Path) -> Result<Self, String> {
        let mut child = Command::new(path)
            .arg("--compiler")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("starting {}: {error}", path.display()))?;
        let mut stdin = child.stdin.take().ok_or("consumer stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("consumer stdout unavailable")?;
        let mut stderr = child.stderr.take().ok_or("consumer stderr unavailable")?;
        let (output_tx, output) = mpsc::channel();
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let errors = output_tx.clone();
        std::thread::spawn(move || {
            for bytes in input_rx {
                if let Err(error) = stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.write_all(b"\n"))
                    .and_then(|()| stdin.flush())
                {
                    let _ = errors.send(Err(format!("writing consumer input: {error}")));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let read = reader
                    .by_ref()
                    .take(FRAME_LIMIT + 1)
                    .read_until(b'\n', &mut line);
                if matches!(read, Ok(0)) {
                    break;
                }
                let value = read
                    .map_err(|error| format!("reading consumer: {error}"))
                    .and_then(|_| {
                        if line.len() as u64 > FRAME_LIMIT {
                            return Err("compiler response exceeds the 256 MiB frame limit".into());
                        }
                        serde_json::from_slice(&line)
                            .map_err(|error| format!("invalid compiler frame: {error}"))
                    });
                let failed = value.is_err();
                if output_tx.send(value).is_err() || failed {
                    return;
                }
            }
            let _ = output_tx.send(Err("compiler consumer closed stdout".into()));
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let target = captured.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = stderr.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut target = target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let keep = count.min(STDERR_LIMIT.saturating_sub(target.len()));
                target.extend_from_slice(&buffer[..keep]);
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            output,
            stderr: captured,
            next_id: 1,
            failed: false,
        })
    }
    fn send(&mut self, value: &impl serde::Serialize) -> Result<(), String> {
        if self.failed {
            return Err("compiler consumer is unavailable after an execution failure".into());
        }
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        self.input
            .as_ref()
            .ok_or("compiler consumer input closed")?
            .send(bytes)
            .map_err(|_| "compiler consumer input writer closed".into())
    }
    fn request(
        &mut self,
        command: CompilerCommand,
        timeout: Duration,
        callback: &mut impl FnMut(protocol::CompilerCallback) -> Result<CompilerCallbackReply, String>,
    ) -> Result<CompilerReply, String> {
        let id = command.id();
        let result = (|| {
            let deadline = Instant::now() + timeout;
            self.send(&CompilerRequest {
                version: protocol::VERSION,
                command,
            })?;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let frame = self.output.recv_timeout(remaining).map_err(|error| {
                    format!("compiler response timeout/disconnection: {error}")
                })??;
                match frame {
                    CompilerOutputFrame::Response(response) => {
                        if response.version != protocol::VERSION || response.id != id {
                            return Err(
                                "compiler response has a foreign version or compilation id".into(),
                            );
                        }
                        return Ok(response.reply);
                    }
                    CompilerOutputFrame::Callback(call) => {
                        if call.id != id {
                            return Err("compiler callback names a foreign compilation id".into());
                        }
                        self.send(&callback(call)?)?;
                    }
                    CompilerOutputFrame::Failure(reason) => {
                        return Err(format!("compiler consumer failure: {reason}"))
                    }
                }
            }
        })();
        if result.is_err() {
            self.failed = true;
            self.input.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        result.map_err(|error| {
            let stderr = self
                .stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stderr.is_empty() {
                error
            } else {
                format!("{error}; stderr: {}", String::from_utf8_lossy(&stderr))
            }
        })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Session {
    client: Arc<Mutex<Client>>,
    id: u64,
    sources: RestoredSources,
}
impl Session {
    pub fn observe(
        &self,
        files: &[Vec<u8>],
        had_errors: bool,
        mut batch: impl FnMut(bool, usize, Vec<Batch>) -> Result<bool, String>,
    ) -> Result<protocol::ObservationOutput, String> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| "compiler transport lock poisoned")?;
        let reply = client.request(
            CompilerCommand::ObserveTypesAndSymbols {
                id: self.id,
                files: files.to_vec(),
                had_errors,
            },
            RESPONSE_TIMEOUT,
            &mut |callback| match callback.call {
                CallbackCall::ObservationBatch {
                    symbols,
                    file_index,
                    rows,
                } => Ok(if batch(symbols, file_index, rows)? {
                    CompilerCallbackReply::Continue
                } else {
                    CompilerCallbackReply::StopDomain
                }),
                CallbackCall::Mapper { .. } => Err("mapper callback outside compilation".into()),
            },
        )?;
        match reply {
            CompilerReply::Observed(value) => Ok(value),
            _ => Err("consumer returned a non-observation reply".into()),
        }
    }
    pub fn assertions(
        &self,
        union_ordering: bool,
        parent_pointers: bool,
    ) -> Result<protocol::AssertionOutput, String> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| "compiler transport lock poisoned")?;
        match client.request(
            CompilerCommand::Assertions {
                id: self.id,
                union_ordering,
                parent_pointers,
            },
            RESPONSE_TIMEOUT,
            &mut |_| Err("unexpected assertion callback".into()),
        )? {
            CompilerReply::Assertions(value) => Ok(value),
            _ => Err("consumer returned a non-assertion reply".into()),
        }
    }
    pub(crate) fn compile_followup(
        &self,
        inputs: Inputs,
        testdata: &crate::TestData,
    ) -> Result<Compilation, CompileError> {
        execute(compile::prepare(inputs, testdata)?, self.client.clone()).map_err(failure)
    }
    pub(crate) fn sources(&self) -> &RestoredSources {
        &self.sources
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut client) = self.client.lock() {
            if !client.failed {
                let _ = client.request(
                    CompilerCommand::Release { id: self.id },
                    Duration::from_millis(250),
                    &mut |_| Err("unexpected release callback".into()),
                );
            }
        }
    }
}
fn failure(error: String) -> CompileError {
    CompileError::Harness(Failure::Input(error))
}
pub(crate) fn compile(
    inputs: Inputs,
    testdata: &crate::TestData,
    path: &Path,
) -> Result<Compilation, CompileError> {
    let prepared = compile::prepare(inputs, testdata)?;
    let client = Client::start(path).map_err(failure)?;
    execute(prepared, Arc::new(Mutex::new(client))).map_err(failure)
}

fn execute(
    prepared: PreparedCompilation,
    client: Arc<Mutex<Client>>,
) -> Result<Compilation, String> {
    let PreparedCompilation {
        inputs,
        config,
        files,
        symlinks,
        symlinks_after,
    } = prepared;
    let mut physical: Vec<_> = files
        .into_iter()
        .map(|(name, content)| protocol::PhysicalFile {
            name,
            content: content.to_vec(),
            loaded: false,
        })
        .collect();
    physical.extend(
        tsr_bundled::LIBRARIES
            .iter()
            .map(|(name, content)| protocol::PhysicalFile {
                name: tsr_tspath::combine(tsr_bundled::LIB_PATH, &[name.as_bytes()]),
                content: content.to_vec(),
                loaded: true,
            }),
    );
    let input = protocol::CompilationInput {
        compiler_options: tsr_tsoptions::options_json::marshal_compiler_options(&config.options)
            .map_err(|error| error.to_string())?,
        root_names: codec::bytes(&config.root_file_names),
        files: physical,
        symlinks_after,
        symlinks: symlinks
            .into_iter()
            .map(|(name, target)| protocol::Symlink { name, target })
            .collect(),
        current_directory: inputs.current_directory.clone(),
        default_library_path: tsr_bundled::LIB_PATH.to_vec(),
        case_sensitive: inputs.harness_options.use_case_sensitive_file_names,
        single_threaded: inputs.mode.single_threaded().0 as i8,
        capture_suggestions: inputs.harness_options.capture_suggestions,
        config: codec::freeze_config(&config)?,
    };
    let scope = compile::ContentMapperScope::open(&config);
    let (id, output) = {
        let mut locked = client
            .lock()
            .map_err(|_| "compiler transport lock poisoned")?;
        let id = locked.next_id;
        locked.next_id = id
            .checked_add(1)
            .ok_or("compiler compilation id exhausted")?;
        let reply = locked.request(
            CompilerCommand::Compile {
                id,
                input: Box::new(input),
            },
            RESPONSE_TIMEOUT,
            &mut |callback| match callback.call {
                CallbackCall::Mapper { request } => Ok(CompilerCallbackReply::Mapper(mapper_call(
                    scope.project.as_deref(),
                    request,
                )?)),
                CallbackCall::ObservationBatch { .. } => {
                    Err("observation callback during compilation".into())
                }
            },
        )?;
        let CompilerReply::Compiled(output) = reply else {
            return Err("consumer returned a non-compilation reply".into());
        };
        (id, *output)
    };
    // Match native CompileFilesEx's deferred shutdown, before later observers.
    drop(scope);
    restore(inputs, config, client, id, output)
}

fn mapper_call(
    project: Option<&dyn tsr_contentmapper::Project>,
    call: protocol::MapperCall,
) -> Result<protocol::MapperReply, String> {
    use protocol::{MapperCall as C, MapperReply as R};
    let project = project.ok_or("consumer requested a mapper without a trusted host project")?;
    let result = match call {
        C::Refresh => project.refresh().map(|()| R::Unit),
        C::Identities => project.identities().map(R::Strings),
        C::Identity { mapper } => project.identity(mapper).map(R::String),
        C::WatchedFiles => project.watched_files().map(R::Strings),
        C::Diagnostics => {
            return Ok(R::Diagnostics(
                project
                    .diagnostics()
                    .iter()
                    .map(mapper_codec::freeze_option)
                    .collect(),
            ))
        }
        C::Transform {
            mapper,
            file_name,
            content,
        } => {
            return match project.transform(
                mapper,
                &tsr_contentmapper::Request {
                    file_name: JsString::from_bytes(file_name),
                    content,
                },
            ) {
                Ok(value) => mapper_codec::freeze_transform(&value).map(R::Transform),
                Err(error) => Ok(R::Error(mapper_codec::freeze_error(&error))),
            };
        }
        C::Close => project.close().map(|()| R::Unit),
    };
    Ok(result.unwrap_or_else(|error| R::Error(mapper_codec::freeze_error(&error))))
}

pub(crate) struct RestoredSources {
    sources: crate::portable::Sources,
    ids: Vec<NodeId>,
    supplemental: HashMap<NodeId, Vec<NodeId>>,
    mapped: Vec<(NodeId, Vec<JsString>)>,
}
impl DiagnosticSources for RestoredSources {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>, tsr_compiler::Error> {
        self.sources.diagnostic_source(id)
    }
    fn supplemental_sources(&self, id: NodeId) -> Result<Vec<NodeId>, tsr_compiler::Error> {
        self.diagnostic_source(id)?;
        Ok(self.supplemental.get(&id).cloned().unwrap_or_default())
    }
}
impl ContentMapperSources for RestoredSources {
    fn mapped_files(&self) -> Result<Vec<(NodeId, Vec<JsString>)>, Box<dyn std::error::Error>> {
        Ok(self.mapped.clone())
    }
}
impl RestoredSources {
    fn source_id(&self, index: usize) -> Result<NodeId, String> {
        self.ids
            .get(index)
            .copied()
            .ok_or("source index outside compiler snapshots".into())
    }
    fn diagnostics(
        &self,
        values: Vec<crate::protocol::DiagnosticRecord>,
    ) -> Result<Vec<Diagnostic>, String> {
        values
            .into_iter()
            .map(|value| {
                let expected = value.clone();
                let diagnostic = codec::thaw_diagnostic(value, &|index| self.source_id(index))?;
                self.check_display(&diagnostic, &expected)?;
                Ok(diagnostic)
            })
            .collect()
    }
    fn check_display(
        &self,
        value: &Diagnostic,
        expected: &crate::protocol::DiagnosticRecord,
    ) -> Result<(), String> {
        let displayed = if let Some(id) = value.file {
            let source = self
                .diagnostic_source(id)
                .map_err(|error| error.to_string())?;
            value.display_message_args_in(&source)
        } else {
            value.message_args.clone()
        };
        if codec::bytes(&displayed) != expected.display_args {
            return Err(
                "restored diagnostic display arguments differ from compiler observation".into(),
            );
        }
        for (value, expected) in value
            .message_chain
            .iter()
            .zip(&expected.chain)
            .chain(value.related_information.iter().zip(&expected.related))
        {
            self.check_display(value, expected)?;
        }
        Ok(())
    }
}

fn restore(
    inputs: Inputs,
    config: tsr_tsoptions::ParsedCommandLine,
    client: Arc<Mutex<Client>>,
    id: u64,
    output: protocol::CompilationOutput,
) -> Result<Compilation, String> {
    for (label, indices) in [
        ("pre", &output.pre_files),
        ("post", &output.post_files),
        ("default library", &output.facts.default_library_sources),
    ] {
        let mut seen = std::collections::HashSet::new();
        for &index in indices {
            if !seen.insert(index) {
                return Err(format!("duplicate {label} source index"));
            }
        }
    }
    let mut lookup_paths = std::collections::HashSet::new();
    for (path, _) in &output.facts.lookups {
        if !lookup_paths.insert(path) {
            return Err("duplicate compiler lookup key".into());
        }
    }
    let counters = tsr_arena::Counters::new();
    let mut factory = AstBuilder::new(SourceText::default(), &counters);
    let ids = output
        .sources
        .iter()
        .map(|source| {
            crate::portable::source_shell(&mut factory, &source.name, &source.path, &source.text)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_id = |index: usize| {
        ids.get(index)
            .copied()
            .ok_or_else(|| "source index outside compiler snapshots".to_owned())
    };
    let mut supplemental = HashMap::new();
    for (source, &node) in output.sources.iter().zip(&ids) {
        let info = source
            .mapped
            .as_ref()
            .map(|mapped| -> Result<_, String> {
                let directives = mapped
                    .diagnostic_directives
                    .iter()
                    .cloned()
                    .map(mapper_codec::thaw_directive)
                    .collect::<Vec<_>>();
                let directives = factory
                    .source_diagnostic_directives(directives)
                    .map_err(|error| format!("source directives: {error:?}"))?;
                supplemental.insert(
                    node,
                    mapped
                        .supplemental_sources
                        .iter()
                        .map(|&index| source_id(index))
                        .collect::<Result<_, _>>()?,
                );
                Ok(ContentMapperSourceFileInfo {
                    content_mapper: JsString::from_bytes(mapped.content_mapper.as_slice()),
                    transform_identity: JsString::from_bytes(mapped.transform_identity.as_slice()),
                    virtual_file_name: JsString::from_bytes(mapped.virtual_file_name.as_slice()),
                    original_text: SourceText::from_loaded_bytes(mapped.original_text.as_slice()),
                    span_map: mapped
                        .span_map
                        .as_ref()
                        .map(|bytes| {
                            tsr_ast::span_map::unmarshal(bytes)
                                .map(Arc::new)
                                .map_err(|error| error.to_string())
                        })
                        .transpose()?,
                    canonical_source_file: mapped.canonical_source.map(source_id).transpose()?,
                    canonical_file_name: mapped
                        .canonical_name
                        .as_ref()
                        .map(|name| JsString::from_bytes(name.as_slice())),
                    supplemental_file_names: codec::strings(mapped.supplemental_names.clone()),
                    diagnostic_directives: directives,
                    ..ContentMapperSourceFileInfo::default()
                })
            })
            .transpose()?;
        let state = factory
            .source_file_mut(node)
            .map_err(|error| format!("source shell metadata: {error:?}"))?;
        state.script_kind = tsr_core::ScriptKind(source.script_kind);
        state.is_declaration_file = source.is_declaration_file;
        if let Some(info) = info {
            state.set_content_mapper_info(info);
        }
    }
    for &index in output
        .pre_files
        .iter()
        .chain(&output.post_files)
        .chain(&output.facts.default_library_sources)
    {
        source_id(index)?;
    }
    let mapped = output
        .facts
        .mapped_files
        .iter()
        .map(|(index, extensions)| Ok((source_id(*index)?, codec::strings(extensions.clone()))))
        .collect::<Result<_, String>>()?;
    let sources = RestoredSources {
        sources: crate::portable::publish_sources(factory, &ids)?,
        ids,
        supplemental,
        mapped,
    };
    let pre = sources.diagnostics(output.pre_diagnostics)?;
    let post = sources.diagnostics(output.post_diagnostics)?;
    let diagnostics = compile::settle_diagnostics(pre, post, &|id| {
        sources
            .sources
            .diagnostic_source(id)
            .map(|source| source.file_name())
            .map_err(|_| tsr_arena::Error::WrongOwner)
    })
    .map_err(|error| format!("diagnostic settlement: {error:?}"))?;
    let mut file_indices = HashMap::new();
    let files = output
        .post_files
        .iter()
        .enumerate()
        .map(|(index, &global)| {
            file_indices.insert(global, index);
            let source = output
                .sources
                .get(global)
                .ok_or("post file index outside source snapshots")?;
            Ok(FileFacts {
                file_name: JsString::from_bytes(source.name.as_slice()),
                path: JsString::from_bytes(source.path.as_slice()),
                text: SourceText::from_loaded_bytes(source.text.as_slice()),
                original_text: source
                    .mapped
                    .as_ref()
                    .filter(|mapped| !mapped.content_mapper.is_empty())
                    .map(|mapped| SourceText::from_loaded_bytes(mapped.original_text.as_slice())),
                content_mapper: JsString::from_bytes(
                    source
                        .mapped
                        .as_ref()
                        .map_or(b"".as_slice(), |mapped| mapped.content_mapper.as_slice()),
                ),
            })
        })
        .collect::<Result<_, String>>()?;
    let mut lookups = Vec::new();
    for (path, index) in output.facts.lookups {
        if let Some(index) = index {
            lookups.push((
                path,
                *file_indices
                    .get(&index)
                    .ok_or("program lookup names a non-post source")?,
            ));
        }
    }
    let options = codec::thaw_options(&output.facts.compiler_options)?;
    if options != config.options
        || output.facts.current_directory != inputs.current_directory
        || output.facts.case_sensitive != inputs.harness_options.use_case_sensitive_file_names
        || output.facts.root_names != codec::bytes(&config.root_file_names)
    {
        return Err("consumer program facts disagree with the prepared options, roots, or host configuration".into());
    }
    let facts = ProgramFacts::from_snapshot(ProgramSnapshot {
        files,
        lookups,
        options: options.clone(),
        current_directory: output.facts.current_directory.clone(),
        case_sensitive: output.facts.case_sensitive,
        common_source_directory: output.facts.common_source_directory,
        content_mapper_extensions: codec::strings(output.facts.content_mapper_extensions),
    })
    .map_err(|error| error.to_string())?;
    let mut trace = Vec::new();
    let mut sanitizer = crate::harness::subtests::Sanitizer::new(
        &output.facts.current_directory,
        output.facts.case_sensitive,
    );
    for entry in output.trace {
        let message = tsr_diagnostics::by_key_bytes(&entry.message_key)
            .ok_or("resolution trace names an unknown pinned message")?;
        let args: Vec<Vec<u8>> = entry
            .args
            .into_iter()
            .map(|argument| match argument {
                protocol::TraceArgument::Text(value) => value,
                protocol::TraceArgument::Bool(value) => value.to_string().into_bytes(),
            })
            .collect();
        let args: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
        let text = tsr_diagnostics::localize(&tsr_locale::DEFAULT, Some(message), b"", &args);
        trace.extend(sanitizer.sanitize(&text, true));
        trace.push(b'\n');
    }
    let mut recorded = Vec::new();
    let mut recorded_indices = HashMap::new();
    let maps = if let Some(emit) = &output.emit {
        // Validate all raw diagnostics even when a writer only consumes maps.
        sources.diagnostics(emit.diagnostics.clone())?;
        for &index in &emit.emitted_files {
            emit.files
                .get(index)
                .ok_or("emitted file index outside output files")?;
        }
        for write in &emit.writes {
            let file = emit
                .files
                .get(write.file)
                .ok_or("write index outside output files")?;
            if let Some(index) = write.source {
                sources.source_id(index)?;
            }
            sources.diagnostics(write.diagnostics.clone())?;
            let key = tsr_tspath::to_path(&write.real_name, b"", output.facts.case_sensitive);
            if key.as_bytes() != write.canonical_real_name {
                return Err("write canonical path disagrees with observed real name".into());
            }
            if let Some(&index) = recorded_indices.get(key.as_bytes()) {
                let (_, content): &mut (Vec<u8>, Vec<u8>) = &mut recorded[index];
                content.clone_from(&file.content);
            } else {
                recorded_indices.insert(key.as_bytes().to_vec(), recorded.len());
                recorded.push((write.real_name.clone(), file.content.clone()));
            }
        }
        Some(
            emit.source_maps
                .iter()
                .map(|map| -> Result<_, String> {
                    let mut raw = tsr_sourcemap::RawSourceMap::default();
                    tsr_json::unmarshal(&map.source_map, &mut raw, tsr_json::Options::default())
                        .map_err(|error| error.to_string())?;
                    Ok(SourceMapEmitResult {
                        input_source_file_names: codec::strings(
                            map.input_source_file_names.clone(),
                        ),
                        source_map: raw,
                        generated_file: map.generated_file.clone(),
                    })
                })
                .collect::<Result<_, _>>()?,
        )
    } else {
        None
    };
    Ok(Compilation {
        backend: CompilationBackend::Portable(Session {
            client,
            id,
            sources,
        }),
        emit: CompilationEmit::Portable(output.emit),
        maps,
        diagnostics,
        recorded,
        options,
        facts,
        config,
        harness_options: inputs.harness_options.clone(),
        symlinks: inputs.symlinks.clone(),
        trace,
        inputs,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::compile::TestFile;
    use crate::harness::baselines::ProgramView;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeChild(std::path::PathBuf);
    impl FakeChild {
        fn new(body: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "tsrust-compiler-transport-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("consumer.py");
            std::fs::write(
                &path,
                format!("#!/usr/bin/env python3\nimport json, os, sys, time\n{body}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for FakeChild {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }
    fn release_frame(id: u64) -> String {
        serde_json::to_string(&CompilerOutputFrame::Response(protocol::CompilerResponse {
            version: protocol::VERSION,
            id,
            reply: CompilerReply::Released,
        }))
        .unwrap()
    }
    #[test]
    fn failed_child_frames_are_errors_and_cannot_be_reused() {
        for body in [
            "sys.stdin.readline(); sys.exit(17)".to_owned(),
            "sys.stdin.readline(); print('{}',flush=True)".to_owned(),
            format!(
                "sys.stdin.readline(); print({:?},flush=True)",
                release_frame(99)
            ),
        ] {
            let child = FakeChild::new(&body);
            let mut client = Client::start(&child.0).unwrap();
            assert!(client
                .request(
                    CompilerCommand::Release { id: 1 },
                    Duration::from_secs(2),
                    &mut |_| panic!("no callbacks expected")
                )
                .is_err());
            assert!(client.failed);
            assert!(client.child.try_wait().unwrap().is_some());
            assert!(client
                .send(&CompilerRequest {
                    version: protocol::VERSION,
                    command: CompilerCommand::Release { id: 1 }
                })
                .is_err());
        }
    }
    #[test]
    fn timeout_kills_the_child_and_finishes_promptly() {
        let child = FakeChild::new("sys.stdin.readline(); time.sleep(10)");
        let mut client = Client::start(&child.0).unwrap();
        let started = Instant::now();
        let error = client
            .request(
                CompilerCommand::Release { id: 1 },
                Duration::from_millis(40),
                &mut |_| panic!("no callbacks expected"),
            )
            .err()
            .unwrap();
        assert!(error.contains("timeout"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(client.child.try_wait().unwrap().is_some());
    }
    fn inputs() -> Inputs {
        let (compiler_options, harness_options) =
            compile::resolve_options(None, None, b"/.src").unwrap();
        Inputs {
            // Deliberately invalid TypeScript: a native fallback would report
            // syntax errors and could never produce the controlled child bytes.
            input_files: vec![TestFile {
                unit_name: b"/.src/entry.ts".to_vec(),
                content: b"const = ;".to_vec(),
            }],
            other_files: vec![],
            harness_options,
            compiler_options,
            current_directory: b"/.src".to_vec(),
            symlinks: std::collections::BTreeMap::default(),
            tsconfig: None,
            mode: crate::Mode::Single,
        }
    }
    const COMPILER: &str = r"
for line in sys.stdin:
    request=json.loads(line); command=request['command']; ident=command['id']
    if command['command']=='compile':
        source=next(file for file in command['input']['files'] if file['name']==list(b'/.src/entry.ts'))
        value={'sources':[{'name':source['name'],'path':source['name'],'text':source['content'],'script_kind':3,'is_declaration_file':False,'mapped':None}],
          'pre_files':[0],'post_files':[0],'pre_diagnostics':[],'post_diagnostics':[],'emit':None,
          'facts':{'compiler_options':command['input']['compiler_options'],'current_directory':command['input']['current_directory'],'case_sensitive':command['input']['case_sensitive'],'root_names':command['input']['root_names'],'common_source_directory':{'Ok':list(b'/.src/')},'content_mapper_extensions':[],'lookups':[[source['name'],0]],'default_library_sources':[],'mapped_files':[]},'trace':[]}
        reply={'result':'compiled','value':value}
    elif command['command']=='assertions':
        reply={'result':'assertions','value':{'union_ordering':{'pid':os.getpid()},'parent_pointers':None}}
    elif command['command']=='observe_types_and_symbols':
        print(json.dumps({'kind':'callback','value':{'id':ident,'call':{'call':'observation_batch','symbols':False,'file_index':0,'rows':[]}}}),flush=True)
        acknowledgement=json.loads(sys.stdin.readline())
        if acknowledgement['reply']!='stop_domain': raise RuntimeError('host did not preserve stop-domain acknowledgement')
        reply={'result':'observed','value':{'types_error':None,'symbols_error':None}}
    elif command['command']=='release': reply={'result':'released'}
    else: raise RuntimeError('unexpected command')
    print(json.dumps({'kind':'response','value':{'version':1,'id':ident,'reply':reply}}),flush=True)
";
    #[test]
    fn compilation_and_followup_use_one_external_child_without_native_work() {
        let child = FakeChild::new(COMPILER);
        let testdata = crate::TestData {
            root: std::path::PathBuf::from("/unused-fixture-path"),
        };
        let compiled = compile(inputs(), &testdata, &child.0).unwrap();
        assert!(compiled.native_program().is_none());
        assert!(compiled.diagnostics.is_empty());
        assert_eq!(
            compiled.facts().source_file(b"entry.ts").unwrap().text,
            b"const = ;"
        );
        let first = compiled
            .portable_session()
            .unwrap()
            .assertions(true, false)
            .unwrap()
            .union_ordering;
        let repeated = compiled
            .repeat(&std::collections::BTreeMap::default(), &testdata)
            .unwrap();
        let second = repeated
            .portable_session()
            .unwrap()
            .assertions(true, false)
            .unwrap()
            .union_ordering;
        assert_eq!(first, second, "followup must reuse the same child process");
        let mut batches = 0;
        compiled
            .portable_session()
            .unwrap()
            .observe(&[b"entry.ts".to_vec()], false, |symbols, index, rows| {
                assert!(!symbols);
                assert_eq!(index, 0);
                assert!(rows.is_empty());
                batches += 1;
                Ok(false)
            })
            .unwrap();
        assert_eq!(batches, 1);
    }
    #[test]
    fn malformed_program_observations_fail_closed() {
        for (needle, replacement) in [
            ("'post_files':[0]", "'post_files':[0,0]"),
            ("'post_files':[0]", "'post_files':[7]"),
            (
                "'lookups':[[source['name'],0]]",
                "'lookups':[[source['name'],0],[source['name'],0]]",
            ),
            (
                "'lookups':[[source['name'],0]]",
                "'lookups':[[source['name'],7]]",
            ),
            (
                "'current_directory':command['input']['current_directory']",
                "'current_directory':list(b'/wrong')",
            ),
            (
                "'root_names':command['input']['root_names']",
                "'root_names':[]",
            ),
        ] {
            let child = FakeChild::new(&COMPILER.replace(needle, replacement));
            let testdata = crate::TestData {
                root: std::path::PathBuf::from("/unused-fixture-path"),
            };
            assert!(
                compile(inputs(), &testdata, &child.0).is_err(),
                "accepted {replacement}"
            );
        }
    }

    #[test]
    fn unused_common_directory_refusal_is_deferred_to_the_writer() {
        let child = FakeChild::new(&COMPILER.replace(
            "'common_source_directory':{'Ok':list(b'/.src/')}",
            "'common_source_directory':{'Err':'controlled directory refusal'}",
        ));
        let testdata = crate::TestData {
            root: std::path::PathBuf::from("/unused-fixture-path"),
        };
        let compiled = compile(inputs(), &testdata, &child.0).unwrap();
        assert!(compiled.diagnostics.is_empty());
        assert!(compiled
            .facts()
            .common_source_directory()
            .unwrap_err()
            .to_string()
            .contains("controlled directory refusal"));
    }

    #[test]
    fn count_mismatch_retains_the_distinct_pre_emit_diagnostic_source() {
        let raw = Diagnostic::external(
            None,
            tsr_core::TextRange::new(0, 1),
            JsString::default(),
            1,
            999,
            JsString::from_bytes(b"controlled pre-emit diagnostic".as_slice()),
        );
        let mut record = codec::freeze_diagnostic(
            &raw,
            &|_| Err("no source expected while constructing control".into()),
            &|value| Ok(value.message_args.clone()),
        )
        .unwrap();
        record.file = Some(0);
        let record = serde_json::to_string(&record).unwrap();
        let body = COMPILER.replace(
            "        reply={'result':'compiled','value':value}",
            &format!(
                "        value['sources'].append(dict(value['sources'][0],text=list(b'changed post text')))\n        value['post_files']=[1]\n        value['facts']['lookups']=[[source['name'],1]]\n        value['pre_diagnostics']=[json.loads({record:?})]\n        reply={{'result':'compiled','value':value}}"
            ),
        );
        let child = FakeChild::new(&body);
        let testdata = crate::TestData {
            root: std::path::PathBuf::from("/unused-fixture-path"),
        };
        let compiled = compile(inputs(), &testdata, &child.0).unwrap();
        assert_eq!(compiled.diagnostics.len(), 1);
        assert_eq!(compiled.diagnostics[0].code, -1);
        let excess = compiled.diagnostics[0].related_information.last().unwrap();
        assert_eq!(excess.code, 999);
        let source = compiled
            .diagnostic_sources()
            .diagnostic_source(excess.file.unwrap())
            .unwrap();
        assert_eq!(source.text().as_bytes(), b"const = ;");
        assert_eq!(
            compiled.facts().source_file(b"entry.ts").unwrap().text,
            b"changed post text"
        );
    }
}
