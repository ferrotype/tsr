//! The compiler-option semantics leaf group: the defaulting getters of the
//! pinned `internal/core/compileroptions.go`, its reflective `Clone`, and the
//! three hand-written renderers that turn an enum into text.
//!
//! Almost every getter is a defaulting rule and the defaults interact, so a
//! request carries the whole option value it wants -- a JSON object keyed by
//! the pinned `json` tags -- and the action names the getter to call. A reader
//! can tell what each action set without reading this file.
//!
//! The `roster!` table below is the single place this module sets, renders and
//! names fields from, in the pinned struct's declaration order, so a field's
//! kind is declared once. `set_field` refuses a name
//! the table does not carry, so a field the roster forgot fails the case
//! loudly instead of being ignored into an accidental agreement; the Go probe
//! refuses the same name for the same reason, and derives its own roster by
//! reflection over the pinned struct rather than repeating this list.
//!
//! Three operations have no production home and are recorded as gaps:
//! `JsxEmit.String`, `ModuleResolutionKind.String` and
//! `NewLineKind.GetNewLineCharacter`. Unlike the five generated stringers, all
//! three are hand written in the pinned file and two of them panic rather than
//! falling back to a numeric form, so there is no generated fallback to port.
//! Nothing here emulates any of them.
//!
//! A panic reachable from this group is raised by the pinned source itself, so
//! its literal text is the contract and is recorded as written -- unlike the
//! runtime panics a neighbouring group has to reduce to a class.

use std::panic::AssertUnwindSafe;

use serde_json::{json, Value};
use tsr_core::{CompilerOptions, ModuleKind, PathMappings, Tristate};
use tsr_jsstring::JsString;

use crate::api::{action_i64, action_op, action_str, actions, ordered, subject, Outcome};

/// Sets one option from its request value. The arm is chosen by the roster's
/// kind token, so a field's kind is declared once and drives set, render and
/// the tristate lookup alike.
macro_rules! field_set {
    (tri, $slot:expr, $value:expr) => {
        set_tristate(&mut $slot, $value)
    };
    (enumeration, $slot:expr, $value:expr) => {
        set_enumeration(&mut $slot.0, $value)
    };
    (text, $slot:expr, $value:expr) => {
        set_text(&mut $slot, $value)
    };
    (texts, $slot:expr, $value:expr) => {
        set_texts(&mut $slot, $value)
    };
    (count, $slot:expr, $value:expr) => {
        set_count(&mut $slot, $value)
    };
    (paths, $slot:expr, $value:expr) => {
        set_paths(&mut $slot, $value)
    };
}

/// Renders one option the way the Go probe renders the same field: a tristate
/// and an enum as their underlying number, a string as text, an absent slice,
/// pointer or path map as null.
macro_rules! field_get {
    (tri, $slot:expr) => {
        json!($slot.0)
    };
    (enumeration, $slot:expr) => {
        json!($slot.0)
    };
    (text, $slot:expr) => {
        text(($slot).as_bytes())
    };
    (texts, $slot:expr) => {
        texts_value(($slot).as_ref())
    };
    (count, $slot:expr) => {
        match $slot {
            Some(value) => json!(value),
            None => Value::Null,
        }
    };
    (paths, $slot:expr) => {
        paths_value(($slot).as_ref())
    };
}

/// The tristate behind a named option, for `GetStrictOptionValue`. Only a
/// tristate field answers; any other kind is a malformed request.
macro_rules! field_tri {
    (tri, $slot:expr) => {
        Some($slot)
    };
    (enumeration, $slot:expr) => {
        None
    };
    (text, $slot:expr) => {
        None
    };
    (texts, $slot:expr) => {
        None
    };
    (count, $slot:expr) => {
        None
    };
    (paths, $slot:expr) => {
        None
    };
}

