# S05 scanner implementation plan

Branch: `codex/s05-scanner`, from merged main `c741505` (PRs #7 and #8).
Upstream: `1f70213d4922b434345f639b441681e470c7cfc1`.
Status: reviewed on 7 September 2026; dispositions below precede implementation.

## Acceptance and scope

S05 implements the byte scanner used by the parser and language service, not a
new interpretation of TypeScript's lexical grammar. The authorities are the
pinned Go functions, `docs/design/text.md`, and ADRs 0011/0013/0015/0016/0017/0018.
Preserve the observable recovery behavior as well as successful tokens. No
mismatch is accepted by changing a baseline or loosening a comparator.

| Sprint requirement | Concrete deliverable | Independent acceptance |
| --- | --- | --- |
| S05-1 token scanning | `tsr_scanner`, lexical modes, comment directives, Go token-dump adapter and frozen inputs | Every frozen file/operation produces the same token kind, full/start/end byte positions, flags, text/value bytes, directives and diagnostics |
| S05-2 diagnostic/value bytes | Structured scanner diagnostics and byte-preserving literal cooking | Explicit malformed, WTF-8, BOM-decoded, escape and rescan cases; E4's scanner criteria consume this producer |
| S05-3 regexp | Slash rescan envelope, regexp grammar/recovery and pinned property tables | Real `ReScanSlashToken(true)` calls, flag/target matrix, all property aliases and Unicode predicate checks |
| S05-4 rescans/JSDoc | Every scanner-only rescan and JSX/JSDoc entry point, state/configuration transitions | Frozen action traces exercise the entry points explicitly, including omitted/false/true regexp error settings |
| S05-5 traceability | Function markers attached to actual implementations | At least 143 of 158 inventoried scanner functions mapped, without placeholder wrappers |

The source inventory is 112 functions in `scanner.go`, 34 in `regexp.go`, and
12 in `utilities.go`. Existing S04 position ports count once. The planned
deferrals are the ten helpers requiring real source-file/node ancestry:
`GetTokenPosOfNode`, `getErrorRangeForArrowFunction`,
`findOriginatingJSDocSatisfiesTag`, `GetErrorRangeForNode`,
`GetSourceTextOfNodeFromSourceFile`, `isJSDocTypeExpressionOrChild`,
`GetTextOfNodeFromSourceText`, `GetTextOfNode`, `GetTextOfJSDocComment`, and
`DeclarationNameToString`. That leaves 148/158, with room to defer the three
SourceFile position wrappers and `GetECMALineStarts` if the owning runtime cannot
supply their inputs. Deferring all fourteen leaves 144/158 (91.1%).
The final inventory, not this estimate, decides the gate. Do not fabricate
parents or SourceFile storage to reach a percentage.

S05 does not complete parsing, AST ownership integration, encoder parity,
checker-created literal types, printing, E1, full E4, E7, or performance gates.

## Runtime boundaries and costs

Add `tsr_scanner` with small modules grouped around invariants:

- `lib.rs` and `state.rs`: public configuration, state transitions, token views,
  diagnostics and snapshot ownership.
- `scan.rs`: main byte dispatch and punctuation, preserving ASCII fast paths.
- `identifier.rs` and generated tables: identifiers, keywords, Unicode escapes
  in names, and the distinct standard/JSX/regexp-group rules.
- `literal.rs` / `escape.rs`: strings, templates, escapes and numeric scanning.
- `trivia.rs` / `rescan.rs`: comments/directives, conflict markers, shebangs,
  trivia helpers, JSX/JSDoc modes and rescanning envelopes.
- `regexp/mod.rs` and `regexp/classes.rs`: regexp state, alternatives/groups,
  captures/references, quantifiers, legacy classes and Unicode sets.
- `utilities.rs`: the source-independent scanner helpers supported in S05;
  reuse the existing S04 line/position implementations rather than copying them.

Use the existing `tsr_ast::SyntaxKind` and `tsr_diagnostics::Message` identities.
Add the real token flags and comment-directive value types at the AST boundary,
with the pinned numeric values. Small target/language-variant definitions must
remain reusable by S06, without importing a fictitious compiler-options API.
Do not alter S03's generated payload layout merely to introduce the scanner.

The scanner borrows source bytes from `JsString`/`SourceText`; it does not decode
or validate the entire source on each scan. Source BOM decoding stays at the
`SourceText` boundary. A scanner text setter also accepts already-decoded raw
bytes, matching Go `SetText`.

Token text and ordinary identifier/raw-literal values are borrowed byte views.
Cooked values use owned shared byte storage only when transformation requires
it. Getters return `&[u8]`; an escaped JavaScript value is constructed explicitly
as `JsString`. Do not allocate an `Arc` or classify a new `JsString` for every
punctuation token. Numeric conversion caches are scanner-owned, lazily allocated,
and cleared on `Reset`, with retained allocation capacity where practical.
No lock belongs on the normal scanner path.

Scanner cursors, getters/resetters and diagnostic positions use `i64` for the
supported 64-bit Go `int` domain. Existing S04 position helpers retain their
`isize` APIs and `i32` line-start storage; `isize` has the same width on all four
supported targets. AST positions and `TextRange` storage remain `i32`, matching
Go's `core.TextPos`. `TokenRange` explicitly narrows through `TextRange::new`,
as Go's `core.NewTextRange` does; this does not clamp the scanner cursor.
The fixed-width scanner API makes its declared Go integer domain explicit
instead of tying it to Rust pointer width; 32-bit targets need a separate review.
Keep these conversions explicit at the later S06 integration boundary.
Checked slice conversions occur at access sites. `ResetPos` rejects a negative
position with the Go contract message but permits positions beyond the end. Distinguish
that operation from a later accessor's bounds panic. Token flags retain their
upstream bit operations; combined escape masks tested with `!= 0` mean any bit,
not an all-bits `contains` operation.

State includes cursor/full-start/token-start, kind, value, flags, directives and
the JSDoc-leading-asterisk counter. Configuration, callback and numeric caches
are separate. `SetText` resets token state while retaining configuration and
caches; `Reset` restores the default scanner, including `skipTrivia=true`, and
clears callback/configuration/caches. `ResetPos` preserves token/value/flags;
`ResetTokenState` clears them. Punctuation and EOF can expose a previous token's
value; JSDoc EOF can retain an earlier token start. Preserve these observations.

Snapshots retain cooked values and directives across `SetText` and later
restoration: `parser/jsdoc.go` saves state, swaps text, restores text, then
rewinds. A directive-count-only mark is insufficient. Expose an opaque,
non-Clone, must-use `Checkpoint`; `mark`, consuming `rewind`, and consuming
`commit` enforce same-scanner LIFO usage. A small active-checkpoint stack and
lazy owner identity enforce this contract. Successful speculative parses use
`commit`. Token values retain borrowed source views or shared cooked bytes;
directives use copy-on-write snapshot storage. Both `SetText` and `Reset` retain
outstanding checkpoints; rewind restores state only, never text/end,
configuration, sink or caches.

This API supports all current compiler call sites without exporting Go's
unrestricted copying of state slice headers and incidental backing-array alias
behavior. Foreign, consumed or out-of-order checkpoints are Rust API misuse,
tested separately; the Go comparison includes only valid LIFO traces. Include
nested rollback/commit, append-after-mark, text replacement/restoration and
rewind-after-Reset tests. Observe restored values separately from TokenText,
whose saved positions may be out of bounds in replacement text.

Diagnostics contain a borrowed static message, signed byte start/length, and
typed argument values. The error sink is optional and is invoked in
callback emission order; disabling it suppresses reporting, not recovery or
token flags. Do not sort diagnostics by source position: regexp flags are
reported before the earlier body, and unresolved references are reported last.
Arguments preserve raw bytes rather than passing through lossy JSON strings.
The harness compares code/key, positions, argument types/bytes and ordering;
English formatting is additional evidence, not a replacement for those fields.

## Necessary dependency slices

### Numbers

Implement the required `tsr_jsnum` slice: `FromString`, `Number.String`,
`ParsePseudoBigInt`, and the directly used validation/conversion helpers.
Document arithmetic and other future jsnum operations as unimplemented.
Port the Go grammar before calling a Rust float parser: signs, radix prefixes,
whitespace, exponent fragments, leading zeros, invalid digits, negative zero,
overflow and underflow remain distinct.

The formatting authority is the pinned `jsnum.Number.String`, including its
NaN/infinity and safe-integer fast paths (the latter formats negative zero as
`0`). Other values go through `internal/json.Marshal`, which delegates to
`github.com/go-json-experiment/json` at
`v0.0.0-20260623181947-01eb4420fa68`. Its `internal/jsonwire.AppendFloat`
selects exponential form for nonzero absolute values below `1e-6` or at least
`1e21`, calls `strconv.AppendFloat` with precision `-1`, then removes a leading
zero from a negative exponent. The thresholds belong to that pinned JSON
dependency; `Number.String` does not call plain `strconv` float formatting.
The oracle calls `Number.String` itself, not a modeled ECMAScript formatter.

Use a shortest-round-trip decimal formatter with reviewed API semantics to
reproduce that authority. The proposed dependency is
[`ryu-js`](https://github.com/boa-dev/ryu-js); the proposed arbitrary-precision
conversion is [`num-bigint`](https://github.com/rust-num/num-bigint), whose
float conversion implements ties-to-even. These supply concrete capabilities:
ECMAScript exponent/digit formatting and exact large-integer conversion.
Select `ryu-js` 1.0.3 (Apache-2.0 alternative, MSRV 1.71) and `num-bigint` 0.5.1
(MIT/Apache-2.0, MSRV 1.60), with its `num-traits` conversion interface. Their
sources/APIs/licenses have been inspected; lock transitive releases and run
dependency policy/MSRV checks, and verify output against the pinned Go code.
Library claims do not establish parity. Validate Go's base-0 bigint grammar
before the library call; underscore acceptance differs. `ParsePseudoBigInt`'s
default decimal branch preserves opaque trimmed bytes, while its recognized
second-byte radix branch panics on invalid base-0 input. Probe both directly.
Number-string whitespace uses its own pinned `unicode.Zs` set: U+0085 and U+200B
must not become numeric whitespace just because the scanner skips them.

Rejected alternative: vendoring a Ryu port inside `tsr_jsnum` would make this
repository maintain and audit the formatter fork without adding a required
capability over pinned `ryu-js`; the Go differential oracle remains necessary
with either choice. Likewise, hand-written arbitrary-precision conversion
would add rounding and large-integer invariants already supplied by `num-bigint`.

Tests include safe-integer edges, halfway rounding, subnormals, infinity,
negative zero, exponent thresholds at 1e-6/1e21, huge radix literals and seeded
float-bit cases. Preserve the scanner's separate legacy-octal behavior:
ignored `strconv.ParseInt(..., 8, 64)` overflow saturates at MaxInt64. Binary
and octal bigints normalize to decimal plus `n`; hex bigints keep lowercase hex.
Leading-zero numbers return before some identifier-adjacency checks. A preceding
minus token changes the legacy-octal diagnostic even across skipped trivia.

### Unicode and spelling suggestions

Identifier tables come from the pinned Unicode 15.1 source, not Rust's Unicode
version. Export table values through access-only Go code, preserve ranges and
strides, emit deterministic Rust, and compare production Go identifier predicates
over the complete Unicode code-point range. Keep table-only checks separately
counted from token traces. Cover literal and escaped integration at boundaries,
including supplementary characters, joiners, JSX hyphens and reserved words.

Regexp property aliases/values come from `scanner/unicodeproperties.go` and are
exact, case-sensitive strings. Exercise every accepted table entry and nearby
rejected spellings. Suggestions port `core.GetSpellingSuggestion` and its bounded
weighted Levenshtein helper, including byte/rune length distinctions and lexical
tie breaking. Suggestion lowercase uses pinned Go simple casing, which is a
different authority from the identifier tables; reuse S04 data where the exact
operation matches rather than silently substituting JavaScript case conversion.
The short-candidate eligibility test separately uses Go `strings.EqualFold`.
Export pinned `unicode.SimpleFold` cycles with the effective Go version and
compare against real EqualFold calls, including malformed bytes. Lowercase
equality would incorrectly match `i` and `İ`; retain that regexp discriminator.

The separate generator is `scripts/s05_tables.py`, exposed through
`python3 scripts/s05.py tables` for verification and
`python3 scripts/s05.py tables --write-manifest` for deliberate regeneration.
It writes `data/s05/tables.json`, `data/s05/tables-manifest.json` and
`crates/tsr_scanner/src/tables_generated.rs`. The manifest records the upstream
pin, named input/output hashes, effective Go version and Unicode authorities;
the generator reads the shared Go pin and workspace rustfmt edition.
`cargo xtask run scanner` verifies drift through the same code. This is not part
of `cargo xtask gen`. Do not modify
canonical `upstream/`, depend on live Unicode downloads during verification, or
expand the S03 generation gate to unrelated scanner implementation bodies.

## Lexical and regexp counterexamples

The implementation must include independently observed cases for:

1. Malformed bytes at token starts versus inside comments, identifiers and raw
   string copies; genuine U+FFFD also enters Go's binary-file marker path.
2. Lone and paired surrogate escapes, mixed fixed/braced pairs, speculative
   low-surrogate scanning that restores flags and avoids duplicate diagnostics,
   escaped malformed bytes, CR/LF/CRLF and U+2028/U+2029 continuations.
3. Tagged versus untagged templates, invalid escape retention, unterminated
   literals, quote flags and template-head/middle/tail transitions.
4. Every punctuation ambiguity, `?.` before a digit, greater/less-than rescans,
   `*=`, `#`, `?`, shebang/conflict markers and comment-directive ranges.
5. JSDoc tag terminators and prefix false positives, nested asterisk counters,
   backtick/comment-text mode, partial `@` completion, JSX names/attributes/text,
   multiline JSX settings and source-range scanning restoration.
6. Regexp omitted/false/true reporting modes: false skips grammar/flag errors
   but still diagnoses an unterminated literal. Always enter the real regexp
   grammar with explicit true in its parity group.
7. Regexp delimiter discovery remains a byte loop with non-nested classes,
   even for `v`; it is not replaced by a Unicode-aware regexp engine.
8. `s`/named captures at ES2018, `d` at ES2022, `v` at ES2024, modifiers and
   duplicate names in exclusive alternatives at ES2025; target None means
   Latest. Do not invent target diagnostics for `u`, `y` or lookbehind.
9. Arbitrarily long decimal quantifiers, reference overflow, forward references,
   group-name escape forms, class ranges, `\p`/`\P`, `\q`, unions/intersections/
   subtraction, string-containing sets and negation restrictions.
10. Regexp source-character decoding: non-Unicode astral input can emit a high
    surrogate without cursor advancement; the next call emits the low surrogate.
    The RuneError branch includes genuine U+FFFD and converts a byte to a Unicode
    character in one mode. A blanket per-parser-step progress assertion or a raw
    byte copy would change behavior.

The frozen state counterexamples have named action traces in
`data/s05/fixtures.json` (action indices are zero-based):

| Contract | Frozen trace and independently observed Go result |
| --- | --- |
| Punctuation and EOF retain previous values | `lexical/punctuation/1`: `?.` at bytes 32..34 retains `0.1`; EOF at byte 43 has empty token text and retains `#` |
| JSDoc EOF retains the prior token start/text/value | `rescan/jsdoc/tokens`: action 18 scans `\u0041` at 26..32 with value `A`; EOF actions 19..23 retain start 26 and the text/value while full-start becomes 32 |
| Reset beyond EOF succeeds independently of later getters | `state/positions`: action 1 resets to 4 in `abc`, action 2 observes end 4; `state/bounds-token-text` separately observes the later getter's bounds panic |

Regexp validation borrows the scanner state and restores temporary end/start/
flags as upstream does. Use production safe `stacker::maybe_grow` wrappers at
`scan_disjunction` and `scan_class_set_expression`, with every recursive call
going through the wrapper before entering the inner body. Start with named
64 KiB red-zone / 1 MiB segment constants. `stacker` 0.1.25 (MIT/Apache-2.0,
MSRV 1.63) stays on the calling thread and supports borrowed mutable closures.
This introduces the workspace's first native build dependency: `stacker` pulls
in `psm`, whose build script compiles C/assembly through `cc`. All four native
CI targets (macOS arm64/x64 and Linux arm64/x64) must compile this locked closure
and execute the production growth/unwind tests in both debug and release.
Dependency-policy and MSRV compilation alone are insufficient.
Validate the frame margin in debug and release on all four native CI targets.
Deep valid/malformed groups, lookarounds, nested v sets and mixed groups/sets
must exceed one segment from a modest 256 KiB starting stack. Test sink unwind
after growth and subsequent independent scanning. A test-only large reserved
stack is insufficient. Preserve resource failure as a failure, not a syntax
diagnostic. This does not establish wasm or Miri growth behavior, or complete
ADR 0011's future parser/checker/emit worker reservations.

## Oracle protocol, corpus and evidence

Build a Go scanner adapter from a fresh clean export of the pin, with only
access bridges where required. The S03 carried extraction patches must never
become the scanner's behavioral oracle. Reuse the verified-pin/subprocess/runtime
code where its contract matches. Use local pinned Go, `GOTOOLCHAIN=local`,
`-trimpath`, readonly modules and caller-compatible caches. Temporary exports
may live under `target/`; persistent Git metadata may not.

Freeze all 12,746 physical source inputs under `tsc/testdata/tests/cases`
(12,395 `.ts`, 349 `.tsx`, two `.js`) and all 108 bundled `.ts` libs. At this pin
that is 12,854 files / 12,427,814 bytes. Scan each with and without trivia:
25,708 raw-file cases. Select JSX by physical extension and keep compiler test
directives as source bytes. This is not virtual-file splitting, test-option
expansion or a claim about the separate E1 denominator.

Add a separately frozen set of explicit action traces, byte fixtures, target
matrices, numeric cases, seeded mutations and Unicode checks. Every request
records its ID, source hash/bytes, options and operations. The manifest freezes
both case identity and request digests; changing eligibility is a reviewed
change. Property/identifier probes do not inflate source-file counts.

Use two persistent processes and strict NDJSON streams. Requests carry version,
case ID, raw source bytes, options and actions. Responses have begin, ordered
observation and end records, with diagnostic events associated with the action
that produced them. Parent comparison validates exact field sets/types, IDs,
action/step sequence, numeric ranges, hex bytes, termination and complete case
coverage before computing metrics. Reject duplicate keys, non-finite numbers,
boolean-as-integer values, missing/extra/reordered fixed actions and unsupported modes.
Do not trust an adapter-supplied `pass` flag or digest as proof of equality.
Validate each stream's own contiguous ordinals, framing and end counts before
comparison. A legitimate different number of token observations is a measured
parity failure, not malformed protocol: drain both streams to their own case
end while retaining the first mismatch and bounded context.

Each case constructs a fresh scanner and callback/configuration before applying
its options; persistent processes share no scanner state across requests. Freeze
the detailed version-1 request/event schema and request manifests before the
scanner behavior port. Version and case/action IDs, discriminated operation and
argument shapes, raw hex byte fields, typed diagnostics, observation ordinals
and terminal/count fields are required. Document record-size and progress limits
against the largest frozen inputs. Validate requests outside panic recovery.
Include repeated content after deliberately changed configuration to detect
state leakage. The schema document and frozen digest are implementation inputs,
not generated from Rust behavior.

Stream observations to avoid retaining the full corpus's token JSON; one bundled
file is over 2 MB of source. Drain stderr separately, retain mismatch context
and raw error payloads, and use bounded progress deadlines. A whole-file scan
must reach EOF within `source_bytes + 2` observations (a conservative bound
including empty input and EOF). Scripted rescans are finite
actions and need not advance the cursor. Accessors are explicit observations:
an accepted `ResetPos(len+1)` must not be mislabeled a reset failure because an
automatic TokenText getter then panics. Unexpected panics fail; expected invalid
operation probes compare a narrow panic class and prescribed message where one
exists. Never accept panic-vs-panic without a reason classification.

Register `scanner` with frozen cases. Derive overall parity from the tracker's
validated per-case rows; compute regexp/rescan and diagnostic/value measurements
only from complete, validated nonempty subsets with frozen membership, mode and
operation coverage. Known witnesses require a binary-file diagnostic, regexp
grammar errors with true, suppressed grammar errors with false, and unterminated
regexp errors in both modes, so omitted callbacks cannot pass vacuously.
A mismatch is a measured
failure with its case and diagnostic retained. A missing tool, invalid protocol
or failed oracle build is a failed capture, not a false compiler result.

Route only E4 `diagnostics` and `token_value_bytes` to `run.scanner.*`, describing
their scanner scope. Keep `token_literal_bytes`, printing and encoder criteria
on their later integration producers. Update scanner.go's PORTS verification
requirements, which currently refer to the broader token-literal criterion.
Both scanner.go and regexp.go consume `run.scanner.diagnostics`.
`ast/diagnostic.go` retains its absent future `run.e4.diagnostics` obligation
alongside E1; scanner callbacks cannot certify AST diagnostic integration.
unicodeproperties.go requires current tables and regexp parity. Add a tracker
regression proving passing scanner metrics leave full E4 and AST diagnostics
incomplete. Audit every consumer so this cannot close future sprints.

Fingerprint actual scanner/jsnum/AST/diagnostic/text dependencies, workspace
manifests, adapters, helper scripts, frozen inputs, table generator and pin.
Include new crate manifests in all required workspace checks. Keep CI captures
independent after prerequisite validation, explicitly execute scanner/jsnum
debug/release tests (build and Clippy only compile tests), enforce all scanner metrics and S05,
and upload artifacts even when another measured gate fails. Verify cold and
restored cache behavior; do not cache mutable Git state through rust-cache.

## Ordered implementation and review checkpoints

1. Review this plan for ownership/cost, source fidelity, evidence scope, numeric
   dependencies, recursion and state snapshots. Resolve findings in writing.
2. Define shared scanner/state/diagnostic interfaces, the exact wire schema and
   dependency slices; freeze corpus/request manifests before the behavior port.
   Implement pinned tables and numeric helpers with focused oracle comparisons.
3. Implement lexical scanning, literal cooking, trivia/directives, rescans and
   JSX/JSDoc. In parallel, implement the regexp subsystem against agreed private
   scanner methods and construct the independent oracle/stream comparator.
4. Establish all frozen corpus/action inputs, run differential comparison and
   fix mismatches by tracing the pinned Go path. Preserve every failing case as
   a reproducer; do not shrink the denominator to obtain a pass.
5. Integrate function/file traceability, E4 routing, CI and sprint gates. Review
   APIs, malformed input, snapshot/error restoration, deep recursion, table drift
   and producer failure paths independently from ordinary token parity.
6. Run debug/release tests, fmt, strict Clippy, dependency policy, tracker tests,
   declared MSRV, scanner evidence and affected existing producers. Enforce
   S01–S05 and all quality metrics; record status from current evidence only.
7. Review the final diff, document results and limitations in `docs/S05.md`,
   create a new commit/PR from this branch and verify all native CI targets.
   Use normal pushes only; preserve unrelated user files.

## Plan review record

Three independent reviews challenged state/ownership, regexp/numeric behavior,
and evidence/tracking. Root reviewed callers, dependency source/licensing and
all metric consumers. The following findings are accepted and resolved above:

| Finding | Resolution and required verification |
| --- | --- |
| Immutable snapshots do not model arbitrary Go slice aliasing | Restrict the API to opaque LIFO checkpoints; support all current compiler usages, including commit and Reset; test Rust misuse separately |
| `GetECMALineStarts` is an owning-cache accessor, not a fresh computation | Explicit possible deferral; count 144/158 after fourteen deferrals |
| Source-sorted diagnostics would change behavior | Compare callback emission order, including flags-before-body and late references |
| Reserved test stacks hide production recursion risk | Guard both recursive grammar entry points with stacker; stress public calls and unwinding on modest native stacks |
| Lowercase equality is not EqualFold | Export pinned fold cycles; retain dotted-I and malformed-byte probes |
| Numeric whitespace and bigint grammar differ from convenient Rust helpers | Port the distinct grammar first; probe whitespace, separator placement and opaque pseudo-bigint inputs |
| Different token counts could be misclassified as protocol failure | Independently validate/drain framed streams; measure semantic divergence; regression splits one Go token into two Rust tokens |
| Empty diagnostics/subsets could pass together | Freeze nonempty groups and required operation/mode coverage; enforce known diagnostic witnesses |
| Protocol/corpus definitions could drift with implementation | Freeze exact schema and requests before behavior port; fresh scanner per case; test limits, framing, stalls and configuration leakage |
| Shared scanner metrics could certify future AST work | Keep AST diagnostic integration unknown, route only scanner criteria and test downstream non-closure |
| Existing CI does not run new crate tests | Add explicit debug/release executions alongside independent capture and S05 gates |

No review blocker remains unresolved. Verification results, any discovered
contract refinements and final limitations will be recorded in `docs/S05.md`;
new parity mismatches must be investigated against the pin, not waived.

### Fable follow-up on the initial plan, 7 September 2026

The review found missing precision in the position-width rationale, native
dependency obligation, rejected formatter alternative, formatting authority, generator commands and
named state traces. Those are now explicit above. The existing frozen traces
were replayed against Go; no runtime or corpus change was needed.

| E4 routing consumer | Reviewed disposition |
| --- | --- |
| `status/experiments.toml`: diagnostics and token-value criteria | Consume only `run.scanner.diagnostics` / `run.scanner.token_value_bytes`; units explicitly describe scanner observations |
| `PORTS.toml`: scanner, regexp and token flags | Consume the scanner metrics; partial scanner files remain `in-progress` |
| `PORTS.toml`: `ast/diagnostic.go` | Keeps absent future `run.e4.diagnostics` together with E1; scanner callbacks cannot verify it |
| Other E4 criteria and later sprint consumers | Literal types, printing and encoding remain unmeasured; full E4, full E3 and S09 stay incomplete |
| `xtask/src/tests.rs`: `scanner_evidence_does_not_complete_e4_or_ast_diagnostic_integration` | Reads the actual policy/ledger and proves passing scanner metrics plus E1 cannot close full E4 or AST diagnostic integration |
