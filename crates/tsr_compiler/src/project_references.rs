//! Project-reference loading: reading every referenced config once and the file
//! mapper that sends a referenced project's sources to their built declarations.
//! The mapper answers loader and program queries; the loader applies the
//! redirects, including the editor's source-of-reference mode.
use crate::Error;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    convert::Infallible,
    sync::{Arc, Mutex},
};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile};
use tsr_tspath as path;
use tsr_vfs::FileSystem;

/// Supplies the parsed configurations owned by a build graph. Returned configs
/// must have their input/output maps initialized before publication. Their source
/// identity distinguishes a circular reference to the root from a fresh parse.
/// The pinned CompilerHost.GetResolvedProjectReference host contract.
pub trait ResolvedProjectReferenceProvider: Send + Sync {
    fn get_resolved_project_reference(
        &self,
        file_name: &[u8],
        path: &[u8],
    ) -> Option<Arc<ParsedCommandLine>>;
}

/// The compiler host's configuration reader: extends and content-mapper
/// packages resolve through the production module resolver over the program's
/// own filesystem, as the pinned `compilerHost` does for `tsoptions`.
pub struct CompilerConfigHost {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
    allow_live: bool,
}

impl CompilerConfigHost {
    /// Command-line configuration loading owns a fresh resolver per operation;
    /// it does not publish a resolver cache over a changing OS filesystem.
    pub fn new_live(fs: Arc<dyn FileSystem>, cwd: JsString) -> Self {
        Self {
            fs,
            cwd,
            allow_live: true,
        }
    }
    pub fn new(fs: Arc<dyn FileSystem>, cwd: JsString) -> Self {
        Self {
            fs,
            cwd,
            allow_live: false,
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "This conversion consumes the error supplied by Result::map_err"
)]
fn module_error(error: tsr_module::Error) -> tsr_vfs::Error {
    match error {
        tsr_module::Error::Host(error) => error,
        tsr_module::Error::MutableHost => {
            tsr_vfs::Error::Unsupported("config resolution requires an immutable host")
        }
        tsr_module::Error::Unsupported(reason) => tsr_vfs::Error::Unsupported(reason),
        tsr_module::Error::MalformedPackageJson(_) => {
            tsr_vfs::Error::Unsupported("malformed package JSON")
        }
    }
}

impl ParseConfigHost for CompilerConfigHost {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve_config(
        &self,
        name: &[u8],
        containing: &[u8],
    ) -> Result<Option<JsString>, tsr_vfs::Error> {
        let result = tsr_module::resolve_config_with_options(
            name,
            containing,
            self.fs.clone(),
            self.cwd.as_bytes(),
            tsr_module::ResolverOptions {
                allow_live_host: self.allow_live,
                ..Default::default()
            },
        )
        .map_err(module_error)?;
        Ok((!result.resolved_file_name.is_empty()).then_some(result.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        tsr_module::resolve_content_mapper_manifest_with_options(
            &self.fs,
            self.cwd.as_bytes(),
            containing,
            package,
            tsr_module::ResolverOptions {
                allow_live_host: self.allow_live,
                ..Default::default()
            },
        )
        .map_err(module_error)
    }
}

/// Reads one referenced config with no inherited options. A missing file is a
/// nil reference; a host failure is not.
/// port: tsc/internal/compiler/host.go:compilerHost.GetResolvedProjectReference
fn get_resolved_project_reference(
    host: &CompilerConfigHost,
    name: &[u8],
    path: JsString,
) -> Result<Option<ParsedCommandLine>, Error> {
    Ok(tsr_tsoptions::get_parsed_command_line_of_config_file_path(
        name,
        path,
        &CompilerOptions::default(),
        &ConfigValue::Null,
        host,
    )?
    .command_line)
}

/// One source of a referenced project and its declaration output. `config`
/// indexes the mapper's parsed references.
/// source: tsc/internal/tsoptions/parsedcommandline.go:SourceOutputAndProjectReference
#[derive(Debug)]
pub(crate) struct ReferenceFile {
    pub(crate) source: JsString,
    pub(crate) output_dts: JsString,
    pub(crate) config: usize,
}

/// The filesystem the mapper may read while the loader runs. Published programs
/// drop it, so later lookups only see what loading recorded.
struct LoaderHost {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
    case_sensitive: bool,
}

/// A reference walk callback: the reference's config path, its parsed config
/// (None when it could not be read), the referencing config and the
/// reference's index there. Returning false stops the walk.
pub(crate) type ReferenceVisitor<'a> =
    dyn FnMut(&JsString, Option<&ParsedCommandLine>, &ParsedCommandLine, usize) -> bool + 'a;

/// source: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper
pub(crate) struct ProjectReferenceFileMapper {
    root_config_path: JsString,
    has_project_references: bool,
    can_use_source: bool,
    preserve_symlinks: bool,
    loader: Option<LoaderHost>,
    configs: Vec<Arc<ParsedCommandLine>>,
    config_to_project_reference: BTreeMap<JsString, Option<usize>>,
    references_in_config_file: BTreeMap<JsString, Vec<JsString>>,
    source_to_project_reference: BTreeMap<JsString, Arc<ReferenceFile>>,
    output_dts_to_project_reference: Arc<BTreeMap<JsString, Arc<ReferenceFile>>>,
    dts_directories: Arc<BTreeSet<path::Path>>,
    realpath_dts_to_source: Mutex<BTreeMap<JsString, Option<Arc<ReferenceFile>>>>,
}

impl ProjectReferenceFileMapper {
    /// A finished program's reference map has no loader host. Copy its lookup
    /// state while retaining the immutable parsed configuration owners.
    pub(crate) fn fork_published(&self) -> Self {
        assert!(
            self.loader.is_none(),
            "only finished reference maps can be reused"
        );
        Self {
            root_config_path: self.root_config_path.clone(),
            has_project_references: self.has_project_references,
            can_use_source: self.can_use_source,
            preserve_symlinks: self.preserve_symlinks,
            loader: None,
            configs: self.configs.clone(),
            config_to_project_reference: self.config_to_project_reference.clone(),
            references_in_config_file: self.references_in_config_file.clone(),
            source_to_project_reference: self.source_to_project_reference.clone(),
            output_dts_to_project_reference: self.output_dts_to_project_reference.clone(),
            dts_directories: self.dts_directories.clone(),
            realpath_dts_to_source: Mutex::new(
                self.realpath_dts_to_source
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        }
    }
    /// The empty mapper every program builds, with or without references.
    pub(crate) fn new(config: &ParsedCommandLine, can_use_source: bool) -> Self {
        Self {
            root_config_path: root_config_path(config.config_file.as_deref()),
            has_project_references: config
                .project_references
                .as_ref()
                .is_some_and(|references| !references.is_empty()),
            can_use_source,
            preserve_symlinks: config.options.preserve_symlinks.is_true(),
            loader: None,
            configs: Vec::new(),
            config_to_project_reference: BTreeMap::new(),
            references_in_config_file: BTreeMap::new(),
            source_to_project_reference: BTreeMap::new(),
            output_dts_to_project_reference: Arc::new(BTreeMap::new()),
            dts_directories: Arc::new(BTreeSet::new()),
            realpath_dts_to_source: Mutex::new(BTreeMap::new()),
        }
    }

    /// Loading has finished: the mapper no longer reads the host.
    pub(crate) fn release_loader(&mut self) {
        self.loader = None;
    }

    pub(crate) fn has_outputs(&self) -> bool {
        !self.output_dts_to_project_reference.is_empty()
    }

    /// Resolution and preserved-symlink lookups share the faked declaration
    /// identities. Parsing still reads the original host after redirecting to
    /// source. The immutable output index is shared without retaining a loader
    /// or forming a host-to-mapper ownership cycle.
    pub(crate) fn resolution_host(
        &mut self,
        original: &Arc<dyn FileSystem>,
    ) -> Arc<dyn FileSystem> {
        if !self.can_use_source || !self.has_outputs() {
            return original.clone();
        }
        let cwd = self
            .loader
            .as_ref()
            .expect("parsed reference outputs have a loader")
            .cwd
            .clone();
        let host = self.resolution_host_for_snapshot(original, &cwd);
        self.loader
            .as_mut()
            .expect("parsed reference outputs have a loader")
            .fs = host.clone();
        host
    }

    pub(crate) fn resolution_host_for_snapshot(
        &self,
        original: &Arc<dyn FileSystem>,
        cwd: &JsString,
    ) -> Arc<dyn FileSystem> {
        if !self.can_use_source || !self.has_outputs() {
            return original.clone();
        }
        let host: Arc<dyn FileSystem> = Arc::new(tsr_vfs::cached::CachedFs::new(Arc::new(
            crate::project_reference_host::ProjectReferenceDtsFakingHost::new(
                original.clone(),
                cwd.clone(),
                self.output_dts_to_project_reference.clone(),
                self.dts_directories.clone(),
            ),
        )));
        host
    }

    /// Whether any referenced source can be redirected to an output. Without
    /// one, no load needs the redirect bookkeeping.
    pub(crate) fn has_sources(&self) -> bool {
        !self.source_to_project_reference.is_empty()
    }

    pub(crate) fn config(&self, index: usize) -> &ParsedCommandLine {
        &self.configs[index]
    }

    pub(crate) fn config_arc(&self, index: usize) -> Arc<ParsedCommandLine> {
        self.configs[index].clone()
    }

    /// Every referenced config the mapper parsed, including the ones a
    /// duplicate or cyclic reference made unreachable from the walk.
    pub(crate) fn configs(&self) -> impl Iterator<Item = &ParsedCommandLine> {
        self.configs.iter().map(AsRef::as_ref)
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getParseFileRedirect
    pub(crate) fn parse_file_redirect(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> Result<Option<JsString>, tsr_vfs::Error> {
        if self.can_use_source {
            let source = match self.project_reference_from_output_dts(path) {
                Some(found) => Some(found.clone()),
                None => self.source_to_dts_if_symlink(path, file_name)?,
            };
            if let Some(source) = source {
                return Ok(Some(source.source.clone()));
            }
        } else if let Some(output) = self.project_reference_from_source(path) {
            if !output.output_dts.is_empty() {
                return Ok(Some(output.output_dts.clone()));
            }
        }
        Ok(None)
    }

    /// The published form of getParseFileRedirect.
    pub(crate) fn published_parse_file_redirect(&self, path: &[u8]) -> Option<JsString> {
        if self.can_use_source {
            self.project_reference_from_output_dts(path)
                .cloned()
                .or_else(|| self.recorded_symlink(path))
                .map(|found| found.source.clone())
        } else {
            self.project_reference_from_source(path)
                .filter(|output| !output.output_dts.is_empty())
                .map(|output| output.output_dts.clone())
        }
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getProjectReferenceFromSource
    pub(crate) fn project_reference_from_source(&self, path: &[u8]) -> Option<&Arc<ReferenceFile>> {
        self.source_to_project_reference.get(path)
    }

    /// Only the source-of-reference mode loads a referenced project's sources as
    /// themselves (the pin's isSourceFromProjectReference, a checker helper).
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.isSourceFromProjectReference
    pub(crate) fn is_source_from_project_reference(&self, path: &[u8]) -> bool {
        self.can_use_source && self.project_reference_from_source(path).is_some()
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getProjectReferenceFromOutputDts
    pub(crate) fn project_reference_from_output_dts(
        &self,
        path: &[u8],
    ) -> Option<&Arc<ReferenceFile>> {
        self.output_dts_to_project_reference.get(path)
    }

    /// Program queries after loading: the symlink index answers only what
    /// loading recorded, as at the pin once the mapper's loader is released.
    pub(crate) fn published_redirect_for_resolution(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> (Option<usize>, JsString) {
        match self.redirect_with(path, file_name, |mapper| {
            Ok::<_, Infallible>(mapper.recorded_symlink(path))
        }) {
            Ok(result) => result,
            Err(never) => match never {},
        }
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getRedirectForResolution
    pub(crate) fn redirect_for_resolution(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> Result<(Option<usize>, JsString), tsr_vfs::Error> {
        self.redirect_with(path, file_name, |mapper| {
            mapper.source_to_dts_if_symlink(path, file_name)
        })
    }

    fn redirect_with<E>(
        &self,
        path: &[u8],
        file_name: &[u8],
        symlink: impl FnOnce(&Self) -> Result<Option<Arc<ReferenceFile>>, E>,
    ) -> Result<(Option<usize>, JsString), E> {
        // Check if outputdts of source file from project reference
        if let Some(output) = self.project_reference_from_source(path) {
            return Ok((Some(output.config), output.source.clone()));
        }
        // Source file from project reference
        if let Some(from_dts) = self.project_reference_from_output_dts(path) {
            return Ok((Some(from_dts.config), from_dts.source.clone()));
        }
        if let Some(from_realpath) = symlink(self)? {
            return Ok((Some(from_realpath.config), from_realpath.source.clone()));
        }
        Ok((None, JsString::from_bytes(file_name)))
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getRedirectParsedCommandLineForResolution
    pub(crate) fn redirect_parsed_command_line_for_resolution(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> Result<Option<usize>, tsr_vfs::Error> {
        Ok(self.redirect_for_resolution(path, file_name)?.0)
    }

    /// getCompilerOptionsForFile while loading, where a symlinked output is
    /// looked up through the host.
    pub(crate) fn compiler_options_for_file<'a>(
        &'a self,
        base: &'a CompilerOptions,
        path: &[u8],
        file_name: &[u8],
    ) -> Result<&'a CompilerOptions, tsr_vfs::Error> {
        self.compiler_options_with(base, path, file_name, |mapper| {
            mapper.source_to_dts_if_symlink(path, file_name)
        })
    }

    /// The published form of getCompilerOptionsForFile: a symlinked output is
    /// only what loading recorded.
    pub(crate) fn published_compiler_options_for_file<'a>(
        &'a self,
        base: &'a CompilerOptions,
        path: &[u8],
        file_name: &[u8],
    ) -> &'a CompilerOptions {
        match self.compiler_options_with(base, path, file_name, |mapper| {
            Ok::<_, Infallible>(mapper.recorded_symlink(path))
        }) {
            Ok(options) => options,
            Err(never) => match never {},
        }
    }

    /// The loader's and the published program's lookups share this body. A
    /// parsed reference always has options, so GetCompilerOptionsWithRedirect
    /// takes them whenever there is a redirect.
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getCompilerOptionsForFile
    fn compiler_options_with<'a, E>(
        &'a self,
        base: &'a CompilerOptions,
        path: &[u8],
        file_name: &[u8],
        symlink: impl FnOnce(&Self) -> Result<Option<Arc<ReferenceFile>>, E>,
    ) -> Result<&'a CompilerOptions, E> {
        let redirect = self.redirect_with(path, file_name, symlink)?.0;
        Ok(redirect.map_or(base, |index| &self.configs[index].options))
    }

    fn recorded_symlink(&self, path: &[u8]) -> Option<Arc<ReferenceFile>> {
        if !self.has_project_references {
            return None;
        }
        self.realpath_dts_to_source
            .lock()
            .expect("realpath index poisoned")
            .get(path)
            .cloned()
            .flatten()
    }

    /// If preserveSymlinks is true, module resolution does not jump the symlink,
    /// but its real path may be the output of a referenced project. Only paths
    /// under node_modules are tried, so ordinary files never pay for realpath.
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getSourceToDtsIfSymlink
    pub(crate) fn source_to_dts_if_symlink(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> Result<Option<Arc<ReferenceFile>>, tsr_vfs::Error> {
        if !self.has_project_references {
            // Only a reference parse installs the loader, so nothing was recorded.
            return Ok(None);
        }
        let mut index = self
            .realpath_dts_to_source
            .lock()
            .expect("realpath index poisoned");
        if let Some(recorded) = index.get(path) {
            return Ok(recorded.clone());
        }
        let Some(loader) = self.loader.as_ref().filter(|_| self.preserve_symlinks) else {
            return Ok(None);
        };
        let key = JsString::from_bytes(path);
        if !file_name
            .windows(14)
            .any(|window| window == b"/node_modules/")
        {
            index.insert(key, None);
            return Ok(None);
        }
        let real = path::to_path(
            loader.fs.realpath(file_name)?.as_bytes(),
            loader.cwd.as_bytes(),
            loader.case_sensitive,
        );
        if real.as_bytes() == path {
            index.insert(key, None);
            return Ok(None);
        }
        let found = self
            .project_reference_from_output_dts(real.as_bytes())
            .cloned();
        index.insert(key, found.clone());
        Ok(found)
    }

    /// `root` is the program's own command line, the parent of its references.
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.rangeResolvedProjectReference
    pub(crate) fn range_resolved_project_reference(
        &self,
        root: &ParsedCommandLine,
        f: &mut ReferenceVisitor<'_>,
    ) -> bool {
        if !self.has_project_references {
            return false;
        }
        let mut seen = HashSet::new();
        seen.insert(self.root_config_path.clone());
        let references = self
            .references_in_config_file
            .get(&self.root_config_path)
            .map_or(&[][..], Vec::as_slice);
        self.range_resolved_reference_worker(references, f, root, &mut seen)
    }

    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.getResolvedProjectReferences
    pub(crate) fn resolved_project_references(
        &self,
    ) -> impl Iterator<Item = Option<&ParsedCommandLine>> {
        self.references_in_config_file
            .get(&self.root_config_path)
            .into_iter()
            .flatten()
            .map(|path| {
                self.config_to_project_reference
                    .get(path)
                    .copied()
                    .flatten()
                    .map(|index| self.configs[index].as_ref())
            })
    }

    /// Visits the child's descendants, excluding the child itself, as the
    /// project's tree-loading filter requires.
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.rangeResolvedProjectReferenceInChildConfig
    pub(crate) fn range_resolved_project_reference_in_child_config(
        &self,
        root: &ParsedCommandLine,
        child: &ParsedCommandLine,
        f: &mut ReferenceVisitor<'_>,
    ) -> bool {
        let Some(config_file) = child.config_file.as_ref() else {
            return false;
        };
        let child_path = root_config_path(Some(config_file));
        let mut seen = HashSet::from([child_path.clone()]);
        let references = self
            .references_in_config_file
            .get(&child_path)
            .map_or(&[][..], Vec::as_slice);
        self.range_resolved_reference_worker(references, f, root, &mut seen)
    }

    /// Preorder, visiting each config path once; a child's parent is the
    /// config that references it.
    /// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.rangeResolvedReferenceWorker
    fn range_resolved_reference_worker(
        &self,
        references: &[JsString],
        f: &mut ReferenceVisitor<'_>,
        parent: &ParsedCommandLine,
        seen: &mut HashSet<JsString>,
    ) -> bool {
        for (index, path) in references.iter().enumerate() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let config = self
                .config_to_project_reference
                .get(path)
                .copied()
                .flatten()
                .map(|index| self.configs[index].as_ref());
            if !f(path, config, parent, index) {
                return false;
            }
            let children = self
                .references_in_config_file
                .get(path)
                .map_or(&[][..], Vec::as_slice);
            // A missing config has no references, so its nil never parents.
            if let Some(config) = config {
                if !self.range_resolved_reference_worker(children, f, config, seen) {
                    return false;
                }
            }
        }
        true
    }
}

/// port: tsc/internal/compiler/projectreferencefilemapper.go:projectReferenceFileMapper.rootConfigPath
fn root_config_path(config: Option<&TsConfigSourceFile>) -> JsString {
    config.map_or_else(JsString::default, |config| {
        config
            .file
            .view()
            .source_file(config.root)
            .expect("retained config source")
            .parse_options()
            .path
            .clone()
    })
}

/// source: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParseTask
struct ParseTask {
    config_name: JsString,
    resolved: Option<usize>,
    sub_tasks: Vec<usize>,
}

/// Parses every referenced config once, then publishes the mapper's maps. The
/// pinned work group only schedules config reads, and its observable results do
/// not depend on the schedule, so this runs sequentially in the single-threaded
/// order: tasks are taken last-queued first.
/// source: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParser
pub(crate) struct ProjectReferenceParser<'a> {
    mapper: &'a mut ProjectReferenceFileMapper,
    reference_provider: Option<&'a dyn ResolvedProjectReferenceProvider>,
    tracing: Option<Arc<dyn tsr_checker::TraceSink>>,
    host: CompilerConfigHost,
    case_sensitive: bool,
    tasks: Vec<ParseTask>,
    tasks_by_file_name: BTreeMap<JsString, usize>,
    queue: Vec<usize>,
}

impl<'a> ProjectReferenceParser<'a> {
    pub(crate) fn new(
        mapper: &'a mut ProjectReferenceFileMapper,
        fs: Arc<dyn FileSystem>,
        cwd: JsString,
        allow_live_host: bool,
    ) -> Self {
        let case_sensitive = fs.use_case_sensitive_file_names();
        Self {
            mapper,
            reference_provider: None,
            tracing: None,
            host: if allow_live_host {
                CompilerConfigHost::new_live(fs, cwd)
            } else {
                CompilerConfigHost::new(fs, cwd)
            },
            case_sensitive,
            tasks: Vec::new(),
            tasks_by_file_name: BTreeMap::new(),
            queue: Vec::new(),
        }
    }

    pub(crate) fn with_tracing(mut self, tracing: Option<Arc<dyn tsr_checker::TraceSink>>) -> Self {
        self.tracing = tracing;
        self
    }

    pub(crate) fn with_reference_provider(
        mut self,
        provider: Option<&'a dyn ResolvedProjectReferenceProvider>,
    ) -> Self {
        self.reference_provider = provider;
        self
    }

    fn to_path(&self, file_name: &[u8]) -> JsString {
        path::to_path(file_name, self.host.cwd.as_bytes(), self.case_sensitive)
    }

    /// port: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParser.parse
    pub(crate) fn parse(
        mut self,
        references: &[JsString],
        root_config_file: Option<&Arc<TsConfigSourceFile>>,
    ) -> Result<(), Error> {
        self.mapper.loader = Some(LoaderHost {
            fs: self.host.fs.clone(),
            cwd: self.host.cwd.clone(),
            case_sensitive: self.case_sensitive,
        });
        let mut roots = create_project_reference_parse_tasks(&mut self.tasks, references);
        self.start(&mut roots);
        while let Some(task) = self.queue.pop() {
            self.parse_task(task)?;
            let mut sub_tasks = std::mem::take(&mut self.tasks[task].sub_tasks);
            self.start(&mut sub_tasks);
            self.tasks[task].sub_tasks = sub_tasks;
        }
        self.init_mapper(&roots, root_config_file);
        Ok(())
    }

    /// Duplicate paths reuse the first task, so file order does not depend on
    /// which task would be started first.
    /// port: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParser.start
    fn start(&mut self, tasks: &mut [usize]) {
        for slot in tasks {
            let path = self.to_path(self.tasks[*slot].config_name.as_bytes());
            if let Some(&loaded) = self.tasks_by_file_name.get(&path) {
                *slot = loaded;
            } else {
                self.tasks_by_file_name.insert(path, *slot);
                self.queue.push(*slot);
            }
        }
    }

    /// port: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParseTask.parse
    fn parse_task(&mut self, task: usize) -> Result<(), Error> {
        let name = self.tasks[task].config_name.clone();
        let path = self.to_path(name.as_bytes());
        let _trace = tsr_checker::TraceScope::new(
            self.tracing.as_ref(),
            tsr_checker::TracePhase::Parse,
            "parseJsonSourceFileConfigFileContent",
            || {
                [(
                    "path".into(),
                    tsr_checker::TraceValue::Str(
                        String::from_utf8_lossy(name.as_bytes()).into_owned(),
                    ),
                )]
                .into_iter()
                .collect()
            },
            false,
        );
        let resolved = if let Some(provider) = self.reference_provider {
            provider.get_resolved_project_reference(name.as_bytes(), path.as_bytes())
        } else {
            get_resolved_project_reference(&self.host, name.as_bytes(), path)?.map(|mut config| {
                config.parse_input_output_names();
                Arc::new(config)
            })
        };
        let Some(resolved) = resolved else {
            return Ok(());
        };
        let sub_references = resolved.resolved_project_reference_paths().to_vec();
        let index = self.mapper.configs.len();
        self.mapper.configs.push(resolved);
        self.tasks[task].resolved = Some(index);
        if !sub_references.is_empty() {
            self.tasks[task].sub_tasks =
                create_project_reference_parse_tasks(&mut self.tasks, &sub_references);
        }
        Ok(())
    }

    /// port: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParser.initMapper
    fn init_mapper(&mut self, roots: &[usize], root_config_file: Option<&Arc<TsConfigSourceFile>>) {
        let mut seen = HashSet::new();
        let references = self.init_mapper_worker(roots, &mut seen, root_config_file);
        let root = self.mapper.root_config_path.clone();
        self.mapper
            .references_in_config_file
            .insert(root, references);
    }

    /// Parent maps are copied before children, so a child overwrites its
    /// parent's entries. The root's own config is skipped by identity: a
    /// reference that reparses the root file is a different config and maps.
    /// port: tsc/internal/compiler/projectreferenceparser.go:projectReferenceParser.initMapperWorker
    fn init_mapper_worker(
        &mut self,
        tasks: &[usize],
        seen: &mut HashSet<usize>,
        root_config_file: Option<&Arc<TsConfigSourceFile>>,
    ) -> Vec<JsString> {
        let mut results = Vec::with_capacity(tasks.len());
        for &task in tasks {
            let path = self.to_path(self.tasks[task].config_name.as_bytes());
            results.push(path.clone());
            // ensure we only walk each task once
            if !seen.insert(task) {
                continue;
            }
            let resolved = self.tasks[task].resolved;
            self.mapper
                .config_to_project_reference
                .insert(path.clone(), resolved);
            if let Some(index) = resolved {
                let config = self.mapper.configs[index].clone();
                let is_root = match (root_config_file, config.config_file.as_ref()) {
                    (Some(root), Some(file)) => Arc::ptr_eq(root, file),
                    (None, None) => true,
                    _ => false,
                };
                if !is_root {
                    for (key, entry) in config.source_to_project_reference() {
                        self.mapper
                            .source_to_project_reference
                            .insert(key.clone(), reference_file(&entry, index));
                    }
                    for (key, entry) in config.output_dts_to_project_reference() {
                        Arc::get_mut(&mut self.mapper.output_dts_to_project_reference)
                            .expect("reference outputs are private until parsing finishes")
                            .insert(key.clone(), reference_file(&entry, index));
                    }
                    if self.mapper.can_use_source {
                        let dir = if config.options.declaration_dir.is_empty() {
                            &config.options.out_dir
                        } else {
                            &config.options.declaration_dir
                        };
                        if !dir.is_empty() {
                            let directory = self.to_path(dir.as_bytes()).into();
                            Arc::get_mut(&mut self.mapper.dts_directories)
                                .expect(
                                    "declaration directories are private until parsing finishes",
                                )
                                .insert(directory);
                        }
                    }
                }
            }
            let sub_tasks = self.tasks[task].sub_tasks.clone();
            let references = self.init_mapper_worker(&sub_tasks, seen, root_config_file);
            self.mapper
                .references_in_config_file
                .insert(path, references);
        }
        results
    }
}

fn reference_file(
    entry: &tsr_tsoptions::SourceOutputAndProjectReference<'_>,
    config: usize,
) -> Arc<ReferenceFile> {
    Arc::new(ReferenceFile {
        source: entry.names.source.clone(),
        output_dts: entry.names.output_dts.clone(),
        config,
    })
}

/// port: tsc/internal/compiler/projectreferenceparser.go:createProjectReferenceParseTasks
fn create_project_reference_parse_tasks(
    tasks: &mut Vec<ParseTask>,
    project_references: &[JsString],
) -> Vec<usize> {
    project_references
        .iter()
        .map(|config_name| {
            tasks.push(ParseTask {
                config_name: config_name.clone(),
                resolved: None,
                sub_tasks: Vec::new(),
            });
            tasks.len() - 1
        })
        .collect()
}