macro_rules! roster {
    ($($json:literal => $field:ident : $kind:ident,)+) => {
        /// Every option the pinned struct declares, in declaration order.
        const FIELD_NAMES: &[&str] = &[$($json),+];

        fn set_field(options: &mut CompilerOptions, name: &str, value: &Value) -> Result<(), String> {
            match name {
                $($json => field_set!($kind, options.$field, value),)+
                _ => Err(format!(
                    "{name:?} is not an option the pinned CompilerOptions declares"
                )),
            }
        }

        fn field_values(options: &CompilerOptions) -> Vec<Value> {
            vec![$(field_get!($kind, options.$field)),+]
        }

        fn field_tristate(options: &CompilerOptions, name: &str) -> Option<Tristate> {
            match name {
                $($json => field_tri!($kind, options.$field),)+
                _ => None,
            }
        }
    };
}

roster! {
    "allowJs" => allow_js: tri,
    "allowArbitraryExtensions" => allow_arbitrary_extensions: tri,
    "allowImportingTsExtensions" => allow_importing_ts_extensions: tri,
    "allowNonTsExtensions" => allow_non_ts_extensions: tri,
    "allowUmdGlobalAccess" => allow_umd_global_access: tri,
    "allowUnreachableCode" => allow_unreachable_code: tri,
    "allowUnusedLabels" => allow_unused_labels: tri,
    "assumeChangesOnlyAffectDirectDependencies" => assume_changes_only_affect_direct_dependencies: tri,
    "checkJs" => check_js: tri,
    "customConditions" => custom_conditions: texts,
    "composite" => composite: tri,
    "emitDeclarationOnly" => emit_declaration_only: tri,
    "emitBOM" => emit_bom: tri,
    "emitDecoratorMetadata" => emit_decorator_metadata: tri,
    "declaration" => declaration: tri,
    "declarationDir" => declaration_dir: text,
    "declarationMap" => declaration_map: tri,
    "deduplicatePackages" => deduplicate_packages: tri,
    "disableSizeLimit" => disable_size_limit: tri,
    "disableSourceOfProjectReferenceRedirect" => disable_source_of_project_reference_redirect: tri,
    "disableSolutionSearching" => disable_solution_searching: tri,
    "disableReferencedProjectLoad" => disable_referenced_project_load: tri,
    "erasableSyntaxOnly" => erasable_syntax_only: tri,
    "exactOptionalPropertyTypes" => exact_optional_property_types: tri,
    "experimentalDecorators" => experimental_decorators: tri,
    "forceConsistentCasingInFileNames" => force_consistent_casing_in_file_names: tri,
    "isolatedModules" => isolated_modules: tri,
    "isolatedDeclarations" => isolated_declarations: tri,
    "ignoreConfig" => ignore_config: tri,
    "ignoreDeprecations" => ignore_deprecations: text,
    "importHelpers" => import_helpers: tri,
    "inlineSourceMap" => inline_source_map: tri,
    "inlineSources" => inline_sources: tri,
    "init" => init: tri,
    "incremental" => incremental: tri,
    "jsx" => jsx: enumeration,
    "jsxFactory" => jsx_factory: text,
    "jsxFragmentFactory" => jsx_fragment_factory: text,
    "jsxImportSource" => jsx_import_source: text,
    "lib" => lib: texts,
    "libReplacement" => lib_replacement: tri,
    "locale" => locale: text,
    "mapRoot" => map_root: text,
    "module" => module: enumeration,
    "moduleResolution" => module_resolution: enumeration,
    "moduleSuffixes" => module_suffixes: texts,
    "moduleDetection" => module_detection: enumeration,
    "newLine" => new_line: enumeration,
    "noEmit" => no_emit: tri,
    "noCheck" => no_check: tri,
    "noErrorTruncation" => no_error_truncation: tri,
    "noFallthroughCasesInSwitch" => no_fallthrough_cases_in_switch: tri,
    "noImplicitAny" => no_implicit_any: tri,
    "noImplicitThis" => no_implicit_this: tri,
    "noImplicitReturns" => no_implicit_returns: tri,
    "noEmitHelpers" => no_emit_helpers: tri,
    "noLib" => no_lib: tri,
    "noPropertyAccessFromIndexSignature" => no_property_access_from_index_signature: tri,
    "noUncheckedIndexedAccess" => no_unchecked_indexed_access: tri,
    "noEmitOnError" => no_emit_on_error: tri,
    "noUnusedLocals" => no_unused_locals: tri,
    "noUnusedParameters" => no_unused_parameters: tri,
    "noResolve" => no_resolve: tri,
    "noImplicitOverride" => no_implicit_override: tri,
    "noUncheckedSideEffectImports" => no_unchecked_side_effect_imports: tri,
    "outDir" => out_dir: text,
    "paths" => paths: paths,
    "preserveConstEnums" => preserve_const_enums: tri,
    "preserveSymlinks" => preserve_symlinks: tri,
    "project" => project: text,
    "resolveJsonModule" => resolve_json_module: tri,
    "resolvePackageJsonExports" => resolve_package_json_exports: tri,
    "resolvePackageJsonImports" => resolve_package_json_imports: tri,
    "removeComments" => remove_comments: tri,
    "rewriteRelativeImportExtensions" => rewrite_relative_import_extensions: tri,
    "reactNamespace" => react_namespace: text,
    "rootDir" => root_dir: text,
    "rootDirs" => root_dirs: texts,
    "skipLibCheck" => skip_lib_check: tri,
    "stableTypeOrdering" => stable_type_ordering: tri,
    "strict" => strict: tri,
    "strictBindCallApply" => strict_bind_call_apply: tri,
    "strictBuiltinIteratorReturn" => strict_builtin_iterator_return: tri,
    "strictFunctionTypes" => strict_function_types: tri,
    "strictNullChecks" => strict_null_checks: tri,
    "strictPropertyInitialization" => strict_property_initialization: tri,
    "stripInternal" => strip_internal: tri,
    "skipDefaultLibCheck" => skip_default_lib_check: tri,
    "sourceMap" => source_map: tri,
    "sourceRoot" => source_root: text,
    "suppressOutputPathCheck" => suppress_output_path_check: tri,
    "target" => target: enumeration,
    "traceResolution" => trace_resolution: tri,
    "tsBuildInfoFile" => ts_build_info_file: text,
    "typeRoots" => type_roots: texts,
    "types" => types: texts,
    "useDefineForClassFields" => use_define_for_class_fields: tri,
    "useUnknownInCatchVariables" => use_unknown_in_catch_variables: tri,
    "verbatimModuleSyntax" => verbatim_module_syntax: tri,
    "maxNodeModuleJsDepth" => max_node_module_js_depth: count,
    "allowSyntheticDefaultImports" => allow_synthetic_default_imports: tri,
    "alwaysStrict" => always_strict: tri,
    "baseUrl" => base_url: text,
    "downlevelIteration" => downlevel_iteration: tri,
    "esModuleInterop" => es_module_interop: tri,
    "outFile" => out_file: text,
    "configFilePath" => config_file_path: text,
    "noDtsResolution" => no_dts_resolution: tri,
    "pathsBasePath" => paths_base_path: text,
    "diagnostics" => diagnostics: tri,
    "extendedDiagnostics" => extended_diagnostics: tri,
    "generateCpuProfile" => generate_cpu_profile: text,
    "generateTrace" => generate_trace: text,
    "listEmittedFiles" => list_emitted_files: tri,
    "listFiles" => list_files: tri,
    "explainFiles" => explain_files: tri,
    "listFilesOnly" => list_files_only: tri,
    "noEmitForJsFiles" => no_emit_for_js_files: tri,
    "preserveWatchOutput" => preserve_watch_output: tri,
    "pretty" => pretty: tri,
    "version" => version: tri,
    "watch" => watch: tri,
    "showConfig" => show_config: tri,
    "build" => build: tri,
    "help" => help: tri,
    "all" => all: tri,
    "runExternalCode" => run_external_code: tri,
    "pprofDir" => pprof_dir: text,
    "singleThreaded" => single_threaded: tri,
    "quiet" => quiet: tri,
    "checkers" => checkers: count,
}

