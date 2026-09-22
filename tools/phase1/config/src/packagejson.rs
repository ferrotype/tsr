//! The `internal/packagejson` group: the part of the package.json surface the
//! s07 dataset never reaches.
//!
//! Four of the sixteen cases are answered by production Rust. The other
//! twelve are recorded gaps, and they are not all the same gap; the split is
//! read off `crates/`, not off the ledger.
//!
//! **What exists.** `crates/tsr_module/src/package_json.rs` is a port of
//! `packagejson.go`, `expected.go`, `jsonvalue.go` and
//! `exportsorimports.go` (its own header says so, package_json.rs:1-5). It
//! carries `parse` (:83), the `Expected<T>` state triple
//! `{actual_type, valid, null}` (:13-18) with `is_present` (:20) and
//! `get_value` (:39), `is_falsy` (:465) and `object_kind` (:481). So the
//! *validity* half of the `TypeValidatedField` quartet is present as data and
//! the `expected-validity` case compares it directly.
//! `PackageJson::version_paths` returns a retrieval with its own lazy mappings;
//! `version_paths_traced` also replays the package's recorded selection traces.
//! The version cases exercise both the contents and these cache boundaries.
//!
//! **What does not.** Four distinct absences, each recorded against the exact
//! file that would carry it:
//!
//! 1. *No declared-type notion at all.* `ExpectedState` stores the type the
//!    JSON actually had and nothing about the type the field wanted
//!    (package_json.rs:13-18); the Rust port reaches its field kinds through
//!    the private `field_kind`/`mapper_kind` name tables (:104, :120), which
//!    are not a per-field accessor and are not public. So
//!    `Expected.ExpectedJSONType` and, with it, `ExpectedOf` -- whose whole
//!    trick is writing the *expected* type into `actualJSONType` -- have
//!    nowhere to live, and `validated.go`'s four-member interface has no Rust
//!    trait.
//! 2. *No JSONValue type at all.* The port models a JSON value as
//!    `serde_json::Value` and absence as `Option::None` from `Fields::get`
//!    (package_json.rs:63). There is no `JSONValueType`, so
//!    `JSONValueType.String` has no renderer -- the nearest thing,
//!    `package_maps.rs:809 json_type`, is a private helper over
//!    `serde_json::Value` with six arms and no not-present and no unknown arm
//!    -- and no checked accessor, so `JSONValue.AsString` has no counterpart:
//!    `package_maps.rs:326` reads a string target by pattern-matching
//!    `Value::String(target)` and `PackageJson::string`
//!    (`resolver.rs:76-78`) answers `Option`, neither of which can panic off
//!    the string type. `JSONValue.IsPresent` is the one where a structural
//!    equivalent does exist -- `Fields::get(name).is_some()` separates absent
//!    from `Some(Value::Null)` exactly as the pin separates NotPresent from
//!    Null -- but it is not a named entry point, and a predicate the harness
//!    spells for the port is not the port answering.
//! 3. *No dependency helpers.* A search of `crates/` for `has_dependency`,
//!    `range_dependencies`, `runtime_dependency` and `RuntimeDependency`
//!    returns nothing. `Fields` (package_json.rs:54-59) stores the four
//!    dependency fields as ordinary typed entries and nothing walks them, so
//!    all three of `DependencyFields`' methods are absent.
//! 4. *No package.json info cache as a type.* The cache exists as a field:
//!    `Resolver.packages: BTreeMap<JsString, (bool, Option<Arc<PackageJson>>)>`
//!    (resolver.rs:87), keyed by `path::to_path(file, cwd, case_sensitivity)`
//!    (resolver.rs:141-145), and `resolver.rs:160-172` is an inlined
//!    `WithPackageDirectory` -- it hands back the cached `Arc` when the
//!    directory matches and builds a new `PackageJson` over the same
//!    `Arc<PackageContents>` when it does not. So the *behaviour* of three of
//!    the eleven cache operations is in the tree, and none of the eleven is a
//!    reachable entry point: the map is private, `insert` is a last-writer
//!    store rather than `LoadOrStore`, `DirectoryExists` lives in the tuple
//!    rather than on the entry, and nothing enumerates the map at all.
//!
//! Preparation records those gaps; it never emulates a missing algorithm to
//! make a comparison run, and it never reads an expected result.

