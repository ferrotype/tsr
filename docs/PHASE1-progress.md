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
| F2a — filesystem, path and matching tests | **pending Linux observation**: `filesystem_prepared: false`; 359 cases, 314 of 316 roster operations accounted for; all 142 baselines prepared (68 exact, 74 owner-approved exceptions). The Linux realpath case must observe `Realpath` and `ignoringEINTR`. |
| F3a — config, command-line and resolution tests | **complete**: `config_prepared: true`; 484 cases, all 402 roster operations prepared, witnessed or exempted, and all 309 reference outputs prepared (F2a's 142 plus F3a's 167, all 167 exact) |
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

## F2a — filesystem, path and matching preparation

**`filesystem_prepared: false`.** Of 316 filesystem operations, 299 are
prepared by a runnable case, 8 witnessed and 7 exempt. Two remain pending:
`nativepath/realpath_linux.go:Realpath` and `nativepath/eintr_unix.go:ignoringEINTR`.
They share the Linux procfs case, which cannot execute on this Darwin host.

359 cases across ten probe groups plus the carried matchFiles renderer: 358
have native observations and one records its native host limitation. Missing
Rust implementations never turn an unavailable native observation into a
prepared result.

| Group | Cases | match | not_impl | different | native_unavailable |
| --- | ---: | ---: | ---: | ---: | ---: |
| tspath | 46 | 13 | 30 | 3 | 0 |
| vfsmatch | 22 | 20 | 1 | 1 | 0 |
| cachedvfs | 32 | 1 | 30 | 1 | 0 |
| vfstest | 19 | 0 | 17 | 2 | 0 |
| wrapvfs | 28 | 0 | 28 | 0 | 0 |
| iovfs | 16 | 0 | 16 | 0 | 0 |
| vfsmock | 10 | 0 | 10 | 0 | 0 |
| osvfs | 23 | 0 | 22 | 0 | 1 |
| glob | 10 | 0 | 10 | 0 | 0 |
| symlinks | 11 | 0 | 11 | 0 | 0 |
| matchFiles | 142 | 0 | 142 | 0 | 0 |
| **total** | **359** | **34** | **317** | **7** | **1** |

Zero harness failures. The seven `different` rows are real divergences between
the port and the pin, listed below. The 317 `not_implemented` rows are the
honest preparation-time answer for a surface the port has barely begun: there is
no `tsr_glob`, no cached, tracking, wrapping or mock filesystem adapter, no
symlink cache reachable from outside `tsr_compiler`, and no baseline renderer.

`inventory --check` publishes `filesystem_prepared` alongside `leaves_prepared`,
and each is false while any operation on that step's roster is neither prepared,
witnessed nor exempted, or while that step's ledger does not validate. F2a also
requires every one of its 142 output cases to remain prepared, with the native
rendering authenticated and either exact or individually excepted. Those cases
have no operation links, so an operation-only count cannot certify them.

### Review corrections

The driver now carries a handler-owned missing-operation identity in its result;
it no longer copies an arbitrary request label. A mutation of `vfsmatch.Usage`
to request the implemented `IsImplicitGlob` reproduced the old false absence
claim. The regression now requires `Usage.String` as the missing identity.
Group-wide gaps validate identities against their reviewed entry points, and
case-specific gaps carry explicit identities. All frozen filesystem gap rows
were exercised through the Rust driver. `record` rejects unclaimed identities
before writing, including single-operation cases; the pilot now emits exact
identities itself instead of relying on a guessing fallback.

A fresh filesystem capture under `target/pr42-review-fixes/filesystem` preserves
all 34 matches and seven differences. Its only classification change is the
Linux case above. The native merge preserves all unavailable reasons so an
earlier probe's generic decline cannot hide the owning probe's platform reason.
The frozen native results, case manifest and generated preparation views were
refreshed. Targeted validation: four Rust driver tests, filesystem-harness
clippy with warnings denied, and 143 Phase 1 Python tests. No compiler corpus or
performance producers were rerun.

The roster machinery is no longer written for one step. It is written once over
`STEP_PACKAGES`, so F2a is held to F1a's bar rather than getting a weaker gate
by being newer: the same arithmetic, the same exemption categories, the same
ledger validation, and the same refusal to call a step complete while its ledger
does not validate. A package may belong to only one step, which is checked at
import; `roster_problems` with no step named validates every declared step, so a
step cannot appear without a ledger; and an exemption filed against a step that
never owned the operation is refused, including one filed against another step's.

### Divergences this step found

Seven cases report `different`, and they are the point of the step rather than a
failure of it. All were found by comparison, not by inspection.

**`tspath`, three, one root cause.** `crates/tsr_tspath/src/lib.rs:116`
`base_name` omits the leading `NormalizeSlashes` the pin performs at
`path.go:877`, so it measures the root on raw bytes: for `//server\share` the
pin answers `share` and the port answers the empty string. `has_extension`
inherits it whole, being `base_name` plus a dot search, and answers `false` for
`//server\share.ts` where the pin answers `true`. Separately, `to_path` routes
every input through `absolute`, which adds a separator to a bare root and strips
one from a longer path, where `path.go:725-726` takes `NormalizePath` and does
neither: `c:` becomes `c:/`.

**`cachedvfs`, one, all nine rows.** `RootLength` rejects a non-absolute path
where the pin accepts it.

**`vfstest`, two**, on the paired-snapshot publication and the mutation-leak
control.

**`vfsmatch`, one**, on the base-path comparer.

The live-OS group measured further differences by running real Go and Rust
programs on this host, which are recorded in its cases rather than as `different`
rows because the port has no adapter to compare against yet:
`std::fs::canonicalize` *corrects* case where the pin does not; Go's simple case
mappings differ from Rust's full ones (U+00DF and U+FB01 are unchanged under
`unicode.ToUpper`/`ToLower` but not under Rust's); `os.Chtimes` wraps outside
roughly 1678..2262; and `metadata.is_file()` is not `!stat.IsDir()` — measured
with a FIFO, where the pin answers `FileExists` true.

One survey claim was corrected by measurement rather than inherited:
`std::env::current_exe()` does **not** canonicalize on macOS.

### Scope corrections

Two operations were recorded as having a Rust counterpart on the strength of a
by-name rule, and reading the code refuted both.
`symlinks/knownsymlinks.go:ProcessResolution` is `implemented_untested` in the
scope, but `PORTS.toml:5339-5350` maps the file to `status = "planned"` with
`rust = []` and `crates/tsr_core` has no symlinks module; the only
implementation is a private re-inlining at
`crates/tsr_compiler/src/checker_module_specifiers.rs:10-88`, which drops the
file half of the resolution entirely. Several `tspath` operations were similar:
`crates/tsr_checker/src/module_specifiers_packages.rs:117` carries no port
marker and returns `"ajs"` where the pin returns `"a.js"`, never prepending the
missing dot.

### The `config/matchFiles` renderer, and the amendment it forced

No pinned Go test writes `config/matchFiles`. The only `baseline.Run` calls with
a `config/` subfolder are `tsconfigparsing_test.go:157` and `:1554`, both writing
`config/tsconfigParsing`; `vfsmatch_test.go` is list assertions whose own comment
at :12-13 says its cases are "modeled after" the TypeScript matchFiles tests,
whose fixture is not in the pin. So 142 frozen outputs existed with no producer,
which is what the owner approved carrying a renderer for.

The renderer reconstructs each baseline's inputs from the baseline itself, runs
the pinned parse through both entry points, renders the envelope and compares
byte for byte. It calls the pinned `printFS` and the pinned
`getWildcardDirectories` rather than reimplementing them.

**A first pass reported "65 of 142" and that number was wrong twice over.** It
merged two disjoint populations, and it rested on a claim that did not hold.

The 142 are 71 scenarios through two entry points. All 71 `jsonSourceFile` rows
rendered; all 71 `json` rows were declined, because `ParsedCommandLine.
WildcardDirectories()` (`parsedcommandline.go:265`) reads
`p.ConfigFile.configFileSpecs` and the JSON path leaves `ConfigFile` nil. The
step recorded that as "the pin cannot reproduce these".

