# Working in this repository

Rust implementation guidance lives in [AGENTS.md](AGENTS.md) and the guide it
points at. This file is only about running things: the commands below are the
ones that are easy to get wrong from a cold start, with the environment facts
each one needs.

## The Go toolchain is not on PATH

`go` is installed through mise and no Claude shell sees it. Export it first:

```sh
export PATH="/Users/cristian/.local/share/mise/installs/go/1.27.1/bin:$PATH"   # or: $(mise where go)/bin
```

Without it, `python3 -m pytest scripts/tests` reports ten failures that are not
regressions: nine errors in `test_inventory.py` and one in
`test_s07_benchmark_build.py`, all `FileNotFoundError: 'go'`. Everything that
runs a Go overlay, oracle or census needs the export too. Check for it before
concluding that a producer is broken.

## Test and check commands

```sh
cargo test -p s08_relater_prototype        # underscores; the hyphenated name is rejected
python3 -m pytest scripts/tests -q         # ~445 tests, including the relater contracts; needs go on PATH
python3 scripts/checks.py fmt              # wraps cargo fmt --all --check
python3 scripts/checks.py clippy           # the workspace lint gate
```

The clippy gate is `cargo clippy --workspace --all-targets --all-features
--locked -- -D warnings`. `clippy::pedantic` is warn-level in `Cargo.toml`, so
the gate's `-D warnings` is what turns an unused `self` or a `match` on one
variant into a failure. Dropping `--all-features` gives a faster inner loop but
compiles less code, so re-run the real command before committing.

Note that `cargo clippy --fix` rewrites `use super::*` and breaks test modules
that relied on the parent glob. Add the explicit imports to `mod tests` instead.

## Comparing the relater implementations

The comparison is over the 21 frozen fixtures in
`data/s08/relater-fixtures.json`, five relation modes each, so 105 groups per
implementation. Point `--output` at a scratch directory: the default
`target/s08/relater` is the official capture directory and the owner's capture
lives there.

```sh
python3 scripts/s08_relater.py build  --output "$SCRATCH/relater-parity"
python3 scripts/s08_relater.py parity --output "$SCRATCH/relater-parity"
```

`parity` prints `matched`, `behavior_matched`, `unsupported` and
`all_cases_match` for both `id` and `reference`. Both must reach 105/105 with
`all_cases_match: true`, because the report derives `same_work` from that and
withholds every ratio metric when it is false. `build` compiles the
`p7_relater` example in both feature sets, which takes a few minutes.

To exercise one program that is not in the frozen inventory, clone a frozen
request and replace `source_hex`; `scripts/s08_relater.py` reads requests only
from the frozen inventory, so ad-hoc programs are a development check and never
evidence.

## Comparing the formatter and printer with Go

`scripts/s09_format.py compare` builds a native Go oracle and the Rust harness,
sends both the same requests, one per distinct parser input of the frozen
inventory (16,120), and reports how many agree on every requested operation.
Point `--output` at a scratch directory; it needs go on PATH.

```sh
python3 scripts/s09_format.py compare --ops position,insert,format,entry --output "$SCRATCH/fmt"   # about 2 minutes
python3 scripts/s09_format.py compare --ops indent --output "$SCRATCH/fmt"                          # about 6 minutes
python3 scripts/s09_format.py compare --ops nav --limit 2000 --output "$SCRATCH/fmt"                # a diagnostic subset
```

Disagreeing inputs land in `failures.ndjson` with both observations. Changing
`tools/s09/format_oracle/*.go` or the script makes `data/s09/format-probes.json`
stale: `freeze` rewrites it and `verify` must reproduce it, about ten minutes
each. `fixtures --freeze` rewrites the insertion rows the `tsr_api` tests read.
A comparison that reports full parity on its first run has to be mutation
checked before it is believed.

## Branch names

Name a branch after the work, for example `s09-ownership`. Do not prefix it with
`claude/` or any other agent name.

## Things to ask about first

- `cargo xtask run <id>` and `cargo xtask status --record` rewrite tracked
  evidence, `STATUS.md`, `status.json` and `docs/status.html`.
- The quiet-host measurement captures are the owner's to run.
- Do not stage `PLAN.md`, `.playwright-mcp/` or `four-robots.png`.

## S07 chain order

`scripts/s07_benchmark_graph.py capture` (the bindworkload producer) rebuilds the
shared binaries and rewrites `target/s07-bindworkload/report.json`, which
invalidates an existing benchmark capture. Run it before
`scripts/s07_benchmark.py capture`, then `cargo xtask run e5` and `e6`.
