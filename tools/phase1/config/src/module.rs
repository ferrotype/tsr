//! The `module/moduleResolution` group: the Rust side of the F3a module
//! resolution schedule.
//!
//! It calls `tsr_module`'s production entry points and records what they
//! answer. It never emulates a missing algorithm to make a comparison run, and
//! it never reads an expected value: where the port has no reachable entry
//! point for an action the case needs, the whole case is recorded as a gap
//! naming the pinned operation, the signature the port would need, and the
//! Rust file that does not have it.
//!
//! Row shape mirrors `tools/phase1/config/module_probe_test.go` exactly,
//! because the comparison is a byte comparison of canonicalised JSON. Ordered
//! data travels as arrays and nothing nested inside an ordered element is a
//! multi-key object, so a trace callback is `[code, [arg, ...]]` and a result
//! is an entry array `[["field", value], ...]`. Byte strings are hex encoded
//! on both sides, because a path is not required to be valid UTF-8 and
//! `JsString` carries bytes.
//!
//! Standalone resolver traces explicitly use a live native-style test host.
//! The production default constructor still requires an immutable snapshot.
//! Redirects, entrypoint discovery and typings-location passes are the remaining
//! recorded gaps; cache mutations and trace toggling call production APIs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde_json::{json, Map, Value};
use tsr_core::{CompilerOptions, ModuleKind};
use tsr_jsstring::JsString;
use tsr_module::{
    get_conditions, get_types_package_name, is_applicable_versioned_types_key,
    mangle_scoped_package_name, resolution_diagnostic, PackageId, ResolvedModule,
    ResolvedTypeReferenceDirective, Resolver, TraceArg,
};
use tsr_vfs::{Entries, Error as VfsError, FileContent, FileInfo, FileSystem, SnapshotId};

use crate::api::{self, Outcome};

/// The case prefix this group owns. The schedule is shared with every other
/// config group and a subject string could collide with a neighbour's, so
/// ownership is keyed on the case id this group was assigned.
const CASE_PREFIX: &str = "config/module/";

/// Every operation a case in this group names when the port cannot run it,
/// with the pinned body that is its authority, the signature the port would
/// need and the Rust home that does not have it.
/// Keyed by the request's own operation id, so a recorded gap names the
/// operation the case was written for rather than the subject it shares.
///
/// Each row was read against `crates/tsr_module` in this session; a row that
/// says "present but not a counterpart" names the Rust function that was read
/// and rejected, so the record cannot be mistaken for "nobody looked".
const MISSING: &[(&str, &str, &str, &str)] = &[(
    "tsc/internal/module/resolver.go:Resolver.tryResolveFromTypingsLocation",
    "tsc/internal/module/resolver.go:339-366, called unconditionally from ResolveModuleName \
         at :324",
    "Resolver::new taking a typings location, a project name and extra extensions, plus the \
         extra resolution pass that runs after an ordinary resolution failed to land on a \
         TypeScript or JSON extension and that announces itself with the project name \
         (resolver.go:339-366). crates/tsr_module/src/resolver.rs:90-104 has no such fields, so \
         the pass never runs, the Auto_discovery_for_typings line is never written, and \
         getPackageScopeForPath walks to the filesystem root where the pin stops at the global \
         cache (resolver.go:495-504 vs resolver.rs:206-210)",
    "crates/tsr_module/src/resolver.rs:90 (no typings location, project name or extra \
         extensions on the resolver)",
)];