pub fn observe(request: &Value) -> Option<Outcome> {
    let replayed = match subject(request) {
        "options.Getters" => getters(actions(request)),
        "options.Clone" => clone_roster(actions(request)),
        "options.ModuleKind" => module_kind(actions(request)),
        "options.JsxEmitText" => return Some(missing_jsx_emit_text()),
        "options.ModuleResolutionText" => return Some(missing_module_resolution_text()),
        "options.NewLineText" => return Some(missing_new_line_text()),
        "options.NewLineFromText" => return Some(missing_new_line_from_text()),
        _ => return None,
    };
    Some(match replayed {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

/// The classifier that turns a newline literal into a NewLineKind. Unlike the
/// renderers above it is a free function rather than a method, and unlike them
/// it has no partial rendering anywhere in the port to point at: the port reads
/// `options.new_line` as an already-classified enum and never parses the text.
fn missing_new_line_from_text() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/compileroptions.go:GetNewLineKind",
        "tsc/internal/core/compileroptions.go:GetNewLineKind",
        "a free function on tsr_core mapping the two-byte literal CR LF to NewLineKind::CRLF, the \
         one-byte LF to NewLineKind::LF, and every other text -- including the empty string, a \
         lone CR, and any text merely containing one of the two -- to NewLineKind::NONE. The \
         pinned switch closes its domain with a default arm rather than a panic, so there is no \
         error path to port",
        "crates/tsr_core/src/compiler_options.rs (NewLineKind declares NONE, CRLF and LF at :52-56 \
         and nothing that reads a literal). The port does classify this text, but not as this \
         operation and not anywhere a caller could reach: \
         crates/tsr_printer/src/change_tracker_writer.rs:282-287 carries the port marker for it on \
         an inline match inside a PrinterOptions struct literal, private to tsr_printer and \
         written for that one call site. crates/tsr_tsoptions/src/fixture_options.rs:194 is not a \
         second one: it substitutes CRLF for NONE on an already-classified value and never looks \
         at text",
    )
}

fn missing_jsx_emit_text() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/compileroptions.go:JsxEmit.String",
        "tsc/internal/core/compileroptions.go:JsxEmit.String",
        "a Display impl or an as_str on tsr_core::JsxEmit rendering preserve/react-native/react/\
         react-jsx/react-jsxdev for 1..=5, and panicking on the two arms the pinned switch panics \
         on -- \"should not use zero value of JsxEmit\" for JsxEmitNone and \"unhandled case in \
         JsxEmit.String\" for every other value. It is hand written, not generated by stringer, \
         so there is no numeric fallback to port: the domain is closed by a panic",
        "crates/tsr_core/src/compiler_options.rs (JsxEmit declares the six constants and nothing \
         else). The nearest rendering in the port is not this operation: \
         crates/tsr_compiler/src/verify_options.rs:576 fn jsx_name is private to that crate, \
         names only REACT_JSX/REACT_JSX_DEV/REACT and answers the empty string for the other \
         three declared values and for the zero value, where the pin panics",
    )
}

