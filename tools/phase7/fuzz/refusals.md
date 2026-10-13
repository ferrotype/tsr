# R5 refusal review

Status: **reviewed inventory; open functionality remains**, 2026-10-11.

This is a source review of the lexical inventory produced by [audit.py](audit.py),
not an approved-exclusion list or a passing metric. The original inventory had
216 matching lines. The initial refreshed inventory had 217: the added line injected
`Unsupported("test visit failure")` in `esmodule_stack_tests.rs`. The fresh
review input is preserved outside `target` at
`../ts-rust-benchmarks/2026-10-11/r5-refusal-semantic-input-20261011.json`.
After the four narrow implementations below, the refreshed lexical inventory has
212 matching lines, saved as `r5-refusal-after-four-narrow-fixes.json` beside that
review input. The subsequent project-reference importer fix leaves 210 matching
lines, saved as `r5-refusal-after-projectref.json`. These counts are not acceptance
metrics.
Function identities below, rather than frozen line numbers, locate behavior.
All paths are relative to the repository root.

The review distinguishes:

- **Open implementation**: Rust refuses a branch for which the pin supplies
  behavior. Implement it and retain a differential regression, or obtain an
  explicit scope decision. Documentation here does not approve the gap.
- **Open invariant**: the refusal protects a representation, recursion or
  dispatch assumption. The local condition is known, but valid-input
  unreachability has not been established by this review. Do not treat absence
  from passing suites as a proof.
- **Contract guard**: an explicit invalid-input, missing-owner, immutable-host
  or wrong-factory boundary. This documents the required caller contract; a
  compiler-produced violation is still a bug.
- **Handled signal**: an internal refusal is intercepted and retried by the
  caller. Its presence does not mean that the feature is unsupported.
- **Approved scope**: only a decision already recorded by the owner qualifies.

The original inventory review ran no compiler batches or performance measurements.
The focused native/Rust regressions below were added afterward. The review
covers the searched wording only:
other errors, panics, unimplemented macros and source outside `crates/**/*.rs`
need their own review. New source can change the lexical inventory without
changing any functionality.

## Remaining review scope

The five narrow branches described below now have pinned behavior and focused
Rust regressions. The tuple-label refusal is a constructor guard, as established
below; it was previously misclassified as missing destructuring display.

