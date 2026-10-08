# Profiling tsrust against tsgo, October 2026

A second profiling pass over every phase, after Phase 6: where the Rust
port spends its time compared with the pinned Go compiler, what was changed
on the strength of those profiles, and what remains. The measurements here
are development captures on the owner's host (macOS arm64, 18 CPUs) taken
while other work ran; they rank causes and show the direction of each
change, and are not `status/perf` recordings.

## Method

Every comparison drives both compilers with the same input from the same
client:

- **Checker**: the sixty heaviest variants of the checker benchmark corpus
  (`p7_checkerbench` normal mode, single-threaded, against
  `go-checkerbench.test` with `TS_TEST_PROGRAM_SINGLE_THREADED=1`), sampled
  with `samply` at 4 kHz; self time attributed per function and per phase
  with a small reader of samply's profile format.
- **API server**: the pinned client's own sync API (`src/api/sync/api.ts`
  under `--conditions @typescript/source`) opening the bench project and
  repeating one request forty to eighty times in one server: the batched
  `getSymbolAtLocation` over the 10,160 identifiers of `program.ts`, and
  `getSymbolAtPosition` over the same positions. The Rust server was sampled
  with macOS `sample` (samply's stdout breaks the msgpack channel).
- **LSP**: the Phase 5 latency harness's smoke pair on the TypeScript
  proposal fixture (first diagnostics, completion, hover, references, rename),
  the Rust server sampled with `samply`.
- **Command line**: `tsc -p` on a copy of the same fixture with a type error
  injected, build info deleted before every run, `--extendedDiagnostics`,
  both compilers `--singleThreaded` for a like-for-like total and once with
  default threading.

## Baseline (before the changes)

| Measure | Go | Rust | Rust / Go |
| --- | ---: | ---: | ---: |
| Checker benchmark, normal mode, median of the corpus | 6.12 s | 13.10 s | 2.14 |
| of which check phase | 2.48 s | 7.95 s | 3.2 |
| Checker top-60 variants, single run | 2.31 s | 5.32 s | 2.30 |
| API `getSymbolAtLocation` x10,160 batched | 17.1 ms | 83.9 ms | 4.9 |
| API `getSymbolAtPosition` x10,160 | 40.3 ms | 89.3 ms | 2.2 |
| API project load (warm) | 45 ms | 190 ms | 4.2 |
| LSP first diagnostics | 21.7 ms | 110.8 ms | 5.1 |
| LSP completion | 1.5 ms | 4.3 ms | 2.9 |
| LSP references | 9.5 ms | 17.6 ms | 1.8 |
| Parse + bind benchmark, one thread | | | 1.15 |
| Parse + bind benchmark, eight threads | | | 1.38 |

The checker ratio is uniform across variant sizes (about 2.1 at every
bucket), so it is structural rather than a few pathological inputs.

## What the profiles said

**API requests were dominated by JSON handling, not by the checker.** In the
batched request the decoder read every byte through a refilling reader
(`Input::at` with 512-byte chunks over a `Cursor`), every string value was
decoded into its own allocation even when only skipped, and the response was
encoded three times: once by the handler, once more token by token when the
committed raw value was re-validated inside the response envelope
(`Encoder::write_value` decoded and re-emitted every token), and the string
encoder walked rune by rune. The checker's own work (`getSymbolAtLocation`
plus the symbol responses) was about a third of the request.

**LSP first diagnostics was dominated by loading, and loading by threads.**
The timeline showed the first 160 ms of the request parsing and binding the
fixture's 286 files one after another, each parse and each bind on its own
one-shot 256 MiB-stack thread (278 thread creations), then about 20 ms of
checking. The pin parses every queued file on its own goroutine and binds in
a parallel work group.

**The checker's extra time is spread, with a few concentrations.** In the
check phase of the top-60 variants: node access (the arena directory, slot
validation and header decode behind `CheckerState::node`, `AstView::node`,
`ProgramContext::ast`) about 20% of self time; spelling suggestions for
unresolved names (`Hooks::suggest_lookup`) 11.8%, almost all of it copying
every candidate symbol's name into an owned string before the length filter
and running the distance; `memmove` 5.9%, mostly those name copies and the
member vectors of `resolve_object_type_members`; and in the whole profile
`get_source_file_of_node` 5.9% inclusive, a parent walk per call where the
file root is one field of the arena, called 958 times from the numeric
literal grammar check alone.