fn gap(request: &Value) -> Outcome {
    let requested = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match MISSING
        .iter()
        .find(|(identity, _, _, _)| *identity == requested)
    {
        Some((identity, authority, signature, home)) => {
            Outcome::missing(*identity, authority, signature, home)
        }
        None => Outcome::Failed(format!(
            "the module group cannot run case operation {requested:?} and has no reviewed \
             missing-operation record for it"
        )),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// Counts what the resolver asks of the filesystem, so an action can report
/// whether it touched the host at all. Only the trait methods the resolver
/// calls are counted; `MemorySnapshot` answers file_exists and
/// directory_exists through its own `stat`, which is below this wrapper and is
/// therefore not counted twice.
struct Counting {
    inner: RwLock<Arc<dyn FileSystem>>,
    files: RwLock<BTreeMap<Vec<u8>, tsr_vfs::vfstest::InputFile>>,
    calls: AtomicUsize,
}

impl Counting {
    fn hit(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl FileSystem for Counting {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner
            .read()
            .expect("fixture host lock")
            .use_case_sensitive_file_names()
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, VfsError> {
        self.hit();
        self.inner
            .read()
            .expect("fixture host lock")
            .read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, VfsError> {
        self.hit();
        self.inner.read().expect("fixture host lock").stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, VfsError> {
        self.hit();
        self.inner.read().expect("fixture host lock").entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, VfsError> {
        self.hit();
        self.inner.read().expect("fixture host lock").realpath(path)
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, VfsError> {
        self.hit();
        self.inner
            .read()
            .expect("fixture host lock")
            .file_exists(path)
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, VfsError> {
        self.hit();
        self.inner
            .read()
            .expect("fixture host lock")
            .directory_exists(path)
    }
}

struct State {
    resolver: Option<Resolver>,
    host: Option<Arc<Counting>>,
}

/// A group-internal failure: a malformed action, a missing required field, or
/// an action this group does not serve. It is never a semantic result.
type Bad = String;

fn field<'a>(action: &'a Value, name: &str) -> Result<&'a Value, Bad> {
    action
        .get(name)
        .filter(|value| !value.is_null())
        .ok_or_else(|| format!("action is missing required field {name:?}"))
}

fn text<'a>(action: &'a Value, name: &str) -> Result<&'a str, Bad> {
    field(action, name)?
        .as_str()
        .ok_or_else(|| format!("field {name:?} is not a string"))
}

fn flag(action: &Value, name: &str) -> Result<bool, Bad> {
    field(action, name)?
        .as_bool()
        .ok_or_else(|| format!("field {name:?} is not a boolean"))
}

fn list<'a>(action: &'a Value, name: &str) -> Result<&'a Vec<Value>, Bad> {
    field(action, name)?
        .as_array()
        .ok_or_else(|| format!("field {name:?} is not an array"))
}

fn mode(action: &Value) -> Result<ModuleKind, Bad> {
    let raw = field(action, "mode")?
        .as_i64()
        .ok_or("field \"mode\" is not an integer")?;
    Ok(ModuleKind(
        i32::try_from(raw).map_err(|_| "mode does not fit in an i32".to_owned())?,
    ))
}

fn options(value: &Value) -> Result<CompilerOptions, Bad> {
    tsr_tsoptions::raw::compiler_options(value)
        .map_err(|error| format!("compiler options do not decode: {error:?}"))
}

fn strings(action: &Value, name: &str) -> Result<Vec<Vec<u8>>, Bad> {
    list(action, name)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|text| text.as_bytes().to_vec())
                .ok_or_else(|| format!("field {name:?} carries a non-string element"))
        })
        .collect()
}

fn trace_rows(traces: Vec<tsr_module::DiagAndArgs>) -> Value {
    Value::Array(
        traces
            .into_iter()
            .map(|trace| {
                let args: Vec<Value> = trace
                    .args
                    .into_iter()
                    .map(|arg| match arg {
                        TraceArg::Text(value) => json!({ "text_hex": hex(value.as_bytes()) }),
                        TraceArg::Bool(value) => json!({ "bool": value }),
                    })
                    .collect();
                json!([trace.message.code, args])
            })
            .collect(),
    )
}

fn diagnostic_codes(diagnostics: &[tsr_ast::Diagnostic]) -> Value {
    Value::Array(
        diagnostics
            .iter()
            .map(|diagnostic| json!(diagnostic.code))
            .collect(),
    )
}

fn package_rows(id: &PackageId, rows: &mut Vec<Value>) {
    rows.push(json!(["package_id_name_hex", hex(id.name.as_bytes())]));
    rows.push(json!([
        "package_id_sub_module_name_hex",
        hex(id.sub_module_name.as_bytes())
    ]));
    rows.push(json!([
        "package_id_version_hex",
        hex(id.version.as_bytes())
    ]));
    rows.push(json!([
        "package_id_peer_dependencies_hex",
        hex(id.peer_dependencies.as_bytes())
    ]));
}