The owner refused that and was right. `tsconfigparsing.go:1328-1373` computes
`validatedIncludeSpecs` and `validatedExcludeSpecs` **unconditionally**; only
`:1375-1377` gates the attachment on `sourceFile != nil`. And
`wildcarddirectories.go:10` takes the specs directly and never needs a
`ConfigFile`. So the adapter could not render it; the pin could. A test-only
accessor in an overlay source — never a change to the pin — took all 71 raw-JSON
rows from declined to rendered.

One honest qualification on that accessor: the pinned worker discards its local
`configFileSpecs`, so nothing on the returned value holds it and it cannot be
captured after the fact. The hook therefore **re-derives** the two slices from
the parse's own result by calling the same pinned unexported functions in the
same order, restating only the selection control flow. Two checks that it is
faithful: the section it produces is identical, values and order, to the
`jsonSourceFile` entry point's — which gets its specs from the real attached
`configFileSpecs` — in 71 of 71; and swapping include for exclude in the hook
drops that to 19 of 71, so the check is live. The `outDir`/`declarationDir`
exclude default is not exercised by this corpus and is therefore unverified.

**The bounded attribution pass then separated the remaining differences**, which
is what the owner asked for instead of closing them as a category:

| attribution | rows | what it means |
| --- | ---: | --- |
| reproduced | 65 | byte for byte |
| `upstream` | 71 | the frozen bytes and the pinned Go genuinely disagree |
| `renderer_defect` | 6 | our renderer is wrong and can be fixed |
| `missing_observation` | 0 | — |

Four causes, established by decomposing *every* difference rather than the first:

- **`raw.compileOnSave`, 71 rows, the sole difference in 65 of them.** Nothing at
  the pin inserts this member into `raw`: `tsconfigparsing.go:977-981` only
  converts a value the config declared, and `:1206-1207` only propagates a true
  one down an extends chain, which no config here uses. The frozen corpus
  disagrees with itself about it — 0 of the 71 paired `jsonSourceFile` baselines
  carry it. Its origin is identifiable: `commandLineParser.ts:3509` assigns it
  unconditionally on the JSON path. Verified constructively, by inserting exactly
  that member into the rendered text and getting a byte-identical match.
- **`options.jsx`, 4 rows.** The pin has `JsxEmitReactNative = 2` and
  `JsxEmitReact = 3` (`compileroptions.go:534-535`), the reverse of TypeScript's
  numbering. A baseline records `raw.jsx = "react-native"` with `options.jsx = 3`;
  the pinned parse yields 2.
- **`Fs::` entry order, 2 rows**, under a case-insensitive host.
- **`wildcardDirectories` key order, 6 rows — our defect, not upstream.** The
  order *is* recoverable: `wildcarddirectories.go:36` iterates
  `for _, file := range include` and inserts in include order, and the pinned Go
  is a line-for-line port of `commandLineParser.ts:4128-4151`, where the same
  loop fills a JS object literal whose key order is insertion order. The frozen
  order is the pin's own insertion order. Our renderer sorted it away.

Two framings of this step's own were wrong and are corrected here: that a
map-ordered section makes the frozen order unreachable by construction, and that
the whole `json` population was unrenderable.

### The amendment

`data/phase1/baseline-exceptions.json` records the owner's amendment to the
requirement that all 309 config reference outputs reproduce exactly. Only
`upstream` may be excepted, and only individually: `renderer_defect` and
`missing_observation` name work rather than a fact about the baseline, so
`exception_problems` refuses them outright. Each entry carries the reason, the
pinned evidence, what the pin produces, what the baseline records, the
TypeScript origin where identifiable, and the Go-versus-Rust semantic
comparison that survives it. It must carry the original file's hash, and it is
refused if the index also records that rendering as verified — **an exception is
not a pass.** The outputs keep their original files and are counted in their own
bucket.

## F3a — config, command-line and resolution preparation

**All 309 reference outputs are prepared.** F2a carried the 142 `config/matchFiles`
outputs (68 exact, 74 owner-approved exceptions); F3a adds the other 167, and all
167 reproduce the frozen bytes exactly. That completes the plan's byte-baseline
denominator, which had stood at 142 since F2a.

| Group | Outputs | Producer | Result |
| --- | ---: | --- | --- |
| `tsoptions/commandLineParsing/parseCommandLine` | 53 | pinned `formatNewBaseline` | 53 exact |
| `tsoptions/commandLineParsing/parseBuildOptions` | 27 | pinned `formatNewBaselineBuild` | 27 exact |
| `config/tsconfigParsing` | 87 | carried assembly over pinned calls | 87 exact |
| `config/matchFiles` (F2a) | 142 | carried renderer | 68 exact, 74 excepted |
| **total** | **309** | | |

These 167 are `rendering_verified: true` in `data/phase1/config-baselines.json`,
so `exception_problems` refuses an exception on any of them: the only passing
state is all 167 reproduced exactly.

**`config_prepared: true`.** All 402 operations on the config roster are
prepared by a runnable case, witnessed by a rust-gated artifact, or removed by a
reviewed ledger entry: 310 prepared, 47 witnessed, 45 exempt, none pending.

**484 cases: 176 match, 299 not_implemented, 9 different, zero harness failures,
zero native_unavailable.**

| Group | Cases | match | not_impl | different |
| --- | ---: | ---: | ---: | ---: |
| `parseCommandLine` outputs | 53 | 0 | 53 | 0 |
| `parseBuildOptions` outputs | 27 | 0 | 27 | 0 |
| `tsconfigParsing` outputs | 87 | 0 | 87 | 0 |
| parse-config host | 11 | 8 | 2 | 1 |
| config parsing | 57 | 54 | 3 | 0 |
| option values | 16 | 14 | 2 | 0 |
| option declarations | 11 | 7 | 4 | 0 |
| config text | 7 | 7 | 0 | 0 |
| file specs | 13 | 12 | 1 | 0 |
| syntax, JSON, wildcards, name maps, defaults, absolute paths | 26 | 4 | 22 | 0 |
| the four option parsers | 9 | 0 | 9 | 0 |
| command-line operations | 70 | 20 | 48 | 2 |
| module resolution | 59 | 42 | 12 | 5 |
| package JSON | 16 | 2 | 13 | 1 |
| diagnostic writer | 18 | 2 | 16 | 0 |
| **total** | **484** | **176** | **299** | **9** |

The 167 baseline outputs are all `not_implemented` on the Rust side, and not
for the same reason. The envelope has no Rust producer for any of them: the only
diagnostic writer in the tree needs a `Program`, which a config parse does not
produce. On top of that the 80 command-line outputs have no argument-vector
parser at all, and the 40 `json`-API outputs have no raw-JSON entry point, while
the 40 `jsonSourceFile` and 7 `jsonParse` outputs do have their parse. Recording
one undifferentiated result across all 167 would say the port is equally far
from each, which is false.

### Divergences this step found

