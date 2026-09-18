# S09-3 formatter port: plan

Owner decision, 2026-09-18: pull the formatter port forward so S09-3 can close,
rather than split `api_scratch_disposal` into a printing half and a formatting
half. This plan covers what that means, in what order, and how each step is
proved. Nothing here changes a threshold, a criterion or a sprint dependency.

## 1. What the criterion needs

`exp.E3.api_scratch_disposal` asserts that repeated API printing and insertion
formatting match Go, release their scratch arenas after returning text, register
no synthetic handles and do not grow session arena counts. Printing is done
([S09-3](S09-3.md)). Insertion formatting is the pinned
`handleFormatNodeForInsertion` (`api/session.go:3045`), which does this:

1. decodes the node and converts the target position from UTF-16 to UTF-8;
2. `printer.PrintAndPositionNode`: prints with `NeverAsciiEscape`,
   `PreserveSourceNewlines` and `TerminateUnterminatedLiterals` through a
   `ChangeTrackerWriter`, then clones the tree with the recorded positions;
3. `printer.CreateSyntheticSourceFile` around the positioned clone;
4. `format.GetLineStartPositionForPosition` and `format.GetIndentation` on the
   **target** file, which is the smart indenter;
5. `format.ShouldIndentChildNode` for the delta;
6. `format.FormatNodeGivenIndentation` over the synthetic file, which is the
   span worker, the formatting scanner and the whole rule table;
7. `core.ApplyBulkEdits`.

Steps 4 and 6 between them reach every file of `internal/format`. There is no
useful subset: the insertion path is a thin entry into the full formatter.

## 2. Inventory

Go, at pin `1f70213d`, from the port ledger:

| file | crate in ledger | loc | functions |
| --- | --- | --- | --- |
| `format/span.go` | `ts_format` | 1,262 | 44 |
| `format/indent.go` | `ts_format` | 821 | 32 |
| `format/rulecontext.go` | `ts_format` | 629 | 88 |
| `format/rules.go` | `ts_format` | 450 | 4 (one rule table) |
| `format/scanner.go` | `ts_format` | 374 | 23 |
| `format/api.go` | `ts_format` | 189 | 12 |
| `format/rulesmap.go` | `ts_format` | 156 | 7 |
| `format/util.go` | `ts_format` | 148 | 8 |
| `format/context.go` | `ts_format` | 121 | 11 |
| `format/rule.go` | `ts_format` | 109 | 6 |
| `astnav/tokens.go` | `ts_astnav` | 793 | 18 |
| `printer/changetrackerwriter.go` | `ts_printer` | 250 | 36 |
| `printer/syntheticfile.go` | `ts_printer` | 53 | 2 |
| `core/textchange.go` | `ts_core` | 30 | 2 |
| `ls/lsutil/formatcodeoptions.go` | `ts_ls` | 141 | 5 |
| `ls/lsutil/{utilities,children,completednode}.go` | `ts_ls` | 486 | partly used |

About 6,000 Go lines, all `planned` today. Printer work inside `printer.go` comes
on top: 20 references to `PreserveSourceNewlines`, the six emit hooks
(`OnBefore`/`OnAfter` for node, node list and token) and
`TerminateUnterminatedLiterals`.

**Correction, found when F2 started.** The first version of this plan called the
Rust printer "about two thirds of Go's" from a line count. That was wrong in the
way that matters: the Rust printer does not print statements at all. Its
fallthrough is the named refusal `statements, declarations and JSDoc nodes`. It
covers type nodes, type members and the expressions the node builder needs.
Measured function by function against `printer.go`:

| | functions | Go lines |
| --- | --- | --- |
| `emit*` functions in Go | 286 | 4,364 |
| with a Rust counterpart by name | 105 | 1,648 |
| without | 181 | 2,716 |

The missing 2,716 lines split into statements 496, declarations 624, JSX 195 and
other 1,392. "Other" includes comment and source-map emission, which positioned
printing does not need because it passes no source file, so the part F2 really
needs is about 1,900 to 2,200 lines. Insertion formatting exists to insert
statements and declarations, so there is no way around it: pulling the formatter
forward also pulls the statement printer forward from Phase 3. The whole port is
therefore about 8,000 Go lines, not 6,000.

What the format package calls outside itself: from `astnav`,
`FindPrecedingToken(Ex)`, `GetTokenAtPosition`, `FindNextToken`,
`FindChildOfKind`, `GetStartOfNode`; from `scanner`, the ECMA line helpers
(`GetECMALineOfPosition` 31 times, `GetECMALineStarts`,
`GetECMALineAndByteOffsetOfPosition`, `GetECMAEndLinePosition`,
`GetECMAPositionOfLineAndByteOffset`), `GetTokenPosOfNode`, `SkipTrivia`, the
comment ranges and a scanner over a source file; from `lsutil`,
`FormatCodeSettings`, `EditorSettings`, the indent style and semicolon
preference enums, `PositionIsASICandidate`, `PositionBelongsToNode` and
`GetFirstToken`.