`typeToTypeNode: unsupported type family`, recursive export-map refusal, and
deferred-property refusal remain open invariants below. They need targeted
construction/reachability work before either implementation closure or an
unreachability claim. The old C1 plan already requires a witness or reviewed
constructor invariant for its representation guards; see
[C1.4](../../../docs/PHASE2-C1-plan.md#c14-arrays-tuples-enums-and-literals).

## Five implemented narrow branches

`tsr_printer/src/emit_context/factory_helpers.rs::get_name(None)` now constructs
exactly the pin's node-generated identifier with a nil source. The pinned
`nil_name_native_test.go` directly executes local, export and declaration name
helpers: all succeed with kind 4, nil source and no emit flags. Printing each
result separately panics on the nil original node; the Rust regression preserves
that invalid-input panic boundary. This closes factory construction only, without
claiming an ordinary source-transformer caller passes nil. Frozen native rows are
`crates/tsr_printer/src/emit_context/nil_name_native.json`.

`tsr_checker/src/external_resolution.rs::resolve_external_module_reference`
now emits the pin's alternate-resolution diagnostic chain, including the
repopulate marker. The caller's module reference and the implicit-any caller's
package ID remain distinct inputs to chain construction. The native
`alternate_resolution_native_test.go` executes `Checker.GetDiagnostics` over a
real parsed/bound program with an explicitly supplied unresolved module result
and nonempty alternate result. Primary and `@types` declaration locations match
Rust's complete TS2307/TS6278 chain, positions, arguments and repopulate fields;
rows are `crates/tsr_compiler/src/alternate_resolution_native.json`.

This supplied-host-state witness proves the removed refusal branch's behavior,
not ordinary resolver reachability. The native resolver currently creates
`AlternateResult` for resolved JavaScript packages; the checker normally consumes
that result earlier via implicit-any handling. Four ordinary package fixtures did
not trigger the former refusal. No reachability claim or parity waiver follows.
Both native witnesses execute pin `1f70213d4922b434345f639b441681e470c7cfc1`
with Go 1.27.1 in temporary exports of its production source.

`tsr_checker/src/node_builder_names.rs::access_from_symbol_chain_worker`
now builds the pin's indexed type query for an entity-name computed property.
A normal parsed/bound source witnesses the branch:
`declare const key: unique symbol; class Symbols { static readonly [key]: unique symbol; } const value = Symbols[key];`.
Querying the initializer's type and printing with default or fully qualified
flags produces `(typeof Symbols)[typeof key]` in both implementations. Querying
the variable instead widens to `symbol`, so that would miss this branch. The
Rust regression also transfers generated syntax, drops its checker/program, and
prints it again to verify retention of the original computed-name expression.
Frozen source and native rows are
`crates/tsr_compiler/tests/fixtures/computed_entity_native.json`; its adjacent Go
producer executes the public checker display API at the pin.

`tsr_checker/src/class_properties.rs::type_of_accessors` now preserves the
pin's circular auto-accessor fallback: type `any`, no file semantic diagnostic,
and a fileless TS2502 at `(0, 0)` with argument `x`. Native direct controls cover
`typeof this.x`, `A["x"]` and `typeof A.prototype.x` annotations. The pin calls
`error(setter, ...)` with a nil setter; `NewDiagnosticForNode(nil)` creates a
fileless diagnostic and `addDiagnostic` stores it globally. It is not a no-op.
The controls compare globals both before and after semantic checking, as well
as the type and file diagnostics. The earlier budget schedule reads globals
before semantic checking, so its zero-diagnostic observation alone did not prove
that no TS2502 exists. Frozen rows and the native producer are
`crates/tsr_compiler/tests/fixtures/circular_accessor_native.json` and its
adjacent `circular_accessor_native_test.go` producer.

`tsr_compiler/src/checker_module_specifiers.rs::module_specifier_paths` now
orders candidate paths relative to a referenced output importer's source.
Specifier computation still uses the actual output's imports, options and
resolution mode. The native project contains source/output `a` and `b` files,
with competing `./a` and `../out/a` imports. Querying the output `b.d.ts` variable
produces `() => import("./a").Foo`; the root importer produces
`() => import("./ref/src/a").Foo`. Both match the focused Rust regression
`an_importer_that_is_a_referenced_output_orders_paths_from_its_source`.
Exact input files, queries and outputs are frozen in
`crates/tsr_compiler/tests/fixtures/project_reference_native.json`; its adjacent
Go producer executes public checker APIs over the parsed project-reference
configuration at the pin.

## Tuple-label constructor guard

`array_or_tuple_node`'s nonidentifier `labeled_declaration` refusal does not
replace pinned binding-pattern label generation. The pin's `getTupleElementLabel`
calls `labeledDeclaration.Name().Text()` immediately when a label is supplied.
`getTupleElementLabelFromBindingElement` is used only without that label, for a
rest parameter. Supplied object/array binding-pattern labels both panic in the
pin with `Unhandled case in Node.Text: *ast.BindingPattern`; the frozen direct
helper witness is `tuple_label_guard_native.json` and its adjacent Go producer
under `crates/tsr_compiler/tests/fixtures`.

Ordinary source tuple labels originate from named tuple members. Signature
inference's `getNameableDeclarationAtPosition` accepts named tuple members or
identifier-named parameters, matching Rust's `parameter_label_at`. Synthetic
spread arguments and tuple normalization copy existing labels. Three native
source controls cover destructured fixed parameters, destructured rest
parameters and ordinary identifier parameters: inferred tuple display omits
the destructuring names and preserves ordinary labels. Rows and producer are
`tuple_label_native.json` and its adjacent Go file. The focused Rust display
control in `checker_semantics.rs` matches all three frozen displays. The shared
[native reproduction command](README.md#reproduce-the-narrow-native-refusal-controls)
reruns all seven producers and checks every frozen JSON byte.

This establishes the constructor contract for the reviewed producers and
rejects the proposed destructuring port; it does not equate Rust's refusal
result with the pin's panic or prove arbitrary constructed ASTs valid. A new
producer that supplies a nonidentifier label remains a bug requiring a witness.

## Root/reference extension guard classification

`tsr_compiler/src/loader.rs::Loader::load_worker` retains an **open invariant**
for an unsupported extension that reaches its task queue. It is not an ordinary
root/path-reference diagnostic implementation gap: `run_inner` passes each root
through `file_reference` before calling `load`, and
`resolve_tripleslash_path_reference` calls the same function before a successful
reference is queued. With `allowNonTsExtensions` disabled, `file_reference`
already returns the pinned unsupported-extension or enable-`allowJs` diagnostic.
With that option enabled, the later `load_worker` guard is disabled as well.
Thus these two ordinary routes do not enter the refusal. Other task producers
(resolver results, library replacement, project-reference redirects and mapped
sources) still require their own constructor/dispatch proof; no global
unreachability or approved exclusion is claimed. This classification comes from
the caller/guard control flow, not a new extension differential capture.

## Entries that are not new production refusals

There are 31 test-only matching lines: injected failures, assertions, expected
messages and platform skip messages. They occur in:

```text
crates/tsr_api/src/printing/scratch_checks.rs
crates/tsr_ast/src/evaluator/tests.rs
crates/tsr_build/src/tests.rs
crates/tsr_checker/src/node_builder_cache_tests.rs
crates/tsr_checker/src/storage_pilot.rs (the cfg(test) assertion only)
crates/tsr_checker/src/tests.rs
crates/tsr_compiler/src/cache/tests.rs
crates/tsr_compiler/src/checker_host_tests.rs
crates/tsr_compiler/tests/checker_project_references.rs
crates/tsr_compiler/tests/checker_semantics.rs
crates/tsr_compiler/tests/diagnostic_writer.rs
crates/tsr_compiler/tests/phase3_emitter.rs
crates/tsr_execute/src/refusal_tests.rs
crates/tsr_fswatch/src/linux/tests.rs
crates/tsr_fswatch/src/watcher/tests.rs
crates/tsr_printer/src/emit_context/emit_tests.rs
crates/tsr_project/src/parse_cache/tests.rs
crates/tsr_sourcemap/src/decoder_tests.rs
crates/tsr_testrunner/src/compile.rs (test ConfigHost::resolve_content_mapper)
crates/tsr_transformers/src/moduletransforms/esmodule_stack_tests.rs
crates/tsr_tsc/src/watchmanager/tests.rs
```

The tests may witness a real limit discussed below; merely matching a test
does not introduce an additional production path.

| Other lexical entries | Review |
| --- | --- |
| `tsr_diagnostics/src/generated.rs`, TS2737 and TS6386 | Pinned language/runtime diagnostic text containing “not available”; no Rust implementation refusal. TS2737 describes target restrictions on BigInt literals. |
| `tsr_ast/src/evaluator.rs::EvaluatedValue::{Unsupported,primitive}` and its doc comment | Value-domain sentinel for a non-nil value outside primitive evaluator conversions. `primitive()` returns `None`; this is not a missing compiler operation. |
| `tsr_ast/src/local_bind.rs::LocalBindScope::set_flow` doc comment | States that incompatible concrete shapes return `false` without mutation. Caller-visible restricted mutation contract, not an emitted Unsupported error. |
| `tsr_module/src/{lib,resolver}.rs` | Two module comments and the error variant declaration. No constructor of `tsr_module::Error::Unsupported` remains in this crate. The comment about remaining package-map/project-reference boundaries is not evidence that those features are currently missing. |
| `tsr_checker/src/lib.rs`, `query.rs` and `node_builder.rs` comments; checker error declaration/Display | Documentation/declaration/formatting only. The old crate-level P2/P3 pending-feature description cannot certify the current implementation surface. Actual constructor paths are reviewed below. |
| `tsr_printer/src/lib.rs`, `tsr_transformers/src/transformer.rs`, `tsr_compiler/src/loader.rs`, `tsr_vfs/src/lib.rs` error declaration/Display | Error vocabulary only. Display strings saying “not support yet” do not distinguish invalid input from a missing implementation. |
| `tsr_compiler/src/emitter.rs::{transform_error,print_error}` and comment; `tsr_execute/src/lib.rs` error classification | Propagation/classification of another component's failure; neither supplies a successful replacement nor adds a semantic refusal. |

## Approved scope and documented boundary behavior

| Location/function | Classification and limit |
| --- | --- |
| `tsr_execute/src/command.rs` profiling branch | **Approved scope**, [ADR 0024](../../../docs/adr/0024-profiling-compatibility-out-of-scope.md). Compatibility profiling warns unavailable; native profiling remains the documented alternative. API/LSP profiling responses use different wording and are outside this lexical result, but the same decision applies. |
| `tsr_fswatch/src/watcher.rs::{Kind::Unsupported,default_watcher,name}` and `lib.rs::Error::Unavailable` Display | **Approved target scope**, [ADR 0002](../../../docs/adr/0002-native-targets-and-the-glibc-floor.md). Linux selects fanotify/inotify; macOS selects FSEvents. Other target defaults are unavailable. Explicit requests for an unavailable backend also fail. Backend/filesystem/kernel unavailability on a supported target is observable failure, not evidence of successful watching. |
| `tsr_ls/src/converters.rs::strict_string` | **Approved transport limit**, [ADR 0019](../../../docs/adr/0019-test-host-protocol-and-transport-contract-tests.md): invalid UTF-8 cannot cross this strict Unicode JSON transport. It must not be silently replaced. This does not relax compiler/source-byte contracts. |
| `tsr_vfs/src/lib.rs::FileSystem::{write_file,append_file,remove,change_times}` | **Contract guard**: default methods on immutable/read-only hosts reject mutation. A host needing mutation must override them; CLI writable OS hosts are a separate implementation. [ADR 0014](../../../docs/adr/0014-host-seams-as-traits.md) defines injected host seams, not permission to drop writes. |
| `tsr_vfs/src/os/snapshot.rs::ScopedOsFs::capture` | **Documented input limit**: snapshot capture accepts regular files/directories/symlinks and refuses other OS entries. FIFO/device/socket snapshot support is not implemented or approved out of scope by this record. Do not read a FIFO indefinitely to disguise this failure. |
| `tsr_sourcemap/src/decoder.rs::MappingsDecoder::next` (three messages) | **Malformed-input contract**: truncated segments after source index/line and extra values after name index set the decoder error and stop iteration. These strings are pinned format-error vocabulary, not absent sourcemap support; the matching decoder tests retain exact messages. |

## Compiler, project, host and diagnostic paths

| Path and function | Classification and exact condition |
| --- | --- |
| `tsr_compiler/src/loader.rs` casing-variant selection | **Contract guard**: a casing-selected file must still exist and have identical source bytes before prior source IDs/ranges are replaced. Missing text or changed bytes is refused. This prevents mixed snapshots; it does not prove that a mutable host cannot trigger it. |
| `tsr_compiler/src/loader.rs::parse_content_mapped_file` | **Contract guard**: a source classified as mapped must resolve to its configured mapper. Missing mapper is inconsistent configuration; failed initialization has its own explicit diagnostic/empty-file behavior. |
| `tsr_compiler/src/loader.rs` root/reference extension branch | **Open invariant** for other task producers; ordinary roots and path references already pass through the diagnostic-producing extension check, as described above. The Error variant declaration is separate. |
| `tsr_compiler/src/checker_module_specifiers.rs::compute_known_symlinks` | Error propagation; mutable-host/package-parser errors are defensive adapter failures. The retained resolver uses a snapshot and keeps malformed JSON as contents, as the call-site comment states. This is not a blanket package JSON refusal. |
| `tsr_compiler/src/checker_module_specifiers.rs::module_specifier_paths` | The project-reference-output importer refusal is removed; exact native/Rust display witness described above. Remaining lexical matches propagate resolver/host failures. |
| `tsr_compiler/src/checker_host.rs::{get_mode_for_usage_location,get_jsx_runtime_import_specifier}` | Error propagation from metadata. AST errors retain their class; unexpected metadata failures become named Unsupported. No substituted mode or JSX path is returned. Review any new metadata error variant at this adapter. |
| `tsr_compiler/src/metadata.rs::usage_mode` | **Contract guard**: module specifier AST must have its parent for context-sensitive mode selection. Parentless constructed syntax is refused. |
| `tsr_compiler/src/project_references.rs::module_error`, `tsr_testrunner/src/harness/config_host.rs::module_error` | Propagate module errors into host errors. Immutable resolver requirement and malformed-package failure remain explicit; these adapters are not evidence of missing project-reference support. |
| `tsr_compiler/src/statistics.rs::CheckedProgram::trace_type_records` | **Contract guard**: dump requires its compiler checker pool and a checker index belonging to that pool. It does not silently return empty statistics for wrong ownership/index. |
| `tsr_compiler/src/diagnostic_writer/mod.rs::File::line_and_character`, `pretty.rs::DiagnosticWriter::snippet` | **Contract guards**: negative/out-of-source positions, reversed span and negative squiggle length are refused. A compiler-produced offending span is a diagnostic bug to reproduce; guards are not waivers. |
| `tsr_compiler/src/diagnostic_writer/mod.rs::{category,color,localized_with_args}` | **Contract guards**: diagnostic category must be 0–3; localization key must exist; formatting arguments must satisfy the message template. Unknown category/key or formatter error remains observable. |
| `tsr_compiler/src/diagnostic_writer/mod.rs::DiagnosticSources::supplemental_sources`, `Program::supplemental_sources`; `tsr_lsp/src/diagnostics.rs::document` | **Contract guards**: independent supplemental names require an owner-aware override; Program/LSP must resolve every supplemental source instead of silently dropping diagnostics. Missing retained source is an ownership/loader bug. |
| `tsr_ls/src/converters.rs::Converters::diagnostic` | **Contract guard**: a related diagnostic exposed as an LSP location requires a source file. Fileless related-information handling is not manufactured. |
| `tsr_project/src/parse_cache.rs::ParseCache::retain` | **Contract guards**: ordinary cache cannot retain mapped bundles; reuse must have the same live source identity. Mapped files use `ContentMappedParseCache` instead. |
| `tsr_project/src/parse_cache.rs::ContentMappedParseCache::retain` | **Contract guards**: retaining a mapped program requires an existing live bundle-cache entry and matching canonical source identity. Arbitrary foreign mapped programs cannot be adopted through this cache. Documented ownership limit, not missing ordinary mapped-project support. |

## Checker paths

Paths in this section are under `crates/tsr_checker/src/`. A default arm is not
automatically an unimplemented language family: several match pinned panic
branches. Conversely, an error string naming a family is not proof that every
valid instance is handled.

| File/function | Classification and review |
| --- | --- |
| `calls.rs::resolve_call_expression` | **Contract guard**: dispatch first handles new, instanceof binary, tagged template, decorator and JSX; remaining input must be CallExpression. Matches the supported dispatch kinds then panic in pinned `resolveSignature`. Not missing all new/tagged/decorator support. |
| `check.rs::check_source_file_ex` | **Open invariant**: recursive entry into a currently Checking source fails and records failure. No valid-program unreachability proof was added here. |
| `check.rs::check_source_file_worker` | **Documented API limit**: semantic source checking accepts TS/JS/TSX/JSX, not JSON/non-script syntax. Compiler entry points must schedule JSON appropriately; the direct non-script rejection is tested. |
| `check.rs::check_source_element_worker` | **Open invariant**: final unknown statement/type arm refuses. The preceding match handles many families and explicit modifier/decorator/no-op cases. Needs an exact valid-source-kind audit, not a claim of broad missing statements. |
| `check.rs::check_variable_like` (three paths) | **Open invariants**: unexpected non-binding/nonliteral names, optional non-property declarations and property-signature initializers refuse. Some malformed TypeScript still reaches semantic checking after parser diagnostics, so these cannot be dismissed as “invalid source” without caller/grammar witnesses. |
| `check_bodies.rs::check_function_expression_context` | **Contract guard**: accepts function/arrow/method kinds; other node kinds refuse. The stale “object literal method” label does not mean all methods are missing. |
| `check_bodies.rs::assign_contextual_body_parameters` | **Open invariant**: synthetic rest parameter without value declaration refuses. Requires a constructor/scheduling witness for the contextual-rest representation. |
| `check_type_syntax.rs::check_array_tuple_syntax` | **Contract guard**: type operators keyof/unique/readonly are dispatched; unknown operator refuses, corresponding to pinned type-operator dispatch. |
| `construct.rs::new_object_type` | **Contract guard plus open constructor boundary**: unknown object flags have no payload. Generic EVOLVING_ARRAY construction refuses although Go has a payload arm. Normal Rust `flow_arrays.rs::evolving_array_type` directly allocates `Payload::EvolvingArray`; this refusal is not evidence of missing evolving-array flow support. Future generic callers must use/extend the correct constructor. |
| `diagnostics.rs::file_name`, `program.rs::program`, `symbols.rs::split_symbol`, `merge.rs::{clone_symbol_table,merge_global_symbol,append_merged_declarations}` | **Contract guards**: foreign syntax/symbol/table/declaration access requires a retained checker program. Local identities are handled without it. Missing owner/program cannot become empty data. |
| `emit_resolver.rs::DeclarationEmitResolver::unsupported` | Error adapter for the printer trait's unsupported hook. The trait default methods must be audited separately by name; the single lexical match cannot certify that all declaration-resolver callbacks are implemented. |
| `expression_errors.rs::entity_name_text` | **Contract guard**: only identifier/qualified/property entity syntax is accepted. Other AST kinds refuse. |
| `expressions.rs::check_binary_operands` | **Contract guard**: unrecognized operator reaches refusal; pinned `checkBinaryLikeExpression` also panics on its default. Malformed constructed operators remain observable. |
| `external_resolution.rs::suggested_import_extension` | Host-error adapter: file-existence failure is refused, not converted to “file absent.” Requires a functioning checker host; not missing ordinary extension suggestions. |
| `flow_assignments.rs::ensure_assignments_marked` (two paths) | **Open invariants**: reentrant root scan or already-visiting ancestor scan is refused; failed states are retained. Need a valid-flow/cycle witness before claiming unreachable. |
| `grammar_modifiers.rs::first_illegal_modifier` | **Open invariant**: unrecognized source family in first-modifier selection refuses. Grammar errors are ordinary source inputs; do not waive this solely because a modifier is illegal. |
| `handles.rs::Operation::literal_value_text`, `template.rs::literal_value_text` | **Documented conversion limit**: computed-enum sentinel has no primitive value to stringify; direct value-text conversion refuses. `enums.rs::computed_enum_type` does produce this representation. Named enum/type display is separate; these public conversions need a direct pin/contract witness before closing C1's representation item. |
| `jsx.rs::jsx_child_elaboration_element` | **Contract guard**: JSX expression/text/element/self-closing/fragment are handled; default matches pinned `jsx.go::getElaborationElementForJsxChild` panic. |
| `jsx.rs::{intrinsic_tag_symbol,parse_isolated_entity_name}` | **Contract guards**: intrinsic name must be identifier/namespaced name; parsed entity components must be identifiers. Component/tag dispatch precedes the intrinsic helper. An offending compiler-produced tree remains a bug. |
| `members.rs::get_type_of_symbol_with_deferred_type` | **Open invariant**: deferred property without stored union constituent types refuses. Establish all producer paths rather than assume the payload always exists. |
| `meta_properties.rs::check_meta_property` | **Contract guard**: new/import keywords handled; other keywords correspond to pinned default panic. |
| `module_alias_like.rs::target_of_require_variable` | **Contract guard**: helper input must be a variable initialized to a require call; general variables use another path. |
| `module_aliases.rs::target_of_alias_declaration` | **Contract guard**: supported import/export/CommonJS branches precede the final kind refusal. The old “external or CommonJS alias” label does not establish missing whole families. |
| `module_exports.rs::module_exports` | **Open invariant**: reentry while resolving an alias-backed export map refuses. The pin explicitly permits circular export-star graphs through its visited-symbol logic. Ordinary cycles being supported does not establish this guard cannot fire on a valid graph. |
| `name_resolution.rs::{suggest_lookup,lookup,lookup_symbol,lookup_symbol_resolving}` (four alias-related matches) | **Handled signal**: unresolved alias flags request `PendingAlias`; the outer resolver resolves and retries, and `lookup_symbol_resolving` handles the same error explicitly. No blanket alias-resolution gap. |
| `name_resolution.rs::get_symbol_of_declaration` | **Open invariant**: unresolved late enclosing symbol missing its late-member symbol refuses. Needs producer/call-order proof. |
| `name_resolution.rs::resolve_entity_name_at_worker` | **Contract guard**: unsupported entity-name AST syntax refuses; valid identifier/qualified/property forms handled. |
| `node_builder.rs::type_node_worker` computed enum arm | **Representation guard**: ENUM_LIKE is handled by `enum_type_node` before literal dispatch; `computed_enum_type` constructs flags ENUM, not ordinary LITERAL. Do not count this stale arm as missing all computed-enum display. C1 still requires an exact constructor/dispatch witness for closure. |
| `node_builder.rs::structured_type_node` | **Open invariant**: final unsupported type family. Union-origin substitution, conditional/template/string mapping/index/indexed access and anonymous/mapped dispatch precede it. Need a concrete remaining valid type/flag producer before identifying an implementation slice. |
| `node_builder_enum.rs::enum_type_node` | **Open invariant**: enum parent serialization must produce ImportType or TypeReference in this branch. Other generated nodes refuse; no proof of exhaustiveness added here. |
| `node_builder_extra.rs::array_or_tuple_node` | **Constructor guard**: reviewed source/signature/synthetic label producers preserve identifier names. Supplied binding-pattern labels panic at the pin; ordinary destructured-parameter inference omits these labels. See the native controls above. |
| `object_grammar.rs::check_object_literal_grammar` | **Contract guard**: invalid constructed object member kind refuses; source property/shorthand/spread/method/accessors are handled. |
| `object_members.rs::resolve_type_members` | **Contract guard**: unsupported structured-type payload family, matching pinned default panic; evolving arrays must first be finalized. |
| `object_members.rs::resolve_anonymous_type_members` | **Open invariant**: class, enum and value module branches are handled before the old class/enum/module refusal label. Remaining symbol flags outside type-literal/function/method refuse. Do not classify ordinary classes/enums/modules as missing from the label alone. |
| `query.rs::check_expression_worker` JsxOpeningElement | **Contract guard**: direct checking of a JSX opening element is forbidden in both implementations; JSX element checking supplies its proper context. |
| `query.rs::widen_literal_type` | **Representation guard**: fresh ENUM_LIKE dispatches to `base_type_of_enum_like` before exact primitive literal flags. The “enum” error label is stale; invalid remaining fresh-literal flags refuse. |
| `query.rs::apparent_primitive_type` (two paths) | **Contract guards**: this helper takes primitive-like inputs and requires initialized global types; broader apparent-type processing occurs elsewhere. Missing initialization is not a fallback to empty properties. |
| `services.rs::{is_valid_property_access,export_specifier_local_target_symbol,symbols_of_parameter_property_declaration}` | **Contract guards**: service query kind must match its documented node domain; parameter property must have both parameter/property symbols. Constructed/wrong-domain nodes refuse; a public LS caller producing these inputs requires a regression. |
| `source_modules.rs::declaration_spaces` | **Contract guard**: unknown declaration kind corresponds to pinned `getDeclarationSpaces` default panic. |
| `storage_families.rs::{run,observation}`, `storage_pilot.rs::Prepared::execute` | **Probe-only limits**: wrong root type/symbol, unknown named builtin, or fresh non-string literal in the deliberately restricted storage pilot refuses. These are executable census/prototype request contracts, not whole compiler restrictions. |
| `union.rs::union_key` | **Contract guard**: unhandled origin flag shape corresponds to pinned `getUnionKey` default panic. A new origin producer must extend both cache key and payload handling. |
| `unused_identifiers.rs::{check_unused_identifiers,check_unused_class_members}` | **Contract guards**: registered declaration/member kinds must match the pass's domain; pinned corresponding switches panic at default. Preserve any valid-source trigger as a finding. |
| `variables.rs::type_for_variable_like_raw` | **Contract guard**: raw helper accepts variable/property-signature/parameter kinds after preceding specialized branches. Other kinds refuse; the helper label is not proof of missing general declaration typing. |

## Transformer and printer paths

| Location/function | Classification and exact condition |
| --- | --- |
| `tsr_transformers/src/estransforms/{classfields.rs::{q,q_node,get_local_name},esdecorator.rs::{q,with_builder},using.rs::builder,utilities.rs::view,optionalchain.rs::visit_optional_expression}`, `moduletransforms/commonjsmodule.rs::builder`, `tstransforms/{runtimesyntax.rs::builder,typeserializer.rs::view}` | **Contract guards**: semantic transforms require a RuntimeFactory backed by AST storage/builder/view. A lazy JSDoc-only factory does not satisfy this. Failures record a named error and prevent successful output settlement; temporary default/fallback values are not success. Ordinary production AstBuilder factories satisfy the requirement. |
| `tsr_transformers/src/estransforms/optionalchain.rs::printer_error` | Error adapter: non-arena RestoreOuterExpressions failures retain named refusal instead of silently continuing. Review new printer error kinds rather than treating this as absent optional-chain lowering. |
| `tsr_transformers/src/inliners/constenum.rs::new_const_enum_inlining_transformer` | **Contract guard**: selected inliner with isolatedModules is invalid. Pin `transformers/inliners/constenum.go` inliner selection asserts the same prerequisite; normal compiler emitter must omit this inliner in that mode. |
| `tsr_printer/src/printer.rs::emit_external_helper_access` | **Contract guard**: a helpers-module identifier itself flagged HELPER_NAME would repeatedly substitute itself at the pin. Rust refuses instead of entering unbounded substitution. No ordinary source unreachability proof or approved divergence is inferred; retain a constructed public-printer witness if this behavior becomes part of a supported contract. |
| `tsr_printer/src/printer.rs::emit_jsdoc_node` | **Pinned absence**: upstream `printer.go::emitJSDocNode` panics “not implemented.” Rust returns Unsupported. Not missing a working pin feature; exact panic/error-class equivalence still belongs to the printer boundary expectations. |
| `tsr_printer/src/printer.rs::{get_opening_bracket,get_closing_bracket}` | **Contract guards**: only brace/parenthesis/angle/square masks are valid. Other list-format bracket masks refuse. |

## Historical example/probe adapters

`crates/tsr_compiler/examples/p2/` and `examples/phase2_assignments.rs` are
independently executable evidence adapters, not the ordinary CLI formatter.
Their bounds remain real for callers of those examples:

| Functions | Documented limit |
| --- | --- |
| `p2/mod.rs::{Error,fmt,failure}`, `phase2_assignments.rs::program` | Declare/report/propagate errors without converting them to passing rows. |
| `p2/mod.rs::utf8` | JSON diagnostic arguments require valid UTF-8. This adapter cannot certify arbitrary-byte compiler behavior. |
| `p2/mod.rs::main` retained-type failure | Failed operations cannot supply a retained type; reports failure instead of fabricating lifetime success. |
| `p2/diagnostics.rs::all_mode` | Combined diagnostics/baseline require completed semantic/declaration phases; failures remain visible. |
| `p2/baseline.rs::{category,lines,error_text,render}` | Bounded plain baseline: known categories; limited line delimiters; nonnegative in-range UTF-8-boundary spans/squiggles; no library/config/duplicate/content-mapper file envelope. These are historical adapter limits, not approved ordinary compiler exclusions. Current shared host baseline writers are separate. |

## Closure requirements

This record accounts for the lexical matches by source path and function group.
It does not close R5: open implementation branches need pinned regressions and
fixes or explicit owner scope decisions; open invariants need producer/dispatch
proofs and targeted witnesses. In particular, an error enum, a test expecting a
refusal, or a function listed in the port ledger supplies no such proof.

The reviewed project-reference importer branch is implemented and witnessed;
tuple-label classification is corrected above. The extension task-queue guard remains an open
invariant for routes other than the two prevalidated callers above. The
implemented package diagnostic-chain branch retains its ordinary-reachability
limitation. Keep all
failures visible in the relevant suite; no approval is created by this review.
