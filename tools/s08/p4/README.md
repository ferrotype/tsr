# P4 full-corpus diagnostic inventory

This driver runs the frozen 9,369 acceptance variants without selecting for
currently supported checker operations. `--tier all` also runs the 1,359
informational variants in their separate tier. There are 1,270 requested
acceptance declaration phases (1,459 across both tiers).

This is a development failure inventory, **not E2 evidence**. It records
Program loading and its ordered input graph, config/program/parser diagnostics,
each source's production semantic diagnostics, global diagnostics, and the exact
reason for every unavailable operation. Bind diagnostics are retained
separately for attribution. `Program::semantic_diagnostics_with_checker` applies
native source selection, bind/check diagnostic composition, plain-JS filtering,
source directives and include diagnostics. The semantic phase records its API;
earlier captures without that marker used raw checker diagnostics. Successful
execution still does not establish baseline decoration, query schedule or
semantic parity. Requested declaration diagnostics now run through the production
declaration transformer and checker emit resolver, and requested suggestion
diagnostics run through the production suggestion entry point. The P5 native
type/symbol baseline walker remains `not_implemented`. No missing phase passes
through an empty baseline.

Use existing authenticated S07 loading requests; the runner checks the complete
10,728-row inventory and every request hash against
`data/s08/baseline-requests.json`. The default cache is
`target/s07-subset/review/loading-requests.candidate.json`; a different cache can
be supplied with `--loading-requests`. Missing caches need the existing pinned
S07 input extraction (`scripts/s07_subset.py`); this driver never starts a Go
capture or substitutes reconstructed inputs. Do not change options to get a
case through the checker.

```sh
python3 scripts/s08_p4.py build --output target/s08/p4-build-01
python3 scripts/s08_p4.py run --build-record target/s08/p4-build-01/build.json --output target/s08/p4-inventory-01
python3 scripts/s08_p4.py replay target/s08/p4-inventory-01
```

For a bounded protocol trial, repeat `--case` with exact frozen IDs. Such a
capture retains its selected request list and cannot claim the full denominator.
For informational cases use `--tier informational` or `--tier all`.

Each variant runs in a fresh process with a configurable positive finite
`--timeout` (60 seconds by default). Raw stdout, stderr and any observation are
kept per ordinal. A timeout, process failure or malformed payload becomes a
named fatal case record; it does not stop the remaining corpus. This process
isolation deliberately makes these runs unsuitable for performance claims.

`--resume` accepts only the identical executable, build-source fingerprint,
frozen inputs, tier, selected cases and timeout. Completed records are verified
against their raw artifacts. An interrupted attempt is retained under a separate
name before retry. `replay --allow-partial` writes `partial-report.json` and
cannot report completion. A new checker implementation requires a new capture. An interrupted capture
with an authenticated source snapshot can resume its original executable after
workspace edits; the existing capture metadata and both snapshots must still
verify unchanged.
The producer copies the compiler-artifact executable into the capture directory,
so later builds cannot replace the binary being run. Source changes during a
long run are recorded as `source_stable: false`; the binary's captured build
identity remains available. Every new build snapshots all fingerprinted source
files before compilation; the run copies that authenticated snapshot alongside
the executable. Replay checks every snapshot hash and rejects extra or missing
files, preserving the exact implementation behind a long-running inventory.

Failures are grouped by operation, failure class and exact reason, retaining
all affected variant IDs and occurrence counts. Informational outcomes do not
enter acceptance counts. Completed diagnostics are not called passes; no E2
metric is emitted by this tool.

## Review regression fixtures

`review-regressions.json` covers circular default initializers, nested alias
resolution and exported-name display, try/finally reachability, 10,000 sequential
assignments, the pinned Date lib suggestion, and ordinary declarations in TSX
files. Its native comparison is separate from the full inventory and E2.

```sh
python3 scripts/s08_p2.py capture --spec tools/s08/p4/review-regressions.json --output target/s08/p4-review-native
cargo run -p ts_compiler --example p2_checker -- target/s08/p4-review-native/requests.json target/s08/p4-review-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p4-review-native --actual target/s08/p4-review-rust.json --output target/s08/p4-review-comparison.json
```

Use fresh output paths. `cargo run` selects the current compiler artifact; the
comparison checks the native requests and every recorded observation rather
than accepting a successful process exit alone. The compiler tests separately
keep a real JSX expression as an explicit repeated failure.

`review-followups.json` uses the same capture/compare commands for DOM
intersections, assignment-backed CommonJS exports/re-exports, and absent rest
arguments. Use separate native, Rust and comparison output paths. It does not
change the frozen full-corpus requests or their options.
