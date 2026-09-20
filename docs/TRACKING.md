# Tracking

Implementation state is declared in the ledger. Verification is computed from current execution evidence. A sprint closes only when every exit check and every required item passes. The status tool validates provenance and arithmetic; the owner still reviews the adequacy of tests, scope exclusions, thresholds and architectural decisions.

## Repository records

| Record | Purpose |
|---|---|
| `PORTS.toml` | Upstream file inventory, implementation state, Rust paths and required verification checks |
| `data/go-functions.tsv`, `data/upstream.json` | Receiver-qualified function inventory and full upstream pin with generated-field/inventory checksums |
| `status/runs.toml` | Reviewed producer commands, source globs, declared inputs, target and configuration |
| `status/experiments.toml` | Typed acceptance criteria for E1–E8 |
| `sprints/*.toml` | Functional tasks, required evidence and sprint exit criteria |
| `docs/adr/` | Proposed, accepted, amended and superseded architectural decisions |
| `status/evidence/` | Immutable run artifacts and a latest-attempt pointer per run |
| `STATUS.md`, `status/status.json`, `docs/status.html` | Generated views of current evidence |
| `status/unmapped-functions.json` | Complete unmapped-function worklist, kept separate from the small status summary |
| `status/history.jsonl` | Deduplicated snapshots linking the evidence behind each point |

## Ledger and pin provenance

The ledger has one entry per non-test upstream Go file. Files are inventory units; sprint tasks should describe smaller functional slices. One Go file can map to several Rust files, and multiple Go entries can reference the same Rust file.

```toml
[[file]]
go = "tsc/internal/scanner/scanner.go"
package = "internal/scanner"
crate = "tsr_scanner"
phase = 0
kind = "source"
status = "ported"
rust = ["crates/tsr_scanner/src/lib.rs", "crates/tsr_scanner/src/literals.rs"]
verify = ["run.scanner.parity == 1", "run.scanner.tests_skipped == 0"]
pin = "<full upstream SHA>"
source_hash = "<SHA-256 of current upstream file bytes>"
loc = 1
```

The editable fields are `status`, `rust` and `verify`; changing them does not require regenerating upstream provenance. Implementation states are `planned`, `in-progress`, `ported` and `out-of-scope`. `ported` requires existing Rust paths. The tool derives `verified` only when the file is synchronized to the current pin and every nonempty `verify` check passes against current run evidence. Writing `status = "verified"` is an error. Missing paths, missing evidence, stale synchronization and failed checks cannot earn verification. `kind` distinguishes `source`, `generated`, `harness` and `out-of-scope`; generated files need generator/drift evidence as well as the applicable crate parity gate.

`pin` is the last synchronization commit; `source_hash` records the current pinned bytes. On a pin bump the generator advances unchanged, previously synchronized files. Changed files retain their old synchronization pin until their port is reviewed and updated. Repeating generation cannot clear that stale state. Evidence must be rerun for the new input fingerprint even if a source file itself was unchanged.

Generate all provenance records together:

```sh
python3 scripts/ledger-init.py                    # uses this repository's upstream/
python3 scripts/ledger-init.py ../TypeScript --pin 1f70213d49  # explicit bootstrap checkout
```

The script requires a clean checkout, resolves the requested pin to the actual full HEAD, reads tracked Git blobs, generates both files, then publishes their hash manifest last. A wrong pin, dirty checkout, malformed Go file or duplicate function ID fails generation. Manifest version 2 stores `ledger_generated_sha256`, the hash of a canonical projection containing the global pin and each file's generated `go`, `package`, `crate`, `phase`, `kind`, `pin`, `source_hash` and `loc` fields. Its inventory checksum covers the complete function-inventory file. `status`, `rust` and `verify` are excluded from that projection, so ordinary progress and gate edits preserve upstream provenance. Go 1.26 or later must be on `PATH` for the inventory generator, its tests and the oracle producer; where Go is installed through mise, export `~/.local/share/mise/shims` or run `mise activate` first.

`cargo xtask validate` checks the generated-field and inventory hashes plus pin agreement. Changes to generated fields require regeneration; editable fields are validated separately and their verification checks are reevaluated against current evidence. These hashes detect drift and partial generation; they do not replace review of the generator or authenticate an arbitrary hand-edited manifest.