use std::sync::Arc;

use serde_json::{json, Value};
use tsr_jsstring::JsString;
use tsr_module::package_json::{self, Fields};
use tsr_module::{PackageJson, Resolver};
use tsr_vfs::MemoryBuilder;

use crate::api::{action_op, action_str, actions, ordered, subject, Outcome};

/// The request subject this group claims.
const SUBJECT: &str = "packageJson";

/// The ordered field vocabulary the `Expected` cases address, identical to the
/// probe's `phase1FieldNames`. Both sides walk this list, so a port that grew
/// or lost a field shows up as a length difference rather than as a silently
/// skipped row.
const FIELD_NAMES: &[&str] = &[
    "name",
    "version",
    "type",
    "tsconfig",
    "main",
    "types",
    "typings",
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
    "contentMapper",
    "contentMapper.exec",
    "contentMapper.compilerOptions",
    "contentMapper.dynamicConfig",
];

/// One reviewed gap per case: the pinned operation the driver found absent,
/// the pinned authority for it, the signature the port owes and the Rust file
/// that does not have it. Keyed on the case id rather than on the operation,
/// because two cases in this group name the same pinned operation and only one
/// of them is a gap.
struct Gap {
    case: &'static str,
    operation: &'static str,
    authority: &'static str,
    signature: &'static str,
    home: &'static str,
}

const EXPECTED_HOME: &str = "crates/tsr_module/src/package_json.rs. ExpectedState \
    (package_json.rs:13-18) carries actual_type, valid and null and has no member for the \
    type the field expected; the declared kinds live in the private name tables field_kind \
    (:104) and mapper_kind (:120), which are not per-field accessors and are not public";

const JSONVALUE_HOME: &str = "crates/tsr_module/src/package_json.rs, which models a JSON \
    value as serde_json::Value and absence as Option::None from Fields::get (:63). There is \
    no JSONValue type and no JSONValueType, so there is nothing for these members to be \
    members of";

const DEPENDENCY_HOME: &str = "crates/tsr_module/src/package_json.rs. Fields (:54-59) keeps \
    the four dependency fields as ordinary typed entries reachable through Fields::field \
    (:70) and Fields::get (:63), and nothing in crates/ walks them: a repository-wide search \
    for has_dependency, range_dependencies, runtime_dependency and RuntimeDependency finds \
    nothing";

const CACHE_HOME: &str = "crates/tsr_module/src/resolver.rs. The package.json info cache is \
    not a type: it is Resolver.packages, a private BTreeMap<JsString, (bool, \
    Option<Arc<PackageJson>>)> (resolver.rs:87) filled at resolver.rs:172-174, so there is no \
    InfoCache to construct, read, store into or enumerate, and DirectoryExists lives in the \
    tuple rather than on the entry";