**Parse and bind are close to the pin per thread**; the eight-thread gap is
scheduling, not parsing.

## Changes

1. **`tsr_json` decoding** (`wire.rs`, `decoder.rs`, `decode.rs`): a
   complete in-memory input is buffered once (`Input::from_slice`, no
   refills, no compaction), `at` is an inlined bounds check with a cold
   refill path, string bodies are copied as runs of plain bytes, a value
   being skipped is validated without decoding its strings (member names are
   still decoded for the duplicate check), and the pointer prefix of a raw
   value read is built only on error. `unmarshal`, the LSP codec and the
   incremental JSON reader use the slice input.
2. **`tsr_json` encoding** (`encoder.rs`): a raw value the encoder would
   reproduce byte for byte (no whitespace between tokens, no `\/` or `\u`
   escapes, no invalid UTF-8) is validated with the non-decoding scan and
   copied verbatim; other raw values take the re-encoding path as before. A
   `Committed` API response, produced by this encoder, is written through
   `write_encoded_value` without a second scan. String encoding copies ASCII
   runs whole.
3. **Parse-ahead in the program loader** (`crates/tsr_compiler/src/preload.rs`,
   `loader.rs`, `cache.rs`): the loader still walks its tasks on one thread,
   because resolution mutates its state, but it now runs on one reserved-stack
   parser worker (so the files it parses itself parse inline) and hands every
   task it has queued but not reached to a pool of parser workers
   (`tsr_parser::spawn_parser_thread`), in the order it will pop them. A
   worker reads, parses and binds exactly as `FileCache::load` would, with
   the same parse options (`Loader::source_file_parse_options`) and, for a
   project cache, the same lease; the loader takes a finished file, waits for
   one in flight, and withdraws a queued one to parse it itself. Redirects,
   content-mapped and unsupported files, build-shared declaration files and
   files with a local reuse candidate stay with the loop. Single-threaded
   programs, traced loads and `traceResolution` keep the sequential order.
4. **Spelling suggestions** (`name_resolution.rs`, `tsr_scanner::spelling`):
   candidate names are borrowed, candidates outside the search's length
   window are not collected at all (the search would never measure them;
   alias flags are still resolved for every candidate, as the pin's
   `getCandidateName` resolves them), and the rune buffers are reused.
5. **`get_source_file_of_node`**: a valid node of a bound arena whose
   binding is its only source answers that binding's source file
   (`AstView::single_source_root`); every other node walks as before. An
   unbound view carries no such guarantee: a builder may hold several
   logical source files, or a fragment whose root is no source file.
6. **`ProgramContext`'s arena directory cache** widened from one entry to
   sixty-four direct-mapped entries: a check alternates between the file
   being checked and the libraries its names resolve into.