fn missing_module_resolution_text() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/compileroptions.go:ModuleResolutionKind.String",
        "tsc/internal/core/compileroptions.go:ModuleResolutionKind.String",
        "a Display impl or an as_str on tsr_core::ModuleResolutionKind rendering Classic/Node10/\
         Node16/NodeNext/Bundler for 1/2/3/99/100, and panicking on the two arms the pinned \
         switch panics on -- \"should not use zero value of ModuleResolutionKind\" for the zero \
         value and \"unhandled case in ModuleResolutionKind.String\" for every other value. The \
         pinned comment says stringer is deliberately not used here because the names are \
         user-facing in --traceResolution, so the panic on the zero value is the contract, not \
         an oversight",
        "crates/tsr_core/src/compiler_options.rs (ModuleResolutionKind declares the six \
         constants and nothing else). The port has two partial renderings instead, neither \
         reachable from outside its crate and neither matching: \
         crates/tsr_module/src/resolver.rs:249 is an inline match in the trace path that answers \
         b\"Unknown\" for Classic, Node10 and the zero value where the pin renders Classic and \
         Node10 and panics; crates/tsr_compiler/src/verify_options.rs:594 fn resolution_name \
         names only NODE16 and NODE_NEXT and raises the pin's \"unhandled case\" text for \
         Classic, Node10 and Bundler, which the pin renders",
    )
}