const GAPS: &[Gap] = &[
    Gap {
        case: "config/packagejson/expected-json-type-is-the-declared-type",
        operation: "tsc/internal/packagejson/expected.go:Expected.ExpectedJSONType",
        authority: "tsc/internal/packagejson/expected.go:51-67, reached through the \
            TypeValidatedField interface (validated.go:6) by resolver.go:1882, which renders \
            it beside ActualJSONType as the `expected {1}, got {2}` diagnostic",
        signature: "a per-field accessor answering the JSON type the field's DECLARED type \
            wants -- the Rust shape of `fn expected_json_type(&self) -> &'static str` on \
            Expected<T>, answered from the field's declared kind and never from the parsed \
            data, with an `unknown` answer for a field whose type is a struct. It is the \
            second member of the four-member TypeValidatedField view, which has no Rust \
            trait either",
        home: EXPECTED_HOME,
    },
    Gap {
        case: "config/packagejson/expected-of-builds-a-present-field-from-no-json",
        operation: "tsc/internal/packagejson/expected.go:ExpectedOf",
        authority: "tsc/internal/packagejson/expected.go:73-75, which constructs a valid \
            field from a value with no JSON in sight and sets its actualJSONType from \
            ExpectedJSONType called on a typed nil receiver",
        signature: "pub fn expected_of<T>(value: T) -> Expected<T>, returning state \
            { valid: true, null: false, actual_type: <the declared type's name> } -- so a \
            field nothing parsed reports as present. It cannot be written until the declared \
            type has a name to read, which is the ExpectedJSONType gap above",
        home: EXPECTED_HOME,
    },
    Gap {
        case: "config/packagejson/json-value-type-names-every-constant",
        operation: "tsc/internal/packagejson/jsonvalue.go:JSONValueType.String",
        authority: "tsc/internal/packagejson/jsonvalue.go:22-39, whose output reaches a user \
            through cache.go:40 and :63 as the third argument of \
            Expected_type_of_0_field_in_package_json_to_be_1_got_2",
        signature: "impl Display (or a `fn name(self) -> String`) for a JSONValueType enum \
            with the pin's seven discriminants, rendering the six named ones and \
            `unknown(<n>)` for every other int8 -- including 0, JSONValueTypeNotPresent, \
            which the pinned switch does not name either. The nearest existing code is the \
            private json_type at crates/tsr_module/src/package_maps.rs:809, which maps the \
            six serde_json::Value variants to names and has neither a not-present state nor \
            an unknown arm",
        home: JSONVALUE_HOME,
    },
    Gap {
        case: "config/packagejson/json-value-presence-separates-absent-from-null",
        operation: "tsc/internal/packagejson/jsonvalue.go:JSONValue.IsPresent",
        authority: "tsc/internal/packagejson/jsonvalue.go:46-48, gating the exports walk at \
            resolver.go:2172 and the package-scope check at compiler/program.go:2193",
        signature: "fn is_present(&self) -> bool on the untyped JSON value, true for an \
            explicit null and false only for an absent field. The port has the \
            DISTINCTION -- Fields::get answers None for an absent field and \
            Some(Value::Null) for an explicit null (crates/tsr_module/src/package_json.rs:63, \
            :155-157) -- but no named member carrying it, so the predicate would today be \
            spelled by each caller rather than by the port",
        home: JSONVALUE_HOME,
    },
    Gap {
        case: "config/packagejson/as-string-panics-off-the-string-type",
        operation: "tsc/internal/packagejson/jsonvalue.go:JSONValue.AsString",
        authority: "tsc/internal/packagejson/jsonvalue.go:79-84, called ten times by \
            resolver.go:2253-2298 behind a single explicit Type test",
        signature: "fn as_string(&self) -> &str on the untyped JSON value, panicking unless \
            the value is a string -- a checked accessor, not a lenient one, so an unguarded \
            call is a crash rather than a silently wrong resolution. Neither existing reader \
            can be it: crates/tsr_module/src/package_maps.rs:326 pattern-matches \
            Value::String(target) and PackageJson::string \
            (crates/tsr_module/src/resolver.rs:76-78) answers Option<&[u8]>",
        home: JSONVALUE_HOME,
    },
    Gap {
        case: "config/packagejson/has-dependency-spans-all-four-fields",
        operation: "tsc/internal/packagejson/packagejson.go:DependencyFields.HasDependency",
        authority: "tsc/internal/packagejson/packagejson.go:34-56",
        signature: "fn has_dependency(&self, name: &str) -> bool over all FOUR dependency \
            fields including devDependencies, skipping a field whose JSON failed to decode \
            rather than treating it as empty",
        home: DEPENDENCY_HOME,
    },
    Gap {
        case: "config/packagejson/range-dependencies-walks-the-fields-in-declaration-order",
        operation: "tsc/internal/packagejson/packagejson.go:DependencyFields.RangeDependencies",
        authority: "tsc/internal/packagejson/packagejson.go:58-87, consumed by \
            ls/autoimport/util.go:235 and ls/string_completions.go:1019",
        signature: "fn range_dependencies(&self, f: impl FnMut(&str, &str, &str) -> bool), \
            entering dependencies, devDependencies, peerDependencies and \
            optionalDependencies in that order, reporting a name once per field it appears \
            in with that field's own version, and returning from the whole walk the moment \
            the callback answers false",
        home: DEPENDENCY_HOME,
    },
    Gap {
        case: "config/packagejson/runtime-dependency-names-exclude-devdependencies",
        operation:
            "tsc/internal/packagejson/packagejson.go:DependencyFields.GetRuntimeDependencyNames",
        authority: "tsc/internal/packagejson/packagejson.go:89-108, consumed by \
            compiler/program.go:2248",
        signature: "fn runtime_dependency_names(&self) -> BTreeSet<JsString> (or the crate's \
            own set), unioning dependencies, peerDependencies and optionalDependencies and \
            EXCLUDING devDependencies, and answering an empty set rather than nothing when \
            none of the three is present",
        home: DEPENDENCY_HOME,
    },
    Gap {
        case: "config/packagejson/info-cache-entry-readers-guard-a-nil-receiver",
        operation: "tsc/internal/packagejson/cache.go:InfoCacheEntry.Exists",
        authority: "tsc/internal/packagejson/cache.go:129-145, the three readers \
            modulespecifiers/specifiers.go:870 and compiler/program.go:2244 call on a lookup \
            that may have missed",
        signature: "an InfoCacheEntry type with exists / contents / directory readers that \
            are all callable on an absent entry -- the Rust shape of the pin's typed nil is \
            Option<&InfoCacheEntry> with the readers on Option, or an entry that carries its \
            own absence. The port has no entry type: the closest value is \
            Arc<PackageJson> (crates/tsr_module/src/resolver.rs:57-61), which carries \
            directory and contents but not DirectoryExists, and absence is the enclosing \
            Option",
        home: CACHE_HOME,
    },
    Gap {
        case: "config/packagejson/with-package-directory-copies-and-does-not-guard-nil",
        operation: "tsc/internal/packagejson/cache.go:InfoCacheEntry.WithPackageDirectory",
        authority: "tsc/internal/packagejson/cache.go:158-167, called by resolver.go:1772 \
            and :1798 to correct an entry the cache handed back under a different directory \
            spelling",
        signature: "fn with_package_directory(&self, directory: &[u8]) -> &Self or an owned \
            copy, returning the RECEIVER ITSELF when the directory already matches and a \
            shallow copy sharing the same contents otherwise. The behaviour exists inlined \
            at crates/tsr_module/src/resolver.rs:160-172, which clones the Arc when the \
            directory matches and builds a new PackageJson over the same \
            Arc<PackageContents> when it does not -- but it is a branch inside \
            Resolver::package_json, not a member anything can call, and the identity the pin \
            offers (result == receiver) is not expressible over an Arc clone",
        home: CACHE_HOME,
    },
    Gap {
        case: "config/packagejson/info-cache-keys-normalise-and-the-first-write-wins",
        operation: "tsc/internal/packagejson/cache.go:InfoCache.Set",
        authority: "tsc/internal/packagejson/cache.go:190-194, whose LoadOrStore return \
            value resolver.go:1772 depends on",
        signature: "fn set(&self, package_json_path: &[u8], info: InfoCacheEntry) -> \
            &InfoCacheEntry with FIRST-WRITER-WINS semantics: a second store of a key leaves \
            the first entry in place and returns it, so the losing caller still gets a \
            usable entry. The port's store is \
            `self.packages.insert(key, (directory_exists, result.clone()))` \
            (crates/tsr_module/src/resolver.rs:172-174), a last-writer store on a private \
            map that returns nothing; the key derivation it shares -- \
            path::to_path(file, cwd, case_sensitivity) at resolver.rs:141-145 -- is the one \
            part that is already there",
        home: CACHE_HOME,
    },
    Gap {
        case: "config/packagejson/info-cache-range-visits-every-entry",
        operation: "tsc/internal/packagejson/cache.go:InfoCache.Range",
        authority: "tsc/internal/packagejson/cache.go:196-198 over \
            collections/syncmap.go:43-60",
        signature: "fn range(&self, f: impl FnMut(&Path, &InfoCacheEntry) -> bool), \
            enumerating every cached entry and ending the walk when the callback answers \
            false. Nothing in crates/ enumerates the package cache at all: \
            Resolver.packages is private and Resolver exposes no iterator over it",
        home: CACHE_HOME,
    },
];