The bootstrap checkout is sufficient for inventory generation. S01's oracle gate separately requires a registered, initialized `upstream/` submodule at that same pin, with the canonical Microsoft URL and a clean worktree.

The canonical submodule is now registered and initialized at `1f70213d4922b434345f639b441681e470c7cfc1`, and the actual oracle build and `--version` smoke test have passing run evidence. ADRs 0006, 0007 and 0013 and their design notes are accepted; S01 passes against the current bootstrap evidence. The S04 leaf portions of E3 and E4 have executable producers; full E1–E8 verification remains pending. Inspect current status when changing selected inputs: recorded bootstrap success is not a permanent waiver of those checks or proof of Rust compiler parity.

## Function traceability

```rust
/// port: tsc/internal/checker/checker.go:Checker.getTypeOfSymbol
fn get_type_of_symbol(/* ... */) { /* ... */ }
```

Canonical identities include the repository-relative file path, receiver type and function name. Pointer receivers are normalized to their base type. Free functions use `path:Function`; `init` declarations use per-file `path:init#1`, `path:init#2`, etc. The inventory carries its full upstream pin, source line ranges and canonical IDs. Parse errors and duplicate identities fail generation.

Use several markers when one Rust item replaces several Go functions, or repeat one marker across Rust items when a function is split. Each upstream identity counts once. File markers such as `//! port: tsc/internal/scanner/scanner.go` are navigation aids and earn no function credit. Unknown function markers fail validation and sprint checks. Counts cover in-scope source files; generated and excluded functions do not distort that denominator.

`status/status.json` is a small summary containing counts and a link to [status/unmapped-functions.json](../status/unmapped-functions.json). The separate worklist contains every unmapped in-scope source-function identity grouped by package. This keeps routine status reads small without truncating the actionable list.

This measures **mapped functions**, not semantic completeness or remaining effort. A marker can accompany a stub. Report mappings, baseline parity and completed gates separately; correctness comes from the required behavioral evidence.

## Execution evidence

A producer is a reviewed command registered in `status/runs.toml`:

```toml
[scanner]
command = ["python3", "scripts/run-scanner-parity.py"]
sources = ["crates/**", "Cargo.toml", "Cargo.lock", ".cargo/**", "rust-toolchain*", "xtask/**", "scripts/**"]
inputs = ["tests/scanner/options.json", "tests/scanner/baselines.json"]
cases = "tests/scanner/cases.json"
target = "aarch64-apple-darwin"
config = "release; frozen scanner corpus"
```

This is an example; the scanner harness does not exist yet. The registered producers are the workspace and oracle bootstrap runs, the `fmt`, `clippy`, `deny` and `selftest` check runs (`scripts/checks.py`), the S03 `gen` run ([contract](S03.md)), and the S04 `e3`/`e4` leaf runs. The [S04 producer contracts](S04.md) specify frozen coverage, measured counters and actual instrumented execution. Add the `scanner`, `binder`, `program`, `testhost`, `checkerbench`, `relater` and remaining experiment producers when their implementations exist; [sprints/README.md](../sprints/README.md) names each one with the metrics it must emit. Numeric and boolean thresholds are defined now so missing implementations remain pending.

Run a producer with `cargo xtask run scanner`. The command executes directly, without a shell, in the repository root. It must write exactly one JSON object to stdout; logs go to stderr:

```json
{"metrics":{"encoder_errors_match":true},"tests":{"literal-a":"pass","literal-b":"fail"}}
```

If `cases` is declared, that file is a nonempty JSON array of unique test IDs. The runner requires exactly those IDs, rejects duplicate or unknown results, and accepts only `pass`, `fail` and `skip`. It derives `tests_total`, `tests_passed`, `tests_failed`, `tests_skipped` and `parity`; the producer cannot override them. Skips stay in the denominator. Producers without a case manifest may emit numeric or boolean metrics, but cannot emit test results. Future nextest/JUnit adapters must preserve case identities and counts through this contract; no adapter is currently claimed to exist.

