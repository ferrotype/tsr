//! Automatic type acquisition. Discovery borrows a project snapshot; npm and
//! post-install resolution use the installer's retained live filesystem.
//! Host callbacks run without cache locks. A host must not reenter the same
//! installer while it is initializing; this is detected before waiting.
mod discovery;
mod npm;
mod types_map;
mod validation;

pub use discovery::{
    discover_typings, is_typing_up_to_date, remove_min_and_version_numbers, DiscoveredTypings,
};
pub use npm::{install_npm_packages, NpmError, NpmExecutor, NpmThrottle};
pub use validation::{
    render_package_name_validation_failure, validate_package_name, NameValidationResult,
};

use crate::logging::Logger;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicI32, Ordering},
    Arc, Condvar, Mutex,
};
use tsr_core::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsr_ipc::{Context, ContextError};
use tsr_json::{Decode, Decoder, Kind, RawValue, Token};
use tsr_jsstring::JsString;
use tsr_module::{Resolver, ResolverOptions};
use tsr_semver::Version;
use tsr_tsoptions::TypeAcquisition;
use tsr_tspath as path;
use tsr_vfs::FileSystem;

fn js(value: impl AsRef<[u8]>) -> JsString {
    JsString::from_bytes(value.as_ref())
}
fn display(value: &JsString) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(value.as_bytes())
}
struct DisplayList<'a>(&'a [JsString]);
impl std::fmt::Display for DisplayList<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[")?;
        for (index, item) in self.0.iter().enumerate() {
            if index != 0 {
                f.write_str(" ")?;
            }
            write!(f, "{}", display(item))?;
        }
        f.write_str("]")
    }
}

pub type TypesRegistry = BTreeMap<JsString, BTreeMap<JsString, JsString>>;

#[derive(Clone, Debug, Default)]
pub struct TypingsInfo {
    pub type_acquisition: TypeAcquisition,
    pub compiler_options: Arc<CompilerOptions>,
    pub unresolved_imports: BTreeSet<JsString>,
}
impl TypingsInfo {
    // port: tsc/internal/project/ata/ata.go:TypingsInfo.Equals
    pub fn equals(&self, other: &Self) -> bool {
        TypeAcquisition::equals(Some(&self.type_acquisition), Some(&other.type_acquisition))
            && self.compiler_options.allow_js() == other.compiler_options.allow_js()
            && self.unresolved_imports == other.unresolved_imports
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedTyping {
    pub typings_location: JsString,
    pub version: Option<Version>,
}

pub struct TypingsInstallerOptions {
    pub typings_location: JsString,
    pub throttle_limit: usize,
}

pub struct TypingsInstallRequest<'a> {
    pub project_id: &'a [u8],
    pub typings_info: &'a TypingsInfo,
    pub file_names: &'a [JsString],
    pub project_root_path: &'a [u8],
    pub fs: &'a dyn FileSystem,
    pub logger: &'a Logger,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypingsInstallResult {
    pub typings_files: Vec<JsString>,
    pub files_to_watch: Vec<JsString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtaError {
    Canceled(ContextError),
    Npm(NpmError),
    FileSystem(tsr_vfs::Error),
    Resolution(tsr_module::Error),
    Reentry,
}
impl std::fmt::Display for AtaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canceled(e) => e.fmt(f),
            Self::Npm(_) => f.write_str("npm install failed"),
            Self::FileSystem(e) => e.fmt(f),
            Self::Resolution(e) => e.fmt(f),
            Self::Reentry => f.write_str("typings installer initialization reentry"),
        }
    }
}
impl std::error::Error for AtaError {}
impl From<tsr_vfs::Error> for AtaError {
    fn from(e: tsr_vfs::Error) -> Self {
        Self::FileSystem(e)
    }
}
impl From<tsr_module::Error> for AtaError {
    fn from(e: tsr_module::Error) -> Self {
        Self::Resolution(e)
    }
}
fn check_context(context: &Context) -> Result<(), AtaError> {
    context
        .err()
        .map_or(Ok(()), |error| Err(AtaError::Canceled(error)))
}

#[derive(Default)]
struct Cache {
    locations: BTreeMap<JsString, CachedTyping>,
    missing: BTreeSet<JsString>,
}
#[derive(Default)]
enum Initialization {
    #[default]
    Empty,
    Loading(std::thread::ThreadId),
    Ready(Arc<TypesRegistry>),
}