fn text(value: &JsString) -> String {
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

/// The validity the `TypeValidatedField` view reports, read off the port's own
/// parsed state. A field the parse never saw has no entry and is not valid,
/// which is the pin's zero-valued `Expected` answering false.
fn field_valid(fields: &Fields, name: &str) -> bool {
    if name == "contentMapper" {
        return fields.content_mapper.state.valid;
    }
    if let Some(inner) = name.strip_prefix("contentMapper.") {
        return fields
            .content_mapper
            .value
            .field(inner)
            .is_some_and(|field| field.state.valid);
    }
    fields.field(name).is_some_and(|field| field.state.valid)
}

/// Build the package through the production resolver, which is the only way to
/// reach a `tsr_module::PackageJson`: `Resolver::package_json` is what
/// constructs `PackageContents` and the version-paths cache that lives on it.
/// The snapshot is in memory, so the case is host independent.
fn load_package(source: &str) -> Result<Arc<PackageJson>, String> {
    let mut files = MemoryBuilder::new(b"/repo", true);
    files.insert_loaded(b"/repo/pkg/package.json", source.as_bytes().to_vec());
    let mut resolver = Resolver::new(
        Arc::new(files.finish()),
        Arc::new(tsr_core::CompilerOptions::default()),
        b"/repo",
    )
    .map_err(|error| format!("resolver construction failed: {error:?}"))?;
    resolver
        .package_json(b"/repo/pkg")
        .map_err(|error| format!("reading /repo/pkg/package.json failed: {error:?}"))?
        .ok_or_else(|| "the memory snapshot did not surface /repo/pkg/package.json".to_owned())
}

fn same_mappings(
    first: Option<&tsr_core::PathMappings>,
    second: Option<&tsr_core::PathMappings>,
) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => std::ptr::eq(first, second),
        (None, None) => true,
        _ => false,
    }
}