fn missing_new_line_text() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/compileroptions.go:NewLineKind.GetNewLineCharacter",
        "tsc/internal/core/compileroptions.go:NewLineKind.GetNewLineCharacter",
        "tsr_core::NewLineKind::new_line_character(self) -> &'static [u8] answering b\"\\r\\n\" \
         for NewLineKindCRLF and b\"\\n\" for every other value. The pinned switch has one named \
         case and a default, so NewLineKindNone, NewLineKindLF and every out-of-domain value all \
         answer the single linefeed; a port written as a three-way match that panicked on None \
         would be wrong",
        "crates/tsr_core/src/compiler_options.rs (NewLineKind declares the three constants and \
         nothing else). crates/tsr_printer/src/printer.rs:69 fn new_line_character has exactly \
         this body but is private to tsr_printer and carries no port marker, so no consumer of \
         tsr_core can reach it",
    )
}

/// The option value an action asks for, plus how many option names it set.
/// A name the roster does not carry fails the case: the Go probe refuses it
/// too, so neither side can quietly ignore a field and agree by accident.
fn build(action: &Value) -> Result<(CompilerOptions, usize), String> {
    let mut options = CompilerOptions::default();
    let Some(raw) = action.get("options") else {
        return Ok((options, 0));
    };
    let Some(members) = raw.as_object() else {
        return Err(format!(
            "action {:?} carries an `options` that is not an object",
            action_op(action)
        ));
    };
    for (name, value) in members {
        set_field(&mut options, name, value)?;
    }
    Ok((options, members.len()))
}

