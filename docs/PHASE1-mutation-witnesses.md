# Phase 1 mutation-kill witnesses

Owner decision, 2026-09-24: the 947 `operation_witness_missing` operations may be
discharged by mutation checks. A mutation witness proves, per operation, what a
passing corpus alone cannot: the Rust home of the operation contributes to a
compared observation, on an input where the pinned Go operation also runs.

Result at the time of recording: **782 of the 947 operations are witnessed**
(section 8). The Phase 1 coverage report's pending entries fall from 1,364 to 582.

## 1. What a kill proves

An operation `op` is credited by a kill of mutant `m` on frozen request row `R`
of an oracle when all of these hold:

1. **Base match.** With no mutant active, the Rust observation of `R` equals the
   frozen native observation in every compared stage, and every native stage is
   `ok`.
2. **Rust reach.** The site of `m` executed on `R` in a production stage.
3. **Go reach.** The pinned Go function `op` was entered (its entry block counter
   is nonzero) on the same request in a production stage of the instrumented
   native driver, and `op` is not an unstable operation (section 5).
4. **Observed difference.** With `m` active every stage of `R` completed `ok`,
   and at least one compared stage differs from native. The differing stages are
   recorded.
5. **Control.** When `m`'s replacement allocates (it calls a `create_*` or
   `new_*` function), `m` has a control mutant at the same site that computes the
   replacement and discards it. Some compared stage must differ both from native
   and from the control in that same stage, so an allocation side effect (a moved
   node or identifier counter) never counts as a kill.

**Homes.** Every production `port:` marker site of `op` is a home: function,
`macro_rules!` function, match arm and statement. Each home that any traced
oracle reaches, in production or while observing, must be killed in the
witness's oracle. A home may be excused only when its mutants have zero
production and zero observation reach on every traced oracle; the recorded reason
names the oracles. A home with no mutant keeps the operation pending.

**Shared sites.** When one site carries markers for several operations, a kill
credits only the operations whose Go function was entered on the kill row. The
`syntax` oracle never credits a multi-operation site: a whole-program row enters
nearly every parser function, so Go reach on the row cannot tell which operation's
path caused the difference (state `not_credited_multi_op`).

Never credited, but recorded: `survived`, `crash` (a panic, abort or changed
outcome proves reach, not discrimination), `timeout`, `not_reached`,
`unsupported` (no sound operator), `build_failed`, and the non-final `budget` and
`not_run`, which make a campaign partial so that it cannot be recorded. A mutant
is never an `equivalent_rust` claim.

## 2. Oracles

| Oracle | Rows | Compared stages | Production stages | Observation stages that count Go reach |
| --- | --- | --- | --- | --- |
| `e1` | 22,343 S06 requests | `parse`, `node_index_before`, `node_index_after`, `encode_source_file` frames | `parse` | `internal/parser` only (lazy JSDoc while encoding) |
| `binder` | 22,343 S07 bind requests | `parsed_graph`, `bound_graph`, `repeated_graph` after `comparable` normalization | `parse`, `bind`, `repeat_bind` | none |
| `facts` | the S06 requests | per-node `(kind, SubtreeFacts)` in document order | `parse`, `subtree_facts` (the public production API per node) | none |
| `syntax` | 15,152 syntax-schedule rows | `scripts/phase1_syntax.py` `COMPARED` | program load, `GetSyntacticDiagnostics` | none |

Native digests are frozen per row and stage in
`data/phase1/mutation/native-<oracle>.json.gz`, with the pin, Go version, oracle
binary and source digests, and the ordered request identities. `verify-native`
re-runs the plain Go oracle and requires identical digests. Every row of every
oracle matched native in the base trace; the `facts` comparison found no
Rust/Go subtree-facts difference.

## 3. The mutant switch and stages

`tools/phase1/mutation/switch` (`phase1_mutants`) is inert in the repository. The
splicer inserts guards into a scratch copy of the workspace only:
`if ::phase1_mutants::hit(ID) { ... }`. With no active mutant every guard is
false, so the spliced build observes exactly what the unspliced build does; the
base trace of every oracle confirms it row by row.

