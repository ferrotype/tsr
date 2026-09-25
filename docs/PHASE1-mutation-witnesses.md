# Phase 1 mutation-kill witnesses

Owner decision, 2026-09-24: the 947 `operation_witness_missing` operations may be
discharged by mutation checks. A mutation witness proves, per operation, what a
passing corpus alone cannot: the Rust home of the operation contributes to a
compared observation, on an input where the pinned Go operation also runs.

Result at the time of recording: **782 of the 947 operations are witnessed**
(section 8). The Phase 1 coverage report's pending entries fall from 1,364 to 582.

Phase 1 closure adds a fifth oracle, `table` (section 9): operations whose
natural inputs are not whole programs or parser runs are witnessed through
columns of operation tables, one `(column, input)` row at a time, with two more
crediting rules.

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

**Table rules.** On the `table` oracle two more rules apply (section 9):

6. **Column parity.** A kill on a row of column C credits only if every row of
   C matched native in the campaign's base trace. The results record each
   column's row count and matches, and the check compares the count with the
   committed inventory's rows of C.
7. **One home per claimed table operation.** No home of an operation a table
   column claims (a spec's `operations`) is excused, whichever witness claims
   it: an extra marked copy must be reached and killed, or lose its marker. The
   results list the operations their columns claim as `one_home_operations`
   and apply them; the scope check applies what the current specs claim, and
   stales the table witness when the specs changed after its selection or its
   results list other operations. An operation the table witness credits only
   through a column's callee keeps the ordinary excusal rule.

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
| `table` | the table inventory `data/phase1/tables/requests.json` (section 9) | `column`: `sha256(canonical(value))` of the column's single value | `column` | none (`setup` is no segment) |

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
- Reach is recorded per row, across threads. The thread that runs the row keeps
  its stage and reach to itself; any other thread is a worker of the running row
  (a thread a production operation starts, such as a parallel breadth-first
  search or a throttle group). A worker runs in the row thread's current stage,
  so its guards activate exactly when the row's would, and its reach is taken
  with the row's. The drivers run one row at a time per process, and a row's
  workers must finish before the row switches stage or ends (scoped threads,
  joined groups). Go's coverage counters are process-wide, so a goroutine's work
  inside a bracketed segment is that segment's Go reach the same way.

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
flips; empty collections and ranges. A marker on a statement mutates that
statement: a plain `if` (or `let x = if ..`) negates its condition, and a unit
statement (`expr;`, a `for` or `while` loop) is skipped. A `return`, `break` or
`continue` is never skipped, since the code after it is typed for its absence.
Functions whose simple mutants would recurse or hang get wrappers or no operator
and are reported as `unsupported`.

## 5. Go reach

`scripts/phase1_mutation_go.py reach` builds the native drivers from a
`git archive` export of the pin with
`-cover -covermode=atomic -coverpkg=<main>,<packages>` (the main package must
be listed or no counters are written). The packages are per oracle
(`Oracle.cover_packages`, recorded in the stage rule): `ast`, `parser`,
`binder`, `scanner` and `compiler` for the four recorded oracles, plus `core`,
`tsoptions` and `tspath` for `table`. The driver snapshots counters
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

With Go 1.27.1 on `PATH`, `WS=target/phase1-mutation/ws`, `M=data/phase1/mutation`
(the table oracle runs the same commands with `--oracle table` after
`python3 scripts/phase1_tables.py select --write`; section 9):

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
  `mutation/binder`, `mutation/facts`, `mutation/syntax` (and, once its
  campaign is recorded, `mutation/table`), each claiming its
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

Phase 1 closure campaign (2026-09-25), five oracles over one manifest of 1,112
operations (1,917 mutants, 276 controls): **1,094 killed**. Per witness:
e1 516, binder 711, facts 546, syntax 27, table 283 (an operation may be
killed by several). By Go file: `parser.go` 400 of 401, `ast/utilities.go`
196 of 200, `ast/ast.go` 150 of 153, `binder.go` 145 of 146, `jsdoc.go` 42 of
42, `reparser.go` 13 of 13, `core/core.go` 19 of 19. The table oracle's 11
groups hold 198 columns over 8,490 rows, every column at native parity.

Not killed: 18 operations: 6 unsupported (no marker or no sound operator for
the return type), 7 not reached, 3 survived, 2 crash only. Those not witnessed
another way stay pending (see the Phase 1 closure report); with the case
families recorded and the equivalent_rust roster entries, Phase 1 coverage has
20 pending operations.

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

## 9. Operation tables (the `table` oracle)

Most operations the four oracles cannot witness are not reached by a parser run
or a whole program at all: AST predicates that only the checker calls, core
helpers, tsoptions accessors, diagnostics. The `table` oracle witnesses them
with columns. The prototype (five operations, 98 rows) matched Go on every row,
killed all ten mutants, and caught seven planted bugs that a naive row choice of
the same size missed.

### Rows, stages and digests

One request row is one `(column, input)` pair. A column names the pinned Go
operation(s) it observes; an unexported operation is credited through the
exported caller its column calls, and only an operation no row can enter (for
example `tsoptions/showconfig.go:computeFn`, run at package init) needs a
bridge.

- `setup` builds the input: parse or bind a source file, parse a tsconfig,
  decode a constructed value, and everything else that is not the operation's
  own work (the walk, container lists, wildcard directories). On the Go side it
  is no coverage segment; on the Rust side it observes, so mutants are off and
  nothing it runs counts as reach.
- `column` calls the operation and nothing else. It is the only production
  segment and stage, so a row's Go reach and Rust reach are what the column
  entered.
- A column whose operation starts threads (the `concurrency` group: BFS
  visitors, throttle-group functions) gets their reach and activation too: the
  threads are workers of the row (section 3), which run in the column stage and
  whose hits are the row's, so a home only a worker executes is reached, killed
  or refused like any other and never excused as unexecuted. The operation must
  join every thread it starts before the column returns, as the pinned Go waits
  for its goroutines; setup never starts one.
- The column's single value is digested as `sha256(canonical(value))`, with
  `canonical` Python's `json.dumps(sort_keys=True, separators=(",", ":"),
  ensure_ascii=True)`. Values hold only null, booleans, integers, strings,
  lists and objects; byte strings (symbol names with the `\xFE` prefix, source
  slices) are carried as hex. A node reference is null for no node, else the
  node's index in its input's walk (below); a node outside the walk has no
  reference, and referring to it stops the Go driver and is an error in Rust.
  A symbol is `[first declaration reference, name as hex]`.
- A column whose callers rely on a panic lists the panic's class in the spec's
  `panic_contract`; Go's `Guard` records `{"panic": class}` as the value, and the
  Rust port returns that class through an explicit check. Classes are
  `nil_dereference`, `index_out_of_range`, `runtime:<text>` for another runtime
  error, or `message:<text>` for a panic value. Any other panic
  is the stage's outcome: a Rust panic is a crash, never a kill, and a Go setup
  or column panic makes the native freeze refuse the row, so each column's input
  domain is the preconditions its callers establish.

One `mutation/table` witness claims what the table campaign credits. The rules
are the five of section 1 plus column parity and one home (rules 6 and 7).

### Files

| Path | What | Owner after stage 1 |
| --- | --- | --- |
| `data/phase1/tables/<group>.json` | the table spec of one group | the group's package |
| `data/phase1/tables/requests.json` | the committed inventory (`select --write`) | integration |
| `tools/phase1/tables/go/main.go` | the Go driver: registry, stages, canonical JSON, input helpers, survey | TB0 |
| `tools/phase1/tables/go/<group>_columns.go` | the group's Go columns | the group's package |
| `tools/phase1/tables/go/bridges/<package>/*.go` | copied into `internal/<package>` of the export | the group's package |
| `tools/phase1/mutation/driver/src/table/mod.rs` | the Rust runtime: dispatch, stages, input helpers | TB0 |
| `tools/phase1/mutation/driver/src/table/<group>.rs` | the group's Rust columns (`COLUMNS`, `build`) | the group's package |
| `scripts/phase1_tables.py` | `check`, `select`, `materialize`, `campaign` | TB0 |

The groups are `runtime` (the harness's own columns, which claim no operation:
the canonical encoding, both walks and the symbol keys under each, so a
differing column is the column's difference and not the harness's), `class`, `modules`,
`positions`, `targets`, `containers`, `diagnostics`, `core`, `concurrency`,
`tsoptions` and `accessors`. Every `.go` file of `tools/phase1/tables/go` is a driver source, so
a new column file needs no registration; the native freeze binds all of them
and the bridges (`oracle_sources`).

A spec column carries exactly `id`, `operations` (ids of
`data/go-functions.tsv`; empty only in `runtime`), `input` (`source`, `bound`,
`source_jsdoc`, `bound_jsdoc`, `config` or `values`), `projection` (what the value holds and why that is the
contract), `evidence` (the caller `file:line`s behind the projection, for
example callers that read only length and elements, so nil and empty are one
value), `rust` (the Rust entry point), `survey` and `synthetic`, and optionally
`panic_contract` and `notes`. `phase1_tables.py check` requires the Go registry
(`Register("<group>", Column{ID, Input, Build, Survey})`) and the Rust
`COLUMNS` to name exactly the spec's columns, group by group, with the same
input kind and survey flag, and checks the committed inventory.

Input helpers, identical on both sides: `ParseSource`/`parse_source` (an S06
parse input: `filename`, `path`, `jsx`, `force`, `script_kind`, `source_hex`),
`BindSource`/`bind_source` (parse, then bind), their `_jsdoc` variants
`ParseSourceJSDoc`/`parse_source_jsdoc` and `BindSourceJSDoc`/`bind_source_jsdoc`,
`ParseConfig`/`parse_config` (tsoptionstest's VFS host over `files`,
`currentDirectory`, `caseSensitive`, the `jsonText` of
`<currentDirectory>/tsconfig.json`, and the column's `args`) and
`DecodeInput`/`decode` for values. `Parsed.Ref`/`node_ref`, `Refs`/`refs` and
`SymbolKey`/`symbol_key` build values.

A parsed input's nodes (`Parsed.Nodes`/`nodes`), which per-node columns
iterate and references index, are one of two walks:

- **`Walk`/`walk`** (`source`, `bound`): the facts oracle's preorder, a node
  and then each child subtree in `ForEachChild` order. Reparsed nodes are in
  it where the parser attaches them: reparsed declarations (JS `@typedef`,
  `@callback`, `@import`, `@overload`) in the enclosing list just before the
  element whose JSDoc holds them, `@typedef`, `@callback` and `@import` moved
  out to the nearest statement list (`parser.go` `parseListIndex`), or after
  the file's statements for the end-of-file token's JSDoc (`parser.go:445`);
  and reparsed parameters, types, type parameters and members as ordinary
  children of their hosts (`reparser.go`). JSDoc comments and everything under
  them are not in it.
- **`WalkJSDoc`/`walk_jsdoc`** (`source_jsdoc`, `bound_jsdoc`): `Walk` with a
  node's JSDoc comments (`Node.JSDoc`, which parses lazy JSDoc in setup) and
  their subtrees visited before its children, the order of
  `ForEachChildAndJSDoc`, except that a comment is visited only under its
  parent: a reparsed `@typedef` or `@callback` declaration lists its source
  statement's comment as its own JSDoc (`reparser.go:100,119`), and that
  comment is visited once, under the statement. Every JSDoc comment, tag and
  type expression then has one reference, and `Walk` is a subsequence of it.
  A column that observes JSDoc nodes (a tag it returns, a JSDoc node it takes
  as its subject, a reparsed clone's original) takes one of these kinds.

A node no walk holds (factory output) is projected by its own fields (kind,
position, flags), never by a reference. Either walk refuses a node it reaches
twice. The harness columns `runtime.walk`, `runtime.walk_jsdoc`,
`runtime.symbols` and `runtime.symbols_jsdoc` show both sides build the same
walks and symbol keys.

### Row selection

Selection chooses inputs only; every expected value comes from the native run.

- **Survey.** For a column with `survey: true` (one of the four parsed input
  kinds), the Go driver's survey mode parses (and binds) each of the 22,343 S06
  primary requests, walking it as the column's setup does, and returns the
  column's selection classes. A per-node column uses
  `NodeClasses`: `(kind, value class)` and `(parent kind, value class)` for
  every node, plus `(kind, parent kind, value class)` for every node whose
  value class is not `0`. Other columns define their own classes.
- **Cover.** Per column, a greedy cover of the union of classes: at each step
  the file that adds the most uncovered classes, ties to fewer bytes and then
  the smaller row id. Then the spec's synthetic inputs, in spec order: inputs
  for classes the corpus lacks, constructed values, configs and grids.
- **Inventory.** Rows are ordered by group, column, survey rows in choice order,
  then synthetic rows. An S06 row records its S06 request id and its
  `request_sha256`; a synthetic row records its input. The inventory records
  the spec digests, each group's survey digest and the S06 inventory it was
  selected from; `selection_sha256` binds the selection and the rows.
  `select --check` reruns the survey and the cover and requires the committed
  inventory byte for byte.

### Freezes, reach and campaigns

`phase1_mutation_go.py requests|native|reach|verify-native|verify-reach
--oracle table` and `phase1_mutation_run.py trace|kill|results|confirm` treat
the table like any batch oracle. The native freeze runs the plain driver twice
from two shardings, refuses any row whose setup or column did not complete,
reruns every row with `PHASE1_TABLE_RAW=1` and requires Python's digest of each
canonical value to equal the driver's and the frozen one, and records the
inventory binding (spec digests, `selection_sha256`) and each column's row
count. `kill` records each column's parity (and refuses every kill on a column
that is not at parity, with the column's first differing rows in the note) and
the operations the traced columns claim; `results` keeps both and never excuses
a home of such an operation on any oracle; `confirm` re-traces the table and
fails if a column with a recorded kill no longer matches native on every row.
The scope check (`phase1_scope`) applies rule 6 to the table witness
(`MUTATION_COLUMN_ORACLES`, against the committed inventory's rows of each
column) and rule 7 to every mutation witness, from the current specs: an
operation any current spec column claims has no excused home in any witness,
whatever results that witness rests on, and a spec that cannot be read stales
every mutation witness. The table witness is also bound to the specs: the
committed inventory must have been selected from the current spec files (their
digests), the native freeze from that selection, and the results'
`one_home_operations` must be exactly what the current specs claim for the
inventory's columns. A spec edit (a claimed operation, a projection, a note)
therefore stales the table witness until the selection and campaign rerun, and
`phase1_tables.py check` fails while the committed inventory is stale.

A **scratch table campaign** of some groups runs every step under one
directory and changes nothing under `data/`:

```sh
python3 scripts/phase1_tables.py check --allow-stale
python3 scripts/phase1_tables.py campaign --groups core --out $SCRATCH/table-core [--confirm]
```

It selects the groups' rows (`--inventory` reuses a selection), materializes
them, freezes native, plans the groups' claimed operations, takes Go reach,
splices the plan into `target/phase1-mutation/ws-table` (`--ws` to change; one
campaign holds the workspace's lock at a time), builds, traces, kills and
merges, and prints each column's parity and each operation's mutation state
(`summary.json`). It exits nonzero when a column diverges. The smoke campaign
over the committed inventory (200 rows, ten columns, six operations) takes
about 50 s: every column at parity, 9 of 9 mutants killed, five operations
killed (`Node.Decorators` has no Rust marker until its port lands), 27 of 27
kill pairs confirmed.

### Adding a column

1. Port the operation faithfully with exactly one `/// port:` marker on its
   single Rust home (rule 7).
2. Add the column to the group's spec, its Go column file and its Rust module,
   with the same id and input kind; keep everything but the operation's own
   work in `Build` and `build`. Choose the input kind whose walk holds every
   node the column refers to: a `_jsdoc` kind for JSDoc nodes.
3. `phase1_tables.py check --allow-stale` (the committed inventory is re-selected
   only at integration), then a scratch campaign of the group: every column
   at parity and every claimed operation killed. A column that differs names
   its first differing rows in `summary.json`; `PHASE1_TABLE_RAW=1` on the Go
   driver prints each row's value.

The committed inventory, native freeze and Go reach are rewritten at
integration (`select --write`, then the consolidated campaign), never by a
group's package.

