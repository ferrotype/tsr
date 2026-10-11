# Bounded compiler, scanner and parser differential fuzzing

This is the first whole-program R5 target. It compares the saved **tsrust**
executable directly with the saved **pinned tsgo** executable. It never builds,
uses a testhost compilation RPC, or supplies a compiler result on either
executable's behalf. Named scanner and parser targets use separately built
pinned-package adapters. Coverage-guided instrumentation and API/wasm/LSP replay
remain separate work.

Use explicit paths to binaries already built from the intended commits:

```sh
python3 tools/phase7/fuzz/run.py \
  --rust /absolute/path/to/tsrust --go /absolute/path/to/pinned/tsgo \
  --output /absolute/path/outside/target/new-fuzz-run \
  --seed 42 --iterations 100 --budget-seconds 60 --case-seconds 5 \
  --memory-mb 1024 --minimize-trials 30 --max-failures 10
```

Select `--target scanner` only with explicitly built S05 adapter executables:

```sh
python3 tools/phase7/fuzz/run.py --target scanner \
  --rust /absolute/path/to/scanner-s05 --go /absolute/path/to/pinned-scanner-oracle \
  --output /absolute/path/outside/target/new-scanner-run \
  --seed 42 --iterations 100 --budget-seconds 60 --case-seconds 5
```

Rebuildable sources are [`crates/tsr_scanner/examples/s05.rs`](../../../crates/tsr_scanner/examples/s05.rs)
and [`scripts/s05_oracle`](../../../scripts/s05_oracle); the existing
[`scripts/s05_build.py`](../../../scripts/s05_build.py) exports/builds the pinned
Go adapter. The fuzz command itself does not build or accept an ordinary
compiler as a substitute. It sends one S05 `scan_all` request with target 99,
variant 0, skipped trivia and no source decoding. Generated/mutated inputs are
clipped to 4 KiB; an explicit `--replay main.ts` over that limit is refused.
Each archive retains exact `main.ts` and `request.ndjson` bytes and named limits.
The S05 validators enforce complete begin/observation/EOF/end framing, types,
ordinals and counts. JSON object key order/whitespace is canonicalized; every
ordered token/value/predicate/directive and diagnostic field remains compared.
No S05 expected-panic qualification is applied: every panic is a finding, even
if identical. Unknown/malformed/truncated/trailing records fail closed.

Scanner stdout/stderr files are each capped at 16 MiB plus one overflow sentinel
byte with `RLIMIT_FSIZE`; only the 16 MiB prefix is retained after overflow and
the case fails explicitly. This also limits other regular files written by the
scanner child. Whole-program CLI capture keeps its existing behavior. These
limits bound protocol capture independently of the informational macOS memory
policy. Manifest replay dispatches by the recorded target and refuses changed
scanner limits or a request inconsistent with its saved input/schedule.
The parser target uses the existing S06 Rust example and stream validator with
the narrow [`parser_oracle/main.go`](parser_oracle/main.go) Go wrapper. It calls
the pinned parser and encoder directly, comparing parse/JS/JSDoc diagnostics,
source metadata, node-index order and lookup/cache observations, and encoded
source bytes. Its fixed schedule is parse, index-before, encode, index-after
for `/s06/fixture.ts` in TS mode. It uses the same input/output limits and
manifested replay as the scanner. No factory/codec test cases are claimed.

```sh
cargo build --locked -p tsr_parser --example s06
python3 tools/phase7/fuzz/build_parser.py --output /absolute/path/outside/target/new-parser-build
python3 tools/phase7/fuzz/run.py --target parser \
  --rust target/debug/examples/s06 --go /absolute/path/outside/target/new-parser-build/parser-go \
  --output /absolute/path/outside/target/new-parser-run \
  --seed 42 --iterations 100 --budget-seconds 60 --case-seconds 5
```

`build_parser.py` verifies the pin, exports its production packages to a temporary
directory, injects only the wrapper, and retains a built executable with its
identity. It uses the repository's Go pin and `GOTOOLCHAIN=local`. The unrelated
old S06 builder's missing adapter files are not needed. Preserve executable
copies outside `target` if a run must remain replayable after cleaning Cargo.
Current-build scanner and parser smokes each completed 100 pairs with no
differences; see [the R5 record](../../../docs/PHASE7-R5.md).

The runner alternates generated small programs with byte/token mutations of
sorted `.ts` corpus files (`--corpus DIR`, default: the adjacent `corpus`).
Malformed syntax and UTF-8 are intentional. The initial corpus covers expressions,
generics, narrowing and classes. Inputs are a single `main.ts`, a fixed minimal
`globals.d.ts`, and a fixed tsconfig with strict checking, ES2020/CommonJS,
declaration emit, no libraries or ambient packages, and emit on diagnostics.
This bounded input model does not claim full library/module/project coverage.
The RNG seed fixes the candidate sequence; iteration/budget/failure/minimization
limits can stop that sequence early.