Rust today:

| have | missing |
| --- | --- |
| scanner with every rescan the formatting scanner uses, `skip_trivia`, comment ranges | `ts_astnav`, entirely |
| ECMA line map on the source file, UTF-16 to UTF-8 position map | `ts_format`, entirely |
| printer at about 4,200 lines against Go's 6,345, with `get_lines_between_nodes` and `get_leading_line_terminator_count` | emit hooks; `PreserveSourceNewlines` is a named `Unsupported`; the separating, closing and effective line helpers |
| protocol-8 decoder into a request-owned `AstBuilder` | `ChangeTrackerWriter`, position assignment, synthetic source file |
| | `TextChange`, `ApplyBulkEdits`, format settings |

The printer has 12 named unsupported boundaries. Positioned printing passes no
source file, so the `comment emission` boundary is not on this path.

## 3. Placement

Crates follow `PLAN.md` section 8 and the ledger: a new `ts_astnav`, a new
`ts_format`, positioned printing in `ts_printer`, text changes in `ts_core`. The
`lsutil` pieces are ledgered to `ts_ls`, which does not exist and should not be
created for four small files. They go in `ts_format::settings` and
`ts_format::lsutil` with `// port:` markers naming the `lsutil` functions.
`ts_ls` re-exports them when it arrives. Their ledger rows keep `crate = "ts_ls"`,
because that column is generated from the package map and validation rejects a
hand edit; the row's `rust` field records where the code actually lives.

Phases in the ledger do not move. These files are ported early; their rows
become `ported` with a note, and the phase column keeps recording where the
roadmap put them.

## 4. Steps

Each step ends on a parity check against frozen native observations, with the
denominator fixed before the Rust code exists. No step is "done" on unit tests.

**F0. Oracle and frozen observations.** Done for the corpus; see
`tools/s09/README.md` for what was built. It differs from the sketch below in
two ways. The corpus comparison is a live differential, as E1's is, so no
per-file observation is committed: the oracle is a persistent process built
inside a fresh export of the pinned tree, and `data/s09/format-probes.json`
freezes the inventory digest, the operation and variant contract and the digest
of the whole native stream. And the denominator is the S06 parser inventory
reduced to its 16,120 distinct parser inputs. Positioned printing and insertion
are corpus-wide probes too, over each file's leading statements, rather than a
handful of fixtures. Still to do under F0: the fourslash recording, before F6,
and the scanner and rule probes when F3 and F4 start.

The original sketch: a Go overlay beside
`tools/s09/printing_test.go`, run with Go 1.27.1 and `GOTOOLCHAIN=local`, that
writes under `data/s09/format-*`:

- navigation probes: for every file of a fixed corpus, at sampled positions and
  at every token boundary, the results of the five `astnav` entry points as
  kind, pos and end;
- indentation probes: `GetIndentation` at every line start and at sampled
  in-line positions, under the default settings and two variants (tabs, and
  indent size 2);
- document formatting: `FormatDocument` on every corpus file, recorded as the
  ordered edit list, not just the resulting text;
- fourslash-derived cases: the pinned fourslash tests that format (177 calls to
  `FormatDocument` and 28 to `FormatSelection` across about 190 files). The
  overlay runs them natively with a recording hook at the formatter's entry
  points, capturing source text, settings, entry point, range and the edit
  list. This gets the corpus without porting fourslash or the language service;
- positioned printing: text plus the position of every node for the decode
  fixtures S09-3 already freezes, extended with multi-line and nested cases;
- insertion cases: full `handleFormatNodeForInsertion` inputs and outputs,
  including non-zero initial indentation, mid-line positions and both newline
  kinds.

The corpus is the S07 subset source inventory, which is already frozen and
already parses identically (E1). Manifests carry request and source hashes, as
the printing fixture does, so stale observations cannot certify new code.

**F1. `ts_astnav`.** The five entry points and what they need. Exit: every
navigation probe matches.

Done. `compare --ops nav` reports 16,120 of 16,120 inputs, 35.1 million rows,
including the one row where the pinned navigation asserts. The first complete
run stood at 15,840. All 280 differences had one cause: upstream's
`VisitEachChild` visits a JSDoc parameter tag's name before its type whatever
order they were written in, while `ForEachChild` follows `IsNameFirst`, and
navigation uses the former. Alongside the crate, the AST gained the token cache
entry point (`SourceFile.GetOrCreateToken`) and the scanner crate
`GetTokenPosOfNode`. `findRightmostNode` has no caller upstream and is not
ported.

**F2. Positioned printing.** First the statement, declaration and JSX
printer described in the correction above, which is the bulk of this step and is
proved the same way: the `position` probe prints each file's leading statements,
so its text rows are a printer parity check over the corpus. Then the six emit
hooks; `PreserveSourceNewlines`, which
needs the three missing line-terminator helpers and removes the named boundary;
`TerminateUnterminatedLiterals`; `ChangeTrackerWriter` with its last-non-trivia
position rule; `AssignPositionsToNode`; `CreateSyntheticSourceFile`. Exit: text
and every node position match for all positioned-printing observations, and the
existing printing fixture still passes.