pub struct TypingsInstaller {
    typings_location: JsString,
    fs: Arc<dyn FileSystem>,
    npm: Arc<dyn NpmExecutor>,
    init: Mutex<Initialization>,
    initialized: Condvar,
    cache: Mutex<Cache>,
    install_run_count: AtomicI32,
    throttle: NpmThrottle,
}

impl TypingsInstaller {
    // port: tsc/internal/project/ata/ata.go:NewTypingsInstaller
    pub fn new(
        options: TypingsInstallerOptions,
        fs: Arc<dyn FileSystem>,
        npm: Arc<dyn NpmExecutor>,
    ) -> Self {
        Self {
            typings_location: options.typings_location,
            fs,
            npm,
            init: Mutex::new(Initialization::Empty),
            initialized: Condvar::new(),
            cache: Mutex::new(Cache::default()),
            install_run_count: AtomicI32::new(0),
            throttle: NpmThrottle::new(options.throttle_limit),
        }
    }

    pub fn typings_location(&self) -> &JsString {
        &self.typings_location
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.IsKnownTypesPackageName
    pub fn is_known_types_package_name(
        &self,
        context: &Context,
        name: &[u8],
        fs: &dyn FileSystem,
        logger: &Logger,
    ) -> Result<bool, AtaError> {
        if validate_package_name(name).0 != NameValidationResult::NameOk {
            return Ok(false);
        }
        Ok(self
            .initialize(context, fs, logger)?
            .contains_key(&JsString::from_bytes(name)))
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.InstallTypings
    pub fn install_typings(
        &self,
        context: &Context,
        request: &TypingsInstallRequest<'_>,
    ) -> Result<TypingsInstallResult, AtaError> {
        let mut result = self.discover_and_install_typings(context, request)?;
        result.typings_files.sort();
        result.files_to_watch.sort();
        request.logger.log(format_args!(
            "ATA:: Got install request for: {}",
            String::from_utf8_lossy(request.project_id)
        ));
        Ok(result)
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.discoverAndInstallTypings
    fn discover_and_install_typings(
        &self,
        context: &Context,
        request: &TypingsInstallRequest<'_>,
    ) -> Result<TypingsInstallResult, AtaError> {
        let registry = self.initialize(context, request.fs, request.logger)?;
        let cached = self
            .cache
            .lock()
            .expect("typings cache lock")
            .locations
            .clone();
        let discovered = discover_typings(
            request.fs,
            request.logger,
            request.typings_info,
            request.file_names,
            request.project_root_path,
            &cached,
            &registry,
        )?;
        check_context(context)?;
        let request_id = self
            .install_run_count
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut result = TypingsInstallResult {
            typings_files: discovered.cached_typing_paths,
            files_to_watch: discovered.files_to_watch,
        };
        if discovered.new_typing_names.is_empty() {
            request.logger.log(format_args!(
                "ATA:: No new typings were requested as a result of typings discovery"
            ));
        } else {
            let filtered =
                self.filter_typings(request.logger, &discovered.new_typing_names, &registry);
            if !filtered.is_empty() {
                self.install_filtered_typings(
                    context,
                    request_id,
                    request.logger,
                    &filtered,
                    &registry,
                    &mut result.typings_files,
                )?;
                return Ok(result);
            }
            request.logger.log(format_args!("ATA:: All typings are known to be missing or invalid - no need to install more typings"));
        }
        Ok(result)
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.filterTypings
    fn filter_typings(
        &self,
        logger: &Logger,
        names: &[JsString],
        registry: &TypesRegistry,
    ) -> Vec<JsString> {
        let mut result = Vec::new();
        for name in names {
            let name_display = display(name);
            let key = JsString::from_bytes(tsr_module::mangle_scoped_package_name(name.as_bytes()));
            let key_display = display(&key);
            if self
                .cache
                .lock()
                .expect("typings cache lock")
                .missing
                .contains(&key)
            {
                logger.log(format_args!(
                    "ATA:: '{name_display}':: '{key_display}' is in missingTypingsSet - skipping..."
                ));
                continue;
            }
            let (validation, part, is_scope) = validate_package_name(name.as_bytes());
            if validation != NameValidationResult::NameOk {
                self.cache
                    .lock()
                    .expect("typings cache lock")
                    .missing
                    .insert(key);
                logger.log(format_args!(
                    "ATA:: {}",
                    render_package_name_validation_failure(
                        name.as_bytes(),
                        validation,
                        part,
                        is_scope
                    )
                ));
                continue;
            }
            let Some(versions) = registry.get(&key) else {
                logger.log(format_args!("ATA:: '{name_display}':: Entry for package '{key_display}' does not exist in local types registry - skipping..."));
                continue;
            };
            if self
                .cache
                .lock()
                .expect("typings cache lock")
                .locations
                .get(&key)
                .is_some_and(|cached| is_typing_up_to_date(cached, versions))
            {
                logger.log(format_args!("ATA:: '{name_display}':: '{key_display}' already has an up-to-date typing - skipping..."));
                continue;
            }
            result.push(key);
        }
        result
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.installTypings
    fn install_filtered_typings(
        &self,
        context: &Context,
        request_id: i32,
        logger: &Logger,
        filtered: &[JsString],
        registry: &TypesRegistry,
        files: &mut Vec<JsString>,
    ) -> Result<(), AtaError> {
        let scoped: Vec<_> = filtered
            .iter()
            .map(|name| JsString::from_bytes([b"@types/", name.as_bytes(), b"@latest"].concat()))
            .collect();
        match self.install_worker(context, request_id, &scoped, logger) {
            Ok(()) => {}
            Err(AtaError::Canceled(error)) => return Err(AtaError::Canceled(error)),
            Err(error) => {
                logger.log(format_args!("ATA:: install request failed, marking packages as missing to prevent repeated requests: {}", DisplayList(filtered)));
                self.cache
                    .lock()
                    .expect("typings cache lock")
                    .missing
                    .extend(filtered.iter().cloned());
                return Err(error);
            }
        }
        check_context(context)?;
        logger.log(format_args!(
            "ATA:: Installed typings {}",
            DisplayList(&scoped)
        ));
        let mut resolver = self.resolver()?;
        let mut installed = Vec::new();
        for name in filtered {
            check_context(context)?;
            let location = self.typing_to_file_name(&mut resolver, name.as_bytes())?;
            if location.is_empty() {
                logger.log(format_args!(
                    "ATA:: Failed to find typing file for package '{}'",
                    display(name)
                ));
                self.cache
                    .lock()
                    .expect("typings cache lock")
                    .missing
                    .insert(name.clone());
                continue;
            }
            let version = discovery::registry_version(
                registry.get(name).expect("filter requires registry entry"),
            );
            self.cache
                .lock()
                .expect("typings cache lock")
                .locations
                .insert(
                    name.clone(),
                    CachedTyping {
                        typings_location: location.clone(),
                        version: Some(version),
                    },
                );
            installed.push(location);
        }
        logger.log(format_args!(
            "ATA:: Installed typing files {}",
            DisplayList(&installed)
        ));
        files.extend(installed);
        Ok(())
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.installWorker
    fn install_worker(
        &self,
        context: &Context,
        request_id: i32,
        packages: &[JsString],
        logger: &Logger,
    ) -> Result<(), AtaError> {
        logger.log(format_args!(
            "ATA:: #{request_id} with cwd: {} arguments: {}",
            display(&self.typings_location),
            DisplayList(packages)
        ));
        let result = install_npm_packages(context, packages, &self.throttle, &|packages| {
            let mut args = vec![js("install"), js("--ignore-scripts")];
            args.extend_from_slice(packages);
            args.push(js("--save-dev"));
            args.push(js(format!(
                "--user-agent=\"typesInstaller/{}\"",
                tsr_core::version()
            )));
            let result = self
                .npm
                .npm_install(context, self.typings_location.as_bytes(), &args);
            if let Err(error) = &result {
                logger.log(format_args!(
                    "ATA:: Output is: {}",
                    String::from_utf8_lossy(&error.output)
                ));
            }
            check_context(context)?;
            result.map(|_| ()).map_err(AtaError::Npm)
        });
        logger.log(format_args!("TI:: npm install #{request_id} completed"));
        result
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.init
    fn initialize(
        &self,
        context: &Context,
        fs: &dyn FileSystem,
        logger: &Logger,
    ) -> Result<Arc<TypesRegistry>, AtaError> {
        let thread = std::thread::current().id();
        let mut state = self.init.lock().expect("ATA initialization lock");
        loop {
            check_context(context)?;
            match &*state {
                Initialization::Ready(registry) => return Ok(registry.clone()),
                Initialization::Loading(owner) if *owner == thread => {
                    return Err(AtaError::Reentry);
                }
                Initialization::Loading(_) => {
                    state = self
                        .initialized
                        .wait_timeout(state, std::time::Duration::from_millis(20))
                        .expect("ATA initialization lock")
                        .0;
                }
                Initialization::Empty => {
                    *state = Initialization::Loading(thread);
                    break;
                }
            }
        }
        drop(state);
        let guard = Initializing(self);
        logger.log(format_args!(
            "ATA:: Global cache location '{}'",
            display(&self.typings_location)
        ));
        self.process_cache_location(fs, logger)?;
        self.ensure_typings_location_exists(fs, logger);
        check_context(context)?;
        logger.log(format_args!(
            "ATA:: Updating types-registry@latest npm package..."
        ));
        let args = [
            js("install"),
            js("--ignore-scripts"),
            js("types-registry@latest"),
        ];
        match self
            .npm
            .npm_install(context, self.typings_location.as_bytes(), &args)
        {
            Ok(_) => logger.log(format_args!("ATA:: Updated types-registry npm package")),
            Err(error) => logger.log(format_args!(
                "ATA:: Error updating types-registry package: {error}"
            )),
        }
        check_context(context)?;
        // npm has just mutated the live host; discovery's snapshot may predate it.
        let registry = Arc::new(self.load_types_registry_file(self.fs.as_ref(), logger));
        *self.init.lock().expect("ATA initialization lock") =
            Initialization::Ready(registry.clone());
        drop(guard);
        Ok(registry)
    }

    fn resolver(&self) -> Result<Resolver, AtaError> {
        Ok(Resolver::with_options(
            self.fs.clone(),
            Arc::new(CompilerOptions {
                module_resolution: ModuleResolutionKind::NODE_NEXT,
                ..Default::default()
            }),
            b"",
            ResolverOptions {
                allow_live_host: true,
                ..Default::default()
            },
        )?)
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.processCacheLocation
    fn process_cache_location(&self, fs: &dyn FileSystem, logger: &Logger) -> Result<(), AtaError> {
        logger.log(format_args!(
            "ATA:: Processing cache location {}",
            display(&self.typings_location)
        ));
        let config_path = path::combine(self.typings_location.as_bytes(), &[b"package.json"]);
        let lock_path = path::combine(self.typings_location.as_bytes(), &[b"package-lock.json"]);
        logger.log(format_args!(
            "ATA:: Trying to find '{}'...",
            String::from_utf8_lossy(&config_path)
        ));
        if fs.file_exists(&config_path)? && fs.file_exists(&lock_path)? {
            let mut config = NpmConfig::default();
            parse_npm_config_or_lock(fs, logger, &config_path, &mut config)?;
            let mut lock = NpmLock::default();
            parse_npm_config_or_lock(fs, logger, &lock_path, &mut lock)?;
            let mut resolver = self.resolver()?;
            for key in config.dev_dependencies.0.keys() {
                let package_key = JsString::from_bytes([b"node_modules/", key.as_bytes()].concat());
                let Some(entry) = lock
                    .packages
                    .0
                    .get(&package_key)
                    .or_else(|| lock.dependencies.0.get(key))
                else {
                    continue;
                };
                let name = JsString::from_bytes(path::base_name(key.as_bytes()));
                if name.is_empty() {
                    continue;
                }
                let location = self.typing_to_file_name(&mut resolver, name.as_bytes())?;
                if location.is_empty() {
                    self.cache
                        .lock()
                        .expect("typings cache lock")
                        .missing
                        .insert(name);
                    continue;
                }
                let existing = self
                    .cache
                    .lock()
                    .expect("typings cache lock")
                    .locations
                    .get(&name)
                    .cloned();
                if let Some(existing) = existing {
                    if existing.typings_location == location {
                        continue;
                    }
                    logger.log(format_args!("ATA:: New typing for package {} from {} conflicts with existing typing file {}", display(&name), display(&location), display(&existing.typings_location)));
                }
                logger.log(format_args!(
                    "ATA:: Adding entry into typings cache: {} => {}",
                    display(&name),
                    display(&location)
                ));
                if !entry.version.is_empty() {
                    let version = Version::must_parse(entry.version.as_bytes());
                    self.cache
                        .lock()
                        .expect("typings cache lock")
                        .locations
                        .insert(
                            name,
                            CachedTyping {
                                typings_location: location,
                                version: Some(version),
                            },
                        );
                }
            }
        }
        logger.log(format_args!(
            "ATA:: Finished processing cache location {}",
            display(&self.typings_location)
        ));
        Ok(())
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.ensureTypingsLocationExists
    fn ensure_typings_location_exists(&self, fs: &dyn FileSystem, logger: &Logger) {
        let config = path::combine(self.typings_location.as_bytes(), &[b"package.json"]);
        logger.log(format_args!(
            "ATA:: Npm config file: {}",
            String::from_utf8_lossy(&config)
        ));
        if !fs.file_exists(&config).unwrap_or(false) {
            logger.log(format_args!(
                "ATA:: Npm config file: '{}' is missing, creating new one...",
                String::from_utf8_lossy(&config)
            ));
            if let Err(error) = self.fs.write_file(&config, b"{ \"private\": true }") {
                logger.log(format_args!("ATA:: Npm config file write failed: {error}"));
            }
        }
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.typingToFileName
    fn typing_to_file_name(
        &self,
        resolver: &mut Resolver,
        name: &[u8],
    ) -> Result<JsString, AtaError> {
        let containing = path::combine(self.typings_location.as_bytes(), &[b"index.d.ts"]);
        Ok(resolver
            .resolve(name, &containing, ModuleKind::NONE)?
            .resolved_file_name
            .clone())
    }

    // port: tsc/internal/project/ata/ata.go:TypingsInstaller.loadTypesRegistryFile
    fn load_types_registry_file(&self, fs: &dyn FileSystem, logger: &Logger) -> TypesRegistry {
        let file = path::combine(
            self.typings_location.as_bytes(),
            &[b"node_modules/types-registry/index.json"],
        );
        if let Ok(Some(content)) = fs.read_file(&file) {
            let mut entries = JsonMap::<JsonMap<JsonMap<JsString>>>::default();
            match tsr_json::unmarshal(
                content.text.as_bytes(),
                &mut entries,
                tsr_json::Options::default(),
            ) {
                Ok(()) => {
                    if let Some(entries) = entries.0.remove(&js("entries")) {
                        return entries
                            .0
                            .into_iter()
                            .map(|(key, value)| (key, value.0))
                            .collect();
                    }
                }
                Err(error) => logger.log(format_args!(
                    "ATA:: Error when loading types registry file '{}': {error}",
                    String::from_utf8_lossy(&file)
                )),
            }
        } else {
            logger.log(format_args!(
                "ATA:: Error reading types registry file '{}'",
                String::from_utf8_lossy(&file)
            ));
        }
        BTreeMap::new()
    }
}

struct Initializing<'a>(&'a TypingsInstaller);
impl Drop for Initializing<'_> {
    fn drop(&mut self) {
        let mut state = self.0.init.lock().expect("ATA initialization lock");
        if matches!(*state, Initialization::Loading(_)) {
            *state = Initialization::Empty;
        }
        drop(state);
        self.0.initialized.notify_all();
    }
}

#[derive(Default)]
struct JsonMap<T>(BTreeMap<JsString, T>);
impl<T: Decode + Default> Decode for JsonMap<T> {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            self.0.clear();
            return Ok(());
        }
        if input.read_token()? != Token::BeginObject {
            return Err(tsr_json::Error::Message("expected object".into()));
        }
        while input.peek_kind() != Kind::EndObject {
            let Token::String(name) = input.read_token()? else {
                return Err(tsr_json::Error::Message("expected object key".into()));
            };
            let mut value = T::default();
            input.value(&mut value)?;
            self.0.insert(JsString::from_bytes(name), value);
        }
        input.read_token()?;
        Ok(())
    }
}
#[derive(Default)]
struct NpmConfig {
    dev_dependencies: JsonMap<RawValue>,
}
impl Decode for NpmConfig {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| {
            if name == b"devDependencies" {
                input.value(&mut self.dev_dependencies)
            } else {
                input.skip_value()
            }
        })
    }
}
#[derive(Default)]
struct DependencyEntry {
    version: JsString,
}
impl Decode for DependencyEntry {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| {
            if name == b"version" {
                input.value(&mut self.version)
            } else {
                input.skip_value()
            }
        })
    }
}
#[derive(Default)]
struct NpmLock {
    dependencies: JsonMap<DependencyEntry>,
    packages: JsonMap<DependencyEntry>,
}
impl Decode for NpmLock {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| match name {
            b"dependencies" => input.value(&mut self.dependencies),
            b"packages" => input.value(&mut self.packages),
            _ => input.skip_value(),
        })
    }
}

// port: tsc/internal/project/ata/ata.go:parseNpmConfigOrLock
fn parse_npm_config_or_lock(
    fs: &dyn FileSystem,
    logger: &Logger,
    location: &[u8],
    config: &mut impl Decode,
) -> Result<(), tsr_vfs::Error> {
    if let Some(content) = fs.read_file(location)? {
        let _ = tsr_json::unmarshal(
            content.text.as_bytes(),
            config,
            tsr_json::Options::default(),
        );
        logger.log(format_args!(
            "ATA:: Loaded content of {}: {}",
            String::from_utf8_lossy(location),
            String::from_utf8_lossy(content.text.as_bytes())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
