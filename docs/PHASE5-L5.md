# L5 implementation record

L5 adds editing to the language service and both server entry points on top of
L4. Source programs stay immutable: change generation retains their owners and
builds new syntax in request-local builders. The server returns edits; the
client applies them.

## Implemented surface

- Symbol rename and preparation: aliases, shorthand properties, import paths,
  string/numeric names, private names and resource-operation capabilities.
  File moves update loaded imports, references, configuration paths and include
  patterns using the production module-specifier generator.
- Document, range and on-type formatting over `tsr_format`, including negotiated
  UTF-8/UTF-16 positions, CRLF, editor preferences and content-map boundaries.
- Organize/sort/remove-unused imports and exports, with attributes, comments,
  ambient modules, type-only preferences, order detection and the pin's ordinal,
  natural and Unicode comparers. Unicode normalization/case/category tables are
  generated from the pinned Go toolchain and `x/text`; no new dependency is used.
- Code actions and fix-all for every provider registered at the pin: missing
  imports/type-only promotion, isolated-declaration annotations/assertions/
  extraction, and classes implementing interfaces. Generated members share the
  L4 synthesis path, and one import accumulator combines their dependencies.

The change tracker preserves enqueue order while serializing generated nodes.
It clones nodes before the printer assigns positions, retaining source ranges
for comments. Edits are mapped before sorting in original coordinates. An
inexact edit or conflicting projections suppresses every edit for that original
file; identical edits from distinct projections deduplicate. Overlap within one
projection remains an internal error, matching Go. Cancellation returns no
partial edit result.

Fix-all collects each public diagnostic phase with the pin's sorting and
deduplication. Class fixes consult bound member tables, including existing and
inherited members. Generated import edits use the compiler newline setting, as
Go's change tracker does; snippet-body formatting retains its separate editor
newline setting. This also fixes completion imports exposed by the expanded
formatting comparison. File-rename include insertion uses the pin's general
node insertion, including its existing JSONC-comment behavior.

## Validation

The scripts under `tools/phase5/lsp` run real Go and Rust servers against the
same temporary project. They compare complete response objects, edit ranges
and replacement text, then apply edits and compare resulting contents. The
quick-fix driver supplies identical native diagnostics to both servers, so it
tests editing independently of diagnostic production. Its only action-order
normalization handles Go's unordered fix-all provider map; edit order remains
significant.

| Bounded native check | Compared observations |
| --- | ---: |
| Document/range/on-type formatting | 168 |
| Symbol rename/preparation | 96 |
| File/config/import moves and resource capabilities | 90 |
| Organize imports: 14 fixtures, 10 settings, 3 actions, 2 encodings | 840 |
| Quick fixes/fix-all: 38 fixtures, 2 encodings | 282 |
| Same quick-fix set with single quotes and nondefault formatting | 282 |
| German action titles and generated method bodies | 26 |

The completion regression comparison passes 735 responses with minimal
capabilities and 746 with rich capabilities, in each encoding. Its formatting
fixture now keeps the dependency loaded: dropping it had made native results
depend on background export-index preparation. The config/root replacement difference is now owner-approved in the L4 record
(2026-10-05), with a matching config-only control and the raw difference checked
separately. Quote preference probes now use the
actual nested setting `preferences.quoteStyle`.

Focused unit tests pass: language service 45, autoimport 14, LSP 27, AST 188,
checker 77, parser 35 and printer 162. They include the pinned path-folding,
formatting-range, empty-file and conflicting-projection regressions, and native
fix-all output that catches insertion-order and duplicate-diagnostic bugs.
Three focused compiler variants retain all 27 comparisons. Changed-crate
clippy with warnings denied, formatting and `cargo xtask validate` pass.

## PR review fixes

Quoted-property rename now uses the non-panicking module-specifier predicate.
Export-equals rename follows the pin's name guard for namespace and import-equals
aliases, preserving differently named imports and reexports. Interface-member
generation selects one accessor pair, uses trivia-free positions for indentation,
and infers quote style for generated names, types and method bodies. Multiline
import insertion uses the formatter's Unicode and tab handling, including zero
tab size.