7. **API node handles**: the file through `get_source_file_of_node` (the
   shortcut above for the checker's files), the path copied once without a
   lossy conversion.
8. **AST navigation** (`tsr_astnav`): the per-child JSDoc single-comment
   filter is evaluated only under a single-comment JSDoc parent, saving two
   node reads per child on every descent.

## Results

Development captures on the same host with other work paused; each
API number is the median of twenty requests in one server, the LSP numbers
one smoke pair, the command line the median of three fresh runs.

| Measure | Go | Rust before | Rust after | Rust / Go |
| --- | ---: | ---: | ---: | ---: |
| Checker top-60 variants, single run | 2.31 s | 5.32 s | 4.43 s | 1.9 |
| API `getSymbolAtLocation` x10,160 batched | 18.1 ms | 83.9 ms | 36.1 ms | 2.0 |
| API `getSymbolAtPosition` x10,160 | 41.6 ms | 89.3 ms | 71.5 ms | 1.7 |
| API project load (warm) | 45 ms | 190 ms | 65 ms | 1.4 |
| LSP first diagnostics | 21.3 ms | 110.8 ms | 29.4 ms | 1.4 |
| LSP completion | 1.6 ms | 4.3 ms | 4.1 ms | 2.6 |
| LSP hover | 0.3 ms | 0.7 ms | 0.5 ms | 1.7 |
| LSP references | 9.4 ms | 17.6 ms | 14.8 ms | 1.6 |
| LSP rename | 0.3 ms | 0.5 ms | 0.4 ms | 1.3 |
| `tsc -p` fixture, `--singleThreaded`, total | 0.46 s | | 1.04 s | 2.3 |
| of which check | 0.41 s | | 0.95 s | 2.3 |
| `tsc -p` fixture, default threading, total | 0.33 s | | 0.61 s | 1.9 |

The checker top-60 number is the median of three runs with the same `outputs_sha256` as before (f178c10d…). The command line had no earlier capture; its single-threaded check ratio
is the checker's structural gap, and its default-threading total shows the
loader's parse-ahead and the parallel check at work on both sides. The
batched API request is now about a third JSON (decode of 10,160 handles,
encode of 10,160 responses), a third checker query and a third symbol
response construction (registry, node handles); the pin's whole request
costs what our checker query alone costs.

## Second pass: the node-access levers measured

The structural estimate above ("another quarter off the check phase") was
checked the same day, on the top-60 variants, two ways: a counting build
(`tsr_ast`'s `access-stats` feature, read per phase by the benchmark's phase
clocks) sized the reads, and one build per candidate change measured its
effect. The machine was busy, so every timing is a concurrent pair: the
baseline and the variant started at the same moment, six pairs per variant,
scored by the per-pair ratio (spread about one percent).

**What the check phase reads** (195 million node reads):

| Reads | Count | Share |
| --- | ---: | ---: |
| serving only a kind check | 92M | 47% |
| serving nothing (validation only) | 9M | 5% |
| serving a parent walk | 43M | 22% |
| decoding node data | 46M | 23% |

With node access at about 21% of the diagnostics pass, one read costs about
4 ns: the cost of a few predictable branches and a bounds-checked load, not
of the page directory.

**What each lever is worth** (ratio to the baseline run):

| Change | Measured | Kept |
| --- | ---: | --- |
| sealed flat arena pages (one vector per completed file) | 0.986 | yes |
| benchmark walker resolving views through the node directory (harness only) | 0.844 (top-60), 0.505 (small variants) | yes |
| one file view per parent walk in the three hot walks | 0.894 (deep variants), neutral elsewhere | yes |
| directory hit carrying the direct-read flag, symbol lookup without a view | 1.013 | no |
| shared declared signature lists, no inherited-name copies, allocation-free lower-casing | 1.005 | no |
| kind bits in `NodeId` | not built; the 92M kind-only reads at 4 ns put it at 8–10% of the check phase | |
| same-owner parent reads | not built; the routing share of 43M reads puts it near 1% | |

So the levers as a set are worth about a tenth of the check phase, not a
quarter: the single-threaded `tsc` ratio would move from 2.3x to about 2.1x.
Even node access made free (the pin's pointer read) would take the ratio to
about 1.85x. The rest of the checker's gap is spread across the algorithmic
areas themselves (flow analysis, name resolution, the relater), where the
port runs the same algorithm with slower operations: `Result` on every
accessor, arena-indexed types and symbols, hashed names. Sizing those needs
a different method, a function-by-function comparison of inclusive time
against the pin's profile on the same workload, which is the next step.

The sealed flat arena is kept: `Arena::seal` moves the pages into one vector
when a parse completes (`AstBuilder::complete`), so a completed file's slot
read is one bounds check and one offset; an open arena keeps its pages. The
counting feature and the phase-split counters stay as development tooling.

### The benchmark's own view lookups

Profiling the top-60 by variant group (the two deep-expression variants, the
two flow-heavy ones, the eight small `nodeModules` ones repeated ten times)
showed the routed view path (`for_node_owner`, `for_arena`,
`owner_retention`, `CompletedFile::view`) at 7 to 38% of each group. Its
callers were not the checker but the benchmark walker's `baseline::ast`,
which scanned every file of the program for the owner of each node it
visited and built a routed view each time. The pin's walker reads AST
pointers, so this inflated every Rust checker number, most of all the small
multi-file variants. `ProgramFile::shared_view` now caches the file's shared
form, and the walker resolves a node through the program's node directory.
In concurrent pairs the fix alone is 0.844 of the previous run on the top-60
and 0.505 on the small group; the recorded S08 checker ratios include that
overhead and should be re-recorded from this harness.

### What the groups show after the fix

| Group | Rust / Go before | Where the Rust time goes now |
| --- | ---: | --- |
| deep expressions | 4.7x and 3.5x | parent walks per identifier (`control_flow_container`, `in_ambient_or_type_node`) at 4 ns a step where the pin pays under 1 ns; node access 42% of the group |
| flow-heavy | 2.3x and 2.2x | the flow walk itself: node reads inside `type_at_flow` and `matching_reference_worker`, name resolution for unresolved names |
| small multi-file | 7x | harness digests and paths, checker creation (`merge_global_symbol`), file-cache compares, type display; no single item |

Hoisting one file view out of the three hot parent walks
(`in_ambient_or_type_node`, `control_flow_container_or_none`,
`flow_this_type_query_worker`) is 0.894 on the deep group and neutral
elsewhere; it is kept. Comparing accessed property names through a stack
buffer instead of a `JsString` per comparison measured 1.008 on the flow
group and was dropped.

## Third pass: the flow reference, and parse-bind at eight threads

### The flow reference's shape

`type_at_flow` compared its reference against every assignment, container
and mutation target it met through `matching_reference_worker`, which peeled
the reference's parentheses, non-null assertions and comma chains and
re-read its access names on every comparison. `ReferenceShape` now captures
the reference once per walk (its access steps with their property names and
element identifiers, and its root: a resolved identifier symbol with its
export form, `this`, `super`, a meta property or a this-type query), and
`matching_shape` walks only the target side; the containment test runs over
the same steps. Concurrent pairs against the previous head, outputs hash
unchanged:

| Group | Ratio |
| --- | ---: |
| flow-heavy variants | 0.718 |
| deep-expression variants | 0.777 |
| top-60 | 0.900 |

### Parse and bind at eight threads

The recorded parse-bind ratios are 1.15 at one worker and 1.40 at eight
(`status/perf/parse-bind`, 2026-09-19). The question was why the port
scales worse than the pin. Measured on the quiet host, runs alternating and
standalone:

| Workers | Go | Rust | Ratio |
| --- | ---: | ---: | ---: |
| 1 | 3.08–3.30 s | 3.42 s | 1.07 |
| 8 | 0.59–0.61 s | 0.75–0.76 s | 1.26 |

Two things moved the one-worker number since the recording. The sealed
arena of the second pass makes parse-bind faster, not slower: the binder
reads the sealed file, and the copy is cheaper than the page directory it
removes (the unsealed variant is 1.061 at one worker and 1.063 at eight).
The scanner's buffered diagnostics were drained after every scanner
operation, constructing and dropping a `Drain` per token (2.4% of the
worker's self time); skipping the empty buffer is 0.990.

To attribute the eight-worker ratio, a copy of each harness recorded the
parse-and-bind time of every file (the pin's bridge in Go, ours in Rust;
nothing else changed), and a simulation of the harness's schedule from
those costs reproduces the measured Rust wall within half a percent (the
harness has no other work). The pieces:

- **The per-file cost ratio does not change with the thread count.** Rust
  spends 3.40 s inside parse and bind at one worker against Go's 2.59–2.70 s
  (1.27–1.31), and 3.79–3.91 s against 2.89–3.03 s at eight (1.28–1.35).
  Both inflate by about 1.12 at eight threads, uniformly across workers;
  the unsealed variant inflates the same (so not memory bandwidth) and
  mimalloc's purge, commit and reserve settings measure 0.995–1.004 (so not
  the allocator).
- **The pin's one-worker wall hides its collector.** 0.48–0.60 s of Go's
  one-worker wall (16–18%) is background marking between files on the
  single P, outside its parse and bind time. At eight workers that share is
  0.04–0.05 s (7–8%): the marking runs on the Ps the schedule leaves idle.
  The port has no such background work, so its wall is its per-file cost
  and nothing else.
- **The harness schedule costs both sides the same.** Files go to workers
  round-robin (`index % workers`) through channels of capacity `workers`,
  in both harnesses by construction. The static assignment leaves worker 6
  with the 8.4 MB file (0.58–0.60 s of its 3.9); the capacity-8 channel
  blocks the sender on the slowest worker and the others run dry. Measured:
  perfect division of the Rust eight-worker cost would be 0.49 s, the
  static-assignment bound 0.598 s (the harness with an unbounded channel
  lands exactly there), the gated harness 0.752 s. Workers are idle 36% of
  the time in Rust and 33% in Go.

So the eight-thread ratio is the per-file cost ratio with the pin's
collector out of the way, and the one-thread ratio flatters the port by the
pin's serialized collector. There is no scaling defect in the port; the
lever is the per-file cost, and the profiles (one worker, samply, about one
sample a millisecond) put it in one place:

| Phase | Go | Rust | Ratio |
| --- | ---: | ---: | ---: |
| parse (with the scanner) | 1,920 | 2,086 | 1.09 |
| bind | 735 | 1,335 | 1.82 |

The pin's parse share includes its allocation (`mallocgc` 9%, `growslice`
7% of its worker) and its marking runs between files (16%); the port's
parse share includes the parent fix-up walk over stored children
(`override_parent_in_immediate_children`, 8%) and the allocator's slow
path (about 5%). The binder is the gap: the port's bind walk reads every
node through the view (`node_kind`, `node_flags`, `AstView::node`, the
`BindRead` decoders: about 15% of its bind time), interns names and inserts
symbols through hashed tables (`NamePool::intern_hashed`, the rehashes,
`SymbolTableMut::insert`, `SymbolTableRead::get`: about 10%), validates the
symbol graph (2%) and encodes flow references; the pin reads fields through
pointers and its tables are Go maps (`mapaccess`, `mapassign`: 8% of its
worker). A binder pass with the same method as the checker's (profile, then
one change per concurrent pair) is the next parse-bind step; kind bits in
`NodeId` (`docs/design/node-kind-bits.md`) would also take the binder's
kind reads.