fn module_rows(resolved: Option<&ResolvedModule>) -> Value {
    let Some(resolved) = resolved else {
        return json!([["nil_result", true]]);
    };
    let mut rows = vec![
        json!(["nil_result", false]),
        json!(["is_resolved", resolved.is_resolved()]),
        json!([
            "resolved_file_name_hex",
            hex(resolved.resolved_file_name.as_bytes())
        ]),
        json!(["original_path_hex", hex(resolved.original_path.as_bytes())]),
        json!(["extension_hex", hex(resolved.extension.as_bytes())]),
        json!([
            "resolved_using_ts_extension",
            resolved.resolved_using_ts_extension
        ]),
        json!([
            "resolved_using_extra_extensions",
            resolved.resolved_using_extra_extensions
        ]),
        json!([
            "is_external_library_import",
            resolved.is_external_library_import
        ]),
        json!([
            "alternate_result_hex",
            hex(resolved.alternate_result.as_bytes())
        ]),
    ];
    package_rows(&resolved.package_id, &mut rows);
    rows.push(json!([
        "diagnostic_codes",
        diagnostic_codes(&resolved.resolution_diagnostics)
    ]));
    Value::Array(rows)
}

fn type_reference_rows(resolved: &ResolvedTypeReferenceDirective) -> Value {
    let mut rows = vec![
        json!(["nil_result", false]),
        json!(["is_resolved", resolved.is_resolved()]),
        json!(["primary", resolved.primary]),
        json!([
            "resolved_file_name_hex",
            hex(resolved.resolved_file_name.as_bytes())
        ]),
        json!(["original_path_hex", hex(resolved.original_path.as_bytes())]),
        json!([
            "is_external_library_import",
            resolved.is_external_library_import
        ]),
    ];
    package_rows(&resolved.package_id, &mut rows);
    rows.push(json!([
        "diagnostic_codes",
        diagnostic_codes(&resolved.resolution_diagnostics)
    ]));
    Value::Array(rows)
}

/// What an action did. `Unreachable` is the port having no entry point for it;
/// it is turned into the case's reviewed gap record by the caller.
enum Step {
    Done,
    Unreachable,
}

fn apply_files(
    files: &mut BTreeMap<Vec<u8>, tsr_vfs::vfstest::InputFile>,
    entries: &[Value],
) -> Result<(), Bad> {
    use tsr_vfs::{
        iofs::{FileMode, MapFile},
        vfstest::InputFile,
    };
    for entry in entries {
        let path = text(entry, "path")?.as_bytes().to_vec();
        let file = match text(entry, "kind")? {
            "file" => InputFile::Text(text(entry, "content")?.as_bytes().to_vec()),
            "symlink" => {
                InputFile::File(tsr_vfs::vfstest::symlink(text(entry, "target")?.as_bytes()))
            }
            "directory" => InputFile::File(MapFile {
                mode: FileMode::DIR,
                ..MapFile::default()
            }),
            other => return Err(format!("unknown fixture entry kind {other:?}")),
        };
        files.insert(path, file);
    }
    Ok(())
}
fn build_host(action: &Value) -> Result<Arc<Counting>, Bad> {
    let mut files = BTreeMap::new();
    apply_files(&mut files, list(action, "files")?)?;
    let fs = tsr_vfs::vfstest::from_map(&files, flag(action, "case_sensitive")?).into_vfs();
    Ok(Arc::new(Counting {
        inner: RwLock::new(Arc::new(fs)),
        files: RwLock::new(files),
        calls: AtomicUsize::new(0),
    }))
}

fn redirect_options(action: &Value) -> Result<(Option<&str>, Option<CompilerOptions>), Bad> {
    let Some(reference) = action.get("redirect").filter(|value| !value.is_null()) else {
        return Ok((None, None));
    };
    let options = reference
        .get("options")
        .filter(|value| !value.is_null())
        .map(options)
        .transpose()?;
    Ok((Some(text(reference, "config_name")?), options))
}

