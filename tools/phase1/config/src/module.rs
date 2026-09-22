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
//! Three whole classes of action have no Rust counterpart, and each is
//! recorded rather than worked around:
//!
//!   * A filesystem whose answers change. `Resolver::new` refuses a host with
//!     no snapshot identity (crates/tsr_module/src/resolver.rs:111-113), so
//!     the `mutate` action cannot be expressed at all.
//!   * Changing `traceResolution` between two requests on one resolver. The
//!     options are an `Arc<CompilerOptions>` fixed at construction
//!     (resolver.rs:106-125), where the pin re-reads the field from the
//!     caller's struct on every request (resolver.go:201-206). This is the only
//!     way to reach a cache WRITE twice for one key, so the two cache-write
//!     cases are unreachable here.
//!   * Project reference redirects, entrypoint discovery, the typings
//!     location and an injected package.json cache, none of which exist in
//!     `tsr_module` at all.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use tsr_core::{CompilerOptions, ModuleKind};
use tsr_jsstring::JsString;
use tsr_module::{
    get_conditions, get_types_package_name, is_applicable_versioned_types_key,
    mangle_scoped_package_name, resolution_diagnostic, PackageId, ResolvedModule,
    ResolvedTypeReferenceDirective, Resolver, TraceArg,
};
use tsr_vfs::{
    Entries, Error as VfsError, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot,
    SnapshotId,
};

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
const MISSING: &[(&str, &str, &str, &str)] = &[
    (
        "tsc/internal/module/resolver.go:resolutionState.getPackageJsonInfo",
        "tsc/internal/module/resolver.go:1764-1809, whose negative entry is written at :1804-1808 \
         and short-circuited at :1766-1779",
        "the operation itself is present at crates/tsr_module/src/resolver.rs:139-199 and caches \
         the negative answer as (exists, None) the same way. What is missing is the ability to \
         OBSERVE the pinned contract: the case needs the host to gain a package.json between two \
         requests, and Resolver::new returns Err(Error::MutableHost) for any host without a \
         snapshot identity (resolver.rs:111-113), so a resolver can never see a second state of \
         the filesystem. A port would need a resolver that accepts a live host, or an explicit \
         invalidation entry point",
        "crates/tsr_module/src/resolver.rs:111 (present but unreachable: no live host, no \
         invalidation)",
    ),
    (
        "tsc/internal/module/cache.go:moduleResolutionCache.Set",
        "tsc/internal/module/cache.go:26-28 (LoadOrStore), written from resolver.go:325 after the \
         read that skips it at :278-283",
        "a resolution cache whose write is first-writer-wins, like the pin's LoadOrStore \
         (cache.go:26-28). crates/tsr_module/src/resolver.rs:298 was read and rejected: it is a \
         plain BTreeMap insert, which is LAST-writer-wins. Observing either semantics also needs \
         the trace mode to change between two requests on one resolver, and the options are an \
         Arc fixed at construction (resolver.rs:106-125) where the pin re-reads \
         TraceResolution per request (resolver.go:201-206)",
        "crates/tsr_module/src/resolver.rs:298 (wrong write semantics, and unobservable: options \
         and host are both fixed for the resolver's lifetime)",
    ),
    (
        "tsc/internal/module/cache.go:typeRefDirectiveResolutionCache.Set",
        "tsc/internal/module/cache.go:46-48 (Store), written from resolver.go:262 after the read \
         that skips it at :241-245",
        "a type-reference cache whose write is last-writer-wins, like the pin's Store \
         (cache.go:46-48). crates/tsr_module/src/type_references.rs:155 is a BTreeMap insert and \
         its semantics DO match, but the case cannot run for the same two reasons as its module \
         companion: no live host and no per-request trace mode",
        "crates/tsr_module/src/type_references.rs:155 (matching semantics, unobservable)",
    ),
    (
        "tsc/internal/module/resolver.go:NewResolverWithOptions",
        "tsc/internal/module/resolver.go:180-199 with ResolverOptions at :159-161 and \
         packagejson.InfoCache.Set at packagejson/cache.go:190-194",
        "pub fn Resolver::with_options(host, options, typings_location, project_name, opts: \
         ResolverOptions) taking an external package.json info cache, plus a public InfoCache \
         type whose Set is a LoadOrStore that RETURNS the winning entry \
         (packagejson/cache.go:190-194) and whose entries carry the three-state \
         absent / present-with-nil-contents / present-with-contents. \
         crates/tsr_module/src/resolver.rs:99 was read and rejected: the cache is a private \
         BTreeMap field of Resolver, it is not injectable, its write is a plain insert, and its \
         value tuple cannot represent a present entry with no contents",
        "crates/tsr_module/src/resolver.rs:99 (private field; no injectable cache type)",
    ),
    (
        "tsc/internal/module/resolver.go:Resolver.PackageJsonCacheEntries",
        "tsc/internal/module/resolver.go:212-214, forwarding to packagejson.InfoCache.Range \
         (packagejson/cache.go:196-198)",
        "pub fn package_json_cache_entries(&self) -> impl Iterator<Item = (&JsString, &Entry)>, \
         reporting the canonical key, the stored package directory and whether contents were \
         read. The cache is the private BTreeMap at crates/tsr_module/src/resolver.rs:99 and \
         nothing exposes it",
        "crates/tsr_module/src/resolver.rs:99 (no accessor)",
    ),
    (
        "tsc/internal/module/resolver.go:GetCompilerOptionsWithRedirect",
        "tsc/internal/module/resolver.go:139-147, called from newResolutionState (:103), \
         ResolveModuleName (:284) and ResolveTypeReferenceDirective (:246)",
        "pub fn compiler_options_with_redirect<'a>(options: &'a CompilerOptions, redirect: \
         Option<&'a dyn ResolvedProjectReference>) -> &'a CompilerOptions, returning the \
         caller's own options both when there is no reference AND when the reference's options \
         are absent. There is no ResolvedProjectReference trait anywhere in the workspace: \
         `grep -rn \"ResolvedProjectReference\" crates --include=*.rs` is empty",
        "crates/tsr_module/src/resolver.rs (absent; project reference redirects are not modelled)",
    ),
    (
        "tsc/internal/module/resolver.go:tracer.traceResolutionUsingProjectReference",
        "tsc/internal/module/resolver.go:216-220, with getRedirectConfigName at cache.go:84-89 \
         feeding both cache keys (cache.go:11-16, :30-36)",
        "the redirect trace line plus the redirect's config name as the fourth component of both \
         cache keys (cache.go:11-16, :30-36, :84-89). tsr_module's keys are \
         {directory, name, mode} (resolver.rs:81-86) and {directory, name, mode, inferred} \
         (type_references.rs:24-30); neither carries a redirect, so two resolutions the pin \
         keeps separate would collide. The message constant exists \
         (tsr_diagnostics::Using_compiler_options_of_project_reference_redirect_0) and has no \
         writer in tsr_module",
        "crates/tsr_module/src/trace.rs (absent) and resolver.rs:81 (cache key omits the redirect)",
    ),
    (
        "tsc/internal/module/resolver.go:Resolver.GetEntrypointsFromPackageJsonInfo",
        "tsc/internal/module/resolver.go:2168-2225 with loadEntrypointsFromExportMap at :2244-2357, \
         createResolvedEntrypointHandlingSymlink at :2227-2242 and ResolvedEntrypoint at \
         :2147-2166",
        "pub fn entrypoints(&mut self, package: &PackageJson, package_name: &[u8], \
         directory_search: bool) -> Vec<ResolvedEntrypoint>, where ResolvedEntrypoint carries \
         the real path, the symlinked path, the module specifier that reaches it, an Ending and \
         the include/exclude condition sets (resolver.go:2133-2159). The whole reverse direction \
         is absent: loadEntrypointsFromExportMap, createResolvedEntrypointHandlingSymlink, \
         getMatchedStarForPatternEntrypoint and extensions.Array have no Rust counterpart. \
         tsr_checker has an unrelated `Ending` from modulespecifiers/preferences.go and a \
         tryGetModuleNameFromExports, which computes the opposite direction",
        "crates/tsr_module/src/package_maps.rs (absent; the crate only resolves forwards)",
    ),
    (
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
    ),
];

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
    inner: MemorySnapshot,
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
        self.inner.use_case_sensitive_file_names()
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.inner.snapshot_id()
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, VfsError> {
        self.hit();
        self.inner.read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, VfsError> {
        self.hit();
        self.inner.stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, VfsError> {
        self.hit();
        self.inner.entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, VfsError> {
        self.hit();
        self.inner.realpath(path)
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, VfsError> {
        self.hit();
        self.inner.file_exists(path)
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, VfsError> {
        self.hit();
        self.inner.directory_exists(path)
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

fn build_host(action: &Value) -> Result<Arc<Counting>, Bad> {
    let cwd = text(action, "cwd")?.as_bytes().to_vec();
    let case_sensitive = flag(action, "case_sensitive")?;
    let mut builder = MemoryBuilder::new(&cwd, case_sensitive);
    for entry in list(action, "files")? {
        let path = text(entry, "path")?.as_bytes().to_vec();
        match text(entry, "kind")? {
            "file" => builder.insert_loaded(&path, text(entry, "content")?.as_bytes().to_vec()),
            "symlink" => builder.insert_symlink(&path, text(entry, "target")?.as_bytes()),
            "directory" => builder.insert_directory(&path),
            other => return Err(format!("unknown fixture entry kind {other:?}")),
        }
    }
    Ok(Arc::new(Counting {
        inner: builder.finish(),
        calls: AtomicUsize::new(0),
    }))
}

#[allow(clippy::too_many_lines)]
fn apply(state: &mut State, action: &Value, row: &mut Map<String, Value>) -> Result<Step, Bad> {
    let op = api::action_op(action);
    match op {
        "new_resolver" => {
            // The pin's resolver carries a typings location, a project name
            // and extra extensions; tsr_module's does not. A case that leaves
            // all three empty is unaffected, so only a case that uses them is
            // recorded as a gap.
            if !text(action, "typings_location")?.is_empty()
                || !text(action, "project_name")?.is_empty()
                || !list(action, "extra_extensions")?.is_empty()
            {
                return Ok(Step::Unreachable);
            }
            let cwd = text(action, "cwd")?.as_bytes().to_vec();
            let host = build_host(action)?;
            let compiler_options = Arc::new(options(field(action, "options")?)?);
            let dynamic: Arc<dyn FileSystem> = host.clone();
            let resolver = Resolver::new(dynamic, compiler_options, &cwd)
                .map_err(|error| format!("Resolver::new refused the fixture host: {error:?}"))?;
            state.host = Some(host);
            state.resolver = Some(resolver);
            row.insert("created".into(), Value::Bool(true));
            Ok(Step::Done)
        }
        // Nothing in `tsr_module` can run these, and each one's reason is the
        // MISSING row of the case's own operation:
        //   * an injected package.json cache, a filesystem whose answers
        //     change and a per-request trace mode are structural gaps;
        //   * project reference redirects, entrypoint discovery and the cache
        //     accessor do not exist at all;
        //   * the five pure helpers exist only as private or foreign-crate
        //     code, so there is no entry point to call.
        "new_resolver_with_options"
        | "mutate"
        | "set_trace"
        | "resolve_with_redirect"
        | "compiler_options_with_redirect"
        | "entrypoints"
        | "package_json_cache_entries" => Ok(Step::Unreachable),

        "resolve" => {
            let resolver = state
                .resolver
                .as_mut()
                .ok_or("action needs a resolver, but the case never built one")?;
            let host = state.host.as_ref().ok_or("no fixture host")?;
            let before = host.calls();
            let outcome = resolver.resolve(
                text(action, "name")?.as_bytes(),
                text(action, "file")?.as_bytes(),
                mode(action)?,
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