The runner records the full Rust revision, the run's source fingerprint, full upstream pin, declared input hashes, command, target/configuration, selected build environment, host, Rust toolchain, exit code and raw stdout/stderr with hashes. `sources` selects tracked and nonignored new files using Git-style glob matching, including `**`. If omitted, it defaults to `crates/**`, `Cargo.toml`, `Cargo.lock`, `.cargo/**`, `rust-toolchain*`, `xtask/**` and `scripts/**`. Include every implementation, shared dependency and producer that can affect the result; reviewing that source set is part of reviewing the producer contract. Build outputs, evidence and generated status views are excluded to avoid circular invalidation. Dirty submodules cannot provide evidence.

The workspace producer declares its own source set, using `Cargo.*` for the root manifest/lockfile alongside the relevant configuration and code. The oracle producer includes the `upstream` gitlink in its source set and `.gitmodules` as a declared input, so its evidence is tied to the actual checkout and submodule registration as well as the pinned provenance.

Documentation, root Markdown files and sprint files are outside the default source set. A producer that consumes them can include them explicitly in `sources`; `inputs` and `cases` are always hashed even when their paths are documentation or otherwise outside the source set. Changes to a run's source set, selected source bytes, command, target/configuration or declared corpus/configuration inputs invalidate that run. The specification of an unrelated run does not invalidate it. The upstream pin and relevant captured build environment must also match. Source-identical commits may reuse results; the original tested revision remains recorded.

Acceptance policy is separate from measurement inputs. Changing experiment thresholds, sprint checks or editable ledger fields reevaluates the recorded metrics without rerunning unaffected producers. If such a file is explicitly consumed through `sources`, `inputs` or `cases`, its change does invalidate that producer. Review policy changes directly; changing a threshold must not manufacture a new measurement timestamp or hide an unmet requirement from the plan.

The top-level `context` in `status/status.json` is a broader repository snapshot used for policy/history tracking. Artifact freshness uses each run's selected-source digest and declared inputs instead. Editing `PLAN.md` can therefore change the status context while every unaffected run remains current; a changed summary context alone does not mean its evidence is stale.

Successful capture requires exit code zero, a valid report, and unchanged source/inputs across execution. A harness may exit zero after successfully measuring imperfect parity; the typed thresholds decide whether that measured result passes. A process failure is not a measurement. A failed or malformed rerun replaces the latest attempt and cannot expose a preceding success. Attempts for one run ID are serialized. An interrupted process can leave `status/evidence/<id>.lock`; remove that empty directory only after confirming no process for that run remains, then rerun.

Artifacts are named by their SHA-256 under `status/evidence/`; `<run-id>.latest` selects the latest attempt. Consumers verify artifact content and compare its inputs with the current repository before exporting `run.<id>.<metric>`. Missing, stale, failed and corrupt records remain visible in the evidence state and supply no passing metrics. Artifact capture proves which producer ran against which inputs, not that the producer's assertions are sufficient; review those contracts with the port.

## Experiments and sprints

Each experiment has multiple typed criteria. **Every criterion must pass.** For example:

```toml
[[E7.criteria]]
id = "checker_parity"
metric = "run.e7.checker_parity"
op = "=="
threshold = 1
unit = "matching checker cases / frozen subset"
```

E7 requires parser size, parser throughput, checker parity and a portable host. E8 requires Node latency, Rust-consumer parity and lifetime checks. E5 includes the per-type memory threshold. Notes and `nature` explain measurement limits but never supply passing values.

E4 has separate early helper and scanner criteria for S04/S05; encoder criteria gate S06. Its complete literal-type and printer integration requirements remain mandatory in S08. S08 also requires separate checker throughput, allocation and retained-memory measurements and a parity-tested, measured arena-reference relater prototype. The measurement-domain checks in E2 require usable samples; they impose no new performance target.

The design notes behind ADRs 0006, 0007 and 0013 name the E3/E4 criterion ids each scenario row covers. Every criterion appears in a scenario row or applies to all rows as a harness-wide check, so a producer case can be traced to a note row and a criterion to its scenarios.

Sprint checks use `<metric> <op> <value>`, with numeric comparisons, boolean equality and ADR/implementation states. Missing metrics cannot pass; malformed definitions, unknown fields and duplicate sprint/item IDs are errors. Required items default to `required = true`. They need nonempty `done_when` checks and block completion until every check passes. Only explicitly optional items may remain informational.