#[allow(clippy::too_many_lines)]
fn apply(state: &mut State, action: &Value, row: &mut Map<String, Value>) -> Result<Step, Bad> {
    let op = api::action_op(action);
    match op {
        "new_resolver" | "new_resolver_with_options" => {
            // The pin's resolver carries a typings location, a project name
            // and extra extensions; tsr_module's does not. A case that leaves
            // all three empty is unaffected, so only a case that uses them is
            // recorded as a gap.
            if !text(action, "typings_location")?.is_empty()
                || !text(action, "project_name")?.is_empty()
                || op == "new_resolver" && !list(action, "extra_extensions")?.is_empty()
            {
                return Ok(Step::Unreachable);
            }
            let cwd = text(action, "cwd")?.as_bytes().to_vec();
            let host = build_host(action)?;
            let compiler_options = Arc::new(options(field(action, "options")?)?);
            let dynamic: Arc<dyn FileSystem> = host.clone();
            let mut settings = tsr_module::ResolverOptions {
                allow_live_host: true,
                ..Default::default()
            };
            if op == "new_resolver_with_options" {
                let cache = Arc::new(tsr_module::InfoCache::new(
                    &cwd,
                    flag(action, "case_sensitive")?,
                ));
                let mut seeds = Vec::new();
                for seed in list(action, "package_json_cache")? {
                    let path = text(seed, "package_json_path")?.as_bytes();
                    let directory = tsr_tspath::directory(path);
                    let contents = match seed.get("contents") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(source)) => Some(Arc::new(
                            tsr_module::PackageJson::parse(&directory, source.as_bytes()),
                        )),
                        _ => return Err("cache contents must be a string or null".into()),
                    };
                    let entry = Arc::new(tsr_module::InfoCacheEntry {
                        package_directory: JsString::from_bytes(directory.as_slice()),
                        directory_exists: flag(seed, "directory_exists")?,
                        contents,
                    });
                    let actual = cache.set(path, entry.clone());
                    seeds.push(json!([
                        hex(path),
                        Arc::ptr_eq(&actual, &entry),
                        hex(actual.package_directory.as_bytes()),
                        actual.exists()
                    ]));
                }
                row.insert("cache_seeds".into(), json!(seeds));
                settings.package_json_cache = Some(cache);
            }
            let resolver = Resolver::with_options(dynamic, compiler_options, &cwd, settings)
                .map_err(|error| format!("Resolver::new refused the fixture host: {error:?}"))?;
            state.host = Some(host);
            state.resolver = Some(resolver);
            row.insert("created".into(), Value::Bool(true));
            Ok(Step::Done)
        }
        // Remaining production gaps are recorded before partial results escape.
        "compiler_options_with_redirect" => {
            let base = options(field(action, "options")?)?;
            let (name, redirected) = redirect_options(action)?;
            let reference = name.map(|name| tsr_module::ResolvedProjectReference {
                config_name: name.as_bytes(),
                compiler_options: redirected.as_ref(),
            });
            let effective = tsr_module::compiler_options_with_redirect(&base, reference);
            row.insert(
                "same_pointer_as_base".into(),
                json!(std::ptr::eq(effective, &raw const base)),
            );
            row.insert(
                "module_resolution".into(),
                json!(effective.module_resolution_kind().0),
            );
            row.insert(
                "trace_resolution".into(),
                json!(effective.trace_resolution.is_true()),
            );
            Ok(Step::Done)
        }

        "mutate" => {
            let host = state.host.as_ref().ok_or("mutate before resolver")?;
            let sensitive = host.use_case_sensitive_file_names();
            let mut files = host.files.write().expect("fixture files lock");
            apply_files(&mut files, list(action, "add")?)?;
            for path in strings(action, "remove")? {
                files.remove(&path);
            }
            let fs = tsr_vfs::vfstest::from_map(&files, sensitive).into_vfs();
            *host.inner.write().expect("fixture host lock") = Arc::new(fs);
            row.insert("file_count".into(), json!(files.len()));
            Ok(Step::Done)
        }
        "set_trace" => {
            let enabled = flag(action, "enabled")?;
            state
                .resolver
                .as_mut()
                .ok_or("set_trace before resolver")?
                .set_trace_resolution(enabled);
            row.insert("trace_resolution".into(), json!(enabled));
            Ok(Step::Done)
        }
        "entrypoints" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("entrypoints before resolver")?;
            let package = resolver
                .package_scope(text(action, "directory")?.as_bytes())
                .map_err(|e| e.to_string())?
                .ok_or("entrypoints without package scope")?;
            let entries = resolver
                .entrypoints(
                    &package,
                    text(action, "package_name")?.as_bytes(),
                    flag(action, "enable_directory_search")?,
                )
                .map_err(|e| e.to_string())?;
            row.insert(
                "entrypoints".into(),
                json!(entries
                    .iter()
                    .map(|entry| json!([
                        hex(entry.resolved_file_name.as_bytes()),
                        hex(entry.original_file_name.as_bytes()),
                        hex(entry.symlink_or_realpath()),
                        hex(entry.module_specifier.as_bytes()),
                        entry.ending as u8,
                        entry.include_conditions.as_ref().map(|values| values
                            .iter()
                            .map(|value| hex(value.as_bytes()))
                            .collect::<Vec<_>>()),
                        entry.exclude_conditions.as_ref().map(|values| values
                            .iter()
                            .map(|value| hex(value.as_bytes()))
                            .collect::<Vec<_>>()),
                    ]))
                    .collect::<Vec<_>>()),
            );
            Ok(Step::Done)
        }
        "package_json_cache_entries" => {
            let mut entries = Vec::new();
            state
                .resolver
                .as_ref()
                .ok_or("cache entries before resolver")?
                .package_json_cache_entries(|key, entry| {
                    entries.push((key.clone(), entry.clone()));
                    true
                });
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            row.insert(
                "entries".into(),
                json!(entries
                    .iter()
                    .map(|(key, entry)| json!([
                        hex(key.as_bytes()),
                        hex(entry.package_directory.as_bytes()),
                        entry.exists()
                    ]))
                    .collect::<Vec<_>>()),
            );
            Ok(Step::Done)
        }
        "resolve" | "resolve_with_redirect" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("action needs a resolver, but the case never built one")?;
            let host = state.host.as_ref().ok_or("no fixture host")?;
            let before = host.calls();
            let (name, redirected) = redirect_options(action)?;
            let reference = name.map(|name| tsr_module::ResolvedProjectReference {
                config_name: name.as_bytes(),
                compiler_options: redirected.as_ref(),
            });
            let outcome = resolver.resolve_with_redirect(
                text(action, "name")?.as_bytes(),
                text(action, "file")?.as_bytes(),
                mode(action)?,
                reference,
            );
            match outcome {
                Ok(resolved) => {
                    let resolved = resolved.clone();
                    row.insert("traces".into(), trace_rows(resolver.take_trace()));
                    row.insert("result".into(), module_rows(Some(&resolved)));
                }
                Err(error) => {
                    // A refusal is recorded as a refusal. Flattening
                    // Error::Unsupported into an unresolved result is exactly
                    // the control the schedule exists to reject.
                    row.insert("error".into(), Value::String(format!("{error:?}")));
                }
            }
            row.insert(
                "touched_filesystem".into(),
                Value::Bool(host.calls() != before),
            );
            Ok(Step::Done)
        }
        "resolve_type_reference" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("action needs a resolver, but the case never built one")?;
            let host = state.host.as_ref().ok_or("no fixture host")?;
            let before = host.calls();
            let outcome = resolver.resolve_type_reference(
                text(action, "name")?.as_bytes(),
                text(action, "file")?.as_bytes(),
                mode(action)?,
            );
            match outcome {
                Ok(resolved) => {
                    let resolved = resolved.clone();
                    row.insert("traces".into(), trace_rows(resolver.take_trace()));
                    row.insert("result".into(), type_reference_rows(&resolved));
                }
                Err(error) => {
                    row.insert("error".into(), Value::String(format!("{error:?}")));
                }
            }
            row.insert(
                "touched_filesystem".into(),
                Value::Bool(host.calls() != before),
            );
            Ok(Step::Done)
        }
        "resolve_package_directory" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("action needs a resolver, but the case never built one")?;
            let host = state.host.as_ref().ok_or("no fixture host")?;
            let before = host.calls();
            match resolver.resolve_package_directory(
                text(action, "name")?.as_bytes(),
                text(action, "file")?.as_bytes(),
                mode(action)?,
            ) {
                Ok(resolved) => {
                    row.insert("result".into(), module_rows(resolved.as_ref()));
                }
                Err(error) => {
                    row.insert("error".into(), Value::String(format!("{error:?}")));
                }
            }
            row.insert(
                "touched_filesystem".into(),
                Value::Bool(host.calls() != before),
            );
            Ok(Step::Done)
        }
        "automatic_type_directive_names" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("action needs a resolver, but the case never built one")?;
            match resolver.automatic_type_directive_names() {
                Ok(names) => {
                    row.insert(
                        "names".into(),
                        Value::Array(
                            names
                                .iter()
                                .map(|name| Value::String(hex(name.as_bytes())))
                                .collect(),
                        ),
                    );
                }
                Err(error) => {
                    row.insert("error".into(), Value::String(format!("{error:?}")));
                }
            }
            Ok(Step::Done)
        }
        "versioned_types_key" => {
            row.insert(
                "keys".into(),
                Value::Array(
                    strings(action, "keys")?
                        .into_iter()
                        .map(|key| json!([hex(&key), is_applicable_versioned_types_key(&key)]))
                        .collect(),
                ),
            );
            Ok(Step::Done)
        }
        "unmangle_scoped" | "package_name_from_types_package_name" => {
            let convert = if api::action_op(action) == "unmangle_scoped" {
                tsr_module::unmangle_scoped_package_name
            } else {
                tsr_module::package_name_from_types_package_name
            };
            row.insert(
                "names".into(),
                json!(strings(action, "names")?
                    .iter()
                    .map(|name| json!([hex(name), hex(&convert(name))]))
                    .collect::<Vec<_>>()),
            );
            Ok(Step::Done)
        }
        "parse_node_module_from_path" => {
            let values = list(action, "inputs")?
                .iter()
                .map(|input| {
                    let path = text(input, "path")?;
                    let folder = flag(input, "is_folder")?;
                    Ok(json!([
                        hex(path.as_bytes()),
                        folder,
                        hex(&tsr_module::parse_node_module_from_path(
                            path.as_bytes(),
                            folder
                        ))
                    ]))
                })
                .collect::<Result<Vec<_>, Bad>>()?;
            row.insert("inputs".into(), json!(values));
            Ok(Step::Done)
        }
        "package_id_string" => {
            let values = list(action, "package_ids")?
                .iter()
                .map(|spec| {
                    let id = PackageId {
                        name: JsString::from_bytes(text(spec, "name")?.as_bytes()),
                        sub_module_name: JsString::from_bytes(
                            text(spec, "sub_module_name")?.as_bytes(),
                        ),
                        version: JsString::from_bytes(text(spec, "version")?.as_bytes()),
                        peer_dependencies: JsString::from_bytes(
                            text(spec, "peer_dependencies")?.as_bytes(),
                        ),
                    };
                    Ok(json!([
                        hex(id.package_name().as_bytes()),
                        hex(id.text().as_bytes())
                    ]))
                })
                .collect::<Result<Vec<_>, Bad>>()?;
            row.insert("package_ids".into(), json!(values));
            Ok(Step::Done)
        }
        "parsed_patterns" => {
            let input = json!({"paths":field(action, "paths")?});
            let mappings = options(&input)?.paths.unwrap_or_default();
            let patterns = tsr_module::ParsedPatterns::new(&mappings);
            let values: Vec<_> = strings(action, "candidates")?
                .iter()
                .map(|candidate| {
                    let matched = patterns.match_pattern_or_exact(candidate);
                    let inner = if matched.is_valid() {
                        matched.matched_text(candidate)
                    } else {
                        b""
                    };
                    json!([
                        hex(candidate),
                        matched.is_valid(),
                        hex(&matched.text),
                        matched.star_index,
                        hex(inner)
                    ])
                })
                .collect();
            row.insert("candidates".into(), json!(values));
            Ok(Step::Done)
        }
        "mangle_scoped" => {
            row.insert(
                "names".into(),
                Value::Array(
                    strings(action, "names")?
                        .into_iter()
                        .map(|name| json!([hex(&name), hex(&mangle_scoped_package_name(&name))]))
                        .collect(),
                ),
            );
            Ok(Step::Done)
        }
        "types_package_name" => {
            row.insert(
                "names".into(),
                Value::Array(
                    strings(action, "names")?
                        .into_iter()
                        .map(|name| json!([hex(&name), hex(&get_types_package_name(&name))]))
                        .collect(),
                ),
            );
            Ok(Step::Done)
        }
        "conditions" => {
            let compiler_options = options(field(action, "options")?)?;
            let conditions = get_conditions(&compiler_options, mode(action)?);
            row.insert(
                "conditions".into(),
                Value::Array(
                    conditions
                        .iter()
                        .map(|condition| Value::String(hex(condition.as_bytes())))
                        .collect(),
                ),
            );
            Ok(Step::Done)
        }
        "resolution_diagnostic" => {
            let compiler_options = options(field(action, "options")?)?;
            let spec = field(action, "resolved")?;
            let resolved = ResolvedModule {
                extension: JsString::from_bytes(text(spec, "extension")?.as_bytes().to_vec()),
                resolved_using_extra_extensions: flag(spec, "resolved_using_extra_extensions")?,
                ..ResolvedModule::default()
            };
            let message = resolution_diagnostic(
                &compiler_options,
                &resolved,
                flag(action, "is_declaration_file")?,
            );
            row.insert(
                "code".into(),
                message.map_or(Value::Null, |message| json!(message.code)),
            );
            Ok(Step::Done)
        }
        "is_resolved" => {
            row.insert(
                "modules".into(),
                Value::Array(
                    strings(action, "module_file_names")?
                        .into_iter()
                        .map(|name| {
                            let resolved = ResolvedModule {
                                resolved_file_name: JsString::from_bytes(name.clone()),
                                ..ResolvedModule::default()
                            };
                            json!([hex(&name), resolved.is_resolved()])
                        })
                        .collect(),
                ),
            );
            row.insert(
                "type_references".into(),
                Value::Array(
                    strings(action, "type_reference_file_names")?
                        .into_iter()
                        .map(|name| {
                            let resolved = ResolvedTypeReferenceDirective {
                                resolved_file_name: JsString::from_bytes(name.clone()),
                                ..ResolvedTypeReferenceDirective::default()
                            };
                            json!([hex(&name), resolved.is_resolved()])
                        })
                        .collect(),
                ),
            );
            Ok(Step::Done)
        }
        other => Err(format!(
            "action {other:?} is not served by the module group"
        )),
    }
}

