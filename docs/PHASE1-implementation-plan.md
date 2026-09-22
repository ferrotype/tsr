# Phase 1 implementation plan: complete the foundations

Status: in progress; F1b prepared implementation scope complete, 2026-09-21. Remaining steps retain their own gates; see PHASE1-progress.md.
Baseline: merged main `03a55ac`, after the S12 closure, 2026-09-20.
Upstream authority: `1f70213d4922b434345f639b441681e470c7cfc1`.

## Execution order

**`a` = prepare tests and checks. `b` = implement production code.**
F0 is preparation only, so it has no `b` step. The detailed F0–F5 sections below
contain separate `a` and `b` checklists with inputs, tasks and completion rules.
Read the shared execution contract before starting F0.

### Reviewed preparation sequence

1. **F0 — Inventory and setup:** identify missing behavior, freeze case identities,
   and connect the existing test tools.
2. **F1a — Foundation tests:** prepare checks for core/collections, strings/numbers,
   JSON, locale, diagnostics and library access.
3. **F2a — Filesystem tests:** prepare path, filesystem/cache and file-matching
   comparisons, including the 142 matching baselines.
4. **F3a — Config/resolution tests:** prepare the full 309 config/options baseline
   comparisons and package/module-resolution probes.

Reviewed PRs do not erase pending platform-specific preparation checks; retain
their recorded results for the F5a review.

### Authorized early implementation

5. **F1b — Foundation implementation:** implement the missing leaf behavior and
   localization support against F1a's checks.

**Ordering amendment, 2026-09-21:** after reviewing F1a through F3a, the owner
approved pausing the remaining `a` steps to start F1b. F1a supplies the native
contracts, Rust adapters and classified leaf queue; F3a supplies downstream
config/diagnostic observations. Re-record the corrected bundled `WalkDir` gap
identity before changing production. This changes sequencing only: it does not
declare Phase A complete or weaken any coverage, parity or freshness rule.

**Ordering amendment, 2026-09-22:** the owner also authorized F2b and the
F3b config entry point, wildcard-directory and renderer prerequisites for
matchFiles. This does not close F3b or replace the F5a coverage review.