- `hit` guards production-only sites. While observing it never activates, but its
  execution is recorded apart as observation reach.
- `hit_parser` guards parser sites: a site in `crates/tsr_parser/src/` whose every
  operation is in `tsc/internal/parser/`. It stays selectable while observing,
  because lazy JSDoc parsing triggered while encoding is production parser work on
  both sides, and e1's Go observation stages count `internal/parser` only.

## 4. Mutants

`scripts/phase1_mutation_plan.py plan` resolves sites with the scope's own marker
rules and runs the splicer (`tools/phase1/mutation/splicer`). The committed plan is
`data/phase1/mutation/manifest.json`: 1,668 mutants plus 277 controls on 938 sites,
961 homes. Each entry records `id`, a stable `key`, `op`/`ops`, `file`,
`function`, `site_kind`, `site_line`, `span`, `span_sha256`, `operator`,
`hit_fn`, `control`/`control_of` and the inserted text. `check_plan` refuses a
plan whose guards, controls or switch functions break the rules above.

Operators are type-directed, at most two per site plus a control: opposite
booleans and tristates as early returns; for `Parser` methods returning nodes,
lists or tokens, result wrappers that run the body first and then replace the
result (`None`, a missing node or list, `Some(missing)` only when the real result
was present, a token scanner that also sets the parser token); same-typed
parameter returns; skipped unit bodies; `0`/`1`/`!0` integers; flag and variant
flips; empty collections and ranges. Functions whose simple mutants would recurse
or hang get wrappers or no operator and are reported as `unsupported`.

## 5. Go reach

`scripts/phase1_mutation_go.py reach` builds the native drivers from a
`git archive` export of the pin with
`-cover -covermode=atomic -coverpkg=<main>,ast,parser,binder,scanner,compiler`
(the main package must be listed or no counters are written), snapshots counters
per row and stage (`ClearCounters`/`WriteCounters`), and maps entry blocks to
operation ids by file and line containment against `data/go-functions.tsv`.
Instrumented output must equal the frozen native digests on every row.

`data/phase1/mutation/go-reach-<oracle>.json.gz` holds the operation → rows index
and a header binding the instrumentation, binaries, native file and
`stage_rule`, whose digest covers the segment map, the observation rule,
`ENTRY_RULE`, `DECODER_VERSION` and the decoder functions' source. Operations whose
reach depends on pool or memo state (for example `scanner.go:cleared`) are listed
as `unstable_ops` and never credited. The files are reproducible:
`verify-reach --oracle X` re-runs the instrumented oracle and requires identical
bytes.

## 6. Running it

With Go 1.27.1 on `PATH`, `WS=target/phase1-mutation/ws`, `M=data/phase1/mutation`:

```sh
python3 scripts/phase1_mutation_go.py requests
python3 scripts/phase1_mutation_plan.py plan --out target/phase1-mutation/plan.json   # then copy to $M/manifest.json
python3 scripts/phase1_mutation_plan.py schemata --plan $M/manifest.json --ws $WS
python3 scripts/phase1_mutation_run.py build --ws $WS
python3 scripts/phase1_mutation_run.py trace --oracle e1 --ws $WS --verify-frames       # and binder, facts, syntax
python3 scripts/phase1_mutation_run.py kill --oracle e1 --ws $WS --plan $M/manifest.json  # and binder, facts
python3 scripts/phase1_mutation_run.py results --plan $M/manifest.json --kills <e1,binder,facts kill files> --out <skip list>
python3 scripts/phase1_mutation_run.py kill --oracle syntax ... --skip-killed <skip list>
python3 scripts/phase1_mutation_run.py results --plan $M/manifest.json --kills <all four> --out $M/results.json.gz
python3 scripts/phase1_mutation_run.py confirm --results $M/results.json.gz --plan $M/manifest.json
python3 scripts/phase1.py record-mutations --results $M/results.json.gz --declare e1 --write   # and binder, facts, syntax
python3 scripts/phase1.py inventory --write && python3 scripts/phase1_coverage.py report --output data/phase1/coverage-report.json.gz
```