## Binder name-path screens, 8 October

Three bounded candidates were built against `81cd7724` (#115), each in
isolation. The initial decision to remove all three used sequential pairs,
contrary to this profiling series' concurrent method. That rejection was
premature. The corrected concurrent measurements below support retaining
the fresh-symbol constructor; the other two candidates remain removed.

1. **Reuse the declaration-name hash.** Carry the lookup hash into the
   insertion of a new or replacement symbol, using the existing prehashed
   table API. Symbol construction still interns the name separately.
2. **Construct fresh symbols directly.** Avoid the generic populated-symbol
   replacement path for `Symbol::new`: six reference fields are known empty,
   so their escape maintenance and encoding are unnecessary. Retain name
   encoding, including its wide-index escape, and creation tracing.
3. **Insert using the symbol's interned name.** Resolve the already-stored
   symbol name inside its owning store and insert that private name identity
   into its table. This removes a second name-pool lookup and the temporary
   `JsString` clone. It retains checked symbol/table identities and the same
   hash-table insertion and promotion rules.

The initial screens used the normal release `ts-bench` binary (no allocation or
profiling feature), the existing 13,094-file S07 workload, and six
alternating **sequential** control/candidate pairs per worker mode after one
warmup each. Builds and correctness checks ran outside the timed batches.
These are Rust-versus-Rust development screens on the 18-CPU macOS host,
not new Go-relative measurements or acceptance captures. Background host
load remained present; one-minute load averages across the first three
screens ranged from about 4.65 to 10.17.

Ratios below are the median of the paired candidate/control ratios; times
are the independently computed medians, in seconds. Lower is better.

| Candidate | Workers | Control | Candidate | Paired ratio |
| --- | ---: | ---: | ---: | ---: |
| Hash reuse | 1 | 3.5227 | 3.5096 | 0.9975 |
| Hash reuse | 8 | 0.8354 | 0.8354 | 1.0092 |
| Fresh-symbol constructor | 1 | 3.4258 | 3.4518 | 1.0095 |
| Fresh-symbol constructor | 8 | 0.7571 | 0.7487 | 0.9935 |
| Interned-name insertion | 1 | 3.4159 | 3.4149 | 0.9976 |
| Interned-name insertion | 8 | 0.8279 | 0.7917 | 0.9566 |
| Interned-name insertion, confirmation | 1 | 3.3669 | 3.3531 | 0.9961 |
| Interned-name insertion, confirmation | 8 | 0.7306 | 0.7304 | 0.9981 |

Within this sequential method, the last candidate's initial eight-worker
result warranted one confirmation
batch using the **same binaries**. Its apparent 4.3% gain did not reproduce;
the confirmation was effectively neutral. All median RSS differences were
below 0.01%; allocation traffic was not instrumented. No memory improvement
is claimed. These sequential screens remain recorded, but do not decide
retention against the concurrent methodology.

### Corrected concurrent comparison

The same saved baseline and candidate binaries were rerun using the direct
process-launch method in Claude's `pairb.py`: start both executables before
waiting for either, alternate launch order, then take the median of the
within-pair `wall_time_ns` ratios. Each mode had six measured pairs after
one warmup pair. The fresh-symbol candidate received a second batch with
the same method and binaries. Binary hashes were checked before and after
each batch. No build or correctness run from this task overlapped timing.

Both processes used the same workload and worker count, so the eight-worker
pair used sixteen workers on the eighteen-CPU host. The launch skew was
below 1.8 ms in every measured pair. This matches the existing launch
method; it does not add a barrier between the binaries' measurement
intervals. Startup, preload and worker-setup fields are retained alongside
the full reports, exit statuses and stderr. Every pair passed the same
input/count checks as the initial screens.

| Candidate | One-worker paired ratio | Eight-worker paired ratio |
| --- | ---: | ---: |
| Hash reuse | 1.0020 | 1.0032 |
| Fresh-symbol constructor | 0.9926 | 0.9856 |
| Interned-name insertion | 0.9976 | 0.9985 |
| Fresh-symbol constructor, confirmation | 0.9895 | 0.9922 |

The fresh-symbol constructor was faster in all twelve measured one-worker
pairs and eleven of twelve eight-worker pairs. Its median improvement was
0.7–1.4% across the four mode/batch combinations. Retain this small,
repeatable improvement: the change skips work for fields known empty at
allocation, without changing storage or the general populated-symbol path.
Median RSS differences were below 0.01%; allocation traffic was not
instrumented. This is a development result on this host, not a new
Go-relative or acceptance measurement.

Hash reuse showed no benefit. Interned-name insertion's approximately
0.2% medians included pairs in both directions; those results do not
justify its additional insertion API. Neither candidate is retained.

For future candidates in this profiling series, preserve the concurrent
method: save the baseline before editing, build with identical features,
start both direct binaries before waiting, alternate launch order, verify
outputs before scoring within-pair ratios, and confirm a proposed win with
the same method. Do not silently replace concurrent pairs with sequential
runs or compare medians from different batches.

Every timed child completed successfully with the same loaded-input digest,
19,593,488 nodes, 2,459,867 symbols, 423 parse diagnostics and 5,250 bind
diagnostics. Counts alone are not graph parity: the fresh-symbol and
interned-name candidates also matched complete graph reports for 67 files
(a strided workload sample plus its two largest files, 960,847 nodes) in
both worker modes, including the raw graph digests and diagnostics.

Targeted regressions exercised merge, present-null, replacement,
early-return, duplicate and missing-name declaration paths through local
and checked backends. The AST candidates additionally exercised empty and
arbitrary-byte names, reference escapes or invalid-owner rejection, and
creation-trace/runtime-identity behavior. The retained fresh-symbol
constructor passed 190 AST and 51 binder unit tests with creation tracing
enabled. Its declaration/default-state regressions are retained with it.
After restoring the exact measured patch, both crates' all-target clippy
checks passed with warnings denied, as did formatting and ledger
validation. Eight focused compiler variants passed all 70 subtests against
the pinned baselines; no parity expectation changed.
No full compiler corpus or Go recapture was run.

Local reproductions, exact patches, build/binary identities, individual
samples, stderr and graph reports are preserved under
`target/perf-binder-names-20261008/`; they are not committed artifacts.
`build.py` builds and copies a candidate. `screen.py` is the initial
sequential runner; `concurrent_screen.py` is the corrected runner. The
concurrent results live in `concurrent-screen/` and
`concurrent-fresh-symbol-confirm/`. The original candidate patches are
`hash-reuse.patch`, `fresh-symbol.patch` and `interned-name-final.patch`.
All start from the same baseline revision, rather than stacking the
unretained changes. The sequential interned-name confirmation and
concurrent fresh-symbol confirmation each reuse their candidate's exact
binary.

These results do not show that the whole sampled name/table group is free
or that binder overhead is solved. The broader node-access cost remains a
separate experiment.

## What remains

- **The binder** is 1.8x the pin's on the parse-bind workload where the
  parser is 1.09x: node reads through the view, hashed name and symbol
  tables, the symbol-graph validation. The one lever left in parse and bind.
- **Node access in the checker.** Kind bits in `NodeId` are the one
  structural lever with a real return (about a tenth of the check phase,
  and the binder's kind reads); the design note is
  `docs/design/node-kind-bits.md`. Flat pages are done, the directory is
  not the cost, parent reads are about one percent.
- **The checker's remaining gap** after the flow work is spread across name
  resolution, the relater and the deep-expression walks; the function-level
  comparison against the pin's profile is the method.
- **The parse-bind harness** measures both sides under the same static
  schedule; its idle time (a third of the workers' time at eight threads)
  is the benchmark's, not the port's, and lets the pin's collector run for
  free. Worth knowing when reading the eight-thread ratio.
- **Navigation.** `getSymbolAtPosition`, completion, hover and references
  descend the tree through the general view routing (`for_node_owner`,
  `owner_retention`, `owning_source`) and allocate a vector of children per
  level; a navigator that reads each child once through the file's direct
  view is the next LSP step.
- **Symbol responses.** Registering symbols and building node handles is now
  a third of the batched request; a per-request cache of the file tables
  would remove most of the remaining `node_handle` cost.
- **Checker allocation**: `resolve_object_type_members` and the relater copy
  member vectors the pin shares.

## Recipes

- API driver: from the Phase 6 worktree,
  `node --conditions @typescript/source <scratch>/api-driver.mts <binary> batched|positions <reps>`;
  the native binary is the pin's `tsgo` from the jsapi harness.
- LSP smoke: `python3 tools/phase5/latency/capture.py --fixture target/phase5/latency/typescript-proposal --scenario tools/phase5/latency/proposals/typescript-pull.json --go-command '[native,"--lsp","--stdio"]' --rust-command '[rust,"--lsp","--stdio"]' --pairs 1 --smoke --output DIR`, then `DIR/samples.json`.
- Checker top-60: `cargo build --release --locked --example p7_checkerbench --manifest-path crates/tsr_compiler/Cargo.toml`, then `<exe> <requests.json> <rows.ndjson>`; stdout reports `interval_ns` and `outputs_sha256`, which must not change.
- Sampling the Rust API server: `/usr/bin/sample <pid> 3 1 -mayDie -file out.txt` on the pid of `tsrust --api`; samply records the LSP server directly.
- Concurrent pairs: start the baseline and the variant at the same moment, alternate the start order, score each pair by its ratio; six pairs resolve one percent on a busy host where sequential rounds drift by ten.
- Parse-bind per-file costs: wrap the parse and bind call of each harness (`tools/s07/benchmark/main.go`, `crates/tsr_bench/src/main.rs`) in a timer writing `index,worker,ns`; simulate the schedule by sending `index % workers` into capacity-`workers` queues (the sender waits when the target queue holds `workers` items not yet started) and take the last finish. The Rust wall reproduces within half a percent.
