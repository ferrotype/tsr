//! Persistent acceptance executor. Every compile/check/query/emit enters the
//! public embedding facade. The host supplies fixture preparation and renders
//! baselines; callbacks carry owned values, never AST or checker handles.
use crate::compiler_codec as codec;
use crate::compiler_protocol as p;
use crate::mapper_codec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tsr_arena::{Counters, NodeId};
use tsr_ast::{Diagnostic, SourceFileRead};
use tsr_core::Tristate;
use tsr_embed::content_mapper as mapper;
use tsr_embed::{
    BuildInfo, BuildInfoReader, DiagnosticKind, EmitOptions, FileCache, Program, ProgramOptions,
    Session, SessionRequest,
};
use tsr_jsstring::JsString;

pub type Callback =
    dyn Fn(p::CompilerCallback) -> Result<p::CompilerCallbackReply, String> + Send + Sync;

pub struct CompilerConsumer {
    compilations: HashMap<p::CompilationId, Compilation>,
    callback: Arc<Callback>,
}
struct Compilation {
    // Both graphs stay alive for diagnostic source identities until Release.
    _pre: Session,
    post: Session,
}
impl CompilerConsumer {
    pub fn new(callback: Arc<Callback>) -> Self {
        Self {
            compilations: HashMap::new(),
            callback,
        }
    }
    pub fn execute(&mut self, request: p::CompilerRequest) -> Result<p::CompilerResponse, String> {
        if request.version != p::VERSION {
            return Err("unsupported compiler protocol version".into());
        }
        let id = request.command.id();
        let reply = match request.command {
            p::CompilerCommand::Compile { input, .. } => {
                if self.compilations.contains_key(&id) {
                    return Err("compilation id already exists".into());
                }
                let (compilation, output) = compile(id, *input, self.callback.clone())?;
                self.compilations.insert(id, compilation);
                p::CompilerReply::Compiled(Box::new(output))
            }
            p::CompilerCommand::ObserveTypesAndSymbols {
                files, had_errors, ..
            } => {
                let compilation = self.compilations.get(&id).ok_or("unknown compilation id")?;
                p::CompilerReply::Observed(observe(
                    id,
                    &compilation.post,
                    &files,
                    had_errors,
                    &self.callback,
                )?)
            }
            p::CompilerCommand::Assertions {
                union_ordering,
                parent_pointers,
                ..
            } => {
                let compilation = self.compilations.get(&id).ok_or("unknown compilation id")?;
                let union_ordering = if union_ordering {
                    Some(
                        compilation
                            .post
                            .with_compiler_operations(|operations| {
                                Ok(crate::assertions::union_ordering_checkers(
                                    &operations.iter().collect::<Vec<_>>(),
                                ))
                            })
                            .map_err(debug)?,
                    )
                } else {
                    None
                };
                let parent_pointers = parent_pointers
                    .then(|| crate::assertions::parent_pointers(compilation.post.program()));
                p::CompilerReply::Assertions(p::AssertionOutput {
                    union_ordering,
                    parent_pointers,
                })
            }
            p::CompilerCommand::Release { .. } => {
                self.compilations
                    .remove(&id)
                    .ok_or("unknown compilation id")?;
                p::CompilerReply::Released
            }
        };
        Ok(p::CompilerResponse {
            version: p::VERSION,
            id,
            reply,
        })
    }
}
fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
fn js(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

// The native acceptance harness deliberately accepts pinned fixture build-info
// versions. This policy belongs to the test caller, never the public facade.
struct FixtureBuildInfoReader(Box<dyn BuildInfoReader>);
impl BuildInfoReader for FixtureBuildInfoReader {
    fn read_build_info(&self, config: &tsr_tsoptions::ParsedCommandLine) -> Option<BuildInfo> {
        let mut value = self.0.read_build_info(config)?;
        value.version = js(tsr_core::version().as_bytes());
        Some(value)
    }
}
fn load(
    options: ProgramOptions,
    project: Option<Arc<dyn mapper::Project>>,
    cache: &mut FileCache,
    counters: &Counters,
) -> Result<Session, String> {
    let program = Arc::new(
        Program::load_with_content_mapper_project(options, project, cache, counters)
            .map_err(debug)?,
    );
    let reader = FixtureBuildInfoReader(tsr_embed::build_info_reader(program.clone()));
    // Match the pinned test harness, which tests the explicit incremental flag.
    if program.options().incremental.is_true() {
        Session::from_program_with_build_info_reader(program, Some(&reader), counters).map_err(debug)
    } else {
        Ok(Session::from_program(program, counters))
    }
}
fn diagnostics(session: &Session, suggestions: bool) -> Result<Vec<Diagnostic>, String> {
    let mut result = Vec::new();
    for kind in [
        DiagnosticKind::Config,
        DiagnosticKind::Program,
        DiagnosticKind::Syntactic,
        DiagnosticKind::Semantic,
        DiagnosticKind::Global,
    ] {
        result.extend(
            session
                .diagnostics(&SessionRequest::default(), kind, None)
                .map_err(debug)?,
        );
    }
    if session.program().options().emit_declarations() {
        result.extend(
            session
                .diagnostics(
                    &SessionRequest::default(),
                    DiagnosticKind::Declaration,
                    None,
                )
                .map_err(debug)?,
        );
    }
    if suggestions {
        result.extend(
            session
                .diagnostics(&SessionRequest::default(), DiagnosticKind::Suggestion, None)
                .map_err(debug)?,
        );
    }
    session
        .program()
        .sort_and_deduplicate_diagnostics(&result)
        .map_err(debug)
}
fn compile(
    id: p::CompilationId,
    mut input: p::CompilationInput,
    callback: Arc<Callback>,
) -> Result<(Compilation, p::CompilationOutput), String> {
    if input.symlinks_after > input.files.len() {
        return Err("symlink insertion boundary exceeds physical file count".into());
    }
    let options = codec::thaw_options(&input.compiler_options)?;
    let config = codec::thaw_config(
        input.config.clone(),
        options,
        codec::strings(input.root_names.clone()),
    )?;
    let mut files = tsr_vfs::MemoryBuilder::new(&input.current_directory, input.case_sensitive);
    let (before_links, after_links) = input.files.split_at(input.symlinks_after);
    let insert_files = |files: &mut tsr_vfs::MemoryBuilder, inputs: &[p::PhysicalFile]| {
        for file in inputs {
            if file.loaded && file.name.starts_with(b"bundled:///") {
                continue;
            }
            if file.loaded {
                files.insert_loaded(&file.name, file.content.clone());
            } else {
                files.insert_physical(&file.name, file.content.clone());
            }
        }
    };
    insert_files(&mut files, before_links);
    for link in &input.symlinks {
        files.insert_symlink(&link.name, &link.target);
    }
    insert_files(&mut files, after_links);
    let libraries = input
        .files
        .iter_mut()
        .filter(|file| file.loaded && file.name.starts_with(b"bundled:///"))
        .map(|file| {
            let name = file
                .name
                .strip_prefix(b"bundled:///libs/")
                .ok_or("loaded bundled snapshot entry is outside the library directory")?;
            Ok((
                js(name),
                Arc::<[u8]>::from(std::mem::take(&mut file.content)),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let fs: Arc<dyn tsr_vfs::FileSystem> = Arc::new(
        tsr_embed::BundledFs::with_libraries(Arc::new(files.finish()), libraries).map_err(debug)?,
    );
    let project: Option<Arc<dyn mapper::Project>> = (config.options.run_external_code
        == Tristate::TRUE
        && config
            .content_mappers
            .as_ref()
            .is_some_and(|mappers| !mappers.is_empty()))
    .then(|| Arc::new(ProxyProject { id, callback }) as Arc<dyn mapper::Project>);
    let mode = u8::try_from(input.single_threaded)
        .ok()
        .filter(|mode| *mode <= 2)
        .ok_or("invalid single_threaded tristate")?;
    let program_options = |config| ProgramOptions {
        config,
        host: fs.clone(),
        current_directory: js(&input.current_directory),
        default_library_path: js(&input.default_library_path),
        skip_module_resolution: false,
        single_threaded: Tristate(mode),
    };
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let mut pre_config = config.clone();
    pre_config.options.trace_resolution = Tristate::FALSE;
    let pre = load(
        program_options(pre_config),
        project.clone(),
        &mut cache,
        &counters,
    )?;
    let pre_diagnostics = diagnostics(&pre, input.capture_suggestions)?;
    let post = load(program_options(config), project, &mut cache, &counters)?;
    let emit = post
        .emit_with_request(&SessionRequest::default(), &EmitOptions::default())
        .map_err(debug)?;
    let post_diagnostics = diagnostics(&post, input.capture_suggestions)?;
    let output = snapshot(
        &pre,
        &post,
        &pre_diagnostics,
        &post_diagnostics,
        emit.as_ref(),
        &input,
    )?;
    Ok((Compilation { _pre: pre, post }, output))
}

struct ProxyProject {
    id: p::CompilationId,
    callback: Arc<Callback>,
}
impl ProxyProject {
    fn call(&self, request: p::MapperCall) -> Result<p::MapperReply, mapper::Error> {
        let result = (self.callback)(p::CompilerCallback {
            id: self.id,
            call: p::CallbackCall::Mapper { request },
        })
        .unwrap_or_else(|error| panic!("mapper callback transport failed: {error}"));
        match result {
            p::CompilerCallbackReply::Mapper(p::MapperReply::Error(error)) => {
                Err(mapper_codec::thaw_error(error)
                    .unwrap_or_else(|error| panic!("invalid mapper error reply: {error}")))
            }
            p::CompilerCallbackReply::Mapper(reply) => Ok(reply),
            other => panic!("unexpected mapper callback reply: {other:?}"),
        }
    }
    fn unit(&self, call: p::MapperCall) -> Result<(), mapper::Error> {
        match self.call(call)? {
            p::MapperReply::Unit => Ok(()),
            other => panic!("unexpected mapper unit reply: {other:?}"),
        }
    }
}
impl mapper::Project for ProxyProject {
    fn refresh(&self) -> Result<(), mapper::Error> {
        self.unit(p::MapperCall::Refresh)
    }
    fn identities(&self) -> Result<Vec<String>, mapper::Error> {
        match self.call(p::MapperCall::Identities)? {
            p::MapperReply::Strings(value) => Ok(value),
            other => panic!("unexpected mapper identities reply: {other:?}"),
        }
    }
    fn identity(&self, mapper: usize) -> Result<String, mapper::Error> {
        match self.call(p::MapperCall::Identity { mapper })? {
            p::MapperReply::String(value) => Ok(value),
            other => panic!("unexpected mapper identity reply: {other:?}"),
        }
    }
    fn watched_files(&self) -> Result<Vec<String>, mapper::Error> {
        match self.call(p::MapperCall::WatchedFiles)? {
            p::MapperReply::Strings(value) => Ok(value),
            other => panic!("unexpected mapper watched-files reply: {other:?}"),
        }
    }
    fn diagnostics(&self) -> Vec<mapper::OptionDiagnostic> {
        match self
            .call(p::MapperCall::Diagnostics)
            .unwrap_or_else(|error| panic!("mapper diagnostics failed: {error:?}"))
        {
            p::MapperReply::Diagnostics(value) => {
                value.into_iter().map(mapper_codec::thaw_option).collect()
            }
            other => panic!("unexpected mapper diagnostics reply: {other:?}"),
        }
    }
    fn transform(
        &self,
        mapper: usize,
        request: &mapper::Request,
    ) -> Result<mapper::TransformResult, mapper::Error> {
        match self.call(p::MapperCall::Transform {
            mapper,
            file_name: request.file_name.as_bytes().to_vec(),
            content: request.content.clone(),
        })? {
            p::MapperReply::Transform(value) => Ok(mapper_codec::thaw_transform(value)
                .unwrap_or_else(|error| panic!("invalid mapper transform reply: {error}"))),
            other => panic!("unexpected mapper transform reply: {other:?}"),
        }
    }
    fn close(&self) -> Result<(), mapper::Error> {
        self.unit(p::MapperCall::Close)
    }
}

fn observe(
    id: p::CompilationId,
    session: &Session,
    files: &[Vec<u8>],
    had_errors: bool,
    callback: &Arc<Callback>,
) -> Result<p::ObservationOutput, String> {
    let mut output = p::ObservationOutput {
        types_error: None,
        symbols_error: None,
    };
    let mut fatal = None;
    session
        .with_compiler_operations(|operations| {
            let mut type_strings = Vec::new();
            for symbols in [false, true] {
                let mut domain_error = None;
                for (file_index, name) in files.iter().enumerate() {
                    let result = (|| -> Result<Vec<p::ObservationRow>, String> {
                        let file = session
                            .program()
                            .source_file(name)
                            .ok_or("observation source unavailable")?;
                        let operation = operations.for_file(file).map_err(debug)?;
                        let mut trace = crate::walker::Trace {
                            record_queries: false,
                            ..Default::default()
                        };
                        let mut timing = crate::walker::NoTiming;
                        let rows = crate::walker::Collector::new(
                            session.program(),
                            operation,
                            had_errors,
                            &mut trace,
                            &mut timing,
                            &mut type_strings,
                            file_index,
                        )
                        .collect(file.source(), symbols)
                        .map_err(debug)?;
                        Ok(rows)
                    })();
                    let result = match result {
                        Ok(rows) => match callback(p::CompilerCallback {
                            id,
                            call: p::CallbackCall::ObservationBatch {
                                symbols,
                                file_index,
                                rows,
                            },
                        }) {
                            Ok(reply) => Ok(reply),
                            Err(error) => {
                                fatal = Some(error);
                                break;
                            }
                        },
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(p::CompilerCallbackReply::Continue) => {}
                        Ok(p::CompilerCallbackReply::StopDomain) => break,
                        Ok(p::CompilerCallbackReply::Failure(error)) => {
                            domain_error = Some(error);
                            break;
                        }
                        Ok(other) => {
                            fatal = Some(format!("unexpected observation reply: {other:?}"));
                            break;
                        }
                        Err(error) => {
                            domain_error = Some(error);
                            break;
                        }
                    }
                }
                if symbols {
                    output.symbols_error = domain_error;
                } else {
                    output.types_error = domain_error;
                }
                if fatal.is_some() {
                    break;
                }
            }
            Ok(())
        })
        .map_err(debug)?;
    if let Some(error) = fatal {
        return Err(error);
    }
    Ok(output)
}

fn snapshot(
    pre: &Session,
    post: &Session,
    pre_diags: &[Diagnostic],
    post_diags: &[Diagnostic],
    emit: Option<&tsr_embed::EmitOutput>,
    input: &p::CompilationInput,
) -> Result<p::CompilationOutput, String> {
    let mut indices = HashMap::new();
    let mut sources = Vec::new();
    // Index first, freeze second: mapped links may point forward in file order.
    for program in [pre.program(), post.program()] {
        for file in program.files() {
            insert_index(&mut indices, file.source());
        }
        for config in program
            .config()
            .config_file
            .iter()
            .chain(&program.config().config_dependencies)
        {
            insert_index(&mut indices, config.root);
        }
    }
    add_diagnostic_sources(pre.program(), pre_diags, &mut indices)?;
    add_diagnostic_sources(post.program(), post_diags, &mut indices)?;
    if let Some(emit) = emit {
        add_diagnostic_sources(post.program(), &emit.diagnostics, &mut indices)?;
        for write in &emit.writes {
            add_diagnostic_sources(post.program(), &write.data.diagnostics, &mut indices)?;
        }
    }
    let mut frozen = HashSet::new();
    for program in [pre.program(), post.program()] {
        for file in program.files() {
            if frozen.insert(file.source()) {
                let source = file.bound().view().source_file().map_err(debug)?;
                sources.push((
                    indices[&file.source()],
                    freeze_source(program, &source, &indices)?,
                ));
            }
        }
        for config in program
            .config()
            .config_file
            .iter()
            .chain(&program.config().config_dependencies)
        {
            if frozen.insert(config.root) {
                let source = config.file.view().source_file(config.root).map_err(debug)?;
                sources.push((
                    indices[&config.root],
                    freeze_source(program, &source, &indices)?,
                ));
            }
        }
    }
    // Referenced configs can own diagnostics without appearing in the root
    // config's dependency list. Resolve their real retained source owners.
    for (&id, &index) in &indices {
        if frozen.contains(&id) {
            continue;
        }
        let (program, config) = [pre.program(), post.program()]
            .into_iter()
            .find_map(|program| program.config_source(id).map(|config| (program, config)))
            .ok_or("diagnostic source was indexed without a retained owner")?;
        let source = config.file.view().source_file(config.root).map_err(debug)?;
        sources.push((index, freeze_source(program, &source, &indices)?));
    }
    sources.sort_by_key(|(index, _)| *index);
    let index = |id| {
        indices
            .get(&id)
            .copied()
            .ok_or_else(|| "source outside retained compilation graphs".to_owned())
    };
    let program = post.program();
    let pre_files = pre
        .program()
        .files()
        .iter()
        .map(|file| index(file.source()))
        .collect::<Result<_, _>>()?;
    let post_files = program
        .files()
        .iter()
        .map(|file| index(file.source()))
        .collect::<Result<Vec<_>, _>>()?;
    let freeze_diags = |program: &Program, values: &[Diagnostic]| {
        values
            .iter()
            .map(|value| {
                codec::freeze_diagnostic(value, &index, &|value| display_args(program, value))
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let mut queries = input
        .files
        .iter()
        .map(|file| file.name.as_slice())
        .chain(input.symlinks.iter().map(|link| link.name.as_slice()))
        .chain(input.root_names.iter().map(Vec::as_slice))
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    for file in program.files() {
        let source = file.bound().view().source_file().map_err(debug)?;
        queries.push(source.file_name().to_vec());
        queries.push(source.path().to_vec());
        if let Some(info) = source.content_mapper_info() {
            queries.extend(codec::bytes(&info.supplemental_file_names));
            if let Some(name) = &info.canonical_file_name {
                queries.push(name.as_bytes().to_vec());
            }
        }
    }
    if let Some(emit) = emit {
        for map in &emit.source_maps {
            queries.extend(codec::bytes(&map.input_source_file_names));
        }
    }
    let mut seen = HashSet::new();
    let mut lookups = Vec::new();
    for query in queries {
        let path = tsr_tspath::to_path(
            &query,
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        );
        if seen.insert(path.clone()) {
            lookups.push((
                path.as_bytes().to_vec(),
                program
                    .source_file(&query)
                    .map(|file| index(file.source()))
                    .transpose()?,
            ));
        }
    }
    let mut default_library_sources = Vec::new();
    let mut mapped_files = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().map_err(debug)?;
        if program.is_lib(source.path()) {
            default_library_sources.push(index(file.source())?);
        }
        if let Some(mapper) = program.content_mapper(&source) {
            mapped_files.push((index(file.source())?, codec::bytes(&mapper.extensions)));
        }
    }
    let facts = p::ProgramFacts {
        root_names: codec::bytes(&program.config().root_file_names),
        compiler_options: codec::freeze_options(program.options())?,
        current_directory: program.current_directory().to_vec(),
        case_sensitive: program.use_case_sensitive_file_names(),
        common_source_directory: program.common_source_directory().map_err(debug),
        content_mapper_extensions: codec::bytes(&program.content_mapper_extensions()),
        lookups,
        default_library_sources,
        mapped_files,
    };
    let trace = program
        .trace()
        .iter()
        .map(|entry| p::ResolutionTrace {
            message_key: entry.message.key.as_bytes().to_vec(),
            args: entry
                .args
                .iter()
                .map(|arg| match arg {
                    tsr_embed::TraceArg::Text(text) => {
                        p::TraceArgument::Text(text.as_bytes().to_vec())
                    }
                    tsr_embed::TraceArg::Bool(value) => p::TraceArgument::Bool(*value),
                })
                .collect(),
        })
        .collect();
    Ok(p::CompilationOutput {
        sources: sources.into_iter().map(|(_, source)| source).collect(),
        pre_files,
        post_files,
        pre_diagnostics: freeze_diags(pre.program(), pre_diags)?,
        post_diagnostics: freeze_diags(program, post_diags)?,
        emit: emit
            .map(|emit| freeze_emit(program, emit, &index))
            .transpose()?,
        facts,
        trace,
    })
}
fn add_diagnostic_sources(
    program: &Program,
    diagnostics: &[Diagnostic],
    indices: &mut HashMap<NodeId, usize>,
) -> Result<(), String> {
    for diagnostic in diagnostics {
        if let Some(id) = diagnostic.file {
            if !indices.contains_key(&id) {
                if program.config_source(id).is_none() {
                    return Err("diagnostic source has no retained owner".into());
                }
                insert_index(indices, id);
            }
        }
        for related in diagnostic
            .related_information
            .iter()
            .chain(&diagnostic.message_chain)
        {
            add_diagnostic_sources(program, std::slice::from_ref(related.as_ref()), indices)?;
        }
    }
    Ok(())
}
fn insert_index(indices: &mut HashMap<NodeId, usize>, id: NodeId) {
    let next = indices.len();
    indices.entry(id).or_insert(next);
}
fn display_args(program: &Program, diagnostic: &Diagnostic) -> Result<Vec<JsString>, String> {
    if let Some(id) = diagnostic.file {
        if let Some(file) = program.file_of_node(id) {
            let source = file.bound().view().source_file().map_err(debug)?;
            return Ok(diagnostic.display_message_args_in(&source));
        }
        if let Some(config) = program.config_source(id) {
            let source = config.file.view().source_file(config.root).map_err(debug)?;
            return Ok(diagnostic.display_message_args_in(&source));
        }
        return Err("diagnostic source owner is unavailable".into());
    }
    Ok(diagnostic.message_args.clone())
}
fn freeze_source(
    program: &Program,
    source: &SourceFileRead<'_>,
    indices: &HashMap<NodeId, usize>,
) -> Result<p::SourceSnapshot, String> {
    let index = |id| {
        indices
            .get(&id)
            .copied()
            .ok_or_else(|| "mapped source outside retained compilation graphs".to_owned())
    };
    let mapped = source
        .content_mapper_info()
        .map(|info| -> Result<_, String> {
            let canonical_source = info
                .canonical_source_file
                .or_else(|| {
                    info.canonical_file_name.as_ref().and_then(|name| {
                        program
                            .source_file(name.as_bytes())
                            .map(|file| file.source())
                    })
                })
                .map(index)
                .transpose()?;
            let mut supplemental_sources = source
                .supplemental_source_files()
                .map_err(debug)?
                .iter()
                .flatten()
                .copied()
                .map(index)
                .collect::<Result<Vec<_>, _>>()?;
            if supplemental_sources.is_empty() {
                for name in &info.supplemental_file_names {
                    supplemental_sources.push(index(
                        program
                            .source_file(name.as_bytes())
                            .ok_or("mapped supplemental source unavailable")?
                            .source(),
                    )?);
                }
            }
            Ok(p::MappedSource {
                content_mapper: info.content_mapper.as_bytes().to_vec(),
                transform_identity: info.transform_identity.as_bytes().to_vec(),
                virtual_file_name: info.virtual_file_name.as_bytes().to_vec(),
                original_text: info.original_text.as_bytes().to_vec(),
                span_map: info
                    .span_map
                    .as_ref()
                    .map(|map| map.marshal().map_err(debug))
                    .transpose()?,
                canonical_source,
                canonical_name: info
                    .canonical_file_name
                    .as_ref()
                    .map(|name| name.as_bytes().to_vec()),
                supplemental_names: codec::bytes(&info.supplemental_file_names),
                supplemental_sources,
                diagnostic_directives: source
                    .diagnostic_directives()
                    .map_err(debug)?
                    .iter()
                    .map(mapper_codec::freeze_directive)
                    .collect(),
            })
        })
        .transpose()?;
    Ok(p::SourceSnapshot {
        name: source.file_name().to_vec(),
        path: source.path().to_vec(),
        text: source.text().as_bytes().to_vec(),
        script_kind: source.script_kind.0,
        is_declaration_file: source.is_declaration_file,
        mapped,
    })
}
fn freeze_emit(
    program: &Program,
    emit: &tsr_embed::EmitOutput,
    index: &impl Fn(NodeId) -> Result<usize, String>,
) -> Result<p::EmitSnapshot, String> {
    let diags = |values: &[Diagnostic]| {
        values
            .iter()
            .map(|value| {
                codec::freeze_diagnostic(value, index, &|value| display_args(program, value))
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let mut files = emit
        .files
        .iter()
        .map(|file| p::PhysicalFile {
            name: file.name.as_bytes().to_vec(),
            content: file.text.clone(),
            loaded: false,
        })
        .collect::<Vec<_>>();
    let emitted_files = (0..files.len()).collect();
    let mut writes = Vec::new();
    for write in &emit.writes {
        let written = emit
            .written_file(write)
            .ok_or("invalid public emit write index")?;
        let file = match write.emitted_file_index {
            Some(index) => index,
            None => {
                files.push(p::PhysicalFile {
                    name: written.name.as_bytes().to_vec(),
                    content: written.text.clone(),
                    loaded: false,
                });
                files.len() - 1
            }
        };
        let real_name = program
            .host()
            .realpath(written.name.as_bytes())
            .unwrap_or_else(|_| written.name.clone());
        let canonical_real_name = tsr_tspath::to_path(
            real_name.as_bytes(),
            b"",
            program.use_case_sensitive_file_names(),
        );
        let build_info = write
            .data
            .build_info
            .as_ref()
            .map(|info| {
                let info = info
                    .downcast_ref::<BuildInfo>()
                    .ok_or("unknown public emit build-info metadata type")?;
                tsr_json::marshal(info, tsr_json::Options::default()).map_err(debug)
            })
            .transpose()?;
        writes.push(p::WriteSnapshot {
            file,
            real_name: real_name.as_bytes().to_vec(),
            canonical_real_name: canonical_real_name.as_bytes().to_vec(),
            source: write.data.source_file.map(index).transpose()?,
            source_map_url_pos: write.data.source_map_url_pos,
            diagnostics: diags(&write.data.diagnostics)?,
            skipped_dts_write: write.data.skipped_dts_write,
            build_info,
        });
    }
    let source_maps = emit
        .source_maps
        .iter()
        .map(|map| {
            Ok(p::SourceMapSnapshot {
                input_source_file_names: codec::bytes(&map.input_source_file_names),
                generated_file: map.generated_file.as_bytes().to_vec(),
                source_map: tsr_json::marshal(&map.source_map, tsr_json::Options::default())
                    .map_err(debug)?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(p::EmitSnapshot {
        emit_skipped: emit.emit_skipped,
        diagnostics: diags(&emit.diagnostics)?,
        files,
        emitted_files,
        writes,
        source_maps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    const GLOBALS: &[u8] = b"interface Array<T> { length: number; [n: number]: T; }\ninterface Boolean {}\ninterface CallableFunction {}\ninterface Function {}\ninterface IArguments {}\ninterface NewableFunction {}\ninterface Number {}\ninterface Object {}\ninterface RegExp {}\ninterface String {}\n";

    fn input() -> p::CompilationInput {
        let options = tsr_core::CompilerOptions {
            no_lib: Tristate::TRUE,
            strict: Tristate::TRUE,
            module: tsr_core::ModuleKind::ESNEXT,
            target: tsr_core::ScriptTarget::ES2022,
            ..Default::default()
        };
        p::CompilationInput {
            compiler_options: codec::freeze_options(&options).unwrap(),
            root_names: vec![
                b"/globals.d.ts".to_vec(),
                b"/main.ts".to_vec(),
                b"/other.ts".to_vec(),
            ],
            files: vec![
                p::PhysicalFile {
                    name: b"/globals.d.ts".to_vec(),
                    content: GLOBALS.to_vec(),
                    loaded: true,
                },
                p::PhysicalFile {
                    name: b"/main.ts".to_vec(),
                    content: b"export const value: string = 1;\n".to_vec(),
                    loaded: false,
                },
                p::PhysicalFile {
                    name: b"/other.ts".to_vec(),
                    content: b"export const other = 2;\n".to_vec(),
                    loaded: false,
                },
            ],
            symlinks: vec![],
            symlinks_after: 3,
            current_directory: b"/".to_vec(),
            default_library_path: b"/lib".to_vec(),
            case_sensitive: true,
            single_threaded: 2,
            capture_suggestions: false,
            config: Default::default(),
        }
    }
    fn request(command: p::CompilerCommand) -> p::CompilerRequest {
        p::CompilerRequest {
            version: p::VERSION,
            command,
        }
    }

    #[test]
    fn public_compiler_retains_checks_emits_observes_and_releases() {
        let batches = Arc::new(Mutex::new(Vec::new()));
        let received = batches.clone();
        let mut consumer = CompilerConsumer::new(Arc::new(move |call| match call.call {
            p::CallbackCall::ObservationBatch {
                symbols,
                file_index,
                rows,
            } => {
                received
                    .lock()
                    .unwrap()
                    .push((symbols, file_index, rows.len()));
                Ok(if symbols {
                    p::CompilerCallbackReply::Continue
                } else {
                    p::CompilerCallbackReply::StopDomain
                })
            }
            p::CallbackCall::Mapper { .. } => panic!("fixture has no mapper"),
        }));
        let p::CompilerReply::Compiled(output) = consumer
            .execute(request(p::CompilerCommand::Compile {
                id: 7,
                input: Box::new(input()),
            }))
            .unwrap()
            .reply
        else {
            panic!("compile response");
        };
        assert_eq!(
            output
                .pre_diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            [2322]
        );
        assert_eq!(
            output
                .post_diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>(),
            [2322]
        );
        for diagnostic in output
            .pre_diagnostics
            .iter()
            .chain(&output.post_diagnostics)
        {
            assert_eq!(output.sources[diagnostic.file.unwrap()].name, b"/main.ts");
        }
        let emit = output.emit.unwrap();
        assert_eq!(emit.files.len(), 2);
        assert!(
            emit.files.iter().any(
                |file| file.name == b"/main.js" && file.content == b"export const value = 1;\n"
            )
        );
        assert_eq!(emit.writes.len(), 2);
        for write in emit.writes {
            assert_eq!(emit.files[write.file].name, write.real_name);
            assert_eq!(write.real_name, write.canonical_real_name);
        }
        assert_eq!(output.facts.root_names, input().root_names);
        let p::CompilerReply::Observed(observation) = consumer
            .execute(request(p::CompilerCommand::ObserveTypesAndSymbols {
                id: 7,
                files: vec![b"/main.ts".to_vec(), b"/other.ts".to_vec()],
                had_errors: true,
            }))
            .unwrap()
            .reply
        else {
            panic!("observation response");
        };
        assert!(observation.types_error.is_none() && observation.symbols_error.is_none());
        let batches = batches.lock().unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|&(symbols, file, _)| (symbols, file))
                .collect::<Vec<_>>(),
            [(false, 0), (true, 0), (true, 1)]
        );
        assert!(batches.iter().all(|&(_, _, rows)| rows > 0));
        let p::CompilerReply::Assertions(assertions) = consumer
            .execute(request(p::CompilerCommand::Assertions {
                id: 7,
                union_ordering: true,
                parent_pointers: true,
            }))
            .unwrap()
            .reply
        else {
            panic!("assertion response");
        };
        assert!(assertions.union_ordering.is_some() && assertions.parent_pointers.is_some());
        assert!(matches!(
            consumer
                .execute(request(p::CompilerCommand::Release { id: 7 }))
                .unwrap()
                .reply,
            p::CompilerReply::Released
        ));
        assert!(
            consumer
                .execute(request(p::CompilerCommand::Release { id: 7 }))
                .is_err()
        );
    }

    #[test]
    fn explicit_bundled_snapshot_compiles_without_static_fallback() {
        let mut consumer = CompilerConsumer::new(Arc::new(|_| panic!("no callbacks expected")));
        let mut fixture = input();
        let name = b"bundled:///libs/custom.d.ts";
        let text = b"\xef\xbb\xbfinterface Custom {}\n";
        fixture.root_names.push(name.to_vec());
        fixture.files.push(p::PhysicalFile {
            name: name.to_vec(),
            content: text.to_vec(),
            loaded: true,
        });
        let p::CompilerReply::Compiled(output) = consumer
            .execute(request(p::CompilerCommand::Compile {
                id: 1,
                input: Box::new(fixture),
            }))
            .unwrap()
            .reply
        else {
            panic!("compile response");
        };
        assert!(
            output
                .sources
                .iter()
                .any(|source| source.name == name && source.text == text)
        );

        let mut missing = input();
        missing
            .root_names
            .push(b"bundled:///libs/lib.es5.d.ts".to_vec());
        let p::CompilerReply::Compiled(output) = consumer
            .execute(request(p::CompilerCommand::Compile {
                id: 2,
                input: Box::new(missing),
            }))
            .unwrap()
            .reply
        else {
            panic!("compile response");
        };
        assert!(
            output
                .post_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == 6053)
        );
        assert!(
            !output
                .sources
                .iter()
                .any(|source| source.name == b"bundled:///libs/lib.es5.d.ts")
        );
    }

    #[test]
    fn physical_library_tail_keeps_native_symlink_insertion_order() {
        let mut consumer = CompilerConsumer::new(Arc::new(|_| panic!("no callbacks expected")));
        let mut fixture = input();
        fixture.files.insert(
            2,
            p::PhysicalFile {
                name: b"/main.ts".to_vec(),
                content: b"export const late = 2;\n".to_vec(),
                loaded: false,
            },
        );
        fixture.symlinks = vec![p::Symlink {
            name: b"/main.ts".to_vec(),
            target: b"/other.ts".to_vec(),
        }];
        fixture.symlinks_after = 2;
        let p::CompilerReply::Compiled(output) = consumer
            .execute(request(p::CompilerCommand::Compile {
                id: 1,
                input: Box::new(fixture),
            }))
            .unwrap()
            .reply
        else {
            panic!("compile response");
        };
        let main = output
            .sources
            .iter()
            .find(|source| source.name == b"/main.ts")
            .unwrap();
        assert_eq!(main.text, b"export const late = 2;\n");
        assert!(output.pre_diagnostics.is_empty() && output.post_diagnostics.is_empty());
        let mut invalid = input();
        invalid.symlinks_after = invalid.files.len() + 1;
        assert_eq!(
            consumer
                .execute(request(p::CompilerCommand::Compile {
                    id: 2,
                    input: Box::new(invalid)
                }))
                .unwrap_err(),
            "symlink insertion boundary exceeds physical file count"
        );
    }

    #[test]
    fn persistent_ids_and_callback_transport_fail_closed() {
        let mut consumer =
            CompilerConsumer::new(Arc::new(|_| Err("callback channel closed".into())));
        consumer
            .execute(request(p::CompilerCommand::Compile {
                id: 1,
                input: Box::new(input()),
            }))
            .unwrap();
        assert!(
            consumer
                .execute(request(p::CompilerCommand::Compile {
                    id: 1,
                    input: Box::new(input())
                }))
                .is_err()
        );
        assert_eq!(
            consumer
                .execute(request(p::CompilerCommand::ObserveTypesAndSymbols {
                    id: 1,
                    files: vec![b"/main.ts".to_vec()],
                    had_errors: true
                }))
                .unwrap_err(),
            "callback channel closed"
        );
        assert!(
            consumer
                .execute(request(p::CompilerCommand::Assertions {
                    id: 99,
                    union_ordering: true,
                    parent_pointers: true
                }))
                .is_err()
        );
        let mut wrong_version = request(p::CompilerCommand::Release { id: 1 });
        wrong_version.version += 1;
        assert!(consumer.execute(wrong_version).is_err());
    }
}
