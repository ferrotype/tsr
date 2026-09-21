# Phase 1 progress record

A results record for [PHASE1-implementation-plan.md](PHASE1-implementation-plan.md),
not a second plan. Each step records what was completed, what is still missing
and the exact command that reproduces it.

Pin `1f70213d4922b434345f639b441681e470c7cfc1`. Gitlink, `data/upstream.json`
and the initialized submodule HEAD all agree, and a capture records both so a
moved pin invalidates it.

| Step | State |
| --- | --- |
| F0 — inventory, manifests and executable setup | **incomplete**: implementing and verifying the approved matchFiles test renderer, and connecting existing evidence to operation ids |
| F1a — foundation leaf tests | **complete**: `leaves_prepared: true`; 225 leaf cases frozen, all 460 inventoried leaf operations prepared, witnessed or exempted by the reviewed ledger, and both divergences triaged |
| F2a — filesystem, path and matching tests | not started |
| F3a — config, command-line and resolution tests | not started |
| F4a — syntax, binder and utility coverage | not started |
| F5a — integration checks and the stage A review | not started |

`python3 scripts/phase1.py inventory --check` computes this: it reports
`f0_complete: false` with the outstanding items, separately from whether the
manifests are internally consistent. Stage A preparation is not Phase 1
implementation; no production behavior has been added or changed.

## F1a — foundation leaf preparation

**`leaves_prepared: true`.** Every operation in the leaf package roster is
linked to a runnable prepared case, a verified rust-gated witness, or a reviewed
exemption in `data/phase1/leaf-roster.json`, and the ledger itself validates.
`python3 scripts/phase1.py inventory --check` publishes the result.

225 leaf cases are frozen, every one with a native observation from the pinned
packages and a classified Rust result.

| Group | Cases | match | not_implemented | different |
| --- | ---: | ---: | ---: | ---: |
| core/collections | 42 | 0 | 42 | 0 |
| core | 21 | 13 | 8 | 0 |
| core helpers | 20 | 1 | 19 | 0 |
| compiler options | 32 | 27 | 4 | 1 |
| JSON | 30 | 0 | 30 | 0 |
| text/number/semver | 31 | 20 | 11 | 0 |
| locale | 12 | 0 | 12 | 0 |
| diagnostics | 27 | 6 | 21 | 0 |
| bundled | 10 | 7 | 3 | 0 |
| **total** | **225** | **74** | **150** | **1** |

Zero `native_unavailable`, zero `harness_failed`, zero `not_run`: every case
runs on both sides. The two divergences this step found were triaged: one was a
defect and is fixed, the other is a deliberate representation difference and is
left recorded as the single `different` row. The 150 `not_implemented` rows are
the honest preparation-time result — no `tsr_core::collections`, `tsr_json` or
`tsr_locale` exists, and neither do most of the generic helpers — and each names
its Go authority, intended signature and production home.

The roster itself: **460 leaf operations**, of which 151 are `covered` by a
matched comparison or a rust-gated witness, and 110 are removed from the roster
by a ledger entry. `equivalent_rust` is no longer zero: eight operations are
recorded as reproduced exactly by a Rust language construct, each held to the
plan's `basis_kind: "review"` bar.

Twelve access-only Go probes drive the pinned packages: collections, core,
options, helpers, json, stringutil, semver, jsnum, locale, locale-default,
diagnostics and bundled. Three of them compile into `package core` and two into
`package locale`, which the harness supports because one overlay file per probe
keeps separate action vocabularies separate, and because the process-global
default locale needs its own process to observe honestly. The bundled probe is
registered with `trimpath: False`, because `bundledSourceDir` locates its
package through `runtime.Caller(0)` and under `-trimpath` returns a wrong path
silently rather than failing.

### What the adversarial pass changed

Each group was written, then reviewed by an independent agent reading the pinned
source, then corrected. 58 findings were accepted and 18 rejected as themselves
wrong. The corrections that mattered most:

