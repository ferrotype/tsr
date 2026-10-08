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

## What remains

- **Node access in the checker.** Measured above: kind bits in `NodeId` are
  the one lever left with a real return (about a tenth of the check phase);
  flat pages are done, the directory is not the cost, parent reads are
  about one percent.
- **The checker's remaining gap** is spread across flow analysis, name
  resolution and the relater; a function-level comparison with the pin's
  profile is the method to size it.
- **Navigation.** `getSymbolAtPosition`, completion, hover and references
  descend the tree through the general view routing (`for_node_owner`,
  `owner_retention`, `owning_source`) and allocate a vector of children per
  level; a navigator that reads each child once through the file's direct
  view is the next LSP step.
- **Symbol responses.** Registering symbols and building node handles is now
  a third of the batched request; a per-request cache of the file tables
  would remove most of the remaining `node_handle` cost.
- **Parse and bind scaling** at eight threads (1.38) was not examined.
- **Checker allocation**: `resolve_object_type_members` and the relater copy
  member vectors the pin shares.

## Recipes

- API driver: from the Phase 6 worktree,
  `node --conditions @typescript/source <scratch>/api-driver.mts <binary> batched|positions <reps>`;
  the native binary is the pin's `tsgo` from the jsapi harness.
- LSP smoke: `python3 tools/phase5/latency/capture.py --fixture target/phase5/latency/typescript-proposal --scenario tools/phase5/latency/proposals/typescript-pull.json --go-command '[native,"--lsp","--stdio"]' --rust-command '[rust,"--lsp","--stdio"]' --pairs 1 --smoke --output DIR`, then `DIR/samples.json`.
- Checker top-60: `cargo build --release --locked --example p7_checkerbench --manifest-path crates/tsr_compiler/Cargo.toml`, then `<exe> <requests.json> <rows.ndjson>`; stdout reports `interval_ns` and `outputs_sha256`, which must not change.
- Sampling the Rust API server: `/usr/bin/sample <pid> 3 1 -mayDie -file out.txt` on the pid of `tsrust --api`; samply records the LSP server directly.