**Ordering amendment, 2026-09-22 (after #48 review):** the owner authorized
merging #48 and proceeding with the remaining F3b implementation. F4a/F5a and
the pending Linux filesystem observations remain separate obligations.

**Ordering amendment, 2026-09-22 (after #51):** the owner authorized F4b on
F4a's branch, then F5a on a new stacked branch in the shared checkout. F1b–F3b
have merged and F4b is in #51. The coverage review below now decides the remaining
attribution and integration work; it does not ask to restart those implementations.
Preparation gaps are still gaps even where production corpus comparisons pass.

### Remaining preparation and coverage review

6. **F4a — Syntax/binder tests:** prepare the missing AST, parser/binder,
   syntax-only diagnostics, navigation and evaluator checks.
7. **F5a — Integration checks:** prepare generator/transport regression checks,
   the final acceptance checklist and a report of what still fails or is missing.

**Review the complete coverage and gap report after F5a.** Tests for remaining
missing behavior stay visibly unmet. F1b must run its affected consumer and
generator checks as it proceeds; future F5a integration tests may expose further
foundation work and are still required.

### Production checkpoints (status at the F5a handoff)

8. **F2b — Filesystem implementation:** merged; retain the named dynamic-type
   differences and Linux-only observations as remaining obligations.
9. **F3b — Config/resolution implementation:** merged; preserve the 309-output
   comparisons and the approved immutable-Parseable qualification.
10. **F4b — Syntax/binder implementation:** implemented in #51; retain the
    full parser/binder/syntax captures and finish exact operation attribution.
11. **F5b — Integration and closure:** fix integration failures, run the required
    correctness checks and close Phase 1.

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

### Complexity and separation of work

This is **medium-to-large production work**, smaller than implementing the
checker but larger than adding tests to completed code. Existing parser/binder
coverage reduces that work substantially. The main uncertainties are missing
leaf contracts and edge behavior, not another AST storage redesign. A reliable
effort estimate needs F0's distinction between missing code and missing coverage.

| Area | Expected difficulty | Reason |
| --- | --- | --- |
| Inventories, baseline mapping and report plumbing | Moderate | Reuse exists, but native test invocations must be mapped correctly and failures must stay distinguishable |
| Command-line/config completion and diagnostic formatting | Moderate to high | Ordered merges, response files, mode-specific validation and exact baseline text interact |
| JSON, Unicode and locale | High semantic risk | Rust libraries do not automatically reproduce the pinned Go byte, number and locale behavior |
| Writable/cached VFS | High integration risk | Mutation, snapshots, cache invalidation, symlinks and platform errors must agree |
| AST/binder/navigation/evaluator tail | Uncertain until inventory | Much already works; an unmapped function is not evidence that its behavior is absent |
| Generation and transport regression work | Mostly integration | Existing implementations and accepted contracts should be reused |

The execution list at the top is the delivery order. F0–F5 name the work areas;
`a` and `b` distinguish preparation from implementation within each area.

### What belongs in an `a` step

This writes test/tooling code, including Python orchestration, access-only Go
adapters and Rust test drivers. It does not implement missing compiler/library
behavior.

- Inventory existing coverage and missing entry points; freeze native inputs,
  expected outputs and required case identities, including the 309 baselines.
- Connect current Rust APIs to the comparisons. If an API does not exist yet,
  record `not_implemented` with the missing operation's identity and expected
  native result. Do not add dummy production APIs or duplicate their algorithms
  inside a test driver to make the harness run.
- Prepare the grouped checks, metrics and failure reports. Exercise a known
  passing case, a controlled mismatch, a missing operation and a harness failure.
  The runner's own tests can pass while feature parity correctly remains unmet.
- Produce a baseline gap report and an implementation ticket per behavior
  family: reproducer, native authority, intended Rust home, dependencies and
  the command that will demonstrate the fix. Reuse valid recorded observations
  and run only the native/test work needed for the missing coverage.

Phase A PRs contain manifests, fixtures, adapters, harness tests and tracker
wiring. Access needed only by tests stays test-only. They do not contain
semantic fixes, production refactors, weakened expected results or ignored
failures. New feature checks remain visibly pending/failing until implemented;
the separate harness-health check must pass. Do not make the existing CI suite
depend on an intentionally incomplete feature gate during preparation.

### What belongs in a `b` step

This contains the actual Rust ports and fixes: command-line parsing,
locale/localized diagnostics, missing JSON/collection operations, writable and
cached filesystem adapters, and whichever resolver/syntax utility gaps the tests establish.

- Implement one coherent family in its production home; add no test-only
  substitute for code that real consumers need.
- Turn that family's prepared failures into exact native matches, then run its
  relevant integration, ownership and quality checks.
- Add newly discovered boundary regressions alongside the fix. Change a frozen
  expectation only when the native observation or request contract was wrong,
  with that correction reviewed explicitly.
- Enable the completed family as a required check and record its passing
  evidence. Preparation alone never completes a production checkpoint.

Use separate preparation and implementation PRs, in the execution order above.
Phase A produces runnable comparisons and the gap report; Phase B implements
against them. Missing APIs can remain explicitly `not_implemented` in Phase A,
so preparation does not require writing the product. Additional boundary tests
discovered during implementation belong with the corresponding Phase B fix.
Keep preparation bounded to these contracts rather than building a general
replacement framework. The initial deliveries were preparation-only; the
ordering amendment above authorizes F1b before the remaining preparation.

Checkpoints describe reviewable implementation batches, not calendar estimates.
Each can take several focused PRs. Finish and review the behavior before
starting an expensive final capture.

The dependencies in this table describe production behavior. Preparing F2a or
F3a does not require F1b/F2b to be implemented: native observations and explicit
missing-operation results make that separation possible.

| Checkpoint | Work | Depends on | Completion evidence |
| --- | --- | --- | --- |
| F0 | Freeze remaining scope and connect existing evidence to Phase 1 | S12 | Complete operation/test inventory; runnable baseline adapters for a small representative set; missing outcomes stay visible |
| F1 | Complete core, collections, text/number, JSON, locale and diagnostic leaves | F0 | Native differential groups and applicable upstream unit cases; localized assets generated without drift |
| F2 | Complete paths, filesystem contracts and matching | F0; relevant F1 leaves | VFS operation traces and all 142 matching baseline outputs |
| F3 | Complete package/config/command-line/module behavior | F1, F2 | All 309 config/options outputs, package/resolution/cache probes, ordered program closure checks |
| F4 | Close syntax, AST, binder, navigation and evaluator gaps | F0; required F1–F3 operations | Exact complete parse/bind inventory, syntax-only diagnostics, utility and ownership counterexamples |
| F5 | Confirm transport/generation integration and close Phase 1 | F1–F4 | Complete Phase 1 gate report and scoped regression checks, preserving S11 boundaries |

F1–F5 completion evidence in this table belongs to the `b` steps. Preparation
retains F0 → F1a → F2a → F3a → F4a → F5a dependencies, with F1b now inserted
after F3a. Each `a` step's inputs and separate readiness criteria are specified
below; starting F1b does not certify the remaining preparation.

Phase 2 work can start once the AST/binder/resolution/options contracts it uses
are ready; locale and unrelated utility work should not serialize that critical
path. That readiness is recorded per contract and does not mark Phase 1 done.

### Execution contract for every step

The numbered list is the order of work. Each step below has required outputs
and a completion checklist. Finish that checklist before reporting the step
complete; a pilot or a proposal to add the remaining fixtures is not completion.
A step may span several PRs. Record progress by completed case/operation IDs,
remaining IDs and concrete blockers, rather than by a percentage guessed from
files edited. Phase A ends at F5a with a coverage review. F1b is a separately
authorized implementation branch, not an extension of a preparation PR.

**Artifacts and ownership.** F0 establishes these conventions and the shared
manifests/dispatcher. Create each family artifact in its own `a` step; do not
build all family adapters or empty placeholder files in F0:

| Path | Content and rule |
| --- | --- |
| `data/phase1/scope.json` | Pinned operation identities, disposition, dependencies, Rust home, evidence/test links and destination phase. Grouping retains the complete member list. |
| `data/phase1/cases.json` | Stable family/case/action IDs, ordered request references, native authority, expected observation contract, host applicability and operation IDs covered. |
| `data/phase1/config-baselines.json` | Exact 309 reference paths/hashes and their native invocation/output mapping. F2a and F3a share this index. |
| `data/phase1/requests/<family>.json` | Only new direct requests; point to existing manifests for reused requests. Represent source bytes and ordered objects without loss. |
| `data/phase1/native/<family>/` | Small new native observations and their provenance when they cannot be referenced in existing data. No second copy of the compiler corpus. |
| `tools/phase1/<family>/` | Access-only Go bridges and Rust observation drivers not already supplied by an existing adapter. Test instrumentation patches are separate from the oracle's production algorithm. |
| `scripts/phase1.py`, `scripts/phase1_*.py` | Thin dispatcher, manifest validation and family comparisons, delegating to existing scripts wherever they already own the contract. |
| `scripts/tests/test_phase1*.py` | Runner, comparison, provenance and failure-path tests; no expensive corpus capture in the script tests. |
| `docs/PHASE1-progress.md` | Per-step completion record, coverage counts, named gaps, exact reproduction commands and implementation queue. This is a results record, not a second plan. |
| `target/phase1/<capture-name>/` | Immutable raw requests/responses, stderr, build/source identities and comparison report for one capture. Never overwrite a capture while comparing it. |

New filenames in this table are planned deliverables, not claims that tools
already exist. Keep existing adapters in their existing homes. The new layer
joins their outputs; it must not become a second implementation of the parser,
resolver, generator or evidence engine.

New Python suites belong in `scripts/tests/`, following that directory's import
setup. `python3 scripts/run_tests.py` is the existing full script-test gate;
`scripts/checks.py::selftest` calls it. It already discovers both `scripts/tests`
and legacy root suites with duplicate identities removed. The focused command
below is for iteration, not a replacement for CI discovery. Extend the discovery
regression to verify the new Phase 1 suites are in the real gate.

**Commands to provide in F0.** Implement this small interface and document its
actual usage in the progress record. Family names are `leaves`, `filesystem`,
`config`, `syntax`, `utilities` and `integration`.

```text
python3 scripts/phase1.py inventory --check
python3 scripts/phase1.py prepare --family FAMILY --output DIRECTORY
python3 scripts/phase1.py freeze --from DIRECTORY
python3 scripts/phase1.py capture --family FAMILY --output DIRECTORY [--case ID ...]
python3 scripts/phase1.py compare --capture DIRECTORY [--require-parity]
python3 scripts/phase1.py report --captures DIRECTORY ... --output FILE
python3 -m unittest discover -s scripts/tests -p 'test_phase1*.py'
```

- `inventory --check` validates committed scope, case coverage, baseline mapping
  and hashes without editing them or building binaries.
- `prepare` exports native inputs and executes observations into a staging
  directory. `freeze` explicitly installs reviewed manifests/observations; it
  never blesses Rust output as expected truth. Preparation may expand the
  reviewed inventory, but capture and comparison cannot change it.
- `capture` preflights before building, writes the exact bytes the children
  consume, and runs selected comparisons through the existing adapters or the
  new narrow drivers. A selection is marked partial and cannot supply a full
  family gate. Reuse authenticated Go observations when their inputs are
  unchanged; changing Rust alone does not require capturing Go again.
- `compare` authenticates and compares saved outputs without running children.
  A valid report with semantic gaps can exit successfully in development mode;
  `--require-parity` fails on any required non-match. Harness/provenance errors
  fail in both modes. Diagnostics and logs stay off producer JSON stdout.
- `report` joins verified reports by case ID and rejects duplicates, incompatible
  contracts and missing rows within a declared selection. In a development
  report, cases outside that selection remain `not_run`; full-family acceptance
  rejects such an incomplete result. It neither runs tests nor manufactures
  passing rows from a prior summary. Separate historical coverage from current
  executable evidence when freshness differs.

Use `scripts/s04_common.py` for strict envelope decoding and existing source/
process helpers where compatible. `scripts/s06_build.py` already exports the
pin; `scripts/s07_config.py` and `tools/s07/config/` already observe config;
`scripts/s07_program_compare.py` already compares program outputs. Reuse their
contracts rather than importing a helper whose serializer or source closure
changes the meaning of a request. In particular,
`scripts/s07_subset.py::json_bytes` preserves `paths`/`config_raw` order but is
not a general ordered-JSON codec: other ordered objects use ordered entry arrays
or raw bytes. Strict JSON applies to the observation envelope; deliberately
invalid/duplicate-key JSON under test travels inside it as bytes.

**Result contract.** Keep execution and semantic agreement separate:

| Result | Meaning | Counts as feature parity? |
| --- | --- | --- |
| `match` | Both sides executed the same scheduled observation and agree, including an expected domain error | Yes |
| `different` | Both executed; values, bytes, identities or ordering differ | No |
| `not_implemented` | The Rust entry point is missing or returns an explicit unsupported boundary | No |
| `native_unavailable` | The native authority did not execute this request; record its guard/reason | No; requires classification, never automatic denominator removal |
| `harness_failed` | Build/protocol/assertion/timeout/unexpected panic/serialization failure prevents comparison | No; no passing aggregate may be emitted |
| `not_run` | The case is outside a bounded selection or has a named prerequisite still missing | No |

Expected native panics/errors are only domain observations when explicitly
specified, isolated and matched by class/payload according to that operation's
contract. A blanket panic-versus-panic match is not sufficient. A missing API
must still have a frozen native request/result, a Rust dispatch entry reporting
`not_implemented`, and a Phase B task; a prose TODO alone is not a prepared test.
At the xtask boundary, required non-matches map to `fail`, not a passing/omitted
case. Infrastructure failures invalidate the capture. Never change the existing
xtask `pass`/`fail`/`skip` protocol to fit this richer development report.

**Validation and run budget.** Every preparation step runs its script tests, a
bounded real native/Rust smoke and a read-only comparison replay. Compile only
its drivers/affected test targets. Use complete native test-table inventories,
but do not repeat already-authenticated full corpus work to prove a dispatcher.
Every Phase B step runs its prepared family suite and affected production
regressions; F5b owns the final complete Phase 1 correctness run. Do not run
benchmarks, checkerbench, relater timings or a new release-platform matrix as
part of these steps. Inspect actual failures before repeating a command.

### F0 — inventory, manifests and executable setup

**Inputs:** `PLAN.md` Phase 1 scope, `PORTS.toml`,
`status/unmapped-functions.json`, `docs/S12-evidence.md`, the S04–S11 case
manifests and the pinned native packages. The worklist is not proof that code is
absent. Check current Rust consumers and test drivers before assigning work.

**Do these in order:**

1. Confirm the initialized submodule matches the stated pin; record the exact
   toolchain requirements from existing manifests. An absent Go/Node runtime
   must give an actionable preflight error, not silently download a different
   toolchain. Reuse the established Go environment, including
   `GOTOOLCHAIN=local`. Do not edit the canonical submodule.
2. Enumerate the Phase 1 source operations and native test tables. For every
   operation, record source path plus symbol identity, current Rust home,
   disposition, callable consumers and dependencies. Use these dispositions:
   `covered`, `implemented_untested`, `missing`, `equivalent_rust`, `later_phase`.
   `covered` needs exact existing case/artifact links; `equivalent_rust` needs
   the replacement mechanism and a behavioral witness. `later_phase` needs the
   destination and reason. Generated/runtime-only mechanisms retain an explicit
   classification and cannot erase an observable operation.
3. Enumerate all baseline files recursively. Freeze 142 matching + 87 config +
   53 command-line + 27 build-option outputs. The matching directory contains
   nested paths; a top-level-only glob misses two outputs. Associate each with
   native test/subtest identity, ordered inputs, host setup, rendering mode and
   expected path/hash. Distinguish one invocation producing several outputs
   from several invocations contributing to a baseline.
4. Obtain requests from native tests or an access-only instrumentation patch to
   their test machinery. Do not parse Go tables with ad hoc regular expressions
   or reconstruct native expectations from Rust. Retain and hash any extraction
   patch, and prove it changes observation only. Record the renderer authority
   per baseline group and implement the shared test-envelope seam described in
   F3. Verify native observation → rendering against the frozen baseline bytes
   before judging Rust. Do not assume all 309 files have a callable Go renderer;
   a missing renderer or request authority is a named F0 blocker, not permission
   to reconstruct expected results from Rust or silently reduce the inventory.
5. Create the manifests and dispatcher above. Freeze the entire inventory now;
   missing observations stay named pending work for F1a–F5a. F0's pilot must
   actually execute representative config, command-line, matching, JSON and
   locale requests, including a known implemented match and an absent Rust API.
   F0 does not need to implement all those APIs or finish every adapter.
6. Define capture provenance: pin; request bytes and ordering; baseline hashes;
   native/Rust adapter sources and transitive build inputs; binary digests;
   toolchain, host/settings; exact case schedule; output/stderr hashes; source
   stability before and after execution. Test recursive source collection,
   including a file under a nested oracle directory and `.cargo/config.toml`.
   Fail early on missing inputs, mismatched inventories or stale native data.
7. Add focused runner tests: duplicate/missing/extra IDs, reordered actions,
   ordered-map serialization, malformed/truncated responses, native harness
   failure versus expected diagnostic, timeout, absent Rust API, tampered raw
   output and a source change during capture. A partial capture must not pass
   the full-inventory check; an empty inventory must not yield parity 1.
8. Add Phase 1 sprint definitions without altering S01–S12 completion or E1's
   threshold. Preparation completion means its exact checklist is met;
   production completion consumes the family metrics specified in section 4.
   Reserve absent metrics as pending. Register a producer only after it can
   validate its complete declared inventory and report honest failures.
9. Publish the initial progress/queue record. For each gap list operation IDs,
   case IDs, native result, Rust result, smallest reproducer, production home,
   dependency and exact command. F1a–F5a fill observations and coverage; they
   must not silently delete difficult entries discovered here.

**Completion checklist:** scope has zero unclassified operations; all 309
outputs have verified invocation mappings; manifests and failure tests pass;
the real pilot has both an observed match and a named missing operation; replay
is read-only; the pending implementation queue is generated from concrete rows.
A pilot alone is insufficient if inventory/mapping work remains incomplete.

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

#### F1a — prepare the foundation tests

**Start from:** F0's leaf roster; existing S04/S05 text/number probes; S07 path,
semver and package helpers; native `internal/core`, `collections`, `stringutil`,
`jsstring`, `jsnum`, `json`, `locale`, `diagnostics`, `bundled` and their tests.
Read the callers of generic Go helpers before choosing their observable Rust
contract. This is coverage preparation, not a new general-purpose standard
library project.

**Required coverage matrix:**

| Group | Requests to freeze | Compare |
| --- | --- | --- |
| Core/collections | Empty/nil; insert, overwrite, delete and reinsert; scope copy/fork and mutation; early-stop iteration; consumer-used concurrent operations | Values, presence, ordered iteration and alias/isolation behavior. For concurrency, assert the specified outcomes, not a scheduler-dependent interleaving. |
| Text/number/semver | Existing byte/casing/number corpus plus uncovered callable operations; malformed UTF-8, lone surrogates, empty/end positions, overflow, NaN/infinities/negative zero; semver parse/range boundaries | Raw result bytes, numeric bit/category distinctions, positions and expected failure classes; never replacement-decoded strings alone. |
| JSON | Ordered members, duplicate names, missing/null/empty, large number tokens, negative zero, malformed bytes/escapes, truncation, streaming boundaries and formatting/options | Raw output and consumed/error positions, tokens/order and domain errors. Marshal and unmarshal get separate cases; no generic JSON normalization of the tested payload. |
| Locale | Every shipped locale plus aliases, case/script/region variants, malformed/unsupported tags, absent versus explicit locale, fallback, repeated and changed requests | Native canonical tag, selected translation/fallback and observable cache behavior under a declared process environment. |
| Diagnostics | Every generated identity; translated coverage/fallback for all 13 tables; zero/multiple/repeated/missing arguments; unknown message; invalid-byte arguments; multiline text | Exact formatted bytes, category/code/flags, fallback choice and expected failure. Formatting work is shared with F3a, not reimplemented in its driver. |
| Bundled libraries | Complete asset index, valid/missing names, wrapper paths and access outside the repository | Asset names/content hashes, errors and checkout-independent access. |

**Tasks:**

1. Map every existing probe to the leaf operation IDs it actually exercises.
   Keep tested S04/S05/S07 requests by reference; add requests only for uncovered
   behavior and the boundary matrix above. Do not relabel a raw-string probe as
   a scanner or locale witness.
2. Export native unit-table inputs through test-only accessors. For operations
   with no native test, add a direct Go probe calling the actual pinned function.
   Retain its declaration/caller reference with the observation. For JSON and
   locale, obtain discriminating native results before proposing a dependency.
3. Add ordered action traces where state matters: collection fork/mutate/read,
   locale selection changed in one process, and repeated formatting. Use fresh
   processes for environment-global defaults when needed. Don't let one case's
   locale or cache leak into the next case accidentally.
4. Connect production Rust APIs already available. For missing JSON, locale or
   formatting operations, emit named `not_implemented` rows and specify the
   intended signature/production home in the queue. An adapter may serialize
   results, render a declared test envelope or expose private test-only state;
   it may not substitute for missing production diagnostic formatting, matching
   or locale behavior. The shared Go baseline renderer below owns the envelope.
5. Prepare a locale-asset manifest mapping the 13 source tables to message keys,
   hashes, fallback obligations and the planned `xtask/src/gen/diagnostics.rs`
   extension. The localization generator itself lands in F1b; F1a tests must
   report its absence rather than accepting hand-maintained translations.
6. Run the leaf comparison over all new small direct requests. Verify negative
   controls for reordered collection/JSON output, wrong fallback locale,
   byte replacement and a missing translation key. Retain one reproduction per
   missing operation, grouping duplicate symptoms by the same production fix.

**Deliver:** leaf cases/requests/native observations, tested Rust dispatch,
locale-asset manifest, per-group results and F1b tasks. Existing coverage links
must resolve to real cases, not just a document claiming the family was done.

**F1a complete when:** every leaf operation is linked to a runnable prepared
case or verified existing witness; all native outputs are available and
attributed; harness checks pass; every Rust non-match is classified. Rust leaf
parity may still fail. Do not port production code to make this step look green.

#### F1b — implement the foundation leaves

1. Work from F1a's queue, beginning with shared collection/text/JSON operations
   required by later leaves. Use existing homes unless F0 established a real
   dependency boundary. Record representation choices for Go nil/order/sharing
   semantics; avoid per-call conversions between two equivalent containers.
2. Complete numeric/text/semver gaps without replacing already-proven byte and
   number code. Run each changed operation's old probes as well as new cases.
3. Implement the caller-visible JSON and locale contracts. Choose a dependency
   only after its behavior passes F1a's native cases; where it differs, provide
   the narrow required adapter rather than weakening the expected results.
4. Extend the generator for localized messages and wire production formatting,
   fallback and errors. Check all 13 tables and English behavior, and keep the
   diagnostic-rendering helper usable by F3 without pulling in CLI execution.
5. Finish library access and package checks where F1a found gaps. Regenerate
   provenance/outputs through the generator; don't manually patch generated
   files. Run changed crates' tests, targeted clippy, formatting, and dependency/
   MSRV checks if manifests or language/library features changed.
6. Run the complete prepared leaf family, plus relevant existing text/number/
   asset regressions. Update scope dispositions and metrics only for passing
   operations; leave any remaining required non-match visible.

**F1b complete when:** all required leaf observations match Go, including error,
ordering and cache behavior; locale/library assets reproduce; dependencies are
acyclic and affected real consumers use the tested implementation.

##### Ordered storage and JSON integration decision — 2026-09-21

The first F1b review identified private vector maps in existing consumers. The
shared collection is production storage, not a second implementation used only
by leaf probes. Apply these dispositions:

| Consumer | Disposition and required witness |
| --- | --- |
| `tsr_tsoptions::file_names_from_specs` | Migrate its three maps now. Preserve literal/wildcard/JSON group order, case-canonical keys and extension priority. Use the direct `config/specs/file-names-*` cases and source-config parsing comparisons. The 142 matchFiles outputs exercise directory matching, not this aggregation by themselves. |
| `ConfigValue::Object` | Migrate to `OrderedMap<JsString, ConfigValue>` with the F1b JSON batch. Keep first insertion position on overwrite and source-order traversal. Preserve the current value-cloning contract; mutable shallow sharing is a separate, still-unapproved difference. Exercise duplicate source properties, config diagnostics, and the F3 config rendering path. |
| `CompilerOptions::paths` / `PathMappings` | Use owned `OrderedMap<JsString, Option<Vec<JsString>>>`. The owner-approved compiler-options clone divergence preserves independent option containers; see [the aliasing audit](PHASE1-aliasing-audit.md). Generic slice-helper identity is a separate contract. Retain the outer `Option`, nil versus allocated-empty target lists, source order and pattern tie-breaking. Test actual config/options serialization and module resolution as well as the five ordered-map JSON leaf cases. Do not convert through a sorted map. |
| `package_json::Object<'a>` | Keep the ordered raw-member sequence. It is a decode stream, not a key/value map: duplicate document fields can partially update prior typed values, while nested typed objects can reject duplicates. Inserting into `OrderedMap` first would erase that information. Existing native witnesses include `duplicate-string-invalid`, `duplicate-deps-merge`, `deps-duplicate-prior` and `exports-order` in `data/s07/packagejson-observations.json`. The post-decode semantic objects currently use `serde_json::Map` with `preserve_order`; audit their migration separately with the JSON decoder, preserving the raw stream and typed-field failure state. |

The planned `tsr_json` layer owns the streaming encode/decode traits and their
ordered-map implementations, and depends on `tsr_core`; core storage must not
depend on the JSON layer. `tsr_tsoptions` implements those traits for its config
values and uses the same writer for actual options/config output. The F1b implementation uses the existing stack-growth dependency and a Rust
token codec checked against the pinned Go JSON module; it introduces no second
JSON value tree or external JSON dependency. The storage and dependency direction
remain as specified. This permits one ordered encoding implementation without
a core/JSON cycle or conversions between private containers at every call.

The JSON batch must reach those real callers before claiming completion. Five
passing collection rows alone do not certify the 309 config/options baselines;
compare the prepared native-renderer bytes through the F3 adapter, with remaining
F3b gaps still explicit. No `equivalent_rust` disposition is added just from a
similar type name or this architectural decision.

Phase 2 follow-up: when implementing generic qualified-name serialization
(`qualified_type_parameter_nodes` / pinned `lookupTypeParameterNodes`), add Go's
fourth scoped `typeParameterSymbolList` as a `CopyOnWriteSet<SymbolId, ...>`.
Include both repeated-symbol suppression sites and nested success/error scope
restoration. `TypeParameterNames::snapshot` explicitly initializes each scoped
field; do not replace that with a derived clone over unreviewed new storage.

##### JSON encoder continuation review — 2026-09-21

The first typed-write increment does not complete F1b. Its next encoder batch
uses the token contract exposed by pinned `internal/json/json.go` and consumed
by `OrderedMap.MarshalJSONTo`. The underlying authority is the exact
`go-json-experiment/json` revision in `upstream/tsc/go.mod`, including its
`jsontext` state and error behavior. Apply the review input as follows:

1. **Build the token state machine before extending streaming.** Route typed
   scalar, array and object encoding through the same primitives as explicit
   tokens. Track object name/value position, container kind, depth, output
   offset and JSON pointer; preserve the native stream's top-level-value rules.
   A rejected token must leave state unchanged, as `WriteToken` specifies.
   Separate that failure from an I/O failure after output was written. Retain
   the existing typed convenience API over these primitives. Mixed-type
   structs already work through `&dyn Encode` (the leaf driver's `Sample`
   exercises this); the benefit is direct sequential member writes without a
   temporary member vector or a second formatting/state implementation.
2. **Use an object-local decoded-name store, not unqualified output offsets.**
   The current `HashSet<Vec<u8>>` copies every repaired name. Pinned
   `jsontext.objectNamespace` also copies names, but into a shared unquoted-name
   buffer with offsets, switching to a map for large objects. Start from that
   bounded-small-object strategy and collision-safe lookup for larger objects.
   Output-buffer offsets alone do not survive streaming flushes, and raw
   spellings such as `"a"` and `"\u0061"` must compare as the same name. Check
   duplicates after UTF-8 repair, nested namespace isolation, raw escapes,
   buffer growth and flushes. Avoid claiming a protocol performance win before
   measuring an actual consumer.
3. **Move stack checks off scalar writes when restructuring recursive encoding.**
   The current `value()` checks stack space for every scalar and Option layer.
   A growth guard must enclose recursive container traversal, not just the
   opening-token function (whose grown stack would end before child visits).
   Keep the 10,000-container boundary and the 512 KiB-stack regression; exercise
   nested objects as well as arrays and custom `Encode` implementations.
   Custom `Encode` wrappers may recurse without entering a JSON container:
   retain guarded dispatch for that path or explicitly establish its recursion
   contract before removing the existing protection. Iterative token writes
   themselves do not require recursive stack growth.
4. **Implement native error information with the state machine.** Rust's present
   `Display` exposes enum names and lacks native cause/name/offset/pointer
   information. Changing four strings is not equivalent to the pinned error
   contract. Capture those fields in the codec, project only language-specific
   type names at the probe boundary, and compare nested failures, retained
   output and retry behavior with the prepared native traces. The current
   top-level float projection does not certify nested or streaming errors.
5. **Expose exact signed and unsigned integer writes.** Add typed `u64` encoding
   and integer token constructors; never route integers through `f64`. Cover
   `i64::MIN`, `i64::MAX`, `u64::MAX` and the binary64 precision boundary with
   native observations, including integers in mixed-type objects. The current
   public API only supplies `i64` and floating-point number writes.

Implement and check these together before widening the decoder or future
protocol callers. Run affected Rust tests and the prepared leaves/config
comparisons, retaining named gaps for raw values, streaming or error paths
until their actual observations match. Re-record captures if any authenticated
input changes, including package assets; do not edit recorded fingerprints to
make previous observations current.

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

#### F2a — prepare filesystem, path and matching tests

**Start from:** `data/s07/path-requests.json`, the direct path/resolver adapters,
S11 filesystem fixtures, `crates/tsr_vfs/src/{lib,os}.rs`,
`crates/tsr_tsoptions/src/glob.rs`, and native `internal/vfs/*`, `tspath` and
`glob`. Reuse `vfstest` for deterministic traces; keep native OS observations
in a separately marked host-specific group.

**Tasks:**

1. Create an operation-by-adapter capability matrix: snapshot, live OS,
   I/O-backed, cached, tracking, wrapping and test filesystem. For each required
   operation, name either its native-supported behavior or its deliberate
   capability error. Absence in today's Rust trait is an implementation gap,
   not a reason to omit the operation.
2. Freeze stateful traces with observations after each action: create/read/
   overwrite/append/remove; stat/entries; both access and modification times;
   wrapper forwarding; cache enabled/disabled/cleared; cached miss followed by
   creation; cached hit followed by replacement/deletion. Preserve the actual
   native cache contract even where it deliberately retains stale results.
3. Add paired snapshot cases: keep snapshot A alive, mutate the live builder,
   publish B, then read both. A must retain its original bytes, identities and
   program owners. Record these as Rust ownership assertions alongside the
   paired filesystem trace rather than inventing a Go snapshot operation.
4. Freeze path and traversal boundaries: relative/drive/UNC/URL roots, `./` and
   `.\\`, case modes, missing/denied paths, file versus directory versus link,
   dangling links and realpath caches. Walk traces record callback path, entry,
   error, skip decision and ordering. Use a controlled clock where appropriate;
   real OS timestamp cases record filesystem precision rather than sleep.
5. Add all 142 matching baseline outputs through the shared config index. Call
   the original `vfsmatch` test setup, retain directory/include/exclude inputs,
   and compare the exact rendered baseline through the authority established
   at F0. Do not equate the Go list-assertion tests with a renderer for all 142
   files. Also enumerate direct unit tests not represented by baseline files;
   142 is not the count of all path and matching behavior.
6. Prepare `internal/glob` cases independently: braces, ranges, separators,
   malformed patterns, casing, Unicode and root matching. Tag each request
   with its dialect so neither adapter can dispatch to the other silently.
7. Keep OS mutation inside per-case temporary roots. Cases requiring unavailable
   privileges/platform support remain named unavailable; don't substitute an
   in-memory passing result. A demonstrated native nontermination gets an
   isolated watchdog and an explicit pending policy decision, never a hang or
   an invented successful oracle response.
8. Run complete small trace/dialect groups and the matching comparisons through
   existing Rust operations; report missing live adapters explicitly. Add
   comparator controls for stale-cache output, swapped walk callbacks, lost
   timestamp arguments and mutation leaking into an old snapshot.

**Deliver:** capability matrix, filesystem/path/glob requests, 142-output
comparison, host applicability and snapshot assertions, plus F2b's ordered gap
queue. Store case IDs for failures, not just a count or combined stderr log.

**F2a complete when:** all native trace results and 142 baselines are mapped and
replayable; every required adapter/operation has a case; order/state/error
comparators reject the controls; missing Rust capabilities are explicit. One
host's OS results do not certify the other host.

#### F2b — implement filesystem, path and matching behavior

1. Introduce only the capability/API changes required by F2a, including both
   timestamps and the real walk callback contract. Preserve immutable
   `MemorySnapshot` publication; mutate through live hosts/builders instead.
2. Implement live OS and I/O-backed behavior, then wrappers/tracking and caches
   on top of those contracts. Preserve error distinctions and avoid bypassing a
   wrapper or cache in real compiler call paths.
3. Fix path and configuration matching gaps, then the separate test/LSP glob
   grammar. Retain dialect-specific APIs even if low-level helpers are shared.
4. Execute each stateful trace after its corresponding fix; add owner/drop,
   retained-source and snapshot regressions when mutation or caching changes.
   Run host-specific OS cases on the two active native targets without
   re-enabling paused CI runners.
5. Run the full F2a suite and all 142 matching outputs. Check the existing S11
   filesystem cases when changing code used by the endpoint; do not expand the
   endpoint's Unicode or callback contract in this step.

**F2b complete when:** required traces match values, errors, state and callback
order; all 142 outputs match; snapshot isolation holds; host-specific evidence
is accurately labeled. Newly proposed divergences still need owner review.

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

#### Baseline rendering decision — prepare the seam in F0

Use **structured Rust observations plus a shared Go test-envelope renderer**.
Do not port the Go test's option-struct JSON layout into the Rust compiler or
invent a second Rust implementation of its headings and section assembly.
This choice concerns the test format; Rust's production JSON and diagnostic
formatting contracts still have their own required comparisons.

The native command-line test makes this separation explicit:
`commandlineparser_test.go::formatNewBaseline` / `formatNewBaselineBuild`
assemble sections from argument lists, marshaled option bytes, joined filenames/
projects and an already-formatted errors string. Config tests also assemble
sections in `tsconfigparsing_test.go::baselineParseConfigWith`. Their option
bytes depend on the pinned Go structs, tags and marshaling behavior, not a Rust
product serialization API.

Implement the seam as follows:

1. Rust calls its actual parser/matcher/config APIs and emits a typed observation:
   all compared option values and presence states, ordered maps and file/project
   lists, raw config, source bytes, and structured diagnostics. Freeze explicit
   conversions for tristates, enums, nil/empty and omitted fields. The bridge
   rejects unknown/missing protocol fields; it cannot silently zero-fill them.
2. A test-only Go bridge maps that observation to the native data types required
   by the renderer. It only converts representations and renders the test
   envelope. It must not call native parsing, matching or resolution to complete
   a Rust result, recompute missing fields, or read expected results to fill it.
   Compare typed observations before rendering, so an omitted JSON field cannot
   conceal a semantic difference.
3. Use the original envelope functions and marshaling calls. Where assembly is
   inline, carry a minimal reviewed test-source patch exposing the same assembly
   over supplied observations. Hash it with the adapter. Native results through
   this seam must reproduce the untouched native test's bytes; mutating an option,
   file order or presence state must either alter the comparison or be rejected.
4. **Keep diagnostic formatting independent.** The Rust observation also carries
   error text produced by Rust's production diagnostic writer, with the native
   newline/color/context settings. Supply those bytes to the shared envelope;
   compare them against Go's production writer and also compare the diagnostic
   structure. For inline config renderers, expose an errors-text injection seam
   without changing assembly. Go-rendered Rust diagnostic structures are useful
   for triage only: they cannot certify Rust formatting. In Phase A, an absent
   Rust writer keeps that case `not_implemented` even if structural parity holds.
5. Keep input-derived headings/FS dumps separate from observed results. Both
   renderings may consume the same frozen input metadata, but Rust-derived
   result sections must come entirely from the Rust observation. Retain raw
   observations, formatted diagnostics and final baseline bytes so a difference
   can be attributed to semantics, production formatting or the test bridge.

**Owner-approved on 2026-09-20: retain all 309 byte-for-byte baselines and carry
a test-only renderer for the 142 `config/matchFiles` outputs.** F2a implements
and verifies this renderer; F3a reuses the same seam. First reproduce the frozen
bytes from native observations, then render Rust observations through it. This
is approved implementation work, not an outstanding authority choice. Keep
unverified outputs pending and retain pinned Go as the semantic authority.

**Do not extrapolate an existing Go renderer to all 309 files.** The pinned `vfsmatch`
tests assert ordered file lists and reference a TypeScript `matchFiles.ts`
fixture that is absent at the referenced path in this pin. That is not proof
of an executable renderer for every `config/matchFiles` baseline. F0 must trace
those 142 files' request and rendering authority separately. If a group only
has a carried test-format implementation, identify it explicitly, keep native
matching as the semantic authority, and verify its native-result rendering
against the frozen files. A new conflict in request or semantic authority still
uses the baseline-authority stop rule; the already-known absence of a renderer
does not require repeated approval. Do not promise a nonexistent Go renderer
or write an envelope that copies the expected result sections.

#### F3a — prepare config, command-line and resolution tests

**Start from:** the F0 baseline index, F2a's matching cases,
`tools/s07/config/`, `tools/s07/config-resolver/`, `tools/s07/packagejson/`,
`tools/s07/program/`, the corresponding `scripts/s07_*.py` adapters and
`data/s07/*requests.json`. Native authority is the original `tsoptions` tests,
`tsoptionstest` rendering, `diagnosticwriter`, package JSON and resolver code.

**Tasks:**

1. Extend the baseline adapter to all four groups. F2a's 142 matching outputs
   retain the same IDs; don't count them again as new cases. Execute/export the
   original native invocations for the other 167 outputs. Verify each native
   result against the committed pin's baseline before comparing Rust.
2. Freeze every input needed to reproduce an invocation: argument vector,
   current directory, case sensitivity, response-file and config bytes,
   filesystem entries, option mode, environment and rendering/color/newline
   settings. Keep file arrays, raw config and `paths` in source order. Hash the
   exact serialized request the children read, not an earlier object.
3. Connect existing config parsing APIs to the typed observation and shared
   renderer defined above. For missing command-line/build-option APIs, record
   separate `not_implemented` operations with their full native output rather
   than emulating parsing in the driver. If structured config agrees but Rust
   production diagnostic rendering is absent, report that component separately
   while the byte-baseline result remains non-passing.
4. Cover the original tests beyond the baseline outputs: response-file read and
   tokenization errors, explicit null/boolean overrides, repeated options,
   aliases, mode-specific unknown/deprecated options, inheritance/cache errors,
   `${configDir}` substitution, raw JSONC/source locations and mapper manifests.
   Enumerate these from native tables; the list here is a minimum, not a filter.
5. Extend package/resolver probes for uncaptured scope entries: ordered exports/
   imports conditions, invalid/null/missing fields, `typesVersions`, self-name
   imports, type references, usage-site ESM/CJS mode, extension/root/symlink/case
   behavior and redirection. Compare resolved identity, failed lookup locations,
   affecting locations, diagnostics and native traces where the API supplies
   them. A trace-only discrepancy is still visible, even if the path agrees.
6. Add paired cold/repeat/invalidate action traces. Exercise cache separation
   by mode, options, importer and snapshot; include negative lookup followed by
   changed host state according to the native invalidation contract. Preserve
   error emission and include reasons on hits as well as misses.
7. Prepare program-loading checks using the real loader and host. Compare
   ordered loaded-file/include-reason graphs, option diagnostics, duplicate
   input/auxiliary ordering and unresolved results. Keep E2's owner-approved
   selection policy intact; direct foundation tests can cover a case outside
   that subset without changing the E2 denominator.
8. Validate the adapter with deliberate controls: reorder two `paths` entries,
   drop one output from a multi-output test, change a diagnostic argument/color
   escape, merge missing with null, return a cached result in the wrong mode,
   and report Unsupported as ordinary unresolved. Each must be rejected by the
   relevant comparator or completeness check.
9. Compare all 309 outputs once with the available Rust APIs; report the 142/
   87/53/27 counts individually. Run the new small direct resolution/loader
   cases and replay all reports without rebuilding. Produce F3b tasks grouped
   by production cause, retaining every affected case ID.

**Deliver:** one complete 309-output report, direct unit/resolver/cache/program
requests and observations, byte and structured comparators, formatting gaps
linked to F1b, and a production queue with exact reproduction commands.

**F3a complete when:** all 309 native outputs are verified and individually
accounted for; every comparison is runnable or names its missing Rust operation;
request order is tested; supplementary direct/native unit inventory is complete.
A structured-only match does not complete a byte-baseline case. No build/watch
executor or mapper process is implemented to finish this preparation step.

#### F3b — implement config/options and resolution gaps

1. Complete ordinary command-line parsing and response-file handling in
   `tsr_tsoptions`, followed by build-option parsing as a distinct mode. Preserve
   option provenance/order, diagnostic distinctions and callback reads. Do not
   add command execution or build scheduling.
2. Close config interpretation, inheritance, raw-value, JSONC diagnostic and
   cache gaps. Wire F1b's formatter or port only the missing native diagnostic
   writer slice; exact output remains the authority. Include **wildcard-directory
   memoization** from #48 review item 9: Go's `ParsedCommandLine.WildcardDirectories`
   uses `sync.Once`, while Rust currently recomputes on every call. Before caching,
   define the immutable input boundary or explicit invalidation for config specs,
   base path and case sensitivity; these fields are currently publicly mutable.
   Keep the derived result lazy, avoid rescanning/cloning specs on cache hits,
   and keep clones consistent with the accepted owned-options policy. Verify
   unused inputs do no work, repeated and concurrent reads compute once, changed
   inputs cannot return stale watches, and returned key order/recursive flags
   still match the pinned wildcard action traces and matchFiles baselines.
3. Close package JSON and module-resolution gaps in the existing `tsr_module`
   homes. Apply invalidation and mode separation at the real cache owner, not
   only in a test host. Recheck the full action trace, not just its final path.
4. Integrate changed options/host behavior into program loading and rerun the
   corresponding ordered graph/diagnostic cases. Preserve informational E2
   selections and later-phase content-mapper execution boundaries.
5. Run all 309 baselines, direct option/package/resolver suites and applicable
   program regressions. If a shared change affects checker requests, use an E2
   bounded recheck selected by that surface and previously matching controls;
   escalate only when its scope or observed regressions justify a full run.

**F3b complete when:** all 309 outputs match exactly, supplementary direct and
cache tests pass, and loader closure/diagnostics agree. No unsupported required
operation is disguised as an empty successful parse or an unresolved module.

##### Follow-up before concurrent resolution on one resolver

F3b's `Resolver::with_redirect` temporarily replaces the resolver's options
handle, base-options handle and parsed-path-pattern cache. `OptionsScope::drop`
restores them on return or panic unwinding. The option values themselves remain
immutable, and `&mut Resolver` prevents overlapping calls. This is a serial
per-resolver contract, not evidence of concurrent resolution support.

The pin keeps redirected compiler options and request state in
`internal/module/resolver.go:resolutionState`, with per-call traces and
synchronized shared caches in `internal/module/cache.go`. Before Phase 2/3
consumers introduce concurrent calls on a shared Rust resolver, move effective
and base options, request flags, traces, probes and scratch results into
per-call state. Define synchronization and result ownership for the module,
type-reference and parsed-pattern caches as part of that change; moving only
the redirected options is insufficient. Preserve redirect/mode cache separation
and the pin's distinction between base and effective options.

Validate overlapping calls using different redirects and resolution modes,
including hits, misses, negative entries and one failing request. Compare each
result and ordered trace with its serial native counterpart, and verify that
failure or unwinding cannot affect another call's options or results. This
follow-up is required before adding shared-resolver concurrency; it does not
reopen the completed serial F3b scope.

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

#### F4a — prepare syntax, binder and utility coverage

**Start from:** `scripts/s07_inventory.py`, the existing E1/binder drivers,
`data/s06/`, `data/s07/binder-*.json`, `data/s07/*helper*`, native
`internal/astnav/tokens_test.go`, `internal/evaluator/evaluator.go`, and
`crates/tsr_compiler/src/syntactic_diagnostics.rs`. Preserve the existing corpus
expander and identity-aware AST/binder comparators.

**Tasks:**

1. Validate membership and request expansion for all 12,721 physical sources and
   108 libraries. Record virtual units, configurations and options per primary
   ID, plus the separate supplemental IDs. Link current valid observations or
   the accepted Phase 0 record with its actual freshness; do not label the
   historical record a new current run. No E2 eligibility filter may remove a
   parser request.
2. Build the syntax-only program schedule from the complete declared compiler
   input/variant inventory using native preprocessing. Per row, retain load
   outcome, test-selection/option guard and whether the syntactic operation can
   run. Rejected loads retain their native diagnostics and explicit phase status;
   they cannot silently become an empty successful syntactic result. Resolve
   unsupported native authority as a named boundary, not a dropped variant.
3. Observe pinned `Program.GetSyntacticDiagnostics` and its diagnostic rendering
   without invoking semantic/declaration phases for comparison. Call Rust's
   production syntactic API. Reuse matching authenticated native observations;
   otherwise capture this native syntax schedule once. Freeze structured
   diagnostics, order/deduplication, codes/arguments/ranges and rendered bytes.
4. Add discriminating syntax requests: JS-only diagnostics, parameter decorators
   with `checkJs`/`experimentalDecorators`, `@ts-check` and `@ts-nocheck`
   precedence, malformed sources, empty/missing nodes and cross-file owner
   rejection. Mark content-mapped filtering as the named Phase 5 boundary; don't
   make an unmapped source stand in for a mapped case.
5. Audit AST/scanner/parser/binder callable utilities against production callers
   and existing probes. Construct only the missing graph witnesses: list order,
   trailing commas and ranges; clone/update/original identity; foreign owners;
   lazy JSDoc/token repeated access; source text; bundle/member retention;
   independent binding/publication and helper caches. Preserve byte and owner
   observations separately from node pretty-printing.
6. Export all native astnav test tables/baseline requests, with source bytes,
   cursor positions and expected token identity/range/kind. Include first/last
   positions, trivia/comments, zero-width nodes, preceding/next/touching-token
   operations and repeated lazy token identity. Compare to real `Navigator`
   operations, not a second scanner in the driver.
7. Freeze evaluator requests with controlled callback observations: literal and
   operator values, string-ness, cross-file/external-reference flags, unknown
   results and callback/short-circuit order. Connect the existing evaluator
   behavior where available; otherwise name the missing reusable callback API.
   Don't duplicate checker name resolution to manufacture a standalone result.
8. Test the comparators with a missing expanded unit, reordered diagnostics,
   changed parent/flow edge, wrong owner/identity, lost trailing comma, and a
   syntactic adapter accidentally appending semantic diagnostics. All must fail.
9. Run a bounded real smoke covering each new utility group and syntax boundary,
   then replay its outputs. Validate full corpus scheduling without rerunning
   full parse/bind just to show the adapter works. Record unexecuted prepared
   cases separately; F5b performs the final full correctness capture.

**Deliver:** complete parse/bind coverage links and syntax schedule, native
syntax observations, utilities/navigation/evaluator requests, tested drivers and
comparators, plus F4b's gaps ranked by actual behavior. Mapping percentages are
not substitutes for this report.

**F4a complete when:** every primary/expanded request is accounted for; the
syntax phase cannot be confused with whole `.errors.txt`; all required utility
operations have executable witnesses; bounded controls pass and pending Rust
behavior is named. Full Rust corpus parity is F4b/F5b's production obligation,
not something to fake or repeatedly capture during preparation.

#### F4b — close syntax/binder and reusable utility gaps

1. Fix the smallest production cause from F4a's queue and add its focused
   regression. Keep explicit bind-before-publication, compact storage and local
   readers; no broad representation rewrite is needed to close utility gaps.
2. Complete syntactic diagnostic aggregation/formatting and option behavior
   through `Program::syntactic_diagnostics`. Keep semantic/declaration checking
   outside this path and retain mapped-source limitations explicitly.
3. Complete AST/lazy/binder utility operations with the prepared identity,
   retention and graph witnesses. Changes to ranges must preserve load-bearing
   list/trailing-comma behavior; changes to caches must preserve the correct
   owner and publication boundary.
4. Finish navigation, then the reusable evaluator API where required. Share the
   existing callback-based implementation with consumers instead of retaining
   a test-only algorithm and a separate checker implementation.
5. Run each affected utility group and selected primary cases during fixes.
   Use release/deep-stack, malformed-byte and ownership instrumentation when
   their corresponding surfaces change. Once fixes settle, capture or validate
   the complete parser/binder and syntax families; F5b reuses these captures if
   the relevant sources and observation contracts are unchanged.

**F4b complete when:** full required parse/bind requests and syntax observations
match; utilities, navigation and evaluator groups pass; there are no
unclassified Phase 1 operations. Phase 1 parse parity must be exactly 1.0 without
changing E1's historical threshold or narrowing its primary corpus.

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

#### F5a — prepare integration checks and the Phase A review

**Start from:** all F0/F1a–F4a outputs, `status/runs.toml`,
`docs/TRACKING.md`, the current generation/package checks, `scripts/s11.py`,
`data/s11/` and the S11 Session/subprocess tests. This step proves that the
prepared pieces form one enforceable contract; it does not implement the
remaining product behavior.

**Tasks:**

1. Join scope → operation → case/action → native authority → Rust driver →
   comparison → producer metric → sprint item. Reject orphaned operations,
   duplicate case ownership, missing outputs and a metric with no contributing
   observations. Reused cases may support several claims but retain one
   identity; the config denominator stays 309, not 309 plus matching again.
2. Prepare integration witnesses for the actual shared boundaries: localized
   diagnostics in config baselines; package/config ordered values through
   resolution; live filesystem changes versus retained program snapshots;
   generated messages/library assets as consumed outside the checkout; syntax
   diagnostic output through the real compiler path. These compose production
   APIs where present; missing dependencies yield named pending rows.
3. Map all 89 existing S11 cases to their continued obligations. Run the existing
   transport smoke when changing an adapter; reuse valid full observations
   otherwise. Add a fixture only if a newly tested foundation operation exposes
   an uncovered transport contract. Don't claim blocked workers, a production
   mapper host or semantic fourslash coverage from these tests.
4. Prepare generator/asset checks for the F1b locale extension and retain current
   AST/diagnostic/API generation and untouched-client equality. Missing locale
   output stays pending; don't count the existing generator's success as proof
   that the new locale tables exist. Identify the actual pinned Node/npm setup
   and reuse its bootstrap path instead of substituting the system version.
5. Implement/finish the three grouped producer adapters and their source/input
   closures described below. Fingerprint every consumed native bridge, request,
   baseline, comparator, transitive Rust dependency and build setting. Verify
   capture and ledger closure agreement with mutation tests, including nested
   files, rather than trusting two similar-looking glob lists.
6. Test aggregate failures by deleting one required case, duplicating a row,
   using an earlier request schedule, tampering with an artifact, substituting
   a partial report and changing a relevant source after capture. Also prove an
   unrelated documentation edit does not invalidate a code-only observation.
   Do not exclude a real dependency merely to avoid evidence becoming stale.
7. Add harness-health checks to existing CI and keep unfinished feature parity
   separately reported. Test that an unmet feature keeps its sprint pending,
   while it does not break unrelated build/quality jobs merely because Phase A
   was merged. No silent skipping and no new performance jobs or target matrix.
8. Generate the Phase A report with totals by family and status; list exact
   required IDs still unobserved, different or missing. Include one concrete
   native/Rust example per root cause, F1b–F5b ownership/dependencies and commands
   for reproduction. Check that every F0 operation has a reviewed disposition
   and that no gap disappeared solely because an adapter omitted it.
9. Review all preparation exits and the report. State which future production
   PRs close each gap, which contracts already pass and which genuine decisions
   need the owner. Stop here for the agreed coverage review, with committed
   runnable tests and reports before F2b–F5b. F1b is the explicit early-start
   exception recorded above, and its delivered behavior participates in this
   review.

**Deliver:** integration fixtures, complete producer/consumer mapping, tested
fingerprints and failure aggregation, CI harness checks, and the finished Phase
A coverage/gap report. The progress record must distinguish test preparation
complete from Phase 1 implementation complete.

**F5a complete when:** every `a` checklist passes; all required behavior is
accounted for by runnable checks; native truth and missing Rust work are clear;
a broken/partial/stale report cannot pass any gate. A set of skeleton test
files, example-only baselines or a TODO list without runnable adapters does not
satisfy this exit.

#### F5b — integrate, validate and close Phase 1

1. Confirm F1b–F4b completion and review the final source/contract changes. Resolve
   integration failures through the owning production path, then rerun the
   affected family. Recheck source fingerprints before planning captures so a
   cheap manifest failure is found before a full run.
2. Run the final complete Phase 1 correctness suite once: all required leaf/VFS/
   utility traces, all 309 baseline outputs, the full parser/binder request
   inventory and the complete syntax schedule. Reuse captures already made at
   F1b–F4b if their normal validators still accept the relevant sources, pin,
   requests and observation contract. Never refresh a measurement merely to
   obtain a newer timestamp.
3. Run final generator drift and untouched-client checks for changed generated
   surfaces; verify locale/library assets and installed consumers when affected.
   Validate all S11 transport/Session cases against the final relevant inputs.
   Keep the documented wire/later-phase boundaries unchanged.
4. Run affected debug/release tests, formatting, clippy, dependency policy,
   declared MSRV and applicable ownership instrumentation on the two active
   native CI targets. Preserve existing quality gates. CI failure diagnosis
   remains scoped; do not respond by scheduling unrelated benchmarks.
5. Record the real producer outputs, regenerate tracker views, validate the
   ledger and run each Phase 1 sprint check. Update `PORTS.toml` only for actual
   implementation homes and verified behavior. The new closure check must not
   depend on stale S07/S08 timing metrics or silently reopen S12.
6. Publish the final operation/coverage report: exact totals, current artifacts,
   scoped accepted qualifications and later-phase obligations. Confirm that
   every required Phase 1 row passes. A newly discovered unsupported case means
   work remains; fix it or obtain an explicit scoped owner decision, without
   changing counts or thresholds to manufacture completion.

**F5b complete when:** the section 4 claims have passing reproducible evidence,
all Phase 1 sprint exits pass, reports match the evidence and no required case
is missing. Historical Phase 0 closure and later-phase work remain accurately
recorded. No timing rerun is a Phase 1 closure prerequisite.

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

### Producer and sprint wiring to implement during preparation

Register new producers only when their adapters exist and can report the whole
inventory honestly. The names below are proposed additions, not commands that
work before F0–F5a implements them. Keep the existing `e1`, `binder`, `gen` and
`testhost` producers and their contracts; do not duplicate their capture logic.

Families select adapter/capture work; producers are ledger records. Their mapping
is explicit and many-to-one:

| Command family | Ledger producer | Production result |
| --- | --- | --- |
| `leaves` | `foundations` | `run.foundations.leaves_complete` |
| `filesystem` | `foundations`; `config` consumes the shared 142 baseline rows | `run.foundations.filesystem_complete`; the 142 outputs also contribute once to `run.config.parity` |
| `config` | `config` | All 309 output rows and `run.config.direct_complete` for supplementary option/package/resolution cases |
| `syntax` | `syntax`; reuse `e1` and `binder` for their existing corpora | Syntax-only observations; existing full parser/binder parity remains separately required |
| `utilities` | `foundations` | `run.foundations.utilities_complete` |
| `integration` | `foundations`; reuse `gen` and `testhost` for their contracts | `run.foundations.integration_complete` for the F5a cross-family witnesses, plus the existing generator/transport requirements |

**Freshness tradeoff:** retain these three new grouped producers. The ledger
uses one source fingerprint per producer, not per metric: a relevant leaf edit
therefore stales the entire `foundations` record, including its filesystem,
utility and integration booleans. Do not claim these remain independently
current. Keep family captures independently authenticated against their actual
request, adapter, binary and transitive source inputs. Re-recording a grouped
producer can replay still-valid family captures and rerun the invalid ones,
then emit a newly validated aggregate. If families share a changed binary or
dependency, all affected captures must be rerun; a family label is not a waiver.
The ledger's source set remains the union of all contributing closures. This
limits unnecessary captures without changing the tracker or introducing a
producer per helper. F5a tests must demonstrate this behavior.

| Producer | Required observations / proposed metrics | Completion rule |
| --- | --- | --- |
| `foundations` | Full frozen leaves, filesystem/path/glob and utility inventories; `inventory_complete`, `leaves_complete`, `filesystem_complete`, `utilities_complete`, `integration_complete` | Each family boolean is computed from its exact required IDs and every comparison; no empty/vacuous true and no unknown operation. Snapshot/ownership assertions and the F5a cross-family witnesses are part of the relevant case results. |
| `config` | Case IDs are the 309 output paths; runner-derived `tests_total`, `parity`; `inventory_complete` also verifies invocation/output mapping | Exactly 309 expected results, parity 1 and complete mapping. Supplementary direct config/package/resolution cases also feed a required `direct_complete` metric; matching 309 alone does not close them. |
| `syntax` | Frozen program syntactic case/phase IDs; runner-derived parity; `inventory_complete` | Every required load/syntactic observation is accounted for and matches. An unexecuted native phase cannot be recorded as an empty successful diagnostic list. |
| Existing `e1`, `binder` | Existing primary/supplemental results and frozen inventories | Phase 1 consumes parity 1 and full membership; E1's historical threshold remains unchanged. Missing helper coverage is supplied by `foundations.utilities_complete`. |
| Existing `gen`, `testhost` | Actual generator/drift/client results, new localization inventory result, S11 parity/controls | Existing success is insufficient for a newly added locale requirement; give that requirement its own computed metric, `run.gen.locale_complete`. |

The new sprint definitions must enumerate the metrics they consume and identify
which family/case inventory establishes each requirement. F0 can register
pending names before producers exist; it cannot populate passing constants.
F5a's tracker tests must demonstrate that each missing/false family metric
prevents its production sprint and Phase 1 closure from passing. Preparation
sprints are separate and consume manifest/harness/coverage readiness, never
production parity. Do not make another experiment pass accidentally through a
bare reused boolean or a slice result masquerading as full coverage.

Use separately computed preparation metrics: `run.foundations.harness_pass`,
`inventory_complete`, `leaves_prepared`, `filesystem_prepared`,
`utilities_prepared`, `integration_prepared`, `run.config.prepared` and
`run.syntax.prepared`. Unqualified names in that list belong to `foundations`.
A `prepared` metric checks complete case/authority/dispatch coverage, the
required smoke and comparator controls, and classified gaps. It may be true
while its corresponding production-completion metric is false. Neither is a
manually entered acknowledgement. F0 readiness uses inventory and harness
checks; later preparation exits add their family checks. Keep the detailed
per-step checklists as review requirements beyond what these metrics can prove.

For producers with `cases`, emit the exact xtask test-result protocol and let
xtask derive parity. For grouped booleans, retain the full authenticated per-case
report and artifact identity behind every aggregate. Structured development
outcomes may be richer than xtask's protocol, but their adapter cannot turn
`not_implemented`, `native_unavailable`, `harness_failed` or `not_run` into pass.

At F5a, document the exact new sprint IDs and runnable `cargo xtask run <id>` /
`cargo xtask check <sprint-id>` sequence in the progress record. At F5b, record
those results and use the existing `cargo xtask validate`, `cargo xtask status`
and `cargo xtask status --check-committed` commands. Do not claim a new command
or gate exists until its implementation has landed.

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

Stop for the scheduled Phase A coverage review after F5a, before starting F5b.
The amendments above authorized F1b–F4b; do not repeat them as a prerequisite.
Outside that planned review, stop for owner input only when a real decision is
needed: a new baseline behavior divergence, a change to an accepted transport/
ownership contract, a new required platform/dependency policy decision, or an
upstream baseline whose authority cannot be established. Routine porting, helper
placement and fixing a measured mismatch do not need repeated approval. Report
such conflicts with a minimal reproducer and pinned Go output.

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
- **Can preparation finish while production is missing?** Yes. Each missing
  operation has native truth, a runnable explicit missing-result path and a
  production task. Preparation readiness and feature parity use separate
  metrics; neither empty output nor a prose TODO can pass as implementation.
- **Can a bounded smoke certify a whole family?** No. Its selected IDs are
  recorded, unselected cases remain `not_run`, and the full-family gate rejects
  incomplete results. Native/reference inventories remain independent of what
  Rust currently supports.

F0 delivered the initial command-line baseline adapter and missing-operation
result. F1a–F4a have been reviewed and F1b–F4b have proceeded under the recorded
amendments. Review the F5a coverage/gap report before F5b; a merged implementation
does not discharge its unwitnessed operations.
