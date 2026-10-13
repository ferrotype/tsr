use crate::protocol as p;
use std::collections::HashMap;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::{AstView, Diagnostic, NodeId};
use tsr_core::Tristate;
use tsr_embed::{
    DiagnosticKind, EmitOptions, EmitOutput, FileCache, Program, ProgramOptions, Session,
    SessionRequest,
};
use tsr_jsstring::JsString;
use tsr_vfs::{FileSystem, MemoryBuilder};

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Prepared,
    Retained,
    Checkpointed,
}

struct OperationInput {
    current_directory: Vec<u8>,
    configuration: p::Configuration,
    operation: String,
}
impl From<p::Input> for OperationInput {
    fn from(input: p::Input) -> Self {
        Self {
            current_directory: input.current_directory,
            configuration: input.configuration,
            operation: input.operation,
        }
    }
}

pub fn discover(input: p::Input, fs: Arc<dyn FileSystem>) -> Result<(), String> {
    if !input.entries.is_empty() {
        return Err("discovery entries must come from the metadata manifest".into());
    }
    if !matches!(input.operation.as_str(), "check" | "check_emit") {
        return Err("unsupported operation".into());
    }
    validate_path(&input.current_directory)?;
    let mut retained = Retained {
        id: "discovery".into(),
        version: 1,
        state: State::Prepared,
        input: input.into(),
        fs,
        counters: Counters::new(),
        session: None,
        diagnostics: Vec::new(),
        emit: None,
        observation: None,
    };
    retained.execute()
}

struct Retained {
    id: String,
    version: u8,
    state: State,
    input: OperationInput,
    fs: Arc<dyn FileSystem>,
    counters: Counters,
    session: Option<Session>,
    diagnostics: Vec<Diagnostic>,
    emit: Option<EmitOutput>,
    observation: Option<p::Observation>,
}

#[derive(Default)]
pub struct Engine {
    retained: Option<Retained>,
    preparing: Option<crate::stream::Preparation>,
    failed: bool,
}