fn render_mappings(paths: &tsr_core::PathMappings) -> Vec<Value> {
    paths
        .iter()
        .map(|(key, values)| match values {
            Some(values) => json!([text(key), values.iter().map(text).collect::<Vec<_>>()]),
            // The pin cannot produce this: GetPaths always stores a slice. It
            // is rendered rather than flattened so a port that lost the values
            // is visible instead of looking empty.
            None => json!([text(key), Value::Null]),
        })
        .collect()
}

/// The `Expected` validity case. Every action is either a parse or a read of
/// the parsed state, so the whole trace runs on `package_json::parse` and the
/// public field readers.
fn expected_validity(request: &Value) -> Outcome {
    let mut fields: Option<Fields> = None;
    let mut rows = Vec::new();
    for action in actions(request) {
        match action_op(action) {
            "load" => {
                let parsed = package_json::parse(action_str(action, "source").as_bytes());
                let parseable = parsed.parseable;
                fields = Some(parsed.fields);
                rows.push(json!({"op": "load", "parsed": parseable}));
            }
            "expected_is_valid" => {
                let Some(fields) = fields.as_ref() else {
                    return Outcome::Failed(
                        "expected_is_valid ran before a load action parsed a document".to_owned(),
                    );
                };
                let result: Vec<Value> = FIELD_NAMES
                    .iter()
                    .map(|name| json!([name, field_valid(fields, name)]))
                    .collect();
                rows.push(json!({"op": "expected_is_valid", "result": result}));
            }
            other => {
                return Outcome::Failed(format!(
                    "the expected-validity case sent unsupported action {other:?}"
                ))
            }
        }
    }
    Outcome::Observed(ordered(rows))
}