| case | what differs |
| --- | --- |
| `module/exports/pattern-with-trailer` | **the pin panics.** `matchesPatternWithTrailer` (resolver.go:2062) returns true when the name carries the pattern's prefix AND its suffix, with no length guard, and :721-723 then slices `moduleName[len(before) : len(moduleName)-len(after)]` with no check that the bounds are ordered. Exports key `"./a*a"` and request `pkg/a` reach it with moduleName `"./a"`: starPos 3, slice `"./a"[3:2]`. The port carries the guard the pin lacks. |
| `module/helpers/parse-node-module-from-path` | **the pin panics** with an index-out-of-range on the `isFolder=false` corner, on a row outside the set upstream's own issue-4373 regression covers. |
| `packagejson/version-paths-mappings-are-rebuilt-per-retrieval` | `GetVersionPaths` returns the struct BY VALUE (cache.go:28) and `GetPaths` memoises into whichever copy it was called on (:98-120), so the `sync.Once` makes the SELECTION happen once per package while the mapping TABLE is rebuilt once per retrieval. Two reads of one retrieved value share a table; two retrievals do not. Go answers `[true, false]`, the port `[true, true]`. |
| `commandlineops/list-option-empty-value-for-a-listorelement` | `fixture_options.rs:73` hoists the empty-value return above the `listOrElement` single-element branch. The pin tests them in the other order (commandlineparser.go:353 before :360), so `ParseListTypeOption(extends, "")` answers `[""]` in Go and `[]` in Rust. |
| `commandlineops/name-map-over-the-build-table` | `option_declarations.rs:93` scans short names with `.iter().find` (first wins) while the full-name scan below it uses `.iter().rev().find` (last wins, matching namemap.go:18-23). In `BUILD_OPTIONS` the short name `d` is declared by `declaration` and by `dry`: the pin answers `dry`, the port `declaration`. |
| `module/exports/dot-object-classification` | exports-object classification for the `.` subpath. Rust's `object_kind` is production-dead: its only caller is inside `#[cfg(test)]` at package_json.rs:541. |
| `module/typeref/relative-reference-normalization` | `normalizePathForCJSResolution` is missing from the port's type-reference path. The same file resolves; the trace differs -- which is exactly the trace-only discrepancy the plan asks to be visible. |
| `module/control/relative-containing-file` | the pin uses the relative containing directory exactly as given; the port's filesystem refuses a relative path outright. |
| `host/relative-path-refusal` | the pinned test filesystem refuses a non-absolute path, by panicking; `tsr_vfs` resolves it against the current directory and answers. |
| `module/control/unsupported-module-resolution-kind` | the two sides refuse in shapes that are not comparable: the pin panics, the port returns `Unsupported`. |

### What the adversarial review changed

Three reviewers found 6 blocking and 15 non-blocking issues across the
integrated groups. The blocking ones were all attribution or evidence defects,
not comparison failures:

* a case claimed `getPackageId` on a path its single action cannot reach -- the
  lookup returns at the first typeRoot, and the empty package id in its own
  frozen row is the proof;
* two cases recorded a GAP for an operation their own driver text says is
  present and correct, publishing `missing` on a false basis. What the port
  actually lacks is the ability to reach a second cache write, because its host
  and options are both fixed for a resolver's lifetime;
* a refusal control could not move: the pinned row carries a `panic` key the
  Rust driver cannot produce for any behaviour, so the regression it claimed to
  reject would have left the verdict unchanged;
* a roster exemption asserted a reachability fact the pin contradicts;
* a contract was one row out and stated its first discriminator backwards.

**The annotation pattern was under-detecting by 9%.** `re.finditer` does not
overlap, and the pattern consumed the `fn` header, so wherever this tree stacks
two `port:` annotations above one function -- 113 times, recording one Rust
function serving two pinned operations -- it read the first and dropped the
second. `format` serves both `WriteFormatDiagnostics` and
`FormatDiagnosticsWithColorAndContext`, the call every one of the 87
`tsconfigParsing` baselines renders its `Errors::` section through, and the
second was invisible. Two refusals were wrong because of it.

### Limitations

* No case in this step distinguishes the module cache's `LoadOrStore` from the
  type-reference cache's `Store`, and none can: the port's host and options are
  fixed at construction, so a second write for one key is unreachable.
* `internal/module`'s three race regressions: two cannot be reproduced by an
  ordered single-threaded trace at any level of the pinned API, because
  `getPackageJsonInfo` short-circuits on a cached nil-`Contents` entry before
  the `Set`, so only losing `LoadOrStore` produces that state.
* The `go_test_harness` operations gated on `TSGO_BASELINE_TRACKING_DIR`, and
  the `filefixture` methods reachable only inside `Benchmark` functions, are not
  reached by an ordinary `go test`.
* `crates/tsr_compiler/examples/p2/baseline.rs:61` carries the same `port:`
  marker as the production flattener on a second, independent implementation.
  Two bodies answer to one marker and nothing compares them. Reported by
  `inventory --check` under `port_annotations_outside_src`, not fixed here.

### What is carried, and what is not

The two command-line groups carry nothing. `formatNewBaseline`
(`commandlineparser_test.go:334`) and `formatNewBaselineBuild` (`:456`) are pure
functions of the sections handed to them, so the probe compiles into the pinned
`tsoptions_test` package and calls them; for the eight outputs that need a
synthesised option declaration it calls the pinned
`createVerifyNullForNonNullIncluded` rather than restating it.

`tsconfigParsing` is the group the plan warned about. Its primary renderer,
`baselineParseConfigWith`, ends in `baseline.Run`, and its secondary renderer is
inline in a test body. Neither can be called as-is: `baseline.Run` writes under
the pinned submodule and calls `t.Errorf` on any difference, so a Rust
divergence would abort the capture instead of recording a `different` row — the
comparison would destroy the evidence it exists to produce. Both assemblies are
therefore carried, which the plan authorises for exactly this case. What is
carried is the section sequencing; every value in it comes from a pinned call
(`printFS`, `json.MarshalIndentWrite`, `FormatDiagnosticsWithColorAndContext`,
`ParseConfigFileTextToJson`, `writeJsonReadableText`), the host is
`tsoptionstest.NewVFSParseConfigHost`, and both entry points are the pinned
`getParsedWithJsonApi` and `getParsedWithJsonSourceFileApi`.

### Where the inputs come from

The 80 command-line outputs recover their argument vector from the baseline's own
`Args::` section, which is an input section. The recovery is proved rather than
assumed: each row re-renders the vector with the pinned writer's rule and refuses
the row unless it equals the line it read. All 80 round trip.

71 of the 87 `tsconfigParsing` outputs read their inputs from the pinned
package-level tables, so nothing is transcribed. The other 16 belong to
`TestParseTypeAcquisition`, whose table is a function-local and unreachable from
an overlay; those recover their config text from the baseline's own `Fs::` and
`configFileName::` sections, with the rest of the input shape taken from the
pinned literal, which is constant across all eight cases. A wrong recovery cannot
pass silently, because `Fs::` is rendered back out of the reconstructed host.

### Mutation checks

Every group reproduced on its first run, which by itself means nothing, so each
was mutation checked and every blast radius was compared against what the corpus
predicts:

| mutation | outputs broken | the corpus says |
| --- | ---: | --- |
| drop the last argument | 79 of 80 | 1 output has an empty `Args::` vector |
| drop the synthesised declarations | 8 | 8 outputs name `--optionName` |
| blank the `Errors::` section | 39 | 39 outputs carry an error |
| join file names with `;` | 3 | 3 outputs have two file names |
| `Errors::` newline `\r\n` → `\n` | 17 | 17 outputs' error text carries a CRLF |
| never emit `TypeAcquisition::` | 38 | 38 outputs carry that section |
| drop a recovered file entry | 16 | 16 outputs are recovered |
| drop the multi-input block separator | 6 | 6 outputs have two blocks |

The file-name join is a coverage fact as much as a control: only 3 of the 80
command-line outputs carry more than one file name, so that join is weakly
exercised.

One prediction was wrong and the probe was not. The first count for the newline
mutation said 23; it had run past the block boundary into the next block's `Fs::`
section, which `printFS` also writes with CRLFs. Bounded to its own block, the
corpus says 17, which is what broke.

### Existing evidence, connected

47 operations move from a rule-guessed disposition to rust-gated, through eight
witness records verified operation by operation against the pinned bodies. The
verification refuted four claims, which is the part worth keeping:

* `jsonvalue.go:unmarshalJSONValue` has zero callers anywhere in the pin. Dead
  code, not evidence — and now a roster exemption.
* `resolver.go:GetAutomaticTypeDirectiveNames` is called by the trace adapter,
  but takes no tracer, contains no write call in its pinned body, and its frozen
  row is structurally forced to `{"traces":[]}`. It witnesses "does not panic".
* `diagnosticwriter.go:WrapASTDiagnostics` and `ToDiagnostics` are a Go-only
  wrapper layer; Rust passes `&[&Diagnostic]` straight in, so nothing
  corresponds. `CompareASTDiagnostics` is skipped by Rust, which sorts with a
  port of the different `ast/diagnostic.go:CompareDiagnostics`.

The field-versus-method trap cost several more: the packagejson adapter reads
`e.Valid`, `e.Null` and `v.Type` as FIELDS, so `Expected.IsValid`,
`Expected.ExpectedJSONType`, `JSONValue.IsPresent` and `JSONValueType.String` are
not exercised, and the dependency FIELDS being asserted is not the
`DependencyFields` methods running.