impl Engine {
    pub fn execute_command(
        &mut self,
        bytes: &[u8],
    ) -> Result<p::Response<p::ResponseValue>, String> {
        if self.failed {
            return Err("budget consumer terminal after failure".into());
        }
        let result = self.command(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn stream_command(&mut self, bytes: &[u8]) -> Result<p::Response<p::ResponseValue>, String> {
        use crate::stream::Command;
        let command = crate::stream::decode(bytes)?;
        let id = command.identity().1.to_owned();
        let kind = if let Command::PrepareBegin {
            input,
            expected_entries,
            expected_files,
            expected_contents,
            ..
        } = command
        {
            if self.retained.is_some() || self.preparing.is_some() {
                return Err("prepare requires empty consumer".into());
            }
            self.preparing = Some(crate::stream::Preparation::begin(
                id.clone(),
                input,
                &expected_entries,
                &expected_files,
                &expected_contents,
            )?);
            "preparing"
        } else {
            let preparation = self
                .preparing
                .as_mut()
                .filter(|p| p.id == id)
                .ok_or("unknown preparing operation")?;
            match command {
                Command::PrepareMetadata { entries, .. } => {
                    preparation.metadata(entries)?;
                    "metadata"
                }
                Command::ContentBegin {
                    path_b64,
                    raw_size,
                    sha256,
                    ..
                } => {
                    preparation.content_begin(&path_b64, &raw_size, sha256)?;
                    "content_started"
                }
                Command::ContentChunk {
                    offset, bytes_b64, ..
                } => {
                    preparation.chunk(&offset, &bytes_b64)?;
                    "content_chunk"
                }
                Command::ContentEnd { .. } => {
                    preparation.content_end()?;
                    "content_verified"
                }
                Command::PrepareEnd { .. } => {
                    let preparation = self.preparing.take().expect("preparing operation");
                    let (input, fs) = preparation.finish()?;
                    self.retained = Some(Retained {
                        id: id.clone(),
                        version: 2,
                        state: State::Prepared,
                        input: input.into(),
                        fs,
                        counters: Counters::new(),
                        session: None,
                        diagnostics: Vec::new(),
                        emit: None,
                        observation: None,
                    });
                    "prepared"
                }
                Command::PrepareBegin { .. } => unreachable!(),
            }
        };
        Ok(p::Response {
            version: 2,
            id,
            kind,
            value: p::ResponseValue::Empty,
        })
    }

    fn command(&mut self, bytes: &[u8]) -> Result<p::Response<p::ResponseValue>, String> {
        let envelope = p::envelope(bytes)?;
        if envelope.version == 2 {
            if bytes.len() > crate::stream::MAX_FRAME {
                return Err("v2 frame exceeds 2 MiB".into());
            }
            if matches!(
                envelope.command,
                "prepare_begin"
                    | "prepare_metadata"
                    | "content_begin"
                    | "content_chunk"
                    | "content_end"
                    | "prepare_end"
            ) {
                return self.stream_command(bytes);
            }
        }
        let command = p::decode(bytes)?;
        if self.preparing.is_some() {
            return Err("snapshot preparation is incomplete".into());
        }
        let mut response = p::Response {
            version: command.version,
            id: command.id.clone(),
            kind: "",
            value: p::ResponseValue::Empty,
        };
        if command.command == "prepare" {
            if self.retained.is_some() {
                return Err("prepare requires empty consumer and input".into());
            }
            let input = command
                .input
                .ok_or("prepare requires empty consumer and input")?;
            if !matches!(input.operation.as_str(), "check" | "check_emit") {
                return Err("unsupported operation".into());
            }
            validate_path(&input.current_directory)?;
            let mut builder = MemoryBuilder::new(&input.current_directory, input.case_sensitive);
            builder.insert_directory(&input.current_directory);
            for entry in &input.entries {
                match entry {
                    p::Entry::Directory { path } => {
                        validate_path(path)?;
                        builder.insert_directory(path);
                    }
                    p::Entry::PhysicalFile { path, bytes } => {
                        validate_path(path)?;
                        builder.insert_physical(path, bytes.as_slice());
                    }
                    p::Entry::Symlink { path, target } => {
                        validate_path(path)?;
                        builder.insert_symlink(path, target);
                    }
                }
            }
            let fs = Arc::new(tsr_embed::BundledFs::new(Arc::new(builder.finish())));
            self.retained = Some(Retained {
                id: command.id,
                version: command.version,
                state: State::Prepared,
                input: input.into(),
                fs,
                counters: Counters::new(),
                session: None,
                diagnostics: Vec::new(),
                emit: None,
                observation: None,
            });
            response.kind = "prepared";
            return Ok(response);
        }
        let retained = self
            .retained
            .as_mut()
            .filter(|r| r.id == command.id && r.version == command.version)
            .ok_or("unknown retained operation")?;
        match command.command.as_str() {
            "execute" => {
                if retained.state != State::Prepared {
                    return Err("execute requires prepared operation".into());
                }
                retained.execute()?;
                retained.state = State::Retained;
                response.kind = "executed";
            }
            "checkpoint" => {
                if retained.state != State::Retained {
                    return Err("checkpoint requires retained operation".into());
                }
                std::hint::black_box(&*retained);
                let peak_rss = crate::metrics::peak_rss_bytes()?;
                let counts = retained.counters.snapshot();
                let mut metrics = crate::allocator::snapshot(counts);
                metrics.insert(
                    "process_peak_rss_before_observation_bytes",
                    peak_rss.to_string(),
                );
                metrics.insert(
                    "process_peak_rss_authority",
                    "getrusage(RUSAGE_SELF).ru_maxrss".into(),
                );
                std::hint::black_box(&*retained);
                response.value = p::ResponseValue::Checkpoint(metrics);
                retained.state = State::Checkpointed;
                response.kind = "checkpoint";
            }
            "observe_begin" | "observe_source" | "observe_write" => {
                if retained.state != State::Checkpointed {
                    return Err("observe requires checkpoint before serialization".into());
                }
                if retained.observation.is_none() {
                    retained.observation = Some(retained.observe()?);
                }
                let observation = retained.observation.as_ref().expect("cached observation");
                match command.command.as_str() {
                    "observe_begin" => {
                        response.kind = "observation_begin";
                        response.value = p::ResponseValue::ObservationBegin(Box::new(
                            p::ObservationHeader::from(observation),
                        ));
                    }
                    "observe_source" => {
                        let index = usize::try_from(crate::stream::decimal(
                            command.index.as_deref().expect("validated row index"),
                        )?)
                        .map_err(|_| "source index exceeds platform range")?;
                        response.kind = "observation_source";
                        response.value = p::ResponseValue::ObservationSource(
                            observation
                                .sources
                                .get(index)
                                .ok_or("source index out of range")?
                                .clone(),
                        );
                    }
                    "observe_write" => {
                        let index = usize::try_from(crate::stream::decimal(
                            command.index.as_deref().expect("validated row index"),
                        )?)
                        .map_err(|_| "write index exceeds platform range")?;
                        response.kind = "observation_write";
                        response.value = p::ResponseValue::ObservationWrite(
                            observation
                                .writes
                                .get(index)
                                .ok_or("write index out of range")?
                                .clone(),
                        );
                    }
                    _ => unreachable!(),
                }
            }
            "observe" => {
                if retained.state != State::Checkpointed {
                    return Err("observe requires checkpoint before serialization".into());
                }
                response.value = p::ResponseValue::Observation(Box::new(retained.observe()?));
                response.kind = "observation";
            }
            "release" => {
                if retained.state != State::Checkpointed {
                    return Err("release requires checkpointed operation".into());
                }
                self.retained = None;
                response.kind = "released";
            }
            _ => return Err("unsupported budget command".into()),
        }
        Ok(response)
    }
}

pub(crate) fn validate_input(input: &p::Input) -> Result<(), String> {
    if !matches!(input.operation.as_str(), "check" | "check_emit") {
        return Err("unsupported operation".into());
    }
    validate_path(&input.current_directory)
}
pub(crate) fn validate_path(path: &[u8]) -> Result<(), String> {
    if !path.starts_with(b"/") || tsr_tspath::normalize(path).as_ref() != path {
        return Err("prototype requires normalized absolute POSIX path".into());
    }
    Ok(())
}
fn js(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}
fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Retained {
    fn execute(&mut self) -> Result<(), String> {
        let host = if self.fs.snapshot_id().is_some() {
            tsr_embed::CompilerConfigHost::new(self.fs.clone(), js(&self.input.current_directory))
        } else {
            tsr_embed::CompilerConfigHost::new_live(
                self.fs.clone(),
                js(&self.input.current_directory),
            )
        };
        let arguments: Vec<_> = self
            .input
            .configuration
            .arguments
            .iter()
            .map(|arg| js(arg))
            .collect();
        let cli = tsr_tsoptions::parse_command_line(&arguments, &host);
        if !cli.root_file_names.is_empty()
            || !cli.options.project.is_empty()
            || cli.options.watch.is_true()
        {
            return Err("arguments must contain compiler overrides only".into());
        }
        let parsed = tsr_tsoptions::get_parsed_command_line_of_config_file(
            &self.input.configuration.config_path,
            &cli.options,
            &cli.raw,
            &host,
        )
        .map_err(display)?;
        let Some(mut config) = parsed.command_line else {
            self.diagnostics = cli.errors;
            self.diagnostics.extend(parsed.read_errors);
            return Ok(());
        };
        let mut errors = cli.errors;
        errors.append(&mut config.errors);
        config.errors = errors;
        if config.options.incremental.is_true()
            || config.options.watch.is_true()
            || config.options.run_external_code.is_true()
            || config
                .content_mappers
                .as_ref()
                .is_some_and(|mappers| !mappers.is_empty())
        {
            return Err("incremental/watch/external content mapper capability unsupported".into());
        }
        let options = ProgramOptions {
            config,
            host: self.fs.clone(),
            current_directory: js(&self.input.current_directory),
            default_library_path: js(b"bundled:///libs"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        };
        let session = if self.fs.snapshot_id().is_some() {
            Session::load(options, &mut FileCache::new(), &self.counters).map_err(display)?
        } else {
            let program = Program::load_live(options, &mut FileCache::new(), &self.counters)
                .map_err(display)?;
            Session::from_program(Arc::new(program), &self.counters)
        };
        let request = SessionRequest::default();
        let mut diagnostics = Vec::new();
        for kind in [
            DiagnosticKind::Config,
            DiagnosticKind::Program,
            DiagnosticKind::Syntactic,
            DiagnosticKind::Global,
        ] {
            diagnostics.extend(session.diagnostics(&request, kind, None).map_err(display)?);
        }
        for file in session.program().files() {
            diagnostics.extend(
                session
                    .diagnostics(&request, DiagnosticKind::Semantic, Some(file))
                    .map_err(display)?,
            );
            if session.program().options().declaration.is_true()
                || session.program().options().composite.is_true()
            {
                diagnostics.extend(
                    session
                        .diagnostics(&request, DiagnosticKind::Declaration, Some(file))
                        .map_err(display)?,
                );
            }
        }
        self.diagnostics = session
            .program()
            .sort_and_deduplicate_diagnostics(&diagnostics)
            .map_err(display)?;
        if self.input.operation == "check_emit" {
            self.emit = Some(session.emit(&EmitOptions::default()).map_err(display)?);
        }
        self.session = Some(session);
        Ok(())
    }

    fn observe(&self) -> Result<p::Observation, String> {
        let program = self
            .session
            .as_ref()
            .map(|session| session.program().as_ref());
        let mut builder = Observer {
            program,
            indices: HashMap::new(),
            sources: Vec::new(),
        };
        let mut observation = p::Observation {
            outcome: "config_failure",
            checkers: "0".into(),
            types: "0".into(),
            nodes: "0".into(),
            identifiers: "0".into(),
            ..Default::default()
        };
        if let Some(config) = program.map(Program::config) {
            observation.roots = config
                .root_file_names
                .iter()
                .map(|name| name.as_bytes().to_vec())
                .collect();
            observation.options_json =
                tsr_tsoptions::options_json::marshal_compiler_options(&config.options)
                    .map_err(display)?;
        }
        if let Some(session) = &self.session {
            observation.outcome = "compiled";
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
            if checkers != 1 {
                return Err(format!("single checker obligation violated: {checkers}"));
            }
            observation.checkers = checkers.to_string();
            observation.types = types.to_string();
            let mut nodes = 0i64;
            let mut identifiers = 0i64;
            for file in session.program().files() {
                builder.add_source(file.source())?;
                let source = file.bound().view().source_file().map_err(display)?;
                nodes = nodes.wrapping_add(source.node_count);
                identifiers = identifiers.wrapping_add(source.identifier_count);
            }
            observation.nodes = nodes.to_string();
            observation.identifiers = identifiers.to_string();
        }
        observation.diagnostics = self
            .diagnostics
            .iter()
            .map(|diagnostic| builder.diagnostic(diagnostic))
            .collect::<Result<_, _>>()?;
        if let Some(emit) = &self.emit {
            for write in &emit.writes {
                let file = emit
                    .written_file(write)
                    .ok_or("invalid retained emit write")?;
                observation.writes.push(p::Write {
                    name: file.name.as_bytes().to_vec(),
                    text: file.text.clone(),
                });
            }
            observation.emitted_files = emit
                .files
                .iter()
                .map(|file| file.name.as_bytes().to_vec())
                .collect();
            observation.emit_skipped = Some(emit.emit_skipped);
            observation.emit_diagnostics = emit
                .diagnostics
                .iter()
                .map(|diagnostic| builder.diagnostic(diagnostic))
                .collect::<Result<_, _>>()?;
        }
        observation.sources = builder.sources;
        Ok(observation)
    }
}

struct Observer<'a> {
    program: Option<&'a Program>,
    indices: HashMap<NodeId, usize>,
    sources: Vec<p::Source>,
}
impl<'a> Observer<'a> {
    fn view(&self, node: NodeId) -> Result<AstView<'a>, String> {
        let program = self
            .program
            .ok_or("diagnostic source has no retained program")?;
        if let Some(file) = program.file_of_node(node) {
            return Ok(file.bound().view().ast());
        }
        program
            .config_source(node)
            .map(|source| source.file.view())
            .ok_or_else(|| "diagnostic source is not retained".into())
    }
    fn add_source(&mut self, node: NodeId) -> Result<usize, String> {
        if let Some(&index) = self.indices.get(&node) {
            return Ok(index);
        }
        let view = self.view(node)?;
        let source = view.source_file(node).map_err(display)?;
        let path = source.parse_options().path.as_bytes();
        let index = self.sources.len();
        self.sources.push(p::Source {
            name: source.parse_options().file_name.as_bytes().to_vec(),
            path: path.to_vec(),
            text: source.text().as_bytes().to_vec(),
            default_library: self.program.is_some_and(|program| program.is_lib(path)),
            node_count: source.node_count.to_string(),
            identifier_count: source.identifier_count.to_string(),
        });
        self.indices.insert(node, index);
        Ok(index)
    }
    fn diagnostic(&mut self, diagnostic: &Diagnostic) -> Result<p::Diagnostic, String> {
        let file = diagnostic
            .file
            .map(|file| self.add_source(file))
            .transpose()?;
        let view = diagnostic.file.map(|file| self.view(file)).transpose()?;
        Ok(p::Diagnostic {
            file,
            pos: diagnostic.loc.pos().to_string(),
            end: diagnostic.loc.end().to_string(),
            code: diagnostic.code,
            category: diagnostic.category,
            message_key: diagnostic.message_key.as_bytes().to_vec(),
            message_text: diagnostic.message_text.as_bytes().to_vec(),
            message_args: diagnostic
                .message_args
                .iter()
                .map(|arg| arg.as_bytes().to_vec())
                .collect(),
            localized: diagnostic.string(view).map_err(display)?,
            chain: diagnostic
                .message_chain
                .iter()
                .map(|d| self.diagnostic(d))
                .collect::<Result<_, _>>()?,
            related: diagnostic
                .related_information
                .iter()
                .map(|d| self.diagnostic(d))
                .collect::<Result<_, _>>()?,
            source: diagnostic.source.as_bytes().to_vec(),
            unnecessary: diagnostic.reports_unnecessary,
            deprecated: diagnostic.reports_deprecated,
            skipped_on_no_emit: diagnostic.skipped_on_no_emit,
            repopulate: diagnostic
                .repopulate_info
                .as_ref()
                .map(|info| p::Repopulate {
                    kind: info.kind,
                    module_reference: info.module_reference.as_bytes().to_vec(),
                    mode: info.mode,
                    package_name: info.package_name.as_bytes().to_vec(),
                }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn entry(path: &str, text: &str) -> Value {
        json!({"kind":"physical_file", "path":path.as_bytes(), "bytes":text.as_bytes()})
    }
    fn prepare(entries: Vec<Value>, operation: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version":1, "command":"prepare", "id":"1", "input":{
                "current_directory":b"/project", "case_sensitive":true,
                "entries":entries, "configuration":{
                    "config_path":b"/project/tsconfig.json", "arguments":[]
                }, "operation":operation
            }
        }))
        .unwrap()
    }
    fn command(
        engine: &mut Engine,
        command: &str,
    ) -> Result<p::Response<p::ResponseValue>, String> {
        engine.execute_command(
            &serde_json::to_vec(&json!({
                "version":1, "command":command, "id":"1"
            }))
            .unwrap(),
        )
    }
    fn observation(engine: &mut Engine) -> Box<p::Observation> {
        command(engine, "execute").unwrap();
        command(engine, "checkpoint").unwrap();
        let p::ResponseValue::Observation(observation) = command(engine, "observe").unwrap().value
        else {
            panic!("expected observation");
        };
        observation
    }

    #[test]
    fn config_resolution_check_emit_and_release_retain_real_results() {
        let entries = vec![
            entry(
                "/project/base.json",
                r#"{"compilerOptions":{"noLib":true,"declaration":true,"listEmittedFiles":true}}"#,
            ),
            entry(
                "/project/tsconfig.json",
                r#"{"extends":"./base.json","include":["src/*.ts"]}"#,
            ),
            entry(
                "/project/src/main.ts",
                "export const value: number = 'invalid';",
            ),
            entry("/project/ignored.ts", "export const ignored = 1;"),
        ];
        let mut engine = Engine::default();
        engine
            .execute_command(&prepare(entries, "check_emit"))
            .unwrap();
        // Snapshot preparation must not parse configuration or load a program.
        assert!(engine.retained.as_ref().unwrap().session.is_none());
        let observed = observation(&mut engine);
        assert_eq!(observed.outcome, "compiled");
        assert_eq!(observed.roots, vec![b"/project/src/main.ts".to_vec()]);
        assert_eq!(observed.checkers, "1");
        assert!(observed.types.parse::<u64>().unwrap() > 0);
        assert!(observed.diagnostics.iter().any(|d| d.code == 2322));
        assert!(observed
            .writes
            .iter()
            .any(|write| write.name == b"/project/src/main.js"));
        assert!(observed
            .writes
            .iter()
            .any(|write| write.name == b"/project/src/main.d.ts"));
        assert_eq!(observed.emit_skipped, Some(false));
        assert!(!observed.emitted_files.is_empty());
        assert!(observed
            .sources
            .iter()
            .any(|source| source.name == b"/project/src/main.ts"));
        command(&mut engine, "release").unwrap();
        assert!(engine.retained.is_none());
        // Owned observations survive release, and the same identifier can be reused.
        assert!(observed.diagnostics.iter().any(|d| d.code == 2322));
        engine
            .execute_command(&prepare(Vec::new(), "check"))
            .unwrap();
    }

    #[test]
    fn check_and_missing_config_have_distinct_observations() {
        let entries = vec![
            entry(
                "/project/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            entry("/project/main.ts", "export const value = 1;"),
        ];
        let mut engine = Engine::default();
        engine.execute_command(&prepare(entries, "check")).unwrap();
        let observed = observation(&mut engine);
        assert_eq!(observed.outcome, "compiled");
        assert_eq!(observed.emit_skipped, None);
        assert!(observed.writes.is_empty());
        command(&mut engine, "release").unwrap();
        engine
            .execute_command(&prepare(Vec::new(), "check"))
            .unwrap();
        let missing = observation(&mut engine);
        assert_eq!(missing.outcome, "config_failure");
        assert_eq!(missing.checkers, "0");
        assert!(!missing.diagnostics.is_empty());
    }

    #[test]
    fn observation_requires_checkpoint_and_failures_are_terminal() {
        let mut engine = Engine::default();
        engine
            .execute_command(&prepare(Vec::new(), "check"))
            .unwrap();
        assert!(engine
            .retained
            .as_ref()
            .unwrap()
            .fs
            .directory_exists(b"/project")
            .unwrap());
        command(&mut engine, "execute").unwrap();
        assert!(command(&mut engine, "observe")
            .err()
            .unwrap()
            .contains("checkpoint"));
        assert!(command(&mut engine, "checkpoint")
            .err()
            .unwrap()
            .contains("terminal"));
    }

    #[test]
    fn unsupported_incremental_configuration_fails_instead_of_changing_operation() {
        let mut engine = Engine::default();
        engine.execute_command(&prepare(vec![
            entry("/project/tsconfig.json", r#"{"compilerOptions":{"incremental":true,"noLib":true},"files":["main.ts"]}"#),
            entry("/project/main.ts", "export {};"),
        ], "check")).unwrap();
        assert!(command(&mut engine, "execute")
            .err()
            .unwrap()
            .contains("unsupported"));
    }
}
