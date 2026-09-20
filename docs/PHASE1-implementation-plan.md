# Phase 1 implementation plan: complete the foundations

Status: proposed for review; implementation has not started.
Baseline: merged main `03a55ac`, after the S12 closure, 2026-09-20.
Upstream authority: `1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Objective and starting point

Complete the foundation contracts needed by the full checker, emit and program
phases. Preserve the implementations delivered by Phase 0, identify the missing
operations and exercise them against the pinned Go implementation. Do not
restart the parser, binder, ownership system or test-host transport.

[S12](S12-closure-plan.md) is closed. Its recent September 19 captures remain the
accepted Phase 0 starting evidence across the crate rename. This plan does not
reopen S12 or require a new benchmark batch before work can begin. Phase 1 adds
coverage and implementation where its own requirements exceed those captures.

The implementation follows the [Rust/Go porting guide](CODEX-RUST-GUIDELINES.md),
the [ownership](design/ownership.md) and [text](design/text.md) contracts, and the
accepted [ADRs](adr/README.md). The pin remains fixed during this phase unless a
separate owner decision changes it.

### What already exists

The evidence below is indexed in [S12-evidence.md](S12-evidence.md). These are
results to preserve, not work to implement again.

| Area | Delivered starting point | What that does not establish |
| --- | --- | --- |
| Scanner, parser/JSDoc, encoder | All 12,829 primary rows: 12,721 physical compiler/conformance files and 108 libraries; exact encoder observations and supplemental decoder/depth/helper checks | Complete behavior of every public AST utility on constructed graphs, or whole-program syntactic diagnostic baselines |
| Binder | Full primary corpus graph comparisons, supplemental resolver/helper checks and both VS Code workload graph modes | Every callable helper/cache branch or every program configuration outside those requests |
| Program/config/module slice | Loader, config interpretation and option verification across 10,728 frozen variants, plus direct package/path/resolution probes | The separate 309 config/options baseline outputs or a complete command-line parser |
| Checker | Frozen E2 acceptance parity and working printer/node-builder paths | Full checker semantics; these remain Phase 2, with complete emit in Phase 3 |
| Ownership and text | E3/E4 contracts, including release behavior, Miri/ASan, byte-preserving strings and retained owners | Actual server integration, watch lifecycles or arbitrary bytes over the strict-Unicode S11 filesystem wire |
| Generators and assets | Schema-driven AST/diagnostic/API generation, untouched client comparison, packaged libraries | Missing localization generation and every generated/source utility contract |
| Test-host transport | S11's 89 cases: 31 filesystem, five actual Go mapper byte streams, 47 subprocess contracts and six Session contracts | Blocked-worker synchronous bridging, parse-cache injection or semantic fourslash |

### Gaps confirmed by source inspection

1. The pinned reference tree has **309** config/options outputs: **142** under
   `config/matchFiles`, **87** under `config/tsconfigParsing`, **53** under
   `tsoptions/commandLineParsing/parseCommandLine`, and **27** under
   `tsoptions/commandLineParsing/parseBuildOptions`. These are baseline files,
   not necessarily one test invocation each. Freeze their invocation/output
   relationship before measuring the denominator.
2. `tsr_tsoptions` already has substantial config parsing and matching. The
   command-line/build-option entry points are absent from its public surface;
   their Go functions also remain unmapped. `ParseBuildCommandLine` belongs here
   as argument interpretation; implementing the build driver does not.
3. `tsr_diagnostics` supplies generated identities and English messages; its
   crate documentation explicitly leaves formatting and locale negotiation to
   later slices. The Go diagnostic implementation includes locale matching,
   translated tables, formatting, unknown-message failures and an argument
   replacement policy. A generated message table alone does not complete it.
4. No dedicated `tsr_json`, `tsr_locale`, `tsr_glob` or `tsr_collections` crate
   exists. This is a location/coverage observation, not proof that every
   operation is missing: inspect current consumers before adding an abstraction.
5. `tsr_vfs::FileSystem` defaults mutation operations to Unsupported; its timestamp
   method does not take Go's access/modification times, and it has no `WalkDir`
   contract. `ScopedOsFs` provides a scoped snapshot, not the complete live OS
   filesystem. Cache/wrapper/I/O filesystem behavior needs a separate inventory.
6. Several planned crate boundaries differ from working code: package JSON is
   in `tsr_module::package_json`/`package_maps`; file matching is in
   `tsr_tsoptions::glob`; string helpers are in `tsr_jsstring`; evaluator behavior
   is partly in `tsr_checker::template`. Preserve working code and make these
   boundaries explicit instead of treating absent crate directories as missing
   implementations.

`PORTS.toml` and `status/unmapped-functions.json` are audit inputs, not a task
count. An unmapped Go method may already be represented by a Rust field, helper
or standard-library operation. Conversely, a mapped method can still lack a
behavioral witness. The first checkpoint distinguishes those cases.

## 2. Scope and boundaries

In scope: core/collections; paths, strings and numbers; JSON and locale;
diagnostic messages/localization and library access; both glob dialects; package
JSON; the VFS family; AST utilities/owners/lazy storage, scanner and parser;
file-owned binding, syntax navigation and the reusable constant evaluator;
module resolution and options; the compiler runner's parse/bind/syntactic
operations; generator drift and the existing transport contracts.

Test infrastructure is included only as needed to run those contracts. The
ledger currently assigns some editor, emit and API test helpers to Phase 1.
Classify their dependencies explicitly; that coarse label does not pull the
language service, emitter or API server into this phase. Do not silently mark
them implemented or delete their later obligations.

Keep these boundaries:

- Full checker diagnostics, `.types`/`.symbols`, inference, JSX/decorator
  semantics and checker services: Phase 2. Existing working slices remain.
- JS/declaration emit, source maps and transforms: Phase 3.
- CLI execution, project-reference build scheduling, watch, tracing, process
  services and native watcher implementations: Phase 4. Parse their relevant
  options here without pretending to execute them.
- LSP/fourslash semantics, production mapper host, synchronous blocked-worker
  callback bridge, parse-cache injection and project-system option completion:
  Phase 5, as [ADR 0019](adr/0019-test-host-protocol-and-transport-contract-tests.md)
  already specifies.
- API client cut-over: Phase 6. Full performance/release, browser and embedding
  acceptance: Phase 7 under [the approved budgets](S12-acceptance.md).

No new performance target or threshold change is proposed. No broad storage
redesign, crates-only reorganization, dependency upgrade or upstream pin bump
is part of this plan.

## 3. Delivery sequence

Checkpoints describe reviewable implementation batches, not calendar estimates.
Each can take several focused PRs. Finish and review the behavior before
starting an expensive final capture.

| Checkpoint | Work | Depends on | Completion evidence |
| --- | --- | --- | --- |
| F0 | Freeze remaining scope and connect existing evidence to Phase 1 | S12 | Complete operation/test inventory; runnable baseline adapters for a small representative set; missing outcomes stay visible |
| F1 | Complete core, collections, text/number, JSON, locale and diagnostic leaves | F0 | Native differential groups and applicable upstream unit cases; localized assets generated without drift |
| F2 | Complete paths, filesystem contracts and matching | F0; relevant F1 leaves | VFS operation traces and all 142 matching baseline outputs |
| F3 | Complete package/config/command-line/module behavior | F1, F2 | All 309 config/options outputs, package/resolution/cache probes, ordered program closure checks |
| F4 | Close syntax, AST, binder, navigation and evaluator gaps | F0; required F1–F3 operations | Exact complete parse/bind inventory, syntax-only diagnostics, utility and ownership counterexamples |
| F5 | Confirm transport/generation integration and close Phase 1 | F1–F4 | Complete Phase 1 gate report and scoped regression checks, preserving S11 boundaries |

F4's inventory and narrow fixes need not wait for unrelated locale/VFS work.
Phase 2 work can start once the AST/binder/resolution/options contracts it uses
are ready; locale and unrelated utility work should not serialize that critical
path. That readiness is recorded per contract and does not mark Phase 1 done.

### F0 — make the remaining work concrete

Create `data/phase1/scope.json` with one identity per pinned Go operation or
explicit operation family. Each entry records its source, Rust home, disposition
(already covered, implemented but untested, missing, equivalent Rust mechanism,
or owned by a later phase), direct dependencies, test identities and acceptance
output. Generated methods, Go runtime machinery and observable source behavior
are separate categories. Later-phase entries need a named destination and
reason; behavioral differences use the existing divergence policy.

Freeze `data/phase1/cases.json` and the config baseline index with pin, exact
file hashes, case/variant identities, host settings and ordered requests. Reuse
`data/s06`, `data/s07` and `data/s11` inventories rather than copying the entire
corpus into another archive. Record the association between the 309 output
files and their native test invocations, including tests with several outputs.

Use a small `scripts/phase1.py` dispatcher with separate family modules and
access-only native overlays under `tools/phase1/`. Reuse existing subprocess,
strict-JSON, build-artifact and provenance helpers; do not build another general
benchmark or schema framework. Default comparison is read-only; an explicit
prepare/freeze operation changes expected inputs. Native setup and adapters
must be validated before their output can be accepted as a language result.

Start the executable adapter with representative native config parsing,
command-line, file-matching and JSON/locale requests. Include one known absent
Rust operation so the report demonstrates an honest pending/failure outcome.
Then enumerate the remaining native test tables without relying on a fragile
regular-expression transcription of Go source.

**Exit:** every in-scope operation has a disposition and test owner; all 309
baseline files have mapped producers; the pilot compares real Go/Rust output;
missing/duplicate/reordered requests and harness failures are rejected. F0
should end with a ranked implementation queue, not weeks of instrumentation.

### F1 — complete foundation leaves

Audit existing implementations before porting:

- **Core and collections:** ordered maps/sets, copy-on-write scope behavior,
  nil/empty distinctions, traversal and pattern helpers, tri-state/default
  option semantics and concurrent-map operations used by consumers. Preserve
  insertion order, deletion/reinsertion behavior, iterator mutation rules and
  sharing across scopes. A standard Rust collection may implement an operation
  if its observable contract matches. Do not reproduce Go GC/arena internals
  where the existing Rust ownership system already supplies the contract.
- **Strings, paths' text helpers and numbers:** fill missing comparer, identifier
  and conversion operations around the tested S04/S05/S07 implementations.
  Separate Go Unicode simple folding, JavaScript casing and byte comparison.
  Include malformed bytes, lone surrogates, negative zero, NaN, infinities,
  overflow/narrowing and failure behavior. Reuse pinned tables and number code;
  a crate name is not a reason to duplicate them.
- **JSON:** implement the declared caller-visible `internal/json` contract,
  including object order, number tokens, duplicate-name policy, streaming/error
  boundaries and formatting. Inspect the pin's default
  `AllowInvalidUTF8(true)` marshal option and distinguish it from unmarshal
  behavior. `serde_json` alone is not an assertion of Go equivalence. Preserve
  the existing order-sensitive config/package readers and test replacement
  behavior only where upstream explicitly performs it. Do not postpone an
  observable package operation merely because E2 does not call it. Go reflection
  and interface machinery may have a different Rust representation, documented
  and exercised through the corresponding caller contract.
- **Locale and diagnostics:** port locale parse/canonicalization, default versus
  explicitly supplied locale, fallback matching and localization-cache behavior
  against `golang.org/x/text/language` at the pinned dependency version. Freeze
  discriminating native cases before selecting a Rust implementation/dependency.
  Generate the 13 translation tables from the pin; preserve message identities,
  categories/flags, placeholder errors and Go's invalid-byte argument repair.
  Extend the existing generator rather than hand-maintaining translations.
- **Libraries:** retain the packaged library assets and generation checks;
  complete bundled path/wrapper/read behavior and test it outside the checkout.

Prefer `tsr_core`/`tsr_jsstring`/`tsr_jsnum` and existing consumer modules where
those homes already work. New `tsr_collections`, `tsr_json`, `tsr_locale` modules
or crates must have a real public contract and dependency reason. Update the
crate map and provenance to actual homes; avoid extracting package JSON or
string utilities solely to satisfy the original directory sketch. Any new
published crate follows the existing packaged-asset/dependency checks.

**Exit:** the frozen leaf groups pass against Go, including errors and cache
behavior; generated locale/default messages and libraries have independent
source authority; imports remain acyclic; affected consumers still compile and
retain their prior behavior.

### F2 — filesystems, paths and the two kinds of matching

Keep immutable program snapshots separate from live mutable filesystems.
Complete the required VFS operations with explicit capabilities: read/stat,
entries, realpath, walk/skip callbacks, write/append/remove and access/modification
times. A writable host must not mutate the snapshot retained by a parsed file
or program. Carry mutation through a live host/builder and publish a new snapshot;
do not make `MemorySnapshot` writable to satisfy the interface.

Implement the missing native OS, I/O-backed, cached, tracking and wrapping
adapters needed by the frozen scope. Audit the mock/test filesystem separately
from OS behavior. Test missing versus denied operations, directory/file/link
classification, deterministic entry order where specified, symlink metadata,
casing, path normalization, cache enable/disable/clear and stale-negative-cache
behavior. Keep all OS mutation tests under their own temporary roots.

The existing `ScopedOsFs` and test-host memory filesystem remain reusable.
`Chtimes` must carry the two actual timestamps; Go `WalkDir`'s callback/skip/error
contract must not be approximated with a flat directory list. Platform-specific
behavior is measured on its supported native host. The paused CI runners are
not silently claimed as tested or re-enabled by this plan.

Distinguish **configuration file matching** (`vfs/vfsmatch`, currently
`tsr_tsoptions::glob`) from **the LSP/test glob grammar** (`internal/glob`). They
are not interchangeable. The latter's Go source itself calls out its limited
scope: port the pin's behavior without claiming it is a verified implementation
of every external glob convention. Exercise braces/ranges/slashes, malformed
patterns, case policy, Unicode and root-relative matching independently.

Complete path operations around the existing implementation: drive/UNC/URL
roots, `./` and `.\\`, casing/canonicalization, extensions, ignored/untitled
paths and symlink caches. Test callable operations directly even if the compiler
corpus happens never to take them.

**Exit:** all 142 matching reference outputs pass; VFS traces preserve return
values, errors, state and callback order; snapshot isolation and affected owner
lifetimes pass. Known Go nontermination (such as a demonstrated symlink cycle)
uses an isolated watchdog and a named policy, not a hanging test or invented
successful result. A newly proposed behavioral divergence needs owner review.

### F3 — config/options, package JSON and module resolution

Complete the **309-output suite**, with the original pinned expected bytes and
native test structure as authority:

1. File matching: 142 outputs from F2.
2. Config parsing: 87 outputs, including JSONC syntax/errors, inheritance,
   ordered merges, path substitution, files/include/exclude, references,
   option validation and mapper configuration parsing.
3. Ordinary command-line parsing: 53 outputs, including aliases, repeated
   options, booleans/list values, unknown/deprecated options, response-file
   reads and errors, and ordering.
4. Build-option parsing: 27 outputs, including mode differences and the
   diagnostic boundary. No build/watch execution is introduced.

Some native config tests render colored diagnostic context through
`diagnosticwriter`. Port or reuse the exact formatting slice needed by those
outputs, including offsets/newlines/escaping, and account for that Phase 4
package dependency explicitly. Comparing only codes or parsed JSON is useful
for triage but does not satisfy a byte-baseline gate. Do not alter native
expected files to conceal a Rust formatting difference.

Preserve object member/source order in the requests, particularly `paths` and
raw config. Freeze the actual bytes the children load; a sorted serializer must
not change the tested program. Native missing/disabled outputs and internal
harness errors are distinct states, and every expected output must be observed.

Complete package JSON and module resolution from their working S07 homes:
exports/imports conditions and order, invalid/missing/null fields, `typesVersions`,
ESM/CJS usage modes, extension handling, type references, root directories,
package self-reference, symlinks/casing and diagnostics/traces. Include cache
hits, misses, negative entries, redirects and option/mode/snapshot separation.
An unresolved module and an unsupported algorithm are different outcomes.
Content-mapper config parsing belongs here; executing plugins remains Phase 5.

Extend the existing direct Go overlays and program probes before introducing a
parallel resolver adapter. Compare ordered file/include-reason graphs and
option diagnostics outside the E2 acceptance subset where the native operation
supports them. Owner-approved E2 selection decisions stay unchanged and do not
silently shrink this phase's foundation tests.

**Exit:** all 309 outputs match, direct package/resolution/cache tests pass, and
supported program-loading operations preserve ordered closure and diagnostics.
No pending Phase 1 branch is represented by a successful empty result.

### F4 — complete syntax, binding and reusable syntax services

Reuse the direct parser and binder corpus, including every virtual unit and
effective configuration. Phase 1 requires **1.0**, not E1's original 0.999
minimum. Keep the 12,721 physical sources and 108 library rows distinct from
supplemental utility tests. A physical source passes only when all its declared
requests pass; no compiler/E2 selection guard may drop a parser request.

Compare AST/encoder output, parent structure, parser diagnostics and binder
symbol/flow graphs with their existing identity-aware comparators. Keep the
existing narrow, demonstrated Go nondeterminism qualifications explicit. Do
not generalize them into unordered comparison or drop diagnostic arguments.

Add a **syntax-only program observation** over the complete declared compiler
input/variant inventory. Call pinned `Program.GetSyntacticDiagnostics`, compare
structured results and the corresponding syntax-only diagnostic rendering, and
call the Rust production `Program::syntactic_diagnostics` path. Exercise JS
syntax, option-dependent diagnostics and `@ts-check`/`@ts-nocheck` behavior.
Full `.errors.txt` files can contain semantic and declaration diagnostics; do
not compare those entire files to syntax-only output and call the difference a
parser bug. Record the phase and native selection status per variant. Mapped
source diagnostic filtering remains with the actual mapper integration in
Phase 5; retain explicit cases for that boundary.

Audit the remaining AST/scanner/parser/binder function inventory against actual
callers. Prioritize missing behavior, not mapping percentages. Constructed-tree
utility checks cover lazy JSDoc/token creation, child/list order and ranges,
clone/update identity, source-text access, original-node links and bundle
retention. Preserve exclusive bind-before-publication, compact payloads and
branded local readers; no fallback to per-node owned clones or general overlays.

Complete `astnav` against its native token-navigation tests and baseline family,
including zero-width/missing nodes, comments/trivia and token-cache identity.
Factor the reusable evaluator only where its shared callback contract requires
it: values, string-ness, cross-file/external-reference flags, short-circuit and
unknown results must match Go. Keep checker symbol resolution in the checker;
a standalone evaluator must not depend on or duplicate a second checker.

**Exit:** every parse/bind primary request matches; complete declared syntax-only
requests match; public utility, navigation and evaluation families pass; no
unclassified operation is left in the Phase 1 inventory. Run affected ownership,
release/stack and malformed-byte witnesses where these changes touch them.

### F5 — integration, generation and closure

Review Phase 1 changes against S11's existing transport contracts. Reuse its
endpoint, case manifests and actual five Go mapper-stream recordings. Extend
fixtures only for newly exercised foundation behavior. A transport-facing VFS
change must retain null/delegate versus missing, casing/symlinks, outstanding
callback cancellation/progress, server-owned option completion and raw stream
bytes/lifecycle. Do not add a wire configuration callback or method-proxy mapper.

ADR 0019's strict-Unicode filesystem transport remains an explicit boundary;
adding a byte-capable local JSON/VFS helper does not implicitly change that wire.
No blocked-worker bridge, parse-cache injection, production mapper host or
semantic fourslash is claimed here. Port the carried fourslash transport patch
when its real Phase 5 consumer exists, rather than inventing a second test host.

Run generator drift/untouched-client equality checks after the final relevant
schema/emitter/localization changes. Keep generator output and package assets
reproducible from the pin, including an installed-consumer check when assets or
crate dependencies change.

Publish the Phase 1 operation/coverage report and accepted boundaries, make the
new sprint checks reproducible, and update `PORTS.toml` based on actual behavior
and implementation homes. Preserve existing measurements and failures.

**Exit:** every Phase 1 criterion below passes or has its already-approved,
explicitly scoped qualification. New semantic deviations cannot be waived by
editing a count or silently broadening the S12 reuse decision.

## 4. Acceptance and evidence design

| Required claim | Evidence and denominator | Existing asset to reuse |
| --- | --- | --- |
| Foundation operations complete | Every required operation/test in the frozen Phase 1 scope has a passing observation or justified equivalent Rust mechanism; zero unclassified entries | Source/provenance ledger and S04/S05/S07 helper probes |
| Full config/options parity | Exactly 309 reference outputs, 142 + 87 + 53 + 27; all outputs and associated native cases observed | Native `tsoptions` tests and S07 config/package/resolver adapters |
| Full parsing | 12,721 physical cases plus 108 libraries, every expanded unit/configuration; exact comparison, parity 1 | `e1` inventory and adapters |
| Binding | Every primary binder graph, plus missing helper/cache branches identified at F0 | `binder` inventory and structural/identity checks |
| Program syntactic diagnostics | Complete frozen syntax-only variant/phase schedule, structured values and rendered output | Production compiler diagnostics and existing corpus preprocessing |
| Filesystems/navigation/evaluator | Exact operation traces and relevant native tests/baselines; all failure and lifecycle branches in the frozen contract | S07 VFS/path probes, S11 filesystem traces and native astnav tests |
| Generation | No generated drift; untouched client equality; complete locale/library assets | `gen` and package-asset checks |
| Transport | All existing 89 S11 cases plus explicitly added contract cases; no missing/ignored rows | `testhost` producer and ADR 0019 |
| Safety and quality | Targeted debug/release tests, appropriate ownership instrumentation, MSRV, formatting/lints/dependency policy | Existing CI and scoped E3 suites |

Register new producers only when their adapters exist. Proposed names are
`foundations` (family groups), `config` (309 outputs), and `syntax` (program
syntactic observations); extend the existing `e1`, `binder`, `gen`, `testhost`
contracts where appropriate. Avoid a new producer for every helper file. These
names are a plan, not commands currently promised to work.

At F0, add sprint definitions for this sequence with exact metric consumers.
Do not make another experiment pass accidentally through a bare reused boolean.
In particular, Phase 1 parse parity 1.0 is its own check and must not silently
change the accepted Phase 0 E1 threshold. A slice boolean never certifies the
entire foundation package. Complete the per-case report before aggregating it.

Reuse valid native observations when their pin, request schedule and observation
contract are unchanged. Compile executable adapters once per coherent batch.
Preflight inventory, source/request hashes, toolchains and protocol before full
runs; capture failures retain the named case and diagnostic. Checkpoints record
pass/different/failed/unavailable counts without representing partial coverage
as full acceptance. Stored archives have one declared purpose; do not commit
another whole corpus or binary tree for each iteration.

The accepted Phase 0 evidence remains the starting point. After actual Phase 1
behavior changes, run the affected differential/regression work rather than
refreshing every unrelated producer or benchmark. At integration, perform the
complete **Phase 1 correctness** suite once on the finished sources; reuse
unchanged capture inputs through their normal validators. An E2 bounded recheck
is warranted for shared checker inputs, escalating to a full E2 regression run
when the touched surface or observed failures justify it. Neither checkerbench,
relater timings, VS Code timing nor WASM/Node timing is a routine Phase 1
completion requirement. Investigate a concrete performance concern with a
bounded measurement before authorizing a broad performance campaign.

Keep the current two active native CI targets (macOS ARM64 and Linux x86-64),
plus the declared MSRV and relevant instrumentation. Do not increase the matrix
or multiply expensive captures as a side effect of this plan. The full four
release targets and later performance acceptance remain recorded obligations.

## 5. Review and stop conditions

Review each PR for semantic deltas, ownership and ordinary-path cost. Identify
specific counterexamples: reordered `paths`; cached miss after file creation;
mutation leaking into an old snapshot; malformed bytes repaired too early;
duplicate diagnostics; build options accepted with the wrong mode; lazy token
identity changing after retention; an Unsupported result counted as unresolved.

Stop for owner input only when a real decision is needed: a new baseline
behavior divergence, a change to an accepted transport/ownership contract, a
new required platform/dependency policy decision, or an upstream baseline whose
authority cannot be established. Routine porting, helper placement and fixing a
measured mismatch do not need repeated approval. Report such conflicts with a
minimal reproducer and pinned Go output.

### Plan review performed before implementation

- **Does this repeat Phase 0?** No. Existing corpus/transport/ownership work is
  retained; the dedicated config/command-line, locale/JSON and VFS gaps have
  explicit new witnesses.
- **Is 309 being confused with the 10,728 program variants?** No. The four
  reference-file groups were counted directly, and their native invocation
  mapping is an F0 deliverable. Both sets keep separate claims.
- **Will testing require a full build driver or checker?** Command-line parsing
  stops at options/diagnostics, and the program comparison separates syntactic
  from semantic/declaration phases. Only the diagnostic-rendering dependency
  slice is pulled forward where native baseline bytes require it.
- **Are unmatched ledger entries automatically ports to write?** No. Existing
  behavior and Rust equivalents must be audited before code or crate moves.
- **Are the two glob grammars conflated?** No. Config matching and Go's LSP/test
  glob retain separate contracts and native observations.
- **Could this repeat the S12 rerun cycle?** No. S12 stays closed, the recent
  pre-rename captures remain accepted, and new runs follow actual behavior and
  coverage changes. No benchmark refresh is scheduled by this plan.

The first implementation PR should deliver F0 and the smallest command-line
baseline adapter through a real failing Rust case. It should make the next
missing behavior directly reproducible, then proceed into F1/F2/F3 without
building a large replacement harness before any feature work lands.