S01 checks the reviewed contracts, provenance, registered upstream pin, actual workspace build, oracle build and version smoke. Merely having inventory entries cannot complete it. S02 to S12 are the Phase 0 implementation plan; [sprints/README.md](../sprints/README.md) gives their order, the producers each registers and the conventions they follow. ADR acceptance is a reviewed human decision; changing an `Accepted` label is not a substitute for the required design note and review.

**S12 evidence reuse across the crate rename (2026-09-20).**
[ADR 0020](adr/0020-phase-0-gate-decision.md) records the owner's confirmation
that the [2026-09-19 captures](S12-evidence.md) remain applicable to S12.
The tracker's `stale` label denotes an exact fingerprint mismatch, not aged
measurements; the crate rename changes those fingerprints. S12's items and exit
check the accepted reuse decision. Original artifact identities and normal
producer, experiment and other sprint checks remain unchanged.

## Commands, publication and history

```sh
cargo xtask validate           # read-only tracking/provenance validation; pending work is allowed
cargo xtask run workspace      # actual locked Cargo workspace build and evidence capture
cargo xtask run oracle         # checked submodule, Go build and --version evidence
cargo xtask status             # regenerate summaries, dashboard and the unmapped-function worklist
cargo xtask status --check-committed # read-only reproduction using the recorded report context
cargo xtask check S01          # fails unless every required item and exit criterion passes
cargo xtask check-metrics 'run.fmt.clean == true' # enforce current metrics independently of sprints
cargo xtask status --record    # append only a distinct source/evidence snapshot
```

If a sandbox cannot write Go's system cache, use a writable workspace cache for the oracle command: `GOCACHE="$PWD/target/go-build" cargo xtask run oracle`. cargo-deny keeps its advisory database at its default location under `$CARGO_HOME`; never point `db-path` inside the repository, because a pruned copy makes cargo-deny reset the enclosing checkout's tracked files. These settings change cache locations, not the required checks.

`run` captures a producer's report; a successful capture can contain failing metrics. Use `check-metrics` to enforce required results independently of sprint completion. It compares the recorded host, Rust compiler identity and build environment with the current runner, rejects missing or stale evidence and fails if any requested comparison fails.

`status --check-committed` checks the four generated views without writing them: `STATUS.md`, `status/status.json`, `status/unmapped-functions.json` and `docs/status.html`. It preserves only the recorded report context (including the renderer's host, Rust compiler identity and environment) and generation date, then recomputes the report from the current ledger, policy and validated raw evidence. Source digests, upstream pin, run specifications, input hashes and artifact checksums must still match. Each artifact's execution identity is compared with the recorded renderer identity, preserving stale results as stale. This lets Linux reproduce a report captured on macOS, including after a source-identical commit, without claiming the measurements ran on Linux. Live metric and sprint gates always use the current host, compiler and environment.

A status workflow is installed at `.github/workflows/status.yml`: on push, pull request, manual dispatch and nightly it validates schemas and provenance, checks committed views using `--check-committed`, reruns the registered producers on macOS and Linux, and enforces their build, smoke, format, lint, dependency and self-test metrics plus S01, S03 and S04. S02 remains informational until complete; its quality checks already gate CI independently. Separate jobs compile the locked workspace and all targets with the declared minimum Rust version, currently 1.96.0, on both operating systems. Regenerated views, including the unmapped-function worklist, and raw evidence are published as workflow artifacts even after a metric gate fails. No run of this workflow is recorded in the repository, and nothing is committed back; evidence in the repository is captured and reviewed locally.

History records pin, repository context and underlying artifacts. Re-rendering the same context and evidence adds no new point. A policy or documentation edit can create a distinct history snapshot while reusing the same current run artifacts; it is not a new measurement. The chart separates upstream pins and legacy history from the current series. A publication date is not a measurement date: nightly publication of old results cannot satisfy the four consecutive weekly measurement runs required for cutover. That future gate must inspect distinct run artifacts and their execution timestamps against the approved workload matrix.

ADRs change when their architectural decisions change. An amendment records a refinement; a replacement decision supersedes the old ADR. Ordinary implementation fixes that follow an existing decision need neither a replacement ADR nor a new approval ceremony.
