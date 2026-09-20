# Phase 1 progress record

A results record for [PHASE1-implementation-plan.md](PHASE1-implementation-plan.md),
not a second plan. Each step records what was completed, what is still missing
and the exact command that reproduces it.

Pin `1f70213d4922b434345f639b441681e470c7cfc1`. Gitlink, `data/upstream.json`
and the initialized submodule HEAD all agree.

| Step | State |
| --- | --- |
| F0 — inventory, manifests and executable setup | complete, with one named blocker |
| F1a — foundation leaf tests | not started |
| F2a — filesystem, path and matching tests | not started |
| F3a — config, command-line and resolution tests | not started |
| F4a — syntax, binder and utility coverage | not started |
| F5a — integration checks and the stage A review | not started |

Stage A preparation is **not** Phase 1 implementation. No production behavior
was added or changed in F0.

## F0 completion checklist

| Requirement | Result |
| --- | --- |
| Scope has zero unclassified operations | 1,678 operations, every one carrying a disposition and a recorded basis |
| All 309 outputs have verified invocation mappings | 167 of 309 mapped to a Go invocation and renderer; **142 blocked**, see below |
| Manifests and failure tests pass | 31 Phase 1 tests, plus the extended discovery regression |
| The real pilot has an observed match and a named missing operation | 2 matches against pinned Go, 4 named missing Rust operations |
| Replay is read-only | `compare` runs no child process; a test substitutes `subprocess.run` to prove it |
| The pending queue is generated from concrete rows | queue below is derived from `data/phase1/scope.json` |

## Blocker: `config/matchFiles` baseline authority

**The 142 `config/matchFiles` reference outputs have no Go invocation and no Go
renderer at this pin.** The plan anticipated this and required it be named
rather than papered over. Evidence, each independently re-derivable:

1. No pinned test writes that subfolder. Across every `*_test.go` under `tsc/`,
   the only `baseline.Options{Subfolder: ...}` values are
   `config/tsconfigParsing` and `tsoptions/commandLineParsing`.
   `phase1_baselines.verify_written_subfolders()` re-derives this on each
   `inventory --check`, so the claim cannot rot silently.
2. The envelope does not match any renderer in the pin. The 142 outputs carry
   `config:`, `Fs::`, `configFileName::` and `Errors::`, and dump the whole
   parsed result as JSON. `baselineParseConfigWith`
   (`tsconfigparsing_test.go:1503`) instead emits `configFileName::`,
   `CompilerOptions::`, `TypeAcquisition::`, `FileNames::` and `Errors::`, and
   has no `config:` heading.
3. The titles do not overlap. Zero of the 142 names appear in the 87
   `config/tsconfigParsing` outputs, so they are distinct content, not a
   re-homed copy.
4. `vfsmatch_test.go` references
   `tsc/testdata/fixtures/testRunner/unittests/config/matchFiles.ts`, which is
   absent at this pin. These are promoted TypeScript outputs whose renderer was
   not ported.

What does exist is the **semantic** authority: `vfsmatch_test.go`'s
`TestReadDirectory` and `TestReadDirectoryMatchesTypeScriptBaselines` assert
ordered `matchFiles()` results against the pinned implementation. The F0 pilot
already compares Rust to that authority and matches.

This is an owner decision, listed under the plan's stop conditions as "an
upstream baseline whose authority cannot be established". The options:

- **A.** Keep 309 as the byte-baseline denominator and carry a reviewed
  test-format implementation for the matchFiles envelope, verifying it
  reproduces the frozen bytes from native results before any Rust comparison.
- **B.** Hold the 142 as ordered-list semantic comparisons against the native
  `vfsmatch` authority, and reduce the byte-baseline denominator to 167 with
  that reduction recorded explicitly.
- **C.** Treat the 142 as unreachable at this pin and record them as a standing
  qualification.

F0 does not choose. The index records the group as `blocked` with its reason,
`inventory --check` reports it, and nothing reconstructs expected results from
Rust in the meantime.

## The 309 index

`data/phase1/config-baselines.json` records every output's exact path, byte
length and hash, plus per-group authority.

| Group | Outputs | Authority | Invocation → output |
| --- | ---: | --- | --- |
| `config/matchFiles` | 142 | **blocked** | unknown |
| `config/tsconfigParsing` | 87 | `baselineParseConfigWith`, plus inline assembly in `TestParseConfigFileTextToJson` | one invocation renders one output, but a single output may concatenate several parsed configs |
| `tsoptions/commandLineParsing/parseCommandLine` | 53 | `formatNewBaseline` | one invocation renders exactly one output |
| `tsoptions/commandLineParsing/parseBuildOptions` | 27 | `formatNewBaselineBuild` | one invocation renders exactly one output |

Two `config/matchFiles` outputs live under a nested directory, because the
originating title contains a slash (`Expands z to z/star...`). A depth-1 glob
enumerates 140 and silently shrinks the denominator; `outputs()` walks
recursively and a test asserts both nested rows are indexed.

## Scope

`data/phase1/scope.json` covers every Go function in the Phase 1 ledger
packages, built from `data/go-functions.tsv` — the complete inventory, not the
unmapped remainder. That distinction matters: building only from
`status/unmapped-functions.json` hides a *mapped* operation that has no
behavioral witness, which is half of what F0 exists to separate.