Each record states the asymmetries a later citation must not gloss over: the
module-trace dataset discards the resolution result on both sides and witnesses
the callback stream only; the config-mappers comparison is not same-entry-point;
four pinned diagnosticwriter operations are two Rust functions with a `pretty`
flag; and `boundary_tests` has no Go row at all, so it gates Rust against
expectations read from the pinned source rather than proving parity.

Two facts came out of checking rather than assuming. The config-mappers manifest
resolution really is exercised — 4 of the 15 frozen rows resolve a manifest with
populated fields — which the survey had left unconfirmed. And
`handleOptionConfigDirTemplateSubstitution` looked like a no-op until a
case-insensitive scan found `source-16`, which carries `${CONFIGDIR}` and a
Turkish dotted capital I: the pinned guard lowercases and the pinned replacement
does not, so both branches run and produce the untouched string, which is
exactly what the frozen row records.

### The config roster

45 exemptions: 24 `later_step`, 17 `go_test_harness`, 4 `unused_at_pin`. Every
entry carries the caller set read at the pin, with a bare-name search each time —
the F1a lesson that a Go method VALUE is a caller a `.Method(` search does not
find.

**Four candidates were disqualified and stay on the roster.**
`parsedcommandline.go`'s output-path cluster is reached from `internal/compiler`,
three of the four only through a chain no direct search shows.
`CommonSourceDirectory` has no direct caller at all — it is reached by interface
dispatch through `outputpaths.OutputPathsHost` — and
`checkSourceFilesBelongToPath` only as a method value. Since `internal/compiler`
is `membership: "partial"` and F4a decides its boundary, none is exemptible here.

**Twelve of the 24 use a transitive reading** of the caller rule, accepted in
the PR #43 re-review after checking the pinned references. Their callers are
themselves owned by later phases, as in F2a's `openMetadata` entry. This includes
the `showconfig.go` helpers whose results are consumed only by
`ConvertToTSConfig`, the option-change helpers used by incremental execution,
and the file-match helpers used by watching and the project system. `computeFn`
runs during package initialization, but only `addImpliedOptions` consumes the
table it builds. These remain real later-phase work, not `unused_at_pin`.
An explicitly assigned Phase 1 operation or an additional in-scope caller would
invalidate this reasoning.

**`go_test_harness` is a new category**, accepted in the same technical review
for the 17 fixture and baseline-bookkeeping operations. None of the
six original categories describes upstream's own test harness, and bending
`build_tooling` would have been the wrong kind of convenience: that category says
the port generates the same artifact elsewhere, and there is no artifact here.
The port must reproduce the 309 reference outputs, and it does; it must not
reproduce the file bookkeeping, because its comparisons run through
`scripts/phase1*.py` and Rust tests that assert against frozen rows.

### A name-match rule that was wrong

`isDoubleQuotedString` was `implemented_untested` on the strength of a Rust
function of the same name — which carries `/// port:
tsc/internal/parser/parser.go:isDoubleQuotedString` and performs the single-quote
token-flag test the `tsoptions` copy does not. Two Go packages, two different
functions, one name.

The row was not corrected; the rule was. A Rust function that declares itself the
port of a different operation is no longer read as evidence for this one. 3,773
Rust functions carry such an annotation, and the rule reclassified nine rows
across `ast`, `binder`, `tspath` and `tsoptions`, each an exported/unexported or
cross-package name collision.

### Defects the gate caught in its own author's work

* `output_preparation` kept the first native row it saw per case, so a probe that
  DECLINES a case shadowed the probe that observed it, and all 87
  `tsconfigParsing` outputs reported "no corresponding native observation" while
  the observations sat right there.
* It then read a probe's observations once per GROUP, so a probe serving two
  groups presented every row twice and tripped the two-observers conflict on its
  own duplicate.
* Eight roster exemptions named operations that do not exist —
  `parsedcommandline.go:GetOutputFileNames` where the scope carries
  `ParsedCommandLine.GetOutputFileNames`. An exemption for an operation nobody
  can find is not an exemption.
* The regression asserting 167 observations and 167 declines broke when a third
  probe joined the family. That was arithmetic, not the property it guarded; it
  now asserts that every output is observed by exactly one probe.

### The parse-config host

`internal/tsoptions/tsoptionstest` is not a `_test.go` file — it ships in the
module and other packages import it — so it has a real caller-visible contract:
it is how every pinned config test, and every F3a probe, turns a
`{path -> content}` map into a `ParseConfigHost`. 11 cases: 8 match, 1 different,
2 `not_implemented`.

The divergence: the pinned filesystem REFUSES a path that is not absolute, by
panicking, while `tsr_vfs` resolves it against the current directory and answers.
Its attribution is deliberate — the cause is in the filesystem the factory
builds, not the factory, so the symlink-normalization case that first surfaced it
was split and the refusal has its own case saying where the behavior actually
lives. The refusal is recorded as a flag, not as the pin's panic wording: what
the host does is the contract, how it phrases its own panic is not.

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

### PR #43 shared harness review

The P2 diagnostic adapter now calls the production formatter and program sorter;
its canonical P2 schedule returned byte-identical observations before and after.
All three Phase 1 drivers share `tools/phase1/harness`, including outcome
serialization, so a missing-operation identity comes from the handler rather
than the request label. The source closure includes the shared crate transitively.
The example-marker check now requires no out-of-src markers and retains a
synthetic regression that proves detection still works.

A Rust-only replay of all 225 leaves, 359 filesystem and 484 config requests
changed exactly one row: `leaves/bundled/wrapper-dispatch-surface` now identifies
`wrappedFS.WalkDir` as the missing operation. `wrapFS` merely constructs the
wrapper (`embed.go:41`), which `BundledFs::new` already implements; the handler
itself identified walking as the blocker. Every other response is unchanged.
At the PR #43 merge, the old recorded `missing_operations: [wrapFS]` was left
untouched as historical evidence, not rewritten to look like a new capture.
The required leaves re-record is documented below. No native
observations, frozen reports or acceptance evidence were refreshed for this
refactor. Cargo dependency changes invalidate affected capture fingerprints
under the existing rules.

The final PR review also repaired two preparation-layer gaps: an observed row
could hide another baseline probe's harness failure, and a bare diagnostic-writer
operation label could leak into `missing_operation` instead of the pinned ID.
Failure injection now rejects both probe orderings; an identity regression checks
the canonical writer ID. The current request schedule is unchanged by these
repairs. The CI publication-policy failure was the omitted private
`phase1_config` package; both it and `phase1_harness` are now registered.

The removed P2 sorter marker required regenerating `data/s07/operations.json`.
The accepted subset review records that the sole inventory change is removal of
that example mapping; the production mapping remains. Replaying existing,
authenticated syntax and loader captures produced byte-identical selected cases
and checker obligations. Only the operation-matrix digest and its review chain
changed, without updating any measured outcome or producer fingerprint.

The second review found that the shared exemption validator still selected
only direct `leaves` cases. It now selects the preparation families of the step
being validated, including `pilot` for filesystem. A regression injects a
conflicting exemption for an unwitnessed direct case in each of the three
steps, for each of `match`, `different` and `not_implemented`. All six config
and filesystem subcases failed before the fix and pass afterward. The existing
rosters contain no such conflicts, so preparation counts and recorded evidence
are unchanged. The focused Phase 1 suite passed (161 tests, 17 subtests); broad
compiler tests, native captures and benchmarks were not repeated.

### F1b starting capture and ordering amendment — 2026-09-21

The owner authorized starting F1b after F3a, pausing the remaining preparation.
The implementation plan now lists F1b before F4a and F5a. Their coverage review
and integration obligations remain required; no incomplete preparation metric
is treated as passing.