- A seed case of mine claimed a nil `*OrderedMap` tolerates `Get` and `Has`.
  Both dereference `m.mp` and panic; only `Size`, `Keys`, `Values`, `Entries`
  and `Clone` carry nil guards. The frozen observations were right all along;
  the prose was wrong.
- The diagnostics group placed English as "the 14th matcher entry". It is the
  first, at index 0 of `loc_generated.go`'s matcher.
- A JSON case claimed the duplicate-name error carries a byte offset and JSON
  pointer. Its own observation records offset 0 and pointer `""`.
- `MarshalEncode`'s newline is a terminator after every top-level value, not a
  separator between them: the first row already shows 14 bytes for a 13-byte
  value.
- Two probe headers justified being in-package by naming unexported symbols
  (`newMapWithSizeHint`, `scheme`) that the files never reference. The
  collections probe moved to `package collections_test`, matching every other
  test file in that directory.
- `locale` claimed `cmn` and `und` are rewritten; both are identities at this
  pin. And `fr-FR` maps the probed key to "Modules", byte-identical to English,
  so that row could not witness table selection.
- `bundled` described `CopyrightNotice.txt` as an embedded asset. It is never
  embedded: `generate.go` reads it only to validate that each library starts
  with it as a header.
- A SyncMap case froze a Go *runtime* panic message as an expected value. No
  Rust port could ever emit it, so the row had no reachable match; it is now
  compared by panic class.

### Existing coverage, mapped honestly

`covered` requires an exact link **and** evidence the comparison passes. A
prepared case that reports `not_implemented` witnesses a gap; it does not close
one. Expanding the direct action links and checking the production bundled
wrapper gives 69 covered operations. The asset-content index does not cover
`wrappedFS.WalkDir`; it remains a named missing API. ReadFile coverage now
comes from calling `BundledFs::read_file`, not its backing asset table.

The same rule applies to existing artifacts, with a correction F2a's survey
forced. `data/s07/path-observations.json` and `semver-observations.json` look
like Rust witnesses because of how they are produced, and on that reading they
are not: their producers run `go test` and never execute Rust, so they are
recorded as `native_authority`, and counting them on the producer's account
would have reported 113 operations covered on the strength of a Go-only run.

But a producer is not the only thing that can gate a port against an artifact.
`crates/tsr_tspath/tests/go_observations.rs` *consumes* the frozen path
observations: it compiles the 4,096 requests and their Go answers in with
`include_str!`, calls `tsr_tspath` for each, and asserts the whole row equal, so
`cargo test` gates eight tspath operations against them after all. That is
recorded separately as `witness/s07-path-observations-rust`, `rust_gated`, and
it names the eight the Go adapter actually drives and the Rust test actually
calls rather than the artifact's whole 88-operation surface. The original
`native_authority` entry stays, because the statement it makes about the
producer is still true. `semver-observations.json` has no such consumer, so it
is unchanged.

| Disposition | Count |
| --- | ---: |
| `covered` | 69 |
| `implemented_untested` | 3,400 |
| `missing` | 1,308 |
| `equivalent_rust` | 0 |
| `later_phase` | 18 |

74 operations now carry a case that runs and reports the Rust entry point
absent, which is a witnessed gap rather than an inferred one.

### The F1a roster and how it is allowed to shrink

F1a's exit condition is that *every leaf operation* is linked to a runnable
prepared case or a verified existing witness. That makes the roster itself a
claim: an operation dropped from it quietly is work hidden behind a green gate.
So the roster is the whole leaf surface computed from the scope, and it shrinks
only through `data/phase1/leaf-roster.json`, a reviewed ledger where every entry
names a category, the owner that does have the operation, and the evidence read
at the pin.