fn getters(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        let (options, applied) = build(action)?;
        rows.push(match op {
            "get_allow_js" => flag(op, applied, options.allow_js()),
            "get_emit_script_target" => {
                number(op, applied, i64::from(options.emit_script_target().0))
            }
            "get_emit_module_kind" => number(op, applied, i64::from(options.emit_module_kind().0)),
            "get_module_resolution_kind" => {
                number(op, applied, i64::from(options.module_resolution_kind().0))
            }
            "get_emit_module_detection_kind" => number(
                op,
                applied,
                i64::from(options.emit_module_detection_kind().0),
            ),
            "get_resolve_json_module" => flag(op, applied, options.resolve_json_module()),
            "get_resolve_package_json_exports" => {
                flag(op, applied, options.resolve_package_json_exports())
            }
            "get_resolve_package_json_imports" => {
                flag(op, applied, options.resolve_package_json_imports())
            }
            "get_jsx_transform_enabled" => flag(op, applied, options.jsx_transform_enabled()),
            "get_isolated_modules" => flag(op, applied, options.isolated_modules()),
            "get_emit_standard_class_fields" => {
                flag(op, applied, options.emit_standard_class_fields())
            }
            "get_emit_declarations" => flag(op, applied, options.emit_declarations()),
            "get_are_declaration_maps_enabled" => {
                flag(op, applied, options.declaration_maps_enabled())
            }
            "uses_wildcard_types" => flag(op, applied, options.uses_wildcard_types()),
            "get_allow_importing_ts_extensions" => {
                flag(op, applied, options.allow_importing_ts_extensions())
            }
            "allow_importing_ts_extensions_from" => {
                let file_name = action_str(action, "file_name");
                json!({
                    "op": op, "applied": applied, "file_name": file_name,
                    "result": options.allow_importing_ts_extensions_from(file_name.as_bytes()),
                })
            }
            "get_use_define_for_class_fields" => {
                flag(op, applied, options.use_define_for_class_fields())
            }
            "is_incremental" => flag(op, applied, options.is_incremental()),
            "should_preserve_const_enums" => {
                flag(op, applied, options.should_preserve_const_enums())
            }
            "get_strict_option_value" => {
                let name = action_str(action, "strict_option");
                let value = field_tristate(&options, name).ok_or_else(|| {
                    format!(
                        "strict_option {name:?} is not a tristate option of the pinned \
                         CompilerOptions"
                    )
                })?;
                json!({
                    "op": op,
                    "applied": applied,
                    "strict_option": name,
                    "option_value": i64::from(value.0),
                    "result": options.strict_option_value(value),
                })
            }
            "get_paths_base_path" => {
                let directory = action_str(action, "current_directory");
                json!({
                    "op": op,
                    "applied": applied,
                    "current_directory": directory,
                    "result": text(options.paths_base_path(directory.as_bytes())),
                })
            }
            "get_effective_type_roots" => {
                let directory = action_str(action, "current_directory");
                // The result is a pair, carried as a two-element list: an
                // object nested in an ordered payload would not survive
                // canonicalisation. The call can panic, and the pinned panic
                // text is the contract, so it is recorded as written.
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    let (roots, from_config) =
                        tsr_module::effective_type_roots(&options, directory.as_bytes());
                    json!([
                        roots
                            .iter()
                            .map(|root| text(root.as_bytes()))
                            .collect::<Vec<Value>>(),
                        from_config,
                    ])
                }));
                json!({
                    "op": op,
                    "applied": applied,
                    "current_directory": directory,
                    "result": result,
                    "panic": panicked,
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn clone_roster(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "clone" => {
                let (source, applied) = build(action)?;
                // The derived Clone is the counterpart of the pinned reflective
                // Clone. The row names every field with the value the copy
                // carries and whether it still matches the source, so a copy
                // that dropped a deprecated or internal field names itself
                // instead of hiding inside a whole-struct comparison.
                let copy = source.clone();
                let before = field_values(&source);
                let after = field_values(&copy);
                let fields: Vec<Value> = FIELD_NAMES
                    .iter()
                    .zip(before.iter().zip(after.iter()))
                    .map(|(name, (source_value, clone_value))| {
                        json!([*name, clone_value, source_value == clone_value])
                    })
                    .collect();
                json!({
                    "op": op,
                    "applied": applied,
                    "field_count": fields.len(),
                    "fields": fields,
                })
            }
            "clone_then_mutate_source" => {
                // The pinned Clone copies a pointer-backed field as the pointer
                // and a slice as its header, so a write through the source
                // afterwards is visible in the clone. The derived Clone here
                // owns its data, so it is not. The field comparison above
                // cannot see the difference, because at the instant of the
                // clone the two agree.
                let (mut source, applied) = build(action)?;
                let copy = source.clone();
                if source.checkers.is_some() {
                    source.checkers = Some(action_i64(action, "mutate_checkers") as isize);
                }
                if let Some(types) = source.types.as_mut() {
                    if let Some(first) = types.first_mut() {
                        *first = JsString::from_bytes(
                            action_str(action, "mutate_type").as_bytes().to_vec(),
                        );
                    }
                }
                // Through the roster's own renderers, not a second copy of
                // them: an absent slice is null here exactly as it is in the
                // field rows, which is what the Go probe's nil-returning
                // phase1OptionStrings answers. Rendering it as an empty list
                // instead made the no-mutation control differ on its own
                // account, which is worse than having no control.
                let read_back = |options: &CompilerOptions| {
                    json!([
                        field_get!(count, options.checkers),
                        field_get!(texts, options.types),
                    ])
                };
                json!({
                    "op": op,
                    "applied": applied,
                    "after_mutation": [
                        ["source", read_back(&source)],
                        ["clone", read_back(&copy)],
                    ],
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn module_kind(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "is_non_node_esm" => {
                let value = action_i64(action, "module_kind");
                let kind = ModuleKind(i32::try_from(value).map_err(|_| {
                    format!("module_kind {value} is outside an int32 in a leaves/options action")
                })?);
                json!({ "op": op, "module_kind": value, "result": kind.is_non_node_esm() })
            }
            "supports_import_attributes" => {
                let value = action_i64(action, "module_kind");
                let kind = ModuleKind(i32::try_from(value).map_err(|_| {
                    format!("module_kind {value} is outside an int32 in a leaves/options action")
                })?);
                json!({
                    "op": op, "module_kind": value,
                    "result": kind.supports_import_attributes(),
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn flag(op: &str, applied: usize, result: bool) -> Value {
    json!({ "op": op, "applied": applied, "result": result })
}

fn number(op: &str, applied: usize, result: i64) -> Value {
    json!({ "op": op, "applied": applied, "result": result })
}

fn unsupported(op: &str) -> Value {
    json!({ "op": op, "unsupported_action": op })
}

/// Bytes as JSON text. Every option string these cases carry is ASCII; the
/// lossy step matches `encoding/json`, which replaces invalid UTF-8 with
/// U+FFFD when it marshals a Go string.
fn text(value: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(value).into_owned())
}

fn texts_value(slot: Option<&Vec<JsString>>) -> Value {
    match slot {
        Some(items) => Value::Array(items.iter().map(|item| text(item.as_bytes())).collect()),
        None => Value::Null,
    }
}

fn paths_value(slot: Option<&PathMappings>) -> Value {
    match slot {
        Some(entries) => Value::Array(
            entries
                .iter()
                .map(|(key, values)| json!([text(key.as_bytes()), texts_value(values.as_ref())]))
                .collect(),
        ),
        None => Value::Null,
    }
}

fn as_i64(name: &str, value: &Value) -> Result<i64, String> {
    value
        .as_i64()
        .ok_or_else(|| format!("option {name:?} needs a whole number, not {value}"))
}

fn as_text(name: &str, value: &Value) -> Result<JsString, String> {
    let item = value
        .as_str()
        .ok_or_else(|| format!("option {name:?} needs a string, not {value}"))?;
    Ok(JsString::from_bytes(item.as_bytes().to_vec()))
}

fn set_tristate(slot: &mut Tristate, value: &Value) -> Result<(), String> {
    let number = as_i64("a tristate", value)?;
    let byte = u8::try_from(number)
        .map_err(|_| format!("tristate {number} is outside the byte a Tristate is"))?;
    *slot = Tristate(byte);
    Ok(())
}

fn set_enumeration(slot: &mut i32, value: &Value) -> Result<(), String> {
    let number = as_i64("an enum", value)?;
    *slot = i32::try_from(number)
        .map_err(|_| format!("enum value {number} is outside the int32 these enums are"))?;
    Ok(())
}

fn set_text(slot: &mut JsString, value: &Value) -> Result<(), String> {
    *slot = as_text("a string option", value)?;
    Ok(())
}

/// A Go `[]string` option. `null` is the nil slice and `[]` the empty non-nil
/// one, which several getters tell apart.
fn set_texts(slot: &mut Option<Vec<JsString>>, value: &Value) -> Result<(), String> {
    if value.is_null() {
        *slot = None;
        return Ok(());
    }
    let items = value
        .as_array()
        .ok_or_else(|| format!("a string-list option needs a list or null, not {value}"))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(as_text("a string-list option", item)?);
    }
    *slot = Some(out);
    Ok(())
}

fn set_count(slot: &mut Option<isize>, value: &Value) -> Result<(), String> {
    if value.is_null() {
        *slot = None;
        return Ok(());
    }
    let number = as_i64("a count option", value)?;
    *slot = Some(
        isize::try_from(number)
            .map_err(|_| format!("count {number} is outside the machine int it is stored in"))?,
    );
    Ok(())
}

/// The `paths` map, carried as an entry list so its insertion order survives
/// the request and a nil value list stays distinguishable from an empty one.
fn set_paths(slot: &mut Option<PathMappings>, value: &Value) -> Result<(), String> {
    if value.is_null() {
        *slot = None;
        return Ok(());
    }
    let entries = value
        .as_array()
        .ok_or_else(|| format!("`paths` needs an entry list or null, not {value}"))?;
    let mut out = PathMappings::with_capacity(entries.len());
    for entry in entries {
        let pair = entry
            .as_array()
            .filter(|pair| pair.len() == 2)
            .ok_or_else(|| format!("a `paths` entry must be a [key, values] pair, not {entry}"))?;
        let key = as_text("a `paths` key", &pair[0])?;
        let values = if pair[1].is_null() {
            None
        } else {
            let items = pair[1]
                .as_array()
                .ok_or_else(|| format!("a `paths` value needs a list or null, not {}", pair[1]))?;
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(as_text("a `paths` value", item)?);
            }
            Some(values)
        };
        out.insert(key, values);
    }
    *slot = Some(out);
    Ok(())
}

/// Runs `call` and records a panic's literal text. Every panic reachable from
/// this group is raised by the pinned source itself and its wording is the
/// contract, so nothing here is reduced to a class. The default hook is
/// silenced for the call, because the panic is the expected result.
fn guarded(call: impl FnOnce() -> Value + std::panic::UnwindSafe) -> (Value, String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(call);
    std::panic::set_hook(previous);
    match result {
        Ok(value) => (value, String::new()),
        Err(payload) => (Value::Null, panic_text(payload.as_ref())),
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
        .to_owned()
}