Before production changes, a complete leaf capture on merged main `e4aebf8`
ran all 225 cases against pinned Go 1.27.1 and the current Rust driver. The
recorder requires a complete family, so the single attribution correction was
recorded through that existing contract, without adding a partial-record bypass.
Results remain **74 match, 150 not_implemented, 1 different**, with no native
unavailability or harness failure. Every native observation equals the previous
frozen observation. The sole case-manifest change is
`leaves/bundled/wrapper-dispatch-surface` naming `wrappedFS.WalkDir` as absent
instead of `wrapFS`. Regenerated scope links now attach the gap to that method.
The options-clone sharing difference remains visible and unapproved.

The replay inputs and both runtimes' observations are retained in
`data/phase1/captures/leaves-f1b-start.tar.gz`; it contains only the 27 JSON
request, observation and provenance files, no binaries or exported upstream
tree. Its capture provenance SHA-256 is
`0de6a34efa0365022e4d4fc14a211a525a5f8d5c3ab0c13cba8c23094419ef45`.
The capture authenticates 498 source inputs and is a starting-point record,
not evidence for subsequent production edits.

Commands used:

```sh
python3 scripts/phase1.py capture --family leaves --output target/phase1/f1b/leaves-start
python3 scripts/phase1.py compare --capture target/phase1/f1b/leaves-start
python3 scripts/phase1.py record --capture target/phase1/f1b/leaves-start --write
python3 scripts/phase1.py inventory --write
```

Go 1.27.1 was placed on `PATH`. Only this leaf family was captured; no full
compiler corpus or performance measurement was needed for the attribution fix.

### F1b first implementation batch: ordered and scoped collections — 2026-09-21

F1b is **in progress**, not complete. The first batch implements the non-JSON
`OrderedMap`/`OrderedSet` operations and the copy-on-write map/set in
`tsr_core::collections`. It uses the existing core dependency rather than adding
a crate. `PORTS.toml` records the actual Rust files as in progress, and the Phase
1 inventory records those homes. Its generated `tsr_collections` planned-crate
classification is retained; changing the package-wide crate map and regenerating
upstream provenance remains part of the final F1b inventory update.

Representation choices:

- Ordered maps hold a key deque and a hash table. Overwrite preserves position;
  delete/reinsert appends; first/last deletion avoids shifting all keys. Borrowed
  iterators serve ordinary reads. An explicit mutable visitor rereads the current
  index and length after each callback, preserving Go's append and deletion-shift
  behavior without cloning the whole collection. Diff notifications retain the
  native order. Diff callbacks borrow both operands, matching the non-mutating
  watcher/project consumers at the pin.
- Nil Go receivers are represented by `Option`; the leaf adapter translates
  nil-tolerant operations and required-receiver failures. This is not a nullable
  production object or a new promise that every Rust method accepts nil.
- Copy-on-write maps retain an optional `Arc<HashMap>`: empty scopes allocate
  nothing, reads borrow, and the first shared write clones entries. Rust values
  representing shared Go objects must themselves preserve identity, such as an
  `Arc` or an ID; cloning the container does not turn owned mutable values into
  shared objects. Exclusive scope guards restore on normal exit and unwind.
  Owned snapshots let the checker's existing serialization callback borrow the
  whole builder. Its three naming tables now share storage on entry instead of
  cloning every table eagerly, and retain the existing fast hasher. Normal and
  `Result`-error exits restore the saved names. Checker panic retirement is
  unchanged. No performance ratio is claimed.

The complete 225-case capture is **100 match, 124 not_implemented, 1 different**,
with zero unavailability, harness failures or unrun cases. All 26 new matches
were previously missing: 17 ordered-map traces, five ordered-set traces and four
scope traces. Every native observation is byte-identical to the starting
capture, and no previously observed Rust row changed. The five ordered-map JSON
cases still name the missing marshal/unmarshal operation; their adapter metadata
now points to the pending integration in the production collection. The options
clone's shallow-sharing difference remains visible and unapproved.

Replay inputs and observations are archived in
`data/phase1/captures/leaves-f1b-collections.tar.gz` (27 JSON files, 247,047 bytes;
no binaries or upstream export). Capture provenance SHA-256:
`2bdef100b467260ab222e10a1ad8f04c73b10658491615b8915e0b13036a09b6`.
The capture binds 506 input files. Results were installed only through
`phase1.py record`, followed by `inventory --write`.

Validation: all 17 core tests pass in debug and release, including copy counts,
unwind restoration, shared value identity, live mutation and deque wraparound.
A checker regression covers nested name scopes and an error followed by a
sibling scope; all **208** frozen P5 display queries still match pinned Go.
Targeted clippy passes with warnings denied. The Phase 1 Python suite passes
**161 tests and 17 subtests**. No generator/dependency inputs changed, and no
full compiler corpus or performance capture was run.

The S07 operation inventory gains 13 ordered-map source mappings and moves three
TextRange marker anchors by one line. All other operation data is unchanged.
Replaying authenticated existing syntax/loader observations leaves subset and
checker-obligation bytes unchanged; only the operation digest and its review
chain change. Historical producer freshness and outcomes are not rewritten.

Next work remains the other collection/core/text helpers, ordered-map JSON plus
the shared JSON layer, locale matching and generated translations, diagnostic
formatting, bundled access, and the explicit options-clone disposition. The
case manifest retains the exact outstanding IDs; this batch does not claim
F1b's full-family or consumer-integration exit.

### F1b consumer review — 2026-09-21

The review's file-collection finding is addressed: `file_names_from_specs` now
uses the shared `OrderedMap` for literal, wildcard and JSON files. Lookup,
overwrite, insertion and unsuccessful removal use hashing instead of scanning
the complete vector. Successful middle removal is still linear, as in Go; this
is not a claim that every workload is linear or a measured compiler speedup.
Final collection consumes map values in insertion order without cloning them.

The API also supplies borrowed `get_mut` and live `visit_keys_mut`, so callers
can update values without requiring `V: Clone`. `visit_entries_mut` delegates
to the same traversal and explicitly retains its per-value clone cost. A test
with a non-Clone payload exercises mutation, append, delete, early stopping and
ordered consumption. An ordered `values_mut` iterator is not needed by these
callers; key visitation plus `get_mut` provides safe mutation without collecting
references or introducing unsafe code.

Scoped maps/sets expose an explicitly named O(1) `snapshot`. The checker now
uses `TypeParameterNames::snapshot`, with an explicit field initializer instead
of `derive(Clone)`. Adding a field requires changing that initializer, and each
current table calls its copy-on-write snapshot method. The struct documents the
cost invariant; the plan records the fourth Go table on the Phase 2 queue.