`scripts/phase1_scope.py:roster_problems` validates the ledger the way any other
claim is validated. An entry is rejected when it names an operation outside the
frozen scope, an operation outside the leaf packages, an unknown category, or no
owner or evidence; when it duplicates another entry; when it claims
`equivalent_rust` but the scope row disagrees; and — the one that has already
caught a real mistake — when the same operation is both exempted and linked to a
prepared case. `leaves_prepared` is false while the ledger does not validate,
not only while operations are pending, because an exemption nobody can defend is
not an answer.

The categories are:

| Category | What it claims |
| --- | --- |
| `build_tooling` | Runs at build time and never in a compile; the port generates the same artifact elsewhere (`xtask/src/gen`, `scripts/package_assets.py`). |
| `go_runtime` | A Go language mechanism — scheduling, sync, arenas, the `go vet` copylocks marker — with no caller-visible contract to reproduce. |
| `generated_assertion` | Not an operation: the blank-identifier compile-time check `stringer` emits. |
| `later_step` | A real operation that a different named step prepares. |
| `equivalent_rust` | The Go contract is reproduced exactly by a Rust language or standard library construct. This one also updates the scope row, and is held to the plan's `basis_kind: "review"` bar. |
| `unused_at_pin` | Exported but called by nothing at the pin, tests included, so no caller fixes the contract. |

`later_step` is decided by the pinned callers, not by intuition: when every
non-test caller of an operation is outside this step, the step that ports those
callers owns it, because the plan requires a generic helper's callers to be read
before its Rust contract is chosen. Two corrections came out of applying that
rule rather than asserting it. Go method *values* — `(*core.CompilerOptions).IsIncremental`
at `upstream/tsc/internal/tsoptions/showconfig.go:43` — are caller references
that a search for `.IsIncremental(` does not find, so the first pass recorded a
false "no Phase 1 caller" for four option getters. And the rule does not
override an operation the plan names as this step's own work:
`docs/PHASE1-implementation-plan.md:432-434` puts "tri-state/default option
semantics" in F1 explicitly, so the compiler-option getters stay on this roster
whoever calls them, and their contract is self-contained rather than
caller-derived.

The derived manifests are no longer hand-maintained. `python3 scripts/phase1.py
inventory --write` rebuilds `scope.json` and `leaves-preparation.json` from
their builders, and `python3 scripts/phase1.py record --capture DIR --write`
writes each case's `last_result` from a real comparison, refusing a capture
whose case set disagrees with the manifest in either direction. Before that
command existed, `last_result` decided coverage and was written by hand, so a
case could claim `match` without the run ever happening.

### A port defect preparation found, and fixed

`crates/tsr_tspath/src/lib.rs` ended its ancestor walk with
`if path.is_empty() { break; }`. The pinned `ForEachAncestorDirectory`
(`upstream/tsc/internal/tspath/path.go:1116`) has no such guard: it stops only
when the parent equals the directory.

The two are otherwise identical, and both languages compute a parent as
`path[..max(root_length, last_slash)]`. For a **rooted** path that always
retains the root and is never empty, so the extra guard cannot fire; for a
**relative** one it fires exactly once, dropping the empty-string ancestor the
pin yields last. So the walks agreed exactly on rooted input and differed by
exactly one element on relative input — a proof from the source rather than a
sample, and the reason the fix is safe: deleting the guard cannot change any
rooted caller's answer.

It surfaced through `GetEffectiveTypeRoots`, where a `configFilePath` of
`sub/tsconfig.json` made Go answer
`["sub/node_modules/@types", "node_modules/@types"]` and Rust answer only the
first. Absolute bases agreed, which is why nothing had noticed.
`leaves/options/effective-type-roots-relative-and-empty-base` now matches, and
616 workspace tests pass with the guard gone.

A sweep for the same class found nothing else.
`crates/tsr_checker/src/module_specifiers.rs:156` looks similar but its
`remaining.is_empty()` exit is behaviour-preserving, because whatever is left is
appended after the loop; `contains_path` carries the identical empty check the
pin has.

### The one divergence left standing: `Clone` shares what it copies