pub fn observe(request: &Value) -> Option<Outcome> {
    let identifier = request.get("case").and_then(Value::as_str).unwrap_or("");
    if !identifier.starts_with(CASE_PREFIX) {
        return None;
    }
    if api::subject(request) != "moduleResolution" {
        return Some(Outcome::Failed(format!(
            "case {identifier} declares subject {:?}, which the module group does not serve",
            api::subject(request)
        )));
    }
    let actions = api::actions(request);
    let mut state = State {
        resolver: None,
        host: None,
    };
    let mut rows = Vec::with_capacity(actions.len());
    for action in actions {
        let mut row = Map::new();
        row.insert(
            "op".into(),
            Value::String(api::action_op(action).to_owned()),
        );
        // Match native per-action recovery. Only known runtime bounds classes
        // are canonicalized; unrelated panics remain harness failures.
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply(&mut state, action, &mut row)
        }));
        let applied = match applied {
            Ok(result) => result,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic");
                let class = if message.starts_with("index out of bounds:") {
                    "runtime: index out of range"
                } else if message.starts_with("slice index starts at ")
                    || message.starts_with("range start index ")
                    || message.starts_with("range end index ")
                {
                    "runtime: slice bounds out of range"
                } else if message.starts_with("Unexpected moduleResolution: ")
                    || message.starts_with("vfs: path ")
                {
                    message
                } else {
                    return Some(Outcome::Failed(format!("unexpected panic: {message}")));
                };
                row = Map::from_iter([
                    ("op".into(), json!(api::action_op(action))),
                    ("panic".into(), json!(class)),
                ]);
                Ok(Step::Done)
            }
        };
        match applied {
            Ok(Step::Done) => rows.push(Value::Object(row)),
            // The first action the port cannot run decides the whole case: a
            // partial trace would be a comparison against a shorter run, not a
            // result. The gap names the operation the case was written for.
            Ok(Step::Unreachable) => return Some(gap(request)),
            Err(error) => {
                return Some(Outcome::Failed(format!(
                    "case {identifier}, action {:?}: {error}",
                    api::action_op(action)
                )))
            }
        }
    }
    Some(Outcome::Observed(api::ordered(rows)))
}