Speed comes from one schemata build for every mutant, per-row reach from one
trace pass, candidate rows = Rust reach ∩ Go reach ∩ base match ordered smallest
first, stopping at the third kill per operation, in-process loops over many
mutants per worker, parallel workers with per-row deadlines, and continuing past a
hanging row (up to three timeouts per mutant). Measured: trace 29 s (e1), 104 s
(binder), 4 s (facts), 67 s (syntax); kill 32 s, 73 s, 27 s; syntax kill 741 s.

**Syntax skip policy.** Each syntax row reloads the bundled libraries, so an
exhaustive syntax campaign would take hours. The syntax campaign therefore runs
only mutants not already killed by e1, binder or facts (`--skip-killed`); the skip
source's digest and the skipped count are recorded in the kill file, and every
skipped mutant carries `skipped_by`. No operation loses a witness this way.

## 7. Evidence, binding and freshness

- `data/phase1/mutation/results.json.gz`: per mutant, per oracle: state,
  candidate count, reach (production, Go, observation), kills with differing
  stages and native, base, mutant and control digests, timeouts, `skipped_by`;
  per operation: state and homes.
- `data/phase1/cases.json`: four `mutation_kill` witnesses, `mutation/e1`,
  `mutation/binder`, `mutation/facts`, `mutation/syntax`, each claiming its
  operations and listing its mutants, excused homes (`not_claimed`) and recorded
  `mutation_evidence` (claims, manifest, results, native and Go-reach digests,
  and every credited kill).
- **P1A (links).** Coverage accepts a mutation witness only while its bindings
  hold, checked without running anything: the claims and artifact digests, the
  current `span_sha256` of every claimed mutant, the kill rows' request digests,
  Go reach and Rust reach of every credited kill, the control rule, the home rule,
  and the current Go oracle sources, instrumentation and stage rule. An edit
  inside a mutated span returns that operation to pending as
  `mutation_witness_stale`; an unrelated edit does not. Staleness is a coverage
  gap, never a manifest or producer health problem.
- **P1B (execution).** `run.foundations.mutation_witnesses_complete` comes from the
  `mutation-witnesses` receipt produced by `confirm`, which splices the full
  manifest, replays every recorded kill pair and its control against current
  sources, requires the recorded stages and digests, re-traces every oracle and
  fails if an excused home executes. Round 3 confirmation: 8,508 of 8,508 pairs,
  2,544 base rows, 227 excused mutants unreached.

## 8. Results and what remains

Witnessed: **782**, per witness e1 499, binder 685, facts 536, syntax 4 (only
that oracle: binder 192, facts 87, e1 6, syntax 4). By Go file: `parser.go` 389 of
397, `binder.go` 140 of 144, `ast.go` 110 of 139, `ast/utilities.go` 58 of 130,
`jsdoc.go` 42 of 42, `reparser.go` 13 of 13, `subtreefacts.go` 7 of 7.

Not witnessed: 165. They are 123 not reached (mostly never entered by Go on any
existing input; a few entered in Go but not through the marked Rust home), 30
unsupported (no marker, or no sound operator for the return type), 6 crash, 5
survived and 1 timeout. They stay `operation_witness_missing` until they have
another witness, new inputs or a reviewed destination. Unrelated to this work
and unchanged: 302 `implementation_unverified`, 92 `later_step_unresolved` and
23 `compiler_destination_unreviewed`.

Known limitations:

- A site's operation set, which decides `hit_parser` and the syntax
  multi-operation rule, includes only operations in the plan. A site that also
  carries markers for already-covered operations is treated by its planned ones
  only; today two mutants are like this and both are unreached.
- The manifest's `root_tree` cannot equal the tree that contains the manifest, so
  `schemata` and `confirm` report "sources changed since planning"; every span is
  verified instead.
- Build the driver and `phase1_syntax` with separate cargo invocations when
  reproducing binary digests; one combined build unifies features differently.
- About 18 predicate mutants per oracle still hang on every candidate row; their
  operations are witnessed by other mutants or remain pending.