`CompilerOptions.Clone` is a field-by-field reflective `Set`
(`upstream/tsc/internal/core/compileroptions.go:180-192`), so a pointer-backed
field is copied as the pointer and a slice as its header. Writing through the
source *after* the clone is therefore read back by the clone: with `Checkers`
(`*int`) at 2 and `Types` at `["alpha", "beta"]`, mutating the source to 7 and
`"rewritten"` leaves the Go clone reading 7 and `"rewritten"`.
`tsr_core::CompilerOptions` derives `Clone` over owned fields, so the port's
clone reads 2 and `"alpha"`. Nine of the pinned struct's 131 fields can share:
`Paths`, `MaxNodeModuleJsDepth`, `Checkers` and six `[]string`.

**Triaged as a deliberate difference, not a defect.** The sharing is incidental
in the pin, not load-bearing:

- Every pinned `Clone()` caller assigns whole fields afterwards —
  `transpile.go:127`, `ls/sourcedefinition.go:149`, and `harnessutil.go:260`
  through `ParseCompilerOptions`, which assigns.
- The one place the pin needs an independent `Paths` it deep-clones
  **explicitly**: `tsoptions/tsconfigparsing.go:1830-1839` does
  `paths = compilerOptions.Paths.Clone()` and reassigns before mutating. That is
  the pin working around its own shallow `Clone`.

The port never had the sharing to lose, either: `paths` is
`Vec<(JsString, Option<Vec<JsString>>)>`, an owned value rather than a pointer
to an `OrderedMap`. Reproducing the sharing would mean `Arc` or interior
mutability across nine fields to carry a property no caller uses.

So the recommendation is to leave the port deep and keep
`leaves/options/clone-shares-pointer-backed-fields` reporting `different`, so
the difference stays visible rather than being waived. F3b confirms it as the
option-consuming callers land. The original field-roster case could not see any
of this: at the instant of the clone the two sides agree, which is exactly why
the mutation case exists. Its third action sets neither field, so the mutation
is a no-op there and both sides agree, keeping the difference attributable to
the sharing rather than to the action.

### F1b queue

The 150 `not_implemented` rows group into coherent ports: the ordered and
copy-on-write containers (`OrderedMap`, `OrderedSet`, `Set`, `MultiMap`,
`CopyOnWriteMap`, `CopyOnWriteSet`, `SyncMap`, `SyncSet`), the caller-visible
JSON contract, locale parse and fallback, and diagnostic formatting with
argument interpolation. `CopyOnWriteMap`'s scope guard and `SyncMap`'s
present-with-nil contract need a deliberate Rust representation decision rather
than a mechanical port.

Two groups are new to this queue and both carry a decision rather than a
transcription:

- **The generic `internal/core` helpers.** Their contract is not "filter
  filters": it is nil versus empty, and result identity. Three neighbouring
  functions in one file give three different answers to "nothing survived" —
  `Filter`'s rejecting arm is `slices.Clone(slice[:i])` and yields a non-nil
  empty slice, `Map` guards `if slice == nil` and otherwise allocates, and
  `MapFiltered`, `FlatMap` and `Flatten` accumulate into `var result []U` and
  yield nil. A port that collapses all three to `Vec::new()` erases a
  distinction that survives into JSON as `[]` versus `null`. Several of them
  also return the *input slice itself* when nothing changed, which the cases
  witness by writing through the result and reading the input back. Where the Go
  contract genuinely is the Rust idiom, the operation is recorded as
  `equivalent_rust` in the roster ledger instead of being ported.
- **The five generated enum stringers.** None of the five has a home on its type
  in the port. Two renderings exist, both private to a consumer crate and
  written for one call site: `module_kind_text`
  (`crates/tsr_checker/src/emit_checks.rs:514`, complete) and
  `script_target_text` (`crates/tsr_compiler/src/include_reason.rs:684`). The
  gap record asks for one shared renderer per type, including the numeric
  fallback for the values in each enum's numbering gaps.