File moves carry exclusion patterns and relative-path preferences into the
production specifier generator. Rooted and empty old specifiers, package-path
endings, and the unresolved relative fallback follow the pin. Extension eligibility
uses the original importer name when moving between `.d.ts` and `.ts`. Organize
imports uses ordinal name ordering when there are no named imports to infer from;
preference parsing uses Go's Unicode lowercasing and accepts boolean case
sensitivity. Empty mapped formatting results serialize as `[]`. The implemented
editing methods have been removed from the interim unsupported-method list.

The expanded native checks pass:

| Command under `tools/phase5/lsp` | Compared observations |
| --- | ---: |
| `edits.py`: formatting / rename | 168 / 104 |
| `actions.py`: 18 fixtures, 18 settings, 3 actions, 2 encodings | 1,944 |
| `quick_fixes.py`: default settings | 314 |
| `quick_fixes.py --case promote-indent --case promote-unicode-indent --config '{"format":{"tabSize":0},"preferences":{"organizeImports":{"typeOrder":"first"}}}'` | 12 |
| `file_edits.py`: moves, preferences and package symlinks | 126 |

Four focused compiler variants also pass all 36 subtests: declaration import
extensions, subpath reexports, enum import-equals references and NodeNext
extensionless package mains. These use a freshly built `tsr-testrunner` through
`parity.py run compiler --id`; targeted runs are development checks, so no full
expectation file was accepted or regenerated.

Final local checks pass: 78 checker tests, 57 LS tests, 28 LSP tests and two
checker doctests; changed-crate clippy with warnings denied, formatting and
`cargo xtask validate`. The native FSEvents test needs host access and timed out
once while the differential servers were active; the serial crate-test run
passed without changing or disabling it.

The follow-up review fixes normalize quote style, module-specifier preference and
ending with pinned Go Unicode lowercasing, for both raw and nested settings.
Completions and inlay hints share the quote parser. Import insertion now detects
trailing comments after Unicode single-line whitespace, while stopping at line
breaks and malformed UTF-8. The NBSP and em-space repros and mixed-case quote
repro differed before the fix. Afterward, 220 native quick-fix requests across
five preference combinations and 132 file-move comparisons match in UTF-8 and
UTF-16. The quick-fix selection is `class-inferred-quotes`, `class-imports`,
`imports-with-paths`, `promote-nbsp-comment` and `promote-em-space-comment`, with
type order `first`; the `(quoteStyle, importModuleSpecifier,
importModuleSpecifierEnding)` combinations are `(DoUbLe, Relative, JS)`,
`(DOUBLE, Relative, JS)`, `(double, relative, js)`, `(auto, shortest, auto)` and
`(SİNGLE, NON-RELATİVE, MİNİMAL)`.
The affected crate tests pass (58 LS, 29 LSP), including raw/nested preference
normalization and Unicode comment-boundary regressions. Clippy with warnings
denied, formatting and marker validation also pass.

## Remaining Phase 5 work

L6 still owns cross-project orchestration, project-reference discovery, ATA,
mapper process/lifecycle integration and the recorded config/registry sequence.
The local editing algorithms and mapping tests do not certify those paths.
In particular, a client without `workspace.willRenameFiles` currently receives
file-move edits from its default project only. L6 must share the project-tree
loading and all-project traversal used by the pin's file-rename worker. The
ordinary `willRenameFiles` loop over all loaded projects is intentional: Go
also traverses every project after loading project trees.
L7 owns the complete fourslash runner, client replay expectations and latency
measurement. There is still no LSP adapter in `parity.py`; bounded comparisons
are development checks and do not claim full fourslash acceptance. No benchmark
or full corpus was run for this increment, and no divergence was approved.


The L4 review fixes are merged into this branch: string-index completion context,
localized import descriptions and one snapshot-wide node_modules watch group.
L5 quick fixes now consume the same localized description as completion
resolution instead of maintaining a second description formatter. The bounded
L4 review and config-replacement probes also run on the combined L5 branch;
French import, namespace, existing-import and generated class-member quick
fixes are compared separately against the pin.

Merge validation passes 175 focused tests (46 LS, 15 autoimport, 87 project,
27 LSP, including the native watcher with host access), targeted clippy with
warnings denied, fmt and `cargo xtask validate`. The bounded L4 regressions and
approved config/control probes pass on L5; 36 French quick-fix requests match
Go in UTF-8 and UTF-16 (`quick_fixes.py --case imports --case existing --case
namespace --case class-imports --locale fr`). No broad corpus or benchmark was
repeated for this merge.