/// Native version selection, retrieval-local mappings and replayed traces.
fn version_paths(request: &Value) -> Outcome {
    let mut package: Option<Arc<PackageJson>> = None;
    let mut rows = Vec::new();
    for action in actions(request) {
        match action_op(action) {
            "load" => match load_package(action_str(action, "source")) {
                Ok(loaded) => {
                    let parseable = loaded.parseable;
                    package = Some(loaded);
                    rows.push(json!({"op": "load", "parsed": parseable}));
                }
                Err(error) => return Outcome::Failed(error),
            },
            "get_version_paths" | "get_version_paths_traced" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "get_version_paths ran before a load action built the package".to_owned(),
                    );
                };
                let mut traces = Vec::new();
                let paths = package.version_paths_traced(|message| traces.push(message.clone()));
                let row = if action_op(action) == "get_version_paths_traced" {
                    let traces: Vec<_> = traces
                        .iter()
                        .map(|message| {
                            let args: Vec<_> = message
                                .args
                                .iter()
                                .map(|arg| match arg {
                                    tsr_module::TraceArg::Text(value) => text(value),
                                    tsr_module::TraceArg::Bool(value) => value.to_string(),
                                })
                                .collect();
                            json!([message.message.code, args])
                        })
                        .collect();
                    json!({"op": action_op(action), "exists": paths.exists(), "traces": traces})
                } else {
                    json!({"op": action_op(action), "result": [paths.exists()]})
                };
                rows.push(row);
            }
            "version_paths_mappings" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "version_paths_mappings ran before a load action built the package"
                            .to_owned(),
                    );
                };
                let result = package
                    .version_paths()
                    .paths()
                    .map_or(Value::Null, |paths| Value::Array(render_mappings(paths)));
                rows.push(json!({"op": "version_paths_mappings", "result": result}));
            }
            "version_paths_identity" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "version_paths_identity ran before a load action built the package"
                            .to_owned(),
                    );
                };
                let retrieval = package.version_paths();
                let other = package.version_paths();
                let first = retrieval.paths();
                let second = retrieval.paths();
                let third = other.paths();
                let sizes = match (first, third) {
                    (Some(first), Some(third)) => first.len() == third.len(),
                    (None, None) => true,
                    _ => false,
                };
                rows.push(json!({
                    "op": "version_paths_identity",
                    "result": [same_mappings(first, second), same_mappings(first, third)],
                    "shape": [first.is_some(), second.is_some(), third.is_some(), sizes],
                }));
            }
            other => {
                return Outcome::Failed(format!(
                    "the version-paths cases sent unsupported action {other:?}"
                ))
            }
        }
    }
    Outcome::Observed(ordered(rows))
}

/// Resolve a case's reviewed gap. The case id selects the record and the
/// request's own operation has to agree with it: subject dispatch alone cannot
/// make a caller's arbitrary operation label authoritative.
fn gap(request: &Value, case: &str) -> Outcome {
    let requested = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match GAPS.iter().find(|gap| gap.case == case) {
        Some(gap) if gap.operation == requested => {
            Outcome::missing(gap.operation, gap.authority, gap.signature, gap.home)
        }
        Some(gap) => Outcome::Failed(format!(
            "case {case:?} requests operation {requested:?}, but its reviewed gap names {:?}",
            gap.operation
        )),
        None => Outcome::Failed(format!(
            "no reviewed packageJson handler or gap for case {case:?}"
        )),
    }
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != SUBJECT {
        return None;
    }
    let case = request
        .get("case")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(match case {
        "config/packagejson/expected-validity-survives-a-repeated-field" => {
            expected_validity(request)
        }
        "config/packagejson/version-paths-selection-and-mappings"
        | "config/packagejson/version-paths-mappings-are-rebuilt-per-retrieval"
        | "config/packagejson/version-paths-traces-replay-on-every-traced-call" => {
            version_paths(request)
        }
        other => gap(request, other),
    })
}