`data/phase1/locale-assets.json` maps the 13 shipped translation tables to their
path, size, sha256 and message-key count, with each fallback obligation and the
planned `xtask/src/gen/diagnostics.rs` extension recorded. The generator itself
lands in F1b.

### Known limitations

These are stated rather than hidden, because a gate that reports them is worth
more than one that does not.

- Two `jsnum` helpers are reached but not separately discriminated: trimming a
  fraction's trailing zeros and an exponent's leading zeros cannot change the
  value the parse produces, so no input in the corpus separates them from doing
  nothing. They are linked because the corpus calls them, not because it pins
  them.
- `leaves/options/clone-field-roster` compares fields at the instant of the
  clone, where the two sides agree by construction, so on the Rust side it
  catches a *roster* that has drifted from the pinned struct rather than a
  wrong clone. The sharing it cannot see is covered by
  `leaves/options/clone-shares-pointer-backed-fields` instead.
- The bundled walk has no Rust counterpart to compare against —
  `tsr_vfs::FileSystem` declares no walk method — so that case is Go-side native
  authority. It witnesses the pinned branches but cannot catch a wrong port
  until the trait grows one.
- `noembed.go:wrapFS` and `noembed.go:IsBundled` are unreachable in every build
  this repository makes, and no case was invented for them. The ledger records
  that as `build_variant` with the build tags as evidence.