| Disposition | Count |
| --- | ---: |
| `covered` | 58 |
| `implemented_untested` | 831 |
| `missing` | 774 |
| `equivalent_rust` | 0 |
| `later_phase` | 15 |
| **total** | **1,678** |

Every disposition carries a `basis` string and a `basis_kind`. All F0 rows are
`basis_kind: "rule"`; none is a review. `equivalent_rust` is deliberately 0 and
cannot be reached by rule — `verify()` rejects an `equivalent_rust` row that is
not marked as reviewed, because the plan requires a behavioral witness for it.

Counts are an audit starting point, not a task list. The rules are conservative
in both directions: a Go name whose snake form is short or generic (`find`,
`identity`) is **not** treated as evidence of a Rust implementation, and where a
package's behavior lives somewhere other than its planned crate the real home is
recorded on the row (`actual_home`) rather than the row being called missing for
the wrong reason.

Missing operations by package, top of the queue:

| Package | Missing | Recorded actual home |
| --- | ---: | --- |
| `internal/tsoptions` | 143 | `tsr_tsoptions` (command-line entry points absent) |
| `internal/core` | 126 | `tsr_core` |
| `internal/collections` | 84 | `tsr_core` and consumer-local collections |
| `internal/module` | 76 | `tsr_module` |
| `internal/tspath` | 56 | `tsr_tspath` |
| `internal/packagejson` | 37 | `tsr_module::package_json` / `package_maps` |
| `internal/bundled` | 27 | `tsr_bundled` |
| `internal/vfs/osvfs` | 22 | `tsr_vfs` |
| `internal/stringutil` | 21 | `tsr_jsstring` |
| `internal/semver` | 19 | `tsr_semver` |
| `internal/vfs/cachedvfs` | 16 | no Rust home at this pin |
| `internal/glob` | 15 | no Rust home; the LSP/test glob is a separate dialect |

## Pilot

The pilot runs real native Go and real production Rust over one request set:

```
match              pilot/matching/include-star-ts
match              pilot/matching/recursive-exclude
not_implemented    pilot/commandline/parse-composite-false
not_implemented    pilot/commandline/parse-build-verbose
not_implemented    pilot/json/marshal-ordered
not_implemented    pilot/locale/select-ja
```

The two matches call `tsr_tsoptions::glob::read_directory` against the pinned
`vfsmatch.ReadDirectory` through an access-only probe. The four
`not_implemented` rows each name the operation, its Go authority, the intended
Rust signature and the production home; the driver does not emulate any of them.
`tsoptions.parseCommandLine` and `parseBuildCommandLine` confirm the plan's gap
2 by execution rather than by inspection: `tsr_tsoptions` has substantial config
parsing but no command-line entry point.

Requests carry their matching dialect, and both the Go probe and the Rust driver
refuse a request whose dialect is not `vfsmatch`, so the configuration matcher
and the `internal/glob` grammar cannot dispatch to each other silently.

## Commands

```sh
export PATH="$(mise where go)/bin:$PATH"

python3 scripts/phase1.py inventory --check
python3 scripts/phase1.py capture --family pilot --output target/phase1/pilot
python3 scripts/phase1.py compare --capture target/phase1/pilot
python3 scripts/phase1.py compare --capture target/phase1/pilot --require-parity   # fails: 4 missing operations
python3 scripts/phase1.py report --captures target/phase1/pilot --output target/phase1/report.json
python3 -m unittest discover -s scripts/tests -p 'test_phase1*.py'
```

`inventory --check` validates the committed manifests, hashes and baseline
mapping against the pin without building anything, and preflights `go` and
`cargo` with an actionable message instead of downloading a toolchain.
`capture` refuses a family with no adapter and names the declared families.
`compare` authenticates every artifact and source input before reading them.

## Implementation queue from F0

Each row names the reproducer, the native authority and the production home.

| Operation | Native authority | Intended Rust home | Owner |
| --- | --- | --- | --- |
| `tsoptions.parseCommandLine` | `commandlineparser.go:ParseCommandLine` | `crates/tsr_tsoptions/src/command_line.rs` (absent) | F3b |
| `tsoptions.parseBuildCommandLine` | `commandlineparser.go:ParseBuildCommandLine` | `crates/tsr_tsoptions/src/command_line.rs` (absent) | F3b |
| `json.marshalOrdered` | `internal/json/json.go:Marshal` | no dedicated home; order-sensitive readers live in consumers | F1b |
| `locale.selectTranslation` | `internal/locale/locale.go` | no Rust home at this pin | F1b |

Reproduce any of them with:

```sh
python3 scripts/phase1.py capture --family pilot --output target/phase1/queue --case pilot/commandline/parse-composite-false
python3 scripts/phase1.py compare --capture target/phase1/queue
```

A selected capture is marked `partial` and its unselected cases report
`not_run`; `--require-parity` rejects such a report, so a bounded smoke can
never stand in for a family gate.

## Producers and sprints

`sprints/P1A.toml` registers the stage A items with pending preparation
metrics. Every item is open and no metric is populated.

The `foundations`, `config` and `syntax` producers are **not** registered in
`status/runs.toml` at F0. The plan registers a producer only once it can
validate its complete declared inventory and report honest failures; only the
`pilot` family has an adapter today. F1a–F5a register them as their family
adapters land, and F5a demonstrates the grouped-producer freshness behavior the
plan describes.

`cargo xtask validate` and `cargo xtask status --check-committed` both pass with
P1A registered, and the tracker still reports 13 sprints with S01–S12 unchanged.