The [ordered-storage decision](PHASE1-implementation-plan.md#ordered-storage-and-json-integration-decision--2026-09-21)
requires migrating config objects and `PathMappings` with the JSON batch and
places encoding traits in the JSON layer above core storage. One review claim
needed narrowing: package.json's raw `Object` is a duplicate-preserving decode
sequence, not a private ordered-map implementation. It remains a sequence so
duplicate/partial-failure behavior survives; native witnesses and the separate
semantic-object audit are recorded in the plan. No equivalence exemption is
granted by this decision.

Both prepared families were recaptured on the final Rust sources:

| Family | Match | Missing | Different | Harness/unavailability/unrun |
| --- | ---: | ---: | ---: | ---: |
| Leaves | 100 | 124 | 1 | 0 |
| Config | 176 | 299 | 9 | 0 |

Every case retains its previous result classification. Leaf Rust observations
are byte-identical after decoding to the first batch. All 56 implemented config
cases attributed to file aggregation match native Go, including the three
direct file-name cases; six other attributed cases still stop at named missing
operations. The 142 matchFiles outputs alone would not test this aggregation.

The new combined archive is `data/phase1/captures/f1b-consumer-review.tar.gz`:
46 JSON files, 490,968 compressed bytes, with no binaries or upstream tree.
Its `leaves/` provenance SHA-256 is
`a818ef69cee442086ef08195812d914e299b49ecffbe2880cfa7725843a5754f`;
`config/` is
`e852d16316e7f14b6dbf45ccfa48a9e64294528f3543d3f4d7b58bb1264891b9`.
The starting and first-batch archives remain historical observations; no hashes
were relabeled as current.

Validation also passes for the 18 core tests and two tsoptions tests in debug
and release, the checker scope-restoration regression, all 208 frozen display
queries, targeted clippy with warnings denied, and 161 Phase 1 Python tests plus
17 subtests. No benchmark or full compiler corpus was run. The regenerated S07
inventory changes only 11 marker anchors; replay preserves the selected cases
and checker-obligation bytes, without refreshing historical producer evidence.

### F1b ordered config storage and typed JSON — 2026-09-21

This continuation implements the next part of the ordered-storage decision.
`ConfigValue::Object` and `CompilerOptions::PathMappings` now use the shared
`OrderedMap`. Source-property overwrite keeps its first position. ConfigDir
substitution borrows and mutates values in order; resolution borrows the winning
path substitutions directly and keeps first-insertion precedence on equal
wildcard prefixes. Empty/nil maps and target lists keep their existing public
representations. CompilerOptions cloning still copies its values; the single
unapproved Go shallow-sharing difference remains visible. Package.json's raw
member stream remains duplicate-preserving, as previously decided.

The new `tsr_json` crate owns typed `Encode` and `Encoder` APIs above `tsr_core`.
Production `StringifyJson` now delegates to it, and `ConfigValue` implements the
trait over its actual fields. This removes the separate config-only encoder.
Ordered-map encoding walks borrowed keys/values; deterministic ordinary-map
encoding sorts borrowed entries by the original key bytes. Neither builds a
second JSON value tree. Ordered consumption and mutation APIs do not clone
payloads. Equality retains the old order-sensitive config/path contract.

This is a **typed-write increment, not completion of the JSON contract**.
It supports byte strings, finite numbers (including negative zero), booleans,
optional values, lists, string-keyed ordered/unordered maps, compact output and
typed indentation. Typed nil slices remain the caller's explicit list encoding,
not the generic `Option<T>` null encoding. Invalid UTF-8 is repaired per rune;
key uniqueness is checked after repair. The 10,000-container depth limit runs
with stack growth and is exercised on a 512 KiB thread stack.

The write implementation reuses the existing byte/rune and number code instead
of selecting another JSON value library: `serde_json::Value` cannot carry the
byte-string inputs, duplicate member stream or raw-number spellings this
contract requires. No new third-party dependency is added. This does **not**
settle the decoder's implementation. Raw JSON token encoding, streaming I/O and
encoder state, decoder offsets/partial destination writes, typed key conversion
and ordered-map decoding are still named gaps. Failed typed encoding currently
returns an error without a partial output document; native partial-error state
is not claimed. The leaf adapter projects only the supported top-level float
error into the Go observation schema; it supplies no raw/nested error positions.

The final prepared-family comparisons are:

| Family | Match | Missing | Different | Harness/unavailability/unrun |
| --- | ---: | ---: | ---: | ---: |
| Leaves | 104 | 120 | 1 | 0 |
| Config | 176 | 299 | 9 | 0 |

Four JSON cases changed from missing to matching: deterministic string-map
order, invalid UTF-8 strings, binary64 edge values/nonfinite rejection, and the
sample struct's missing/null/empty fields. **Every previously observed Rust
payload in both families is byte-identical after decoding** to the PR #44
capture. The real F3 config adapters exercise the new storage/writer; this does
not claim all 309 baseline outputs pass. Original Go config-mapper JSON,
package.json, config-resolution and module-resolution trace tests also pass.

`data/phase1/captures/f1b-typed-json.tar.gz` contains 46 JSON files, 495,314
compressed bytes, with no binaries, exported upstream tree or duplicate
per-probe request copies. Both families replay successfully from its extraction.
Provenance SHA-256:

- leaves: `58a8bdbeec13995e886e947c9a0153fd3af03cbf825e61c4bcf39d3dc0f9b347`
- config: `27fd6b439f3d441dccaacad98abc7ccafb9aa3907b0323d8fc87ca12c4b5ce0a`

Validation: 35 affected Rust tests, targeted clippy with warnings denied,
Rust 1.96 checks for the new JSON crate and its options consumer, formatting,
and 161 Phase 1 script tests plus 17 subtests. The S07 operation inventory gains
two typed JSON wrapper homes and moves twelve marker anchors; its source call
closure, selected variants and checker-obligation bytes are unchanged. Historical
producer evidence was not relabeled or refreshed. No compiler corpus or
performance benchmark was run. F1b remains in progress; the remaining JSON
contracts and the other recorded leaf gaps remain on its implementation queue.

### F1b packaging and encoder review — 2026-09-21

The new public JSON crate was missing from the publication policy. Added
`tsr_json` to `tools/packaging/packages.json`, the package table and the release
order (after its dependencies, before `tsr_tsoptions`). Synced its NOTICE through
`package_assets.py`; the standard copy had been shortened incorrectly. The
check now verifies 169 assets for 29 public / 49 total packages. A direct audit
also confirms that the documented policy names every package exactly once and
that every retained internal dependency precedes its dependent in release order.
The Cargo archive contains the exact source, LICENSE and NOTICE and normalized
dependencies without local paths. Publication remains deferred by the owner;
no name reservation or release was uploaded.

The [encoder continuation review](PHASE1-implementation-plan.md#json-encoder-continuation-review--2026-09-21)
records the next batch: shared token state, structured errors, exact unsigned
integers, container-scoped stack growth and decoded-name storage. Mixed member
types already work via `&dyn Encode`; sequential writes remove the temporary
member list. Pinned Go copies names into a separate decoded-name buffer too, so
using output-buffer offsets without a flush/escape strategy is not adopted.
These are planned changes, not new claims of JSON coverage in this increment.

The NOTICE edit invalidates the authenticated leaf/config inputs. Both prepared
families were re-captured and recorded, with every Rust observation unchanged:
leaves **104 match / 120 missing / 1 different**, config **176 match / 299 missing
/ 9 different**, no harness failures or unavailable/unrun cases. The previous
archive is retained. The new
`data/phase1/captures/f1b-typed-json-packaging.tar.gz` contains 46 JSON files,
491,287 compressed bytes, and replays from its extracted contents. Provenance
SHA-256:

- leaves: `48714d2f16e791c994488dc3fa0ba7d5d6bcc0cb74bcd862b9f3321ec83668aa`
- config: `4f416b1c1d4c5e5438de7d553b6ab3d31c52db46fe01f919279f72efc9e7e8d2`

No Rust implementation, compiler corpus or benchmark changed in this follow-up.

### F1b foundation completion — 2026-09-21

This is the pre-review result. The ownership correction below supersedes the
compiler-options representation and the exact-match count, retaining one
owner-approved difference.

F1b's prepared implementation scope is complete: **230 / 230 leaf cases match
pinned Go**, with no missing operations, differences, unavailable observations or
harness failures. This closes the 120 missing rows and the compiler-options
clone difference left by the prior increment. Five new native cases cover exact
signed/unsigned integer encoding, heterogeneous object members, partial marshal
output and two slice-growth alias boundaries. Expected observations come from
the pinned probes, including error classes, offsets, pointers, partial state,
callback traces and native panics; they were not rewritten to match Rust.

The affected config consumer remains **176 match / 299 missing / 9 different**
over its 484 cases. Its complete Rust observations are byte-for-byte identical
to the prior packaging capture. Those remaining config/command-line and
module-resolution obligations belong to F3b. Passing F1b does not complete F2b,
F3b, the remaining preparation, all 309 baseline ports or Phase 1.

#### Implementation and representation decisions

- `tsr_json` now has a shared token state machine for typed values, raw values
  and incremental `Read`/`Write` streams. It validates container state, member
  position, duplicate names, depth and exact number syntax. Typed decode
  preserves successful prior fields and allocation state on later failure.
  Errors retain the native category, byte offset, JSON pointer, target type and
  cause. `marshal_partial` exposes Go's partial-output contract; the existing
  `marshal` convenience API discards failed output explicitly. No external JSON
  dependency or second production value tree was added.
- The encoder review is implemented: sequential tokens support heterogeneous
  records; exact i64/u64 writes never round through f64; built-in scalar writes
  avoid stack-growth checks while recursive container and custom-codec calls
  remain guarded. Duplicate-name storage uses a decoded-name buffer plus
  offsets, promoting larger namespaces to a map. Offsets into the flushable
  output alone would lose names and confuse escaped equivalents. No protocol
  performance claim is made from these leaf tests.
- New core homes implement sets, concurrent maps/sets, multimaps, slice helpers,
  range algebra and open-enum names. Callback-bearing slice helpers release
  their read borrow before invoking user code. Concurrent map callbacks run
  outside the lock. Memoization retries after an initializer panic. Existing
  consumers use the common enum text and diagnostic format implementations.
- `CompilerOptions::clone` now retains Go pointer fields and slice headers.
  `SharedValue` distinguishes mutating a pointee from replacing a field;
  `SharedSlice` retains backing, range and logical capacity. Paths use the
  existing ordered map behind the shared pointer, with shared target slices.
  Config substitution and module lookup use that representation directly.
  Append models pinned Go element sizes and capacity rounding because growth
  changes observable aliasing; Rust still allocates ordinary owned elements.
  The two new native growth traces distinguish per-element and batch appends.
  This representation is reserved for contracts that expose sharing; ordinary
  vectors elsewhere remain ordinary vectors.
- Text helpers preserve raw bytes, Go rune/simple-case comparisons and the
  pinned character classes. Scanner identifier predicates and their generated
  tables now live in the shared text crate; the scanner calls that same home.
  Existing number/string arithmetic remains in its established crates.
- `tsr_locale` owns BCP 47 recovery, canonicalization, context/default identity
  and diagnostic-language fallback. Its matcher is specialized to the pinned
  fourteen-language roster, not a new arbitrary-roster matching API. The
  generator exports registry/CLDR data and the compiled matcher index from
  pinned x/text; it refuses an unsupported index continuation after pin drift.
  Additional generated native tests cover recovery and matching, including
  transform extensions and excessive variants. The module cache and pinned
  submodule are never edited.
- `cargo xtask gen` now exports and verifies all thirteen localization assets.
  `tsr_diagnostics` initializes each decoded table once and caches requested
  locale outcomes, including untranslated tags; unspecified English bypasses
  the caches. English/translated formatting, arbitrary argument bytes, missing
  keys and ad-hoc messages are production operations used by the leaf driver.
  The generated translations are uncompressed JSON string literals (about
  4.3 MB before linking), decoded lazily. This avoids another compression
  dependency; it is not a footprint optimization or a new performance result.
- The bundled filesystem exposes the native walk/delegation and mutation
  behavior. Its test-only source-directory accessor is feature-gated. The
  complete asset roster and byte hashes are compared with native observations.
- The raw package.json member stream is retained: it must preserve duplicate
  fields and partial typed updates before normalization. Feeding it through an
  ordered map first would erase those events. Its existing prepared native
  comparisons remain unchanged; this batch does not claim a new decoder port
  for the outstanding F3b package.json cases.

`tsr_locale` is registered in the publication policy, public-package table and
release order before diagnostics; JSON also precedes diagnostics and options.
Package assets include the required Go/x/text attribution. No registry upload
was made. Capture and ledger source lists that name crate closures explicitly
now include both JSON and locale dependencies.

#### Evidence and validation

The final archive is `data/phase1/captures/f1b-foundations-complete.tar.gz`.
It retains native/Rust observations, exact requests and provenance, without
binaries, build trees or exported upstream sources. Older captures are kept.
Reproduction commands (Go 1.27.1 on PATH):

```sh
python3 scripts/phase1.py capture --family leaves --output target/phase1/f1b/leaves
python3 scripts/phase1.py compare --capture target/phase1/f1b/leaves --require-parity
python3 scripts/phase1.py capture --family config --output target/phase1/f1b/config
python3 scripts/phase1.py compare --capture target/phase1/f1b/config
python3 scripts/generate_locale_tables.py --check
cargo xtask gen --check
python3 scripts/s05.py tables
```

Validation includes tests of the affected production crates, JSON stream/error
regressions, the generated locale conformance test and generator negative
fixtures; workspace clippy with all targets/features and warnings denied; fmt;
Rust 1.96 checks for the changed foundation/options dependency closure; deny;
package asset policy; script tests; tracker validation and committed views.
The existing E4 string oracle was rerun and recorded on these inputs. No full
checker corpus or performance benchmark was run.

The S07 operation matrix was regenerated for new homes and relocated markers.
Its Go operation data is unchanged; existing authenticated native source and
loader observations replay with byte-identical selected cases and checker
obligations. Only source mappings and their review chain change. Historical
correctness/performance captures retain their original freshness state; the
source review does not re-certify them.

The final archive contains 46 JSON files, 501,966 compressed bytes, and replays
successfully after extraction. SHA-256:

- archive: `88e953aacace4f2428f8c8f02131d214ae574327c0c3b6ef23a2bf7fe46815e2`
- leaf provenance: `e010b61328f41875187a7715007d661f0bd62c80b57532adaa35837b448aa482`
- config provenance: `40795b67f528224b9bc1a987bbbe828ae518f27c1bd4cb0481369570d57b9c5c`

Two preparation tests previously depended on live missing/different leaf rows.
They now inject those states explicitly, so completion does not disable their
negative checks. Port annotations on types/variables retain source references;
generated stringers use file-level markers plus exact operation comments,
matching the tracker's generated-source model.

Final checks: **681 Python tests and 1,210 subtests pass**, with one existing
skip. E4 records **76,001 probes, zero failures**.

### F1b aliasing audit and ownership correction — 2026-09-21

The [completed audit](PHASE1-aliasing-audit.md) restores all nine compiler-option
fields to owned containers/scalars and removes `SharedValue`. The owner approved
the clone-isolation divergence; the original native observation remains intact.
The broad sharing change had also broken `${configDir}` substitution: it changed
the original through a clone, whereas Go copies the affected containers first.
Regression tests now cover all nine fields and substitution isolation.

The remaining `SharedSlice` users are the explicitly alias-observing generic
helpers and multimap, their adapters, and tests. No production Rust compiler
consumer uses them. The audit accounts for all fourteen slice-register cases
and three multimap cases. Identity is used by real Go checker callers;
mutation after retaining a multimap view is also independently observed. These
contracts retain their existing sharing. The Go growth model remains confined
to this opt-in compatibility API because two native traces observe its detach
boundary through values. It no longer affects options or module resolution.
Unused write-lock callback APIs were removed. No blanket aliasing waiver was
introduced.

Fresh full-family captures report **229 exact leaf matches / one approved
difference**, with no missing, failed, unavailable or unrun cases. Only
`leaves/options/clone-shares-pointer-backed-fields` changed its Rust observation;
every other leaf observation is identical to the prior completion capture.
The raw comparator still reports `different` and rejects `--require-parity`.
The no-mutation control still agrees. Config remains **176 match / 299 missing /
9 different**, with its entire Rust observation document unchanged.

The archive is `data/phase1/captures/f1b-aliasing-audit.tar.gz`: 46 JSON files,
507,451 compressed bytes, excluding binaries/build trees. Reproduction uses the
same family capture/compare commands above, without `--require-parity` for the
leaf family. SHA-256:

- archive: `8cc1d88c85dadb40ddf7d06da9c8d6a0f9b09a4f580f3891f4c664def63d8fe8`
- leaf provenance: `90b5593770aab7db83d352a390543351c7077f49d42f5bf045e163c3ff9c8f83`
- config provenance: `8c20cf1f1b65ea294d69f3bbafdaf310bafdb832399c9d9e339a47967ebb9ba1`

Validation: 37 core/options/module Rust tests; compiler and both driver builds;
clippy on those packages and dependencies, all targets/features with warnings
denied; fmt; 163 Phase 1 Python tests plus 30 subtests. The operation inventory
moves 35 Rust mapping anchors only. Authenticated native source/loader replay
preserves byte-identical selected cases and checker obligations. Historical
correctness/performance captures are not re-certified; no benchmark or full
checker corpus was run.

### F1b compatibility feature boundary — 2026-09-21

`tsr_core/go-slice-compat` is disabled by default. It gates `SharedSlice`, all
ten shared-slice helpers, and the whole `MultiMap` implementation. The leaf
harness enables it explicitly; ordinary borrowed helpers and the options-clone
isolation test remain available in the default build. The API behavior and the
approved options exception are unchanged.

CI now checks production packages separately with
`python3 scripts/check_production_features.py`. All 33 packages under `crates/`
compile with their default features, and the guard verifies that the actual
`tsr_core` artifact does not enable compatibility. Two real miniature-workspace
tests prove that a harness's feature unification cannot hide accidental API use,
and that a production dependency enabling the feature is rejected even when its
code compiles.

Fresh leaf and config captures are byte-identical to the aliasing audit's Rust
observation documents: **229 match / one approved difference** for leaves and
**176 match / 299 missing / 9 different** for config. No expectations or
comparison rules changed. Archive `data/phase1/captures/f1b-feature-guard.tar.gz`
contains 46 JSON files (507,423 bytes) and replays after extraction. SHA-256:

- archive: `5235a094619f69b7444ac6ab08d62919c75481d9836af3c1e8f3c7131dced991`
- leaf provenance: `0ca7bf2f4ebc8c4d50040da80e39f541381c6035c4ee8663eb8580ac564bdb92`
- config provenance: `305abfb6cf0f5e4669871edc423b2b2ca1cf1f8f056c8b9de783fc9b867d89cb`

Validation: 21 core tests with default features and 23 with compatibility;
targeted core/leaf clippy with all targets/features and warnings denied; fmt;
163 Phase 1 Python tests and the two production-feature tests; publication
policy and Phase 1 inventory checks. The S07 mapping update moves 14 Rust
anchors only; replay of authenticated native observations preserves identical
selected cases and checker obligations. Historical benchmark and correctness
evidence is not re-certified.

### F2b first batch: paths, matching, glob, symlinks and the in-memory filesystems — 2026-09-21

F2b was authorized after the F1b merge and is **in progress**. This batch
implements six of the eleven filesystem groups against the F2a checks. The
filesystem family moves from **34 match / 317 missing / 7 different** to
**122 match / 232 missing / 4 different**, with one case still unavailable on
this host. No native observation or expectation changed, and no previously
matching row changed.

| Group | Cases | Before | Now |
| --- | ---: | --- | --- |
| tspath | 46 | 13 match, 30 missing, 3 different | **46 match** |
| vfsmatch | 22 | 20 match, 1 missing, 1 different | **22 match** |
| glob | 10 | 10 missing | **9 match**, 1 different |
| symlinks | 11 | 11 missing | **11 match** |
| vfstest | 19 | 17 missing, 2 different | **17 match**, 2 different |
| iovfs | 16 | 16 missing | **16 match** |
| cachedvfs | 32 | unchanged | 1 match, 30 missing, 1 different |
| wrapvfs, vfsmock, osvfs | 61 | unchanged | 60 missing, 1 unavailable |
| matchFiles | 142 | unchanged | 142 missing |

Every group that reached a full match on its first run was mutation checked:
one production function was broken, the comparison reported `different`, and
the function was restored.

#### Implementation and representation decisions

- **`tsr_tspath`** gains the root predicates and separator family, the unreduced
  splitter and the reducer, the cheap normaliser, the comparer wrappers, a typed
  `Path`, callback-shaped ancestor walks, common parents and the extension
  tables. Two defects the comparison found are fixed: `base_name` measured the
  root before normalising separators, and `to_path` made every input absolute
  where the pin only normalises a rooted disk path.
- **`vfsmatch`**: `getBasePaths` sorts include base paths with the plain string
  comparer, not a path comparison, so `/out/./z` sorts before `/out/z` and
  containment keeps only the first. `Usage` carries the open `int8` name table.
- **`tsr_glob`** is a new crate for the language-server glob grammar. It keeps the
  pin's behaviours: a negated range stores its flag and ignores it, a star cannot
  meet the separator that follows it, and a separator run that reaches the end
  of the input panics. It joins the package policy, release order and BSD notice.
- **`tsr_module::symlinks`** is the known-symlinks cache. The ledger places it in
  `tsr_core`, which cannot depend on `tsr_tspath`, so it lives in `tsr_module`.
  The compiler's private copy had no file half and returned before recording
  one; it is deleted in favour of the shared cache. The checker's private path
  helpers now call `tsr_tspath`.
- **`tsr_vfs::iofs`** ports the parts of Go's `io/fs` the adapters are written
  against, and `fstest.MapFS`. **`tsr_vfs::vfstest`** ports the test filesystem,
  and **`tsr_vfs::iovfs`** the root dispatcher and the `io/fs` adapter. Where Go
  type-asserts a backing for optional resolve and write capabilities, a backing
  states them up front.
- `FileInfo` gains the name, modification time and mode the pinned `Stat`
  reports, and `change_times` carries both instants.

#### Differences left visible, for owner review

1. `filesystem/glob/match-group-branch-buffer`, one row: the probe builds an
   element of a foreign Go type to reach the matcher's defensive panic. A closed
   Rust enum cannot represent that element, so the row reports
   `unrepresentable_element`.
2. `filesystem/vfstest/from-map-rejects-malformed-maps`, one row: an input whose
   Go value has a foreign dynamic type. A typed input enum cannot carry it, so
   the row reports `unrepresentable_input`.
3. `filesystem/vfstest/snapshot-mutation-leak-control`: the pin assigns a
   modification time through its stored pointer, so a held entry changes under
   its holder. Stored entries are immutable here, consistent with the
   compiler-options ownership decision, and the leak is not reproduced.
4. `filesystem/cachedvfs/root-length-rejects-non-absolute` is unchanged from F2a
   and belongs to the cached group, which is not implemented yet.

None of these is waived. The comparator reports them and `--require-parity`
rejects them.

#### Remaining F2b work

The cached, wrapping, tracking and mock filesystems sit directly on the adapter
and test filesystem delivered here. The live operating-system filesystem is its
own group. The 142 `config/matchFiles` outputs need the `json` configuration
entry point, `ParsedCommandLine::wildcard_directories` and the carried test
renderer, which overlaps F3b.

The S07 operation mapping is regenerated: the new port markers move Rust
anchors and flip 38 functions from `no_source_marker` to
`source_marker_present`. That goes beyond the standing mapping-only approval, so
the owner approved the subset re-freeze on 2026-09-21. `subset.json` and
`checker-obligations.json` are byte-identical; only the rule's operation matrix
digest changes (finding `PHASE1-F2b-2026-09-21-first-batch`).

The archive is `data/phase1/captures/f2b-first-batch.tar.gz`: the 47 JSON
request, overlay, observation and provenance files of the filesystem family
(1,368,570 bytes), no binaries or exported upstream tree. Its capture
provenance SHA-256 is
`d786b1d0546c5234498f57abc6a388be2878a1b07459a9ee3ecc543080190cac`, and
comparing the extracted archive reproduces 122 match, 232 not_implemented,
4 different and 1 native_unavailable.

#### PR #47 review corrections — 2026-09-21

The review fixed the new timestamp's zero semantics: Go's zero time is the
year-one instant, not an absent value. Parsing that instant, comparison,
clock advancement and conversion to `SystemTime` now agree. `Time::unix`
returns its seconds/nanoseconds pair directly, including for zero, and
`from_unix` normalizes excess nanoseconds.

RFC3339 parsing now rejects malformed separators, invalid dates and out-of-range
time fields. It retains the pinned parser's permissive cases (one-digit hours,
comma fractions, truncation beyond nanoseconds and inclusive 24/60 offset
components). A Go 1.27.1 probe supplied the timestamp expectations; all three
new timestamp tests failed before the fix. Broken-symlink classification also
unwraps path errors, as the pin's `errors.AsType` does, with a regression for
a nested message/path wrapper.

The focused VFS suite passes (five unit tests and three integration tests),
and VFS clippy with warnings denied is clean. A fresh filesystem capture was
recorded and archived as `data/phase1/captures/f2b-review.tar.gz` (47 JSON
files, 1,359,496 compressed bytes; provenance SHA-256
`ac92576946f9145db306be11225d45dded3648bb48bc0631197518fdd9a8a001`).
Every native and Rust observation is identical to the first-batch archive;
the counts and four unwaived differences above are unchanged. The original
archive is preserved. Tracker views are regenerated for the reviewed sources.