**F3. Formatter foundations.** Settings, `TextChange` and `ApplyBulkEdits`,
`rule.go`, `context.go`, `util.go`, and the formatting scanner. Exit: a scanner
probe (token, trivia and rescan decisions per corpus file) matches. That probe
is added to F0 when this step starts, since its shape depends on the port.

Done. The `scan` probe drives the formatting scanner over each whole file, with
the token-level node from navigation as the container of every token, which is
what gives the rescan predicates realistic input. The oracle reaches the
unexported scanner through a bridge file copied into the export's
`internal/format`. `compare --ops scan,nav` reports 16,120 of 16,120 at the first
complete run; with the greater-than rescan disabled, 13 of the first 3,000
inputs differ, so the check can fail. `ApplyBulkEdits` returns the slice bounds
upstream panics on instead of panicking. The crate carries a temporary
`allow(dead_code)`, because the rules, the indenter and the span worker that
consume these foundations are the next steps; it goes away with F6.

**F4. Rules.** `rules.go`, `rulesmap.go` and the 88 predicates of
`rulecontext.go`. Exit: for every adjacent token pair in the corpus, the rules
selected and their order match. This isolates a wrong predicate from a wrong
span walk, which otherwise look the same in the output.

**F5. Smart indenter.** `indent.go`. Exit: every indentation probe matches in
all three settings variants.

**F6. Span worker and entry points.** `span.go` and `api.go`. Exit: every
`FormatDocument` edit list and every fourslash-derived case matches, byte for
byte and edit for edit.

**F7. API and ownership.** `ts_api` gains insertion formatting beside printing.
The request owns the decoded tree, the positioned clone, the synthetic source
file, the formatting scanner and the edit list; the target file is borrowed
from the snapshot and never copied into scratch. Disposal tests mirror the
printing ones: output outlives scratch, decoder errors and panics, unsupported
printer input, formatter errors, repeated requests beside a live registry with
no arena growth and no registered handle. `scripts/s09_ownership.py` then
publishes `api_scratch_disposal` as printing and formatting together, in all
four modes, and the informational printing metric stays as its component.

## 5. How failure shows up

- Exact equality against native observations everywhere. No tolerance, no
  normalization of whitespace, no comparing final text where an edit list exists.
- A construct the port does not handle returns a named `Unsupported`; it never
  formats "approximately". The count of distinct named boundaries is reported.
- Each step's parity check is mutation-checked once, as S09-1 and S09-2 were.
- Miri and AddressSanitizer run the ownership suites, not the corpus. The corpus
  runs in debug and release.

## 6. Evidence and ledger

Every ported function carries a `// port:` marker; ledger rows flip as each file
completes; `cargo xtask validate` stays clean throughout. The formatter parity
numbers are published as informational metrics on the `e3` run, the way the
printing metric is. A formatter parity gate of its own would be a new criterion
in `status/experiments.toml`, which is the owner's call; this plan adds none.

## 7. Risks

- **Printer coverage.** The insertion path prints arbitrary decoded statements,
  and the Rust printer prints none; see the correction in section 2. After F2
  the remaining named boundaries (decorators, accessor bodies, generated names)
  still refuse, and the `position` probe turns that into a count.
- **`PreserveSourceNewlines`** changes list emission and line-break decisions
  across the printer, so F2 can disturb S08 display parity. The S08 printing
  and display suites run after every F2 change.
- **Columns are UTF-16** in the writer and the formatter's line math. The
  position map exists; the places that need it are easy to miss, so F0 includes
  non-ASCII corpus files and insertion cases.
- **Tree-shape dependence.** The formatter walks the parse tree and rescans
  tokens. Parse parity is already at 1 on this corpus (E1), which is why the
  corpus is reused rather than invented.
- **Size.** The format package alone is 4,259 lines against the scanner's
  4,330, and the dependencies bring the total to about 6,000, so this is a
  sprint of its own. F1 and F2 are independent and small; F4 is wide but
  mechanical; F5 and F6 carry the subtle logic.

## 8. Order of work

F0 first, because it fixes every denominator. F1 next: the rule predicates, the
indenter and the span worker all call into navigation. Then F3, F4, F5, F6 in
that order, since each consumes the previous. None of those touches the printer.
F2 is independent of all of them and, after the correction in section 2, the
largest single step; it is needed by F7 only. F7 last. S09-5 can close once F7's metric is
true and the E3 evidence is refreshed.

## 9. Decisions this plan assumes

1. The `lsutil` pieces live in `ts_format` until `ts_ls` exists.
2. No new gate; formatter parity is reported, not enforced, until the owner adds
   a criterion.
3. The corpus is the S07 subset plus the recorded fourslash formatting calls.