Each pair uses new temporary compiler directories with identical input bytes
and CLI arguments. Complete stdout/stderr bytes are compared after replacing
only the temporary case-root path and CRLF line endings. Message text, related
messages, ordering, and non-diagnostic output remain part of the comparison.
Every emitted relative path and file byte digest must match exactly; missing
outputs fail. Exit statuses are compared; signals, unexpected statuses outside
the compiler's 0/1/2 statuses, and timeouts are findings even on both sides.

`--case-seconds` is the total wall allowance for a pair; each compiler receives
half. A pair starts only when the remaining run budget can cover a complete
case. Process groups are killed after success, crash or timeout, including
lingering descendants. The implementation requires POSIX. Linux enforces a
per-process `RLIMIT_AS` address-space limit inherited by descendants; this is
not an aggregate RSS limit. macOS memory enforcement is explicitly unsupported
and reported as informational in stdout and `run.json`; it is never described
as an enforced memory limit. Failed limit setup is a runner error. No peak-RSS
or coverage measurement is claimed. The budget controls compiler/minimizer work;
startup, hashing, process cleanup and writing retained artifacts add overhead.

The output directory must be new and outside any `target` component. `run.json`
retains the pin, binary paths and SHA-256 identities, limits, seed, corpus hashes,
execution counts and finding inventory. `cases.ndjson` records every completed
pair and its full normalized observations. Each finding retains `original/`
and `minimized/` inputs/options, observations, raw stdout/stderr, and all emitted
bytes. Chunk deletion is bounded by trials and the shared run budget; a candidate
is retained only after both compilers reproduce the original failure categories and termination statuses.
The original is saved before minimization. Findings are not automatically
accepted as parity expectations or promoted into regression fixtures.

Each newly saved original/minimized case has a creation-time `manifest.json`
covering every archived file's digest, input inventory, arguments, binary
identities and limits. `run.json` anchors both manifest hashes. Replay one case
without modifying the archive:

```sh
python3 tools/phase7/fuzz/replay.py \
  --case /absolute/path/new-fuzz-run/failure-000000/minimized \
  --output /absolute/path/outside/target/new-replay \
  --attempts 3 --budget-seconds 30
```

Replay verifies the manifest, all archived bytes and both saved executable
identities before running. It uses the archived input/configuration bytes and
arguments, independently of current generator defaults, through the same
bounded child-process runner. Supply `--run-sha256 SHA256` to check an
independently retained `run.json` digest. These are integrity checks against
that anchor, not an authenticity or build-provenance certificate. Historical
archives created before manifests existed are refused; they remain unchanged.

For a fixed Rust build, explicitly add `--candidate-rust /absolute/path/tsrust`.
The original archive and saved Go executable remain verified. The saved Rust
executable may then be replaced or unavailable: original and candidate Rust
identities are recorded separately, and the report labels this candidate mode.
A reproduced candidate observation does not claim replay of the original Rust
binary. Default replay still refuses changed executable bytes.

Every fresh attempt retains raw outputs, emitted files, manifests and full
normalized observations. `replay.json` records each attempt and the aggregate:

- Exit 1, `reproduced`: identical failure categories, termination, diagnostics
  and emitted observations occurred against the selected pair.
- Exit 0, `resolved`: the pair agreed on every attempted replay. This is an
  observation for those attempts, not proof of a fix for nondeterministic input.
- Exit 2, `execution-failure`: integrity failure, insufficient budget, launch
  error or a changed failure/observation. `changed_observations` names changed fields; retained results expose their bytes.

An execution failure takes precedence; otherwise any reproduced attempt keeps
the aggregate reproduced. Alternating Go or Rust outcomes remain visible.
Existing output directories are refused, and the source archive is read only.
The older `run.py --replay main.ts` mode merely selects source for a new fuzz
run; it does not verify an archive or preserve its recorded options. This also
applies when selecting the scanner target.

The fuzz runner exits 1 for findings, 0 when one or more completed pairs agree,
and nonzero for runner errors or zero completed pairs. A zero-execution or
budget-stopped run is not evidence of coverage or a parity pass; inspect the
retained execution count. After a real finding, preserve its fixture after the
fix and add a regression to the compiler/transpile parity or unit-test
machinery that owns the behavior.

Fast runner controls (no real compiler execution):

```sh
python3 -m unittest scripts.tests.test_phase7_fuzz
```

Static refusal review inventory (no compiler execution):

```sh
python3 tools/phase7/fuzz/audit.py --output /absolute/path/outside/target/new-inventory.json
```

This inventories lexical `Unsupported` and `not available` matches in regular
`crates/**/*.rs` files, including comments, tests and generated sources. Entries
retain exact file/line/text and a source fingerprint. It makes no semantic,
reachability, approval or pass-metric claim. See [the current R5 record](../../../docs/PHASE7-R5.md)
for retained smoke/replay evidence and remaining work.