- Some cases record a boolean where the pinned implementation's short-circuit is
  genuinely unobservable through the API (`SyncSet.IsEmpty`,
  `CopyOnWriteMap`'s ownership restore). Those claims were narrowed to what the
  rows witness rather than dropped.

## F0 checklist

| Requirement | Result |
| --- | --- |
| Scope has zero unclassified operations | 4,795 operations, each with a disposition, basis, case links and dependencies |
| `covered` carries exact case/artifact links | **69 of 4,795.** 2,694 mapped operations have only file-level producer metrics; see Scope |
| All 309 outputs have verified invocation mappings | **167 of 309.** 142 blocked; see below |
| Manifests and failure tests pass | 115 Phase 1 tests, plus the extended discovery regression |
| The real pilot has an observed match and a named missing operation | 2 matches against pinned Go, 4 named missing Rust operations, each with a native expectation |
| Replay is read-only | `compare` spawns no build or observation child, and a test asserts neither `go` nor `cargo` is invoked |
| The pending queue is generated from concrete rows | derived from `data/phase1/scope.json` |

## Approved: carry the `config/matchFiles` test renderer

**The 142 `config/matchFiles` reference outputs have no Go invocation and no Go
renderer at this pin.** The plan anticipated this and required it be named
rather than papered over. Five independent checks, each re-derivable:

1. No pinned test writes that subfolder. Across every `*_test.go` under `tsc/`,
   the only `baseline.Options{Subfolder: ...}` values are
   `config/tsconfigParsing` and `tsoptions/commandLineParsing`.
   `phase1_baselines.verify_written_subfolders()` re-derives this on every
   `inventory --check`.
2. **Executed, not just inspected.** Running the whole pinned `tsoptions` test
   package with `baseline.Run` instrumented records exactly 167 invocations:
   87 `config/tsconfigParsing` and 80 `tsoptions/commandLineParsing`. Zero
   write `config/matchFiles`.
3. The envelope matches no renderer in the pin. The 142 carry `config:`,
   `Fs::`, `configFileName::` and `Errors::`, and dump the whole parsed result
   as JSON. `baselineParseConfigWith` emits `configFileName::`,
   `CompilerOptions::`, `TypeAcquisition::`, `FileNames::` and `Errors::`, with
   no `config:` heading.
4. The titles do not overlap. Zero of the 142 names appear in the 87
   `config/tsconfigParsing` outputs.
5. `vfsmatch_test.go` references
   `tsc/testdata/fixtures/testRunner/unittests/config/matchFiles.ts`, absent at
   this pin. These are promoted TypeScript outputs whose renderer was not ported.

The **semantic** authority does exist: `vfsmatch_test.go`'s `TestReadDirectory`
and `TestReadDirectoryMatchesTypeScriptBaselines` assert ordered `matchFiles()`
results, and the pilot already matches Rust against it.

**Owner-approved on 2026-09-20: keep all 309 byte-for-byte baselines.** Carry a
test-only implementation of the matchFiles envelope, retaining pinned Go
matching/configuration behavior as the semantic authority. First prove that
native observations rendered through it reproduce all 142 frozen files; then
render Rust observations through the same test-envelope seam. Expected result
sections must never be copied from the baseline or completed using native
semantics on Rust's behalf.

The authority decision is settled. The remaining work is request/invocation
mapping, renderer implementation and native-byte verification, owned by F2a's
matching preparation and reused by F3a. Keep the manifest's authority blocked
until that proof exists; approval alone is not a successful observation. This
does not block F1a's leaf preparation and does not require another approval to
implement the agreed renderer.

## The 309 index and its per-output mapping

`data/phase1/config-baselines.json` records every output's path, byte length,
hash, **and the concrete test that renders it**. The mapping is produced by
`phase1.py map-baselines`, which runs the pinned `tsoptions` package with an
access-only instrumentation patch inserted into
`internal/testutil/baseline/baseline.go`. The patch records the calling test,
the output and a digest of the rendered content. Because the pinned
`writeComparison` still runs, a recorded digest equal to the frozen file's
digest is an observation that the native rendering reproduces the committed
bytes — not an assumption about a generic renderer.

| Group | Outputs | Verified | Authority |
| --- | ---: | ---: | --- |
| `config/matchFiles` | 142 | **0** | carried test renderer approved; implementation and verification pending |
| `config/tsconfigParsing` | 87 | 87 | `baselineParseConfigWith`, plus inline assembly in `TestParseConfigFileTextToJson` |
| `.../parseCommandLine` | 53 | 53 | `formatNewBaseline` |
| `.../parseBuildOptions` | 27 | 27 | `formatNewBaselineBuild` |

Each verified row names its subtest, for example
`TestCommandLineParseResult/parseCommandLine/Handles_did_you_mean_for_misspelt_flags`.
The instrumentation asserts the exact pinned `Run` body it hooks, so a moved
pin fails loudly instead of silently recording nothing.

Two `config/matchFiles` outputs live under a nested directory, because the
originating title contains a slash (`Expands z to z/star...`). A depth-1 glob
enumerates 140 and silently shrinks the denominator; enumeration walks
recursively and a test asserts both nested rows are indexed.

## Scope

`data/phase1/scope.json` covers **4,795 operations** across
45 packages.

Membership is declared against the plan's section 2 scope, not the ledger's
`phase` column. Taking the ledger literally was wrong in both directions: it
excluded AST, scanner, parser and the compiler runner's syntactic operations
(labelled phase 0) while retaining editor, emit and API test helpers the plan
pushes to later phases. Every one of the 97 pinned packages now carries an
explicit membership decision with a reason, and the 52
excluded packages each record a destination phase.

`internal/compiler` is `partial`: only the runner's parse/bind/syntactic
operations are Phase 1 obligations, and F4a enumerates that exact surface.

| Disposition | Count |
| --- | ---: |
| `covered` | 69 |
| `implemented_untested` | 3,400 |
| `missing` | 1,308 |
| `equivalent_rust` | 0 |
| `later_phase` | 18 |

`covered` requires an **exact operation-level link**, which is what F0 asks for.
The ledger's `verify` field cannot supply one: its entries are file-level
producer metric expressions such as `run.e1.parity >= 0.999`. They say a source
file's port is exercised by a producer; they do not say which operation any
single metric witnesses. Treating their presence as coverage marked 2,703
operations `covered` with no artifact behind any of them. Those metrics are now
retained per row as `ledger_verification` for context, and a mapped operation
without an exact link is `implemented_untested`.

Connecting the existing S04–S11 evidence to operation ids is therefore
outstanding F0 work in its own right, reported by `inventory --check`, and it is
what F1a's "map every existing probe to the leaf operation IDs it actually
exercises" will discharge. `verify()` rejects a `covered` row with no links and
an `implemented_untested` row that has them.

Every row carries a `basis`, a `basis_kind`, the cases that witness it and its
package's internal dependencies read from the pinned Go imports. All F0 rows
are `basis_kind: "rule"`; none is a review. `equivalent_rust` is 0 and cannot be
reached by rule — `verify()` rejects an `equivalent_rust` row not marked
reviewed, because the plan requires a behavioral witness for it.

Counts are an audit starting point, not a task list. The rules are conservative
in both directions: a generic name (`find`, `identity`) is not treated as
evidence of a Rust implementation, and where a package's behavior lives outside
its planned crate the real home is recorded on the row.

## Pilot

Real native Go and real production Rust over one request set. Four Go probes
serve the schedule — `vfs/vfsmatch`, `tsoptions`, `json` and `locale` — so every
case has a native expectation, including the ones Rust cannot yet answer:

```
match              pilot/matching/include-star-ts
match              pilot/matching/recursive-exclude
not_implemented    pilot/commandline/parse-composite-false   (native: observed)
not_implemented    pilot/commandline/parse-build-verbose     (native: observed)
not_implemented    pilot/json/marshal-ordered                (native: observed)
not_implemented    pilot/locale/select-ja                    (native: observed)
```

The command-line probe is also the **renderer pilot**. It compiles into the
pinned `tsoptions_test` package, so it calls the original `formatNewBaseline`
and `formatNewBaselineBuild` instead of reimplementing their section assembly.
Its rendered envelope for `--composite false 0.ts` reproduces
`parseCommandLine/allows setting option type boolean to false.js` byte for byte.

That probe cannot be built with `-trimpath`: the package links
`internal/testutil/baseline`, whose `init` calls `repo.TestDataPath()`, and
`repo` panics under `-trimpath`. `run_probe` drops the flag for that probe only
and records the choice in provenance.

## Capture integrity

Two properties are enforced, both with regression tests and both verified
end to end:

**A production change stales the capture.** The source closure is derived from
`cargo metadata`, not hand-listed, so it contains the driver package's whole
workspace dependency closure — 444 pilot inputs and 238 leaf inputs, including
`crates/tsr_tsoptions/src/glob.rs`, all of `tsr_vfs`, the example target,
`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` and `.cargo/**`. It also
covers what the *native* side executes: `data/s04/toolchains.toml` (which
selects the required Go version), `scripts/s04.py`, `scripts/s04_runtime.py`
and `scripts/tracking-bootstrap.py`, which `verified_upstream` loads to
authenticate the submodule. Each of those has been verified to invalidate a
capture. Appending a comment to `glob.rs` makes `compare` fail with
`capture input crates/tsr_tsoptions/src/glob.rs changed after the capture`.
Replay recomputes the expected key set rather than trusting the recorded one,
so a capture that recorded too few inputs cannot authenticate; the workspace
package list is itself authenticated, and a dependency added since the capture
is caught through the `Cargo.toml`/`Cargo.lock` hashes. All regular package
files are included, including embedded `.d.ts` libraries and their notice;
production asset edits now stale the leaf capture as source edits do.

**A malformed response cannot reach parity.** Each response is validated as an
ordered sequence before anything is indexed by case id, checking count, order,
case identity, operation identity, per-side status vocabulary and payload
shape. Indexing first accepted duplicates, extra rows, reordering and unknown
statuses. Against a real capture:

| Tamper | Result |
| --- | --- |
| duplicate row | `rust response has 7 rows for 6 requests` |
| extra failing row | `rust response has 7 rows for 6 requests` |
| reordered rows | `row 0 reports case '...' where the request schedule has '...'` |
| unknown status | `unknown rust status 'looks_fine'; allowed statuses are ...` |

Unknown action markers are harness failures, not comparable observations.
Malformed action arrays are rejected, and an observed trace must retain one
result per requested action. Native action decoding no longer turns malformed
JSON into an empty successful trace.

A side may only report its own statuses: a Rust driver cannot claim
`native_unavailable`, and a native probe cannot claim `not_implemented`.

**A native harness failure cannot be merged away.** Failures are collected
across every probe and raised before any merging happens. Merging first hid
them two ways: an earlier `native_unavailable` won the `setdefault`, and an
`observed` row from the owning probe overwrote a failure reported by another.
Injecting a failure into a probe that sorts before the observing one, and into
one that sorts after, now both fail with the probe, case and cause named.

**Freezing validates contents, not just bytes.** Authentication checks hashes,
the pin, the gitlink and the source closure, but never reads a row, so a
correctly hashed capture containing a `harness_failed` row would still install.
`freeze` now runs the same full validation `compare` does: every response is a
valid ordered sequence, and neither side reports a harness failure. Rust parity
is deliberately not required — `not_implemented` is the expected
preparation-time result for an operation this phase has still to write, while
`harness_failed` means the observation never happened.

It also writes one directory per probe (it previously copied a single
`native/observations.json`, which no longer exists) and stages the whole tree
before swapping it in, so a rejected freeze leaves the existing frozen
inventory byte-for-byte unchanged with no staging directories left behind.

## Commands

```sh
export PATH="$(mise where go)/bin:$PATH"

python3 scripts/phase1.py inventory --check
python3 scripts/phase1.py map-baselines --output target/phase1/invocations [--write]
python3 scripts/phase1.py capture --family pilot --output target/phase1/pilot
python3 scripts/phase1.py compare --capture target/phase1/pilot
python3 scripts/phase1.py compare --capture target/phase1/pilot --require-parity   # fails: 4 missing operations
python3 scripts/phase1.py report --captures target/phase1/pilot --output target/phase1/report.json
python3 -m unittest discover -s scripts/tests -p 'test_phase1*.py'
```

## Implementation queue from F0

| Operation | Native authority | Intended Rust home | Owner |
| --- | --- | --- | --- |
| `tsoptions.parseCommandLine` | `commandlineparser.go:ParseCommandLine` | `crates/tsr_tsoptions/src/command_line.rs` (absent) | F3b |
| `tsoptions.parseBuildCommandLine` | `commandlineparser.go:ParseBuildCommandLine` | `crates/tsr_tsoptions/src/command_line.rs` (absent) | F3b |
| `json.marshalOrdered` | `internal/json/json.go:Marshal` | no dedicated home; order-sensitive readers live in consumers | F1b |
| `locale.selectTranslation` | `internal/locale/locale.go:Parse` | no Rust home at this pin | F1b |

```sh
python3 scripts/phase1.py capture --family pilot --output target/phase1/queue --case pilot/commandline/parse-composite-false
python3 scripts/phase1.py compare --capture target/phase1/queue
```

A selected capture is `partial` and its unselected cases report `not_run`;
`--require-parity` rejects such a report, so a bounded smoke can never stand in
for a family gate.

## Producers and sprints

`sprints/P1A.toml` registers the stage A items with pending preparation
metrics. Every item is open and no metric is populated.

The `foundations`, `config` and `syntax` producers are **not** registered in
`status/runs.toml`. The plan registers a producer only once it can validate its
complete declared inventory and report honest failures. The `pilot` and
`leaves` families have adapters; the leaf preparation inventory is still
incomplete. F1a–F5a register producers when their declared inventory is ready.

`cargo xtask validate` and `cargo xtask status --check-committed` both pass with
P1A registered, and S01–S12 are unchanged.
