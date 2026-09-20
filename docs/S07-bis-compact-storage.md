# S07-bis integrated compact storage

This is the implementation record for the typed-storage candidate selected in
[S07-bis](S07-bis-performance-plan.md). The retained control is CP1, manifest
`3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931`.
The lookup-reuse candidate remains rejected. Expanded tracing and replay remain
on hold. This record does not change any sprint or performance gate.

## Physical representation

All 192 core syntax shapes use a 24-byte header and generated typed payload rows.
The header stores independent kind and shape tags, flags, signed int32 positions,
a local parent word, and a payload ordinal. Its internal shape flag records
explicit binding-record presence without changing the concrete shape. Owned
`Node`/`NodeData` remain factory construction values; their sizes do not describe
retained core nodes.

Payload rows use four-row typed pages and ordinary scalar fields. Only the 72
composite shapes carry an atomic subtree-facts word. Applicable binding fields
are generated from the audited 84-shape inventory, including the direct
fallthrough-flow field. The five shapes with no syntax fields but with binding
fields receive distinct mutable rows. Truly empty rows need no allocation.

Core headers currently retain the existing arena policy (2, 4, ... 256 slots,
then 256-slot pages). This is not the model's fixed 32-header-page candidate;
its actual slack and directory cost must be charged by the integrated run.
The model is not an allocation or RSS result for this implementation.

Local node and auxiliary references use u32 words. Zero remains nil, and the
maximum word selects a field-specific full-ID escape for foreign namespaces or
the full u32 slot boundary. Public IDs stay owner-qualified. Identifier text uses
an even source-suffix length or an odd exception-pool handle; all other text
uses the pool. Finalizing a matching identifier range releases its temporary
pool slot, which later identifiers can reuse. Changing its range cannot change
its semantic bytes. Extended pool handles preserve the full supported domain.

Core syntax lists store u32 edges in 256-word pages, preserving stable auxiliary
backing identities, copied slice headers, nil versus empty, and overlapping
ranges. Reads resolve their physical owner and backing before indexing. Parser
construction still uses temporary vectors: removing the final boxed backing
is not the same as eliminating this temporary request traffic.

## Publication and mutation

Exclusive parsing and binding borrow the core's typed storage directly. Narrow
header and binding setters avoid reconstructing payloads; binding setters check
new link identities while retaining the parse proof. The result's symbol, table,
and flow namespaces are distinct from the syntax namespace. Ordinary eligible
core binding fields need neither the old binding map nor paged flow slots.

Already-published compatibility binding keeps cold side records. Lazy nodes
created under the publication lock retain owned payloads; their node and
auxiliary records publish together. A retained lazy node holds that payload
before later reads, so reading it cannot reenter the lazy lock. Lazy nodes created
during exclusive binding use the same checked cold binding path.

`NodeRead` carries physical-owner context; shared read helpers use sealed
`NodeAccess`. `BindResult` observations take an AST view rather than maintain a
node-to-row locator. Escaping text and nodes still require explicit ownership.
Unrestricted `NodeMut` edits stage one construction value and write it back when
the guard drops. Same-shape edits preserve binding fields and caches. Rare shape
changes preserve otherwise unrepresentable binding values in a cold override;
already-computed facts crossing a noncomposite shape are parked only for explicit
owned conversion and mutation, without adding a hot lookup to facts reads.
Runtime identity is assigned lazily in an owner-side map. Replacing a complete
node with a fresh construction value resets that comparison identity as before.

## Review and validation

Independent backend review found and reproduced three compatibility regressions:
subtree facts across a composite/noncomposite/composite mutation, runtime identity
after complete node replacement, and staged mutation's owner-error precedence.
All three have fixes and regression tests. Binder depth testing also exposed an
unguarded recursive initialized-pattern helper: its 464-byte debug frame at
1,200 levels exceeds the unchanged 512 KiB test stack. It now uses the existing
stack-growth guard; neither the test depth nor stack limit was relaxed.

The completed focused checks include 24 arena tests, 86 AST library tests,
subtree and other AST integration tests, seven compile-fail ownership doctests,
25 parser tests, and 28 binder tests. Workspace all-target/all-feature Clippy with
warnings denied, Rust 1.96 all-target compilation, formatting, generator drift,
and generator tests pass. The frozen candidate also passed the 13,094-file graph
comparison at one and eight workers, with all files binding in place and zero
fallbacks. These checks do not replace the broader ownership producers or final
Go-relative performance gates.

## First integrated screen: memory reduction, CPU regression

Candidate manifest:
`3f7e1f072e9d4e10fa247aefa0485ba68ebcba58333e11d5fe3bdf3ef357373e`.
The fixed screen retained all eight warmups and 56 measured children. Receipt
verification passed. CPU MAD is below 1% in both variants and worker modes; the
regression is not a noisy borderline result.

| Metric | CP1 control | Compact candidate | Candidate/control |
| --- | ---: | ---: | ---: |
| One-worker wall | 4.524 s | 7.265 s | 1.606 |
| Eight-worker wall | 1.053 s | 1.624 s | 1.543 |
| Allocated bytes, worse median | 4.643 GB | 3.185 GB | 0.686 |
| Peak RSS, worse median | 4.509 GB | 2.837 GB | 0.629 |

The wall-ratio upper 95% bootstrap bounds are 1.635 and 1.584. This candidate
**cannot replace CP1**: its measured memory saving comes with a large CPU
regression. The raw capture remains preserved while a bounded repair of the new
read path is tested; none of the acceptance thresholds changes.

Distance to the historical Go planning limits remains explicit:

| Metric | Candidate | Historical gate limit | Remaining reduction |
| --- | ---: | ---: | ---: |
| One-worker wall | 7.265 s | 2.938 s | 4.327 s |
| Eight-worker wall | 1.624 s | 0.634 s | 0.990 s |
| One-worker allocated bytes | 3.184523 GB | 2.035226 GB | 1.149296 GB |
| Eight-worker allocated bytes | 3.184523 GB | 2.035767 GB | 1.148757 GB |
| One-worker peak RSS | 2.834252 GB | 2.208948 GB | 0.625304 GB |
| Eight-worker peak RSS | 2.837283 GB | 2.217045 GB | 0.620238 GB |

These rounded historical denominators are planning comparisons, paired with the
same worker mode. Final E5/E6 acceptance uses the worse worker-mode memory ratio
and still requires fresh qualified Go/Rust captures.

A single Time Profiler capture of the exact normal candidate executable produced
8,165 running-CPU rows, including 7,215 ms on the worker. It adds no source hooks,
adapter or field replay. It identifies new read overhead: `AstView::node`,
`NodeRead::resolved`, and `StorageView::node_here` account for 594, 537, and 352 ms
of self samples, respectively. `AstPayloadStore::read` contributes another 272 ms
and `NodeRead::data` 99 ms. These samples justify inspecting the new routing and
known-shape payload-selection paths; they do not establish achievable savings.
The normal binary has no phase wrappers, so this capture does not claim exact
exclusive parse/bind attribution. Field-key hashing also appears in self samples
and warrants checking whether supposedly exceptional operations run on ordinary
local-field writes.

The subsequent [hashing audit](../tools/s07/performance-experiments/results/2026-09-09-compact-typed-first/fieldkey-audit.txt)
attributes a 423 ms stack union to field-key hashing, including its nested hasher
frames. The pinned `HashMap::remove` hashes before checking table emptiness.
Ordinary node construction, parent updates and inline binding writes were calling
it to clear nonexistent escapes. An explicit empty-map guard preserves nonempty
overwrite cleanup while avoiding that work. Tests cover foreign/full-slot to
local/nil overwrites in all five reference namespaces, with another escape still
present, and reuse of an allocated-but-empty map. This diagnoses a removable
operation; the combined repair's pipeline effect remains to be measured.

## Bounded read-path repair

Already-checked exclusive core reads now construct their borrowed `NodeRead`
directly. Generic resolution keeps its lazy path separate, and source metadata
is stored only where it is not already present in the core context. This retains
the owner-before-slot error order and the lazy publication guard.

Generated `NodeDataSource` selectors borrow that read and resolve only the
requested typed row after checking the actual shape. Known-shape visitors,
factory updates and handwritten field readers use them; exhaustive `data()`
matches keep their existing semantics. Shape remains independent of public kind.
The empty exceptional-reference cleanup described above is part of the same
candidate. No new profiling hooks or field replay are introduced.

Independent review of these changes found no actionable correctness issue.
Validation passes: 88 AST, 28 binder and 25 parser library tests; 11 arena/AST
ownership doctests, including explicit direct-selector lifetime protection;
workspace all-target/all-feature Clippy with warnings denied; Rust 1.96 all-target
compilation; formatting; pinned generation and observer drift. The first lint
failure and its narrow generator correction are retained in the logs. The
changed candidate passed its frozen graph comparison at both worker counts, with
13,094 files binding in place and zero fallbacks. Its fixed screen and receipt
verification also completed, retaining all eight warmups and 56 samples.

| Metric | Same-screen CP1 | Repaired compact candidate | Candidate/control |
| --- | ---: | ---: | ---: |
| One-worker wall | 4.363 s | 6.099 s | 1.398 |
| Eight-worker wall | 0.968 s | 1.315 s | 1.358 |
| Allocated bytes, worse median | 4.643 GB | 3.185 GB | 0.686 |
| Peak RSS, worse median | 4.509 GB | 2.837 GB | 0.629 |

Candidate manifest:
`2e6b9eab562633c98c47ae5465abf3fa07e7da9370617ff1001d0865d9f9d1b9`.
The [second review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-typed-repair/README.md)
retains the changed binary/source, complete graphs, samples and exported profile;
the unchanged CP1 bundle is referenced from the first archive.
The wall upper 95% bootstrap bounds are 1.408 and 1.376, with both variants' MAD
below 0.6%. **Still not promoted.** The controls differ between captures, so the
two candidate medians must not be subtracted as a paired measurement of the
repair. The remaining same-screen CPU regression is about 40% / 36%.

| Metric | Repaired candidate | Historical same-mode limit | Remaining reduction |
| --- | ---: | ---: | ---: |
| One-worker wall | 6.098883 s | 2.937627 s | 3.161256 s |
| Eight-worker wall | 1.314566 s | 0.633963 s | 0.680603 s |
| One-worker allocation | 3.184523 GB | 2.035226 GB | 1.149297 GB |
| Eight-worker allocation | 3.184523 GB | 2.035767 GB | 1.148756 GB |
| One-worker peak RSS | 2.834317 GB | 2.208948 GB | 0.625369 GB |
| Eight-worker peak RSS | 2.837316 GB | 2.217045 GB | 0.620270 GB |

A second single native CPU sample uses the repaired normal binary. Its worker
has 6,083 ms sampled CPU; `AstView::node`, `BindBuilder::node`,
`StorageView::node_here` and `for_arena` contribute 649, 265, 320 and 120 ms self
samples. No exact phase attribution or removable-cost sum is inferred. The
remaining repair therefore targets the physical read descriptor: borrow the
already-selected owner and defer payload context until a payload is requested.
Transaction reads must preserve their split owner borrows and lazy guard rules.

Completion validation still constructs `NodeDataRead` and immediately rematches
it. Generate stored-row reference validation directly while preserving every
check and its order. Restrict text-range dispatch to suffix-capable identifiers;
other text fields are independent of node ranges. This forms the next changed
candidate. Keep small typed pages and boxed public construction inputs unchanged
until their costs warrant a separate decision. CP1 stays the control; neither
completed compact screen qualifies for the infrastructure exception or promotion.

Text attribution identifies another new cost: converting raw-word `NodeText`
into owned `JsString` reclassifies the source slice, whereas the old owned text
cloned its existing validity tag. Of 240 ms under `from_utf8` in this sample,
136 ms occur under compact owned conversion, including 83 ms in contextual
keyword checking. Keyword lookup only needs borrowed bytes, so keep the text
borrow scoped to that lookup and release it before diagnostic mutation. Leave
declaration-name ownership and the S04 string contract intact. Ordinary borrowed
compact text reads already avoid classification. This caller change belongs
to the combined repair, not a separate claimed CPU win.

The thin physical-owner implementation is complete. A build-artifact size check
measures `NodeRead` at 40 bytes (previously 56) and `NodeDataSource` at 16 bytes.
Core and transaction reads borrow their selected physical owner/transaction;
lazy reads retain their publication guard. No source clone, owner retention or
wider import permission is added. Direct row validation preserves parent-first
and schema callback order, including nil handling and immediate error return.

Independent review finds no actionable ownership issue. The 89 AST, 28 binder
and 25 parser library tests pass, including the newly added validator regression
and raw/cooked contextual-keyword fixture. Eleven arena/AST ownership doctests,
workspace all-feature/all-target Clippy, Rust 1.96 all-target compilation,
generation/observer drift and formatting pass. These are correctness and size
checks. Its frozen full-workload graph comparison and fixed screen have now
completed, with all 13,094 files binding in place and zero fallbacks in both
worker modes. Receipt verification passes; all eight warmups and 56 samples
remain intact.


## Thin-owner result and one row-allocation trial

| Metric | Same-screen CP1 | Thin-owner candidate | Candidate/control |
| --- | ---: | ---: | ---: |
| One-worker wall | 4.909 s | 6.411 s | 1.306 |
| Eight-worker wall | 1.380 s | 1.802 s | 1.306 |
| Allocated bytes, worse median | 4.643 GB | 3.185 GB | 0.686 |
| Peak RSS, worse median | 4.509 GB | 2.837 GB | 0.629 |

Candidate manifest:
`5a96a275e555e3156520d02c90b02866a55f2323ac080cb8e4cde1e48a8a2750`.
The [third review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-thin/README.md)
retains the frozen sources/binaries, complete graph streams and all measurements.
Both CPU modes still fail: upper 95% bootstrap ratios are 1.374 and 1.337,
with relative MAD below 2% for both variants. **Not promoted; CP1 remains the
control.** Both variants are slower in this capture than the preceding one;
compare their same-screen ratios, not candidate medians across captures.

| Metric | Thin-owner candidate | Historical same-mode limit | Remaining reduction |
| --- | ---: | ---: | ---: |
| One-worker wall | 6.410711 s | 2.937627 s | 3.473084 s |
| Eight-worker wall | 1.802183 s | 0.633963 s | 1.168220 s |
| One-worker allocation | 3.184524 GB | 2.035226 GB | 1.149298 GB |
| Eight-worker allocation | 3.184527 GB | 2.035767 GB | 1.148760 GB |
| One-worker peak RSS | 2.834350 GB | 2.208948 GB | 0.625402 GB |
| Eight-worker peak RSS | 2.837283 GB | 2.217045 GB | 0.620238 GB |

Test one allocation-policy change next: an ordinary `Vec<T>` for each populated
shape, replacing four-row boxed pages. Row references borrow the owner; exclusive
mutable construction cannot grow storage while those references are usable.
Published core rows never grow, and lazy payloads use separate storage. Stable
addresses during exclusive growth were an internal test condition, not a public
requirement. Preserve ordinal bounds, atomic facts and every owner/lazy lifetime
check. Keep normal vector growth, without workload-specific reserves or sizing.

This candidate can remove the page directory and per-four-row allocations, but
vector growth adds requests and capacity slack. Measure both costs with the same
full graph comparison and fixed pipeline screen against CP1. No page-size matrix,
field trace or replay harness is added. If it also fails, reassess the integrated
representation's remaining costs before any further policy tuning. Memory savings
alone do not qualify a CPU-regressing implementation for promotion.


## Vector policy result: rejected

The fourth candidate (`e31f485`, manifest
`5d2c44ae63d0399ef79ce4f9902557efcf4bcf894134b9cc19170a8c6cc5c503`)
passes all 13,094 graphs at one/eight workers, with zero binding fallbacks.
All fixed measurements and receipt verification completed.

| Metric | Same-screen CP1 | Vector candidate | Candidate/control |
| --- | ---: | ---: | ---: |
| One-worker wall | 4.535 s | 5.828 s | 1.285 |
| Eight-worker wall | 1.038 s | 1.307 s | 1.259 |
| Allocation, worse median | 4.643 GB | 3.536 GB | 0.762 |
| Peak RSS, worse median | 4.509 GB | 2.869 GB | 0.636 |

CPU upper 95% ratios are 1.301 / 1.274; relative MAD is below 1.2% for both
variants and all modes. This still fails CPU non-regression, and allocation
is approximately 351 MB higher than the thin paged candidate's capture.
That memory comparison is descriptive across captures, not a paired timing
claim. Ordinary vector growth is rejected and its six implementation files are
restored to the thin four-row-page version. No further row-policy matrix is
queued. The [fourth archive](../tools/s07/performance-experiments/results/2026-09-09-compact-vec/README.md)
retains the rejected implementation, binaries, all graphs and samples.

| Metric | Vector candidate | Historical same-mode limit | Remaining reduction |
| --- | ---: | ---: | ---: |
| One-worker wall | 5.828119 s | 2.937627 s | 2.890492 s |
| Eight-worker wall | 1.307331 s | 0.633963 s | 0.673368 s |
| One-worker allocation | 3.535607 GB | 2.035226 GB | 1.500381 GB |
| Eight-worker allocation | 3.535607 GB | 2.035767 GB | 1.499841 GB |
| One-worker peak RSS | 2.865840 GB | 2.208948 GB | 0.656892 GB |
| Eight-worker peak RSS | 2.868740 GB | 2.217045 GB | 0.651695 GB |

## Next combined implementation: binding storage and request traffic

Continue the planned CP4 work on the paged compact implementation, still as an
unpromoted combined candidate against CP1. A native sample of that exact frozen
thin paged binary is retained with the fourth archive to audit remaining CPU
costs. It uses no field trace or replay and does not establish phase timings by
itself. Do not add another small routing-policy trial.

The historical census still prices 2,459,867 symbols at 112 bytes, 2,950,559 flow
records at 48 bytes, 1,297,945 flow lists at 16 bytes, and 2,446,623 declaration
backings containing only 2,454,685 capacity cells. Straightforward width changes
explain about 162 MB of used-record arithmetic; they do not explain the entire
remaining 0.62 GB RSS or 1.15 GB allocation gap. Name/table duplication, temporary
traffic and capacity must also be charged and measured.

- Store ordinary flow records in 20 bytes with an independent full tag and open
  flags, outlined synthetic switch/reduce payloads, and full-ID escapes. Flow
  list cells use two local words. Borrowed readers and narrow writes avoid
  reconstructing full payloads for flags or antecedents. Keep cold unrestricted
  mutation only for actual compatibility callers.
- Pool declaration cells into 256-word pages with stable backing descriptors.
  Keep nil versus allocated empty, full capacity, copied slice headers, shared
  writes, reslicing, pinned Go growth rounding and old backing visibility. Append
  from nil writes directly without allocating a temporary vector.
- Compact symbols with owner-relative links and names shared with table entries.
  Use a byte-equivalent name pool and table facade; preserve absent versus
  present-null entries and arbitrary malformed/generated names. Keep exact
  runtime symbol identity and retained-owner behavior. This work includes both
  record storage and consumers, not a second name cache layered over old maps.
- Remove the parser's eager 16-element reservations in both list builders as
  part of request-traffic work. Keep ordinary vector growth and all list behavior;
  measure the whole candidate, rather than assigning a predicted saving.

The table/name implementation can use the already locked `hashbrown` 0.17.1
`HashTable` with byte equality and the existing randomized hashing semantics.
Prefer that safe table API over writing a bespoke hash table or duplicating
`JsString` keys in both a map and a pool. Making the existing transitive dependency
direct requires the same dependency, MSRV and four-native-target checks.

Preserve checked owner/slot namespaces, full public ID domains, error timing,
cycles and lazy/published compatibility. Add focused counterexamples for compact
escapes, flow flags versus payload tags, declaration capacity aliasing and name
bytes. Run affected tests and complete graphs before the next fixed screen.
The new combined result must satisfy the existing CPU and memory rules; neither
model arithmetic nor independent category savings authorize promotion.


### CP4 implementation and review

The combined implementation is present. Stored symbols are **56 bytes**,
including their existing per-symbol `AtomicU64` runtime identity; no sparse-ID
saving is claimed. Their names refer to the same canonical byte pool used by
symbol tables. Ordinary table entries are eight bytes; a table that receives a
foreign symbol or a name index beyond its compact domain converts to full-ID
entries. Full public symbol slots remain representable. The pool uses eight-byte
range descriptors, with rare full-`usize` range escapes, and stores each selected
byte sequence once per owner. Owned `JsString` reconstruction is explicit and
reserved for actual owning consumers and cold unrestricted edits.

Flows use the tested 20-byte layout and eight-byte list cells. Synthetic
switch/reduce payloads currently use cold slot-keyed maps, whose overhead must
be included in measurement. Narrow flow and symbol setters resolve the receiver
once before changing encoding state. Declaration backings share 256-word pages
with stable descriptors; singleton append avoids a temporary Vec. Both parser
list builders now use ordinary empty-vector growth instead of reserving sixteen
entries eagerly. Published/lazy compatibility still passes through its existing
owner checks.

Resolvers use semantic symbol/table/declaration readers, including owned
transient symbols where appropriate. Retained symbols retain the exact binding
owner and expose scoped symbol reads. The graph observer preserves byte sorting,
reference identities, private-name canonicalization and full declaration
capacity visibility. These API changes do not relax graph comparisons.

Focused tests and the affected library suites pass: 106 AST, 28 binder, 25 parser
and 12 compiler tests. Fourteen arena/AST ownership doctests include new symbol
and name-pool lifetime checks. Independent review caught a public-supertrait leak
of the runtime-ID atomic; the fix exposes only controlled assign/observe
operations, with a compile-fail regression against direct atomic access. The
follow-up review found no further issue. Workspace all-target/all-feature Clippy
and Rust 1.96 compilation pass. Pinned generation, observer drift, formatting
and offline dependency bans/licenses/sources checks also pass. Broader E3 instrumentation and fresh Go-relative
acceptance remain prerequisites for promotion, after a promising fixed screen.

A supplementary native CPU sample of the preceding thin paged candidate has
6,251 ms of worker samples: actual entry-frame unions attribute 3,467 ms to
parse, 2,767 ms to bind, no overlap and 17 ms to neither. These are sampled CPU
weights, not phase elapsed times; 1,179 ms of other/main-thread work is excluded.
The AST-access union is 1,205 ms, while full borrowed payload reconstruction is
132 ms inclusive. Exact completion validation is 352 ms and construction-edge
validation 167 ms with no overlap. These overlapping categories must not be
summed; the source/sample does not establish that all access or allocation work
is removable. They support the integrated storage/traffic work without another
field trace or lookup-only trial.

### CP4 result: memory improves, CPU still fails

The combined candidate (`b731d85`, manifest
`5495f69555cd2ea580e6e2351ab649b6f228005529ff838654c0bb57c3e648c3`)
passes all 13,094 complete workload graphs at both worker counts. Every file
binds in place, with zero fallbacks. All eight warmups and 56 samples were
retained, and receipt verification passes.
The [fifth review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-binding/README.md)
retains the frozen changed source/binaries, complete graphs, raw measurements,
validation logs and this candidate's native CPU exports.

| Metric | Same-screen CP1 | CP4 candidate | Candidate/control |
| --- | ---: | ---: | ---: |
| One-worker wall | 4.546 s | 6.202 s | 1.364 |
| Eight-worker wall | 1.058 s | 1.399 s | 1.322 |
| Allocation, worse median | 4.643 GB | 2.574 GB | 0.554 |
| Peak RSS, worse median | 4.509 GB | 2.505 GB | 0.555 |

CPU upper 95% bootstrap ratios are 1.380 / 1.344. Relative MAD is below 1.3%
for both variants and all modes. **Not promoted; CP1 remains the control.**
This implementation removes substantial memory, but still fails the unchanged
CPU rule. Differences between candidate medians in separate captures are not
paired improvement estimates. No threshold or sample count was changed.

| Metric | CP4 candidate | Historical same-mode limit | Remaining reduction |
| --- | ---: | ---: | ---: |
| One-worker wall | 6.202202 s | 2.937627 s | 3.264575 s |
| Eight-worker wall | 1.398779 s | 0.633963 s | 0.764816 s |
| One-worker allocation | 2.573774 GB | 2.035226 GB | 0.538548 GB |
| Eight-worker allocation | 2.573778 GB | 2.035767 GB | 0.538012 GB |
| One-worker peak RSS | 2.501706 GB | 2.208948 GB | 0.292758 GB |
| Eight-worker peak RSS | 2.504884 GB | 2.217045 GB | 0.287839 GB |

These historical limits support planning; fresh qualified Go-relative evidence
is still required for acceptance. The exact frozen CP4 normal binary also
completed a native CPU capture with the full input digest and expected node,
symbol and diagnostic counts. Review its sampled stack unions before selecting
further CPU work; memory progress does not explain away the regression.

### CP5 implementation decision: retain final parent and metadata checks

Keep checked construction and carry a private construction-edge proof into
completion. A clean builder may omit only the final payload/list/backing checks
already established by construction. Final parent validation remains: parent
links are assigned after insertion, and the public parent setter deliberately
defers invalid-target failure until completion. Source metadata also remains
validated because its public mutable access is used during every parse.

Unrestricted node/list edits, including edits from factory hooks, invalidate the
proof and use the existing full completion scan with its existing failure order.
Add narrow list-location and modifier-flag setters for the ordinary parser path;
these cannot change graph edges. Do not introduce per-node proof maps or move
the parent check into every write: parsing may assign a parent repeatedly, so
that would move or increase checks rather than establish a saving.

This refines CP5's initially conservative completion-scan policy: checked
construction stays authoritative for the specific immutable edges it proves,
while completion checks the remaining obligations. Counterexamples must cover
invalid final parents, source metadata, mutated payloads/list backings, hooks,
imported owners and the clean versus dirty completion paths. The preceding thin
sample's 352 ms whole completion weight is an upper bound, not an expected
saving; the required parent/metadata work remains. No CPU-gate closure is claimed.

The proof is now implemented. A builder with hooks, retained imports or any lazy
reservations uses the full scan. A failed lazy initializer still burns its
reservations and therefore conservatively disables the shortcut. Independent
review found no production issue; it corrected a test that incorrectly expected
such a builder to remain core-only. All 112 AST, 25 parser and 28 binder library
tests pass, including nine construction/publication proof tests. Workspace
all-target/all-feature Clippy and Rust 1.96 compilation pass. These checks do not
constitute a new performance result for CP5.

### Matched phase elapsed diagnostic: both phases regress

The [phase archive](../tools/s07/performance-experiments/results/2026-09-09-compact-phases/README.md)
contains separately built frozen CP1 and CP4 production sources with the same
existing phase adapter. Both use the consuming binding path, Rust 1.97.1,
normal mimalloc, the same release configuration and `tsr_ast/layout-profile`.
The adapter source is unchanged and identical between builds. One warmup and
three alternating measured children per revision ran on one worker; all eight
children preserve the complete input digest, work counts, 13,094 in-place files
and zero fallbacks. The schedule was fixed before capture and was not extended.

| Elapsed phase median | CP1 | CP4 | Difference | CP4/CP1 |
| --- | ---: | ---: | ---: | ---: |
| Parse, including completion | 2.568171 s | 3.509063 s | +0.940892 s | 1.366 |
| Bind and publication | 2.137196 s | 2.971233 s | +0.834037 s | 1.390 |
| Diagnostic pipeline | 4.720425 s | 6.495556 s | +1.775131 s | 1.376 |

These clocks include timer overhead and descheduling. Binding includes the full
publication/validation transition; it is not an isolated binder CPU timer.
Three samples provide attribution, not bootstrap acceptance or a new control.
Do not compare these adapter medians numerically with a differently built normal
screen or add separately taken phase medians as an exact pipeline decomposition.

The result rules out the proposed explanation that a binding improvement is
hidden by slower parsing: both phase groups are slower in this comparison. It
does not isolate the causal contribution of inline fields from the other layout
changes. The compact implementation has not delivered its predicted net binding
speedup; smaller memory use and removed maps did not establish that outcome.

The exact CP4 native sample supports this interpretation without substituting
for the clocks: 6,251 ms worker CPU weight splits into 3,387 ms parser-entry,
2,854 ms consuming-binder-entry and 10 ms unattributed samples, with no overlap.
AST access has 1,190 ms of worker samples across four actual paths. Exact outer
completion validation is 369 ms, constructor edge validation 151 ms, and payload
insertion 242 ms. These overlapping categories include necessary work and must
not be summed as removable cost. New name/table/symbol/flow/declaration families
have a combined observed union of 362 ms; they do not justify another isolated
hasher or small lookup experiment.

### Next combined construction and access candidate

Keep the implemented CP5 proof and remove avoidable intermediates on the ordinary
typed construction/read path. Judge the complete changed implementation in one
full-workload screen; these components do not face separate promotion gates.

Generate concrete `Factory::new_<shape>_data(kind, XxxData)` entry points. Default
implementations forward to the existing `new_node` path, preserving custom and
lazy factories. `AstBuilder` validates the supplied edges in the same order,
inserts the selected typed payload directly and then runs its create hook.
Keep the generic constructor as the compatibility entry. Reuse the same generated
packing routines from both paths. Ordinary construction should not allocate a
boxed owned payload merely to unpack it into an already selected typed row.
Preserve existing text-counter-before-validation behavior, and validate before
node-count increment or ID/row allocation. Finish-time parent attachment is
unchanged, including hook-visible edits.

The original measured boxed-payload widths joined with verified physical core
shape counts price 3,149,779 such payloads at 139.760 MB. This is a request-saving
opportunity, not a current allocation attribution or promised saving: extra
construction/replacement, allocator behavior and optimized code still need to be
measured. The native profile attributes 242 ms to payload insertion, including
92 ms of allocator samples; required row storage remains within those figures.

Also provide direct checked core reads from an already selected physical owner.
For parsed views, or the matching exclusive binding result, construct `NodeRead`
from that borrowed core header without first constructing a generic `StorageRead`.
Keep namespace/slot checks and the same borrow lifetime. Published overlays,
mapped siblings, imported owners and lazy records retain the general resolver.
This does not cache headers or change the number of semantic reads; it removes
an intermediate representation within the combined construction/access change.
The 1,190 ms AST-access union is a cost bound, not a saving estimate.

Countertest default/custom factory dispatch, hooks and counter/error order,
kind-versus-shape mismatches, large payloads, lazy/imported graphs and overlay
selection. Complete graph comparisons and the fixed combined screen before
claiming a net improvement. Hold auxiliary compaction while testing this change:
its current estimate is only 95–107 MB retained and requires a core/lazy auxiliary
API split; neither that estimate nor smaller rows establish a CPU benefit.

The combined implementation now includes CP5, all 192 generated typed factory
entry points and the selected-core read path. The generated generic and typed
constructors share the same packing methods. Compatibility paths remain in
place for custom/lazy factories and published overlays or foreign owners.
Independent review found no substantive issue in dispatch, validation/counter/
hook order, kind/payload pairing or read routing.

Validation passes: 112 AST, 25 parser, 28 binder and 12 compiler library tests;
seven generated-factory integration tests; the selected-core arena test; four
arena and eleven AST documentation tests; pinned generation and drift checks;
workspace all-target/all-feature Clippy; and Rust 1.96 compilation. These checks
establish implementation readiness for the frozen combined screen, not a
performance result or promotion. Full workload graph comparison precedes that
screen; broader ownership instrumentation and fresh Go acceptance remain later
prerequisites.

### Combined construction/access screen: memory retained, CPU still slower

Frozen revision `f3426ac`, manifest
`825fe92c286d1fb6c72324636e9afa8a07badc6b30263bc491f52d2e2ddbe69e`,
combines CP5 with direct generated typed construction and selected-core reads.
The normal executable SHA is
`ae3adb7fe14078c08d4d04e4bc032d8a3a17289dbeadac3f8f6e779383c8f88d`.
All 13,094 graphs match Go at one and eight workers; all files use in-place
binding, with zero fallbacks. The fixed eight warmups and 56 measured children
complete successfully, and receipt replay verifies their full artifact inventory.

| Metric | One worker: candidate / CP1 | Ratio | Eight workers: candidate / CP1 | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Wall | 5.177888 / 4.714439 s | 1.0983 | 1.504122 / 1.397319 s | 1.0764 |
| Allocated | 2.434015 / 4.642571 GB | 0.5243 | 2.434017 / 4.642571 GB | 0.5243 |
| Peak RSS | 2.502164 / 4.506649 GB | 0.5552 | 2.505589 / 4.509188 GB | 0.5557 |

CPU upper 95% ratios are **1.1231 / 1.1128**; lower bounds are 1.0891 / 1.0276.
Timing relative MAD is 0.56% / 0.72% for the candidate and 0.80% / 2.15% for
CP1. The measured combination saves 2.209 GB of requests and about 2.004 GB RSS
against its same-screen control, while adding 463 ms / 107 ms wall time.
It therefore remains **experimental, not promoted**. CP1 remains the control.

This compares the whole compact candidate with CP1. It does not isolate CP5,
typed construction or the read path, nor establish a paired improvement against
CP4. The 139.759 MB difference from CP4's recorded allocation median is consistent
with the box-traffic projection, but comes from separate captures and is not a
new component attribution. The eight-worker control is slower than in the CP4
capture; cross-capture wall-time subtraction is particularly misleading here.

| Remaining distance to historical Go-derived limits | One worker | Eight workers |
| --- | ---: | ---: |
| Wall above limit | 2.240 s (1.763× limit) | 0.870 s (2.373× limit) |
| Allocated above limit | 398.789 MB | 398.250 MB |
| Peak RSS above limit | 293.216 MB | 288.544 MB |

These are planning distances using historical Go medians, not fresh paired Go
acceptance. Both CPU modes and both memory gates still need to pass together.
The measured memory result supports retaining the combined experiment for
further work; it does not waive the CPU gates or justify a weighted score.

The exact new normal binary's native sample validates all workload counts and
the loaded digest. It has 6,031 ms of worker CPU samples: 3,065 under parsing,
2,938 under consuming binding and 28 unattributed, without phase overlap.
Completion validation under its new method name accounts for 83 ms; querying
only the old method would incorrectly report its disappearance. Parent
attachment and header finishing together account for 541 ms of disjoint sample
weight, including required parent writes and preservation of identifier text.
The native run takes 6.694 s versus the separate normal-screen median 5.178 s;
this perturbation prevents treating sampled weights as a prediction of elapsed
savings. The retained audit script resolves the exact executable and XML hashes.

No demonstrated single bounded change closes both remaining gates. Continue with
one combined finishing/allocation direction: use concrete owner-local finishing
and parent attachment where it preserves the generic path's observation order,
and price the current physical allocation/capacity costs before changing their
policy. Preserve custom factories, hooks, imported/lazy nodes, final range text,
and construction proofs. Do not reopen a row-policy matrix or promote isolated
parts. The 541 ms sampled region is an upper bound on work to inspect, not a
forecast of removable time.

The [complete review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-construction-access/README.md)
retains this screen, exact sources/binaries, generator sources, graph streams,
raw observations, successful and failed development checks, and native CPU
exports with their audit. The regenerated tracker views reflect stale evidence
as pending; this diagnostic screen has not been substituted for tracker capture.

### Next combined implementation: parent attachment and auxiliary records

Checked owner-local parent attachment is implemented. It streams parent writes
only while construction has proved the immutable child/backing structure, the
owner is core-only, and no reference escapes require mutation of the read side.
The parser still finishes the header, clears its error flag, then attaches
parents. The default custom/hooked/imported/lazy/dirty paths retain scratch
gathering and their exact failure behavior; a later malformed backing fails
before writes, while a later invalid child ID can fail after earlier writes.
Nonempty scratch after a recovered failed enumeration also retains the default.
The binder's checked local read has an inline hint and a separate compatibility
helper, with its existing predicate and borrow lifetime unchanged. No individual
CPU saving is claimed.

Independent review found no actionable issue. Validation passes 119 AST,
26 parser and 26 arena library tests, six arena documentation tests, pinned
generation/drift, workspace Clippy, Rust 1.96 compilation and formatting.
The [CP6 accounting](../tools/s07/performance-experiments/results/2026-09-09-cp6-accounting/README.md)
rules out treating payload-page overhead as the full remaining memory deficit.
Keep that policy and implement the
[compact auxiliary plan](S07-bis-auxiliary-plan.md) next. Screen this coherent
combination after implementation; these intermediate checks are correctness
evidence, not another performance capture.

### Parent/auxiliary combined screen: memory closer, CPU still slower

Frozen revision `2acbc06`, manifest
`994668cbfcee24edd93f47844a227dacb16eb1ef71f5a544d654f9684cc90dbb`,
combines owner-local parent attachment and the narrow binder read hint with
compact core auxiliary records. Normal executable SHA:
`940bbf2ab71d7d6473d850acfde6ccc93298c743f5d69cb2df662a92afe995c0`.
All 13,094 graphs match Go in both worker modes, with all files bound in place
and zero fallbacks. All eight warmups and 56 measured runs complete; receipt
replay verifies the raw observations, artifact inventory and graph prerequisite.

| Metric | One worker: candidate / CP1 | Ratio | Eight workers: candidate / CP1 | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Wall | 5.262963 / 4.704426 s | 1.1187 | 1.158306 / 1.082522 s | 1.0700 |
| Allocated | 2.330772 / 4.642571 GB | 0.5020 | 2.330776 / 4.642570 GB | 0.5020 |
| Peak RSS | 2.318549 / 4.506698 GB | 0.5145 | 2.321760 / 4.509270 GB | 0.5149 |

CPU upper 95% ratios are **1.1750 / 1.1015**, with lower bounds 1.0905 / 1.0386.
Timing relative MAD is 1.55% / 0.53% for the candidate and 1.30% / 2.48% for
CP1. Against its same-screen control, the whole combination saves about
2.312 GB of requests and 2.188 GB RSS while adding 559 ms / 76 ms wall time.
The runner records `regressing_or_uncertain`; both timing intervals are above
parity. This remains **experimental, not promoted**, with CP1 retained as control.

The implemented auxiliary policy's static projection is 115.677 MB less retained
storage and 96.154 MB fewer requests, including its actual four-row directories
and 1.813 million additional allocation calls. The combined allocation median
is about 103.24 MB below the preceding construction/access capture, but that is
a separate capture of several changes and not an isolated auxiliary saving.
Neither modeled retained bytes nor that difference predicts RSS. In particular,
the preceding eight-worker CP1 median was 1.397 s versus this batch's 1.083 s;
subtracting candidate wall medians across those batches would be misleading.

| Remaining distance to historical Go-derived limits | One worker | Eight workers |
| --- | ---: | ---: |
| Wall above limit | 2.325 s (1.792× limit) | 0.524 s (1.827× limit) |
| Allocated above limit | 295.546 MB | 295.009 MB |
| Peak RSS above limit | 109.601 MB | 104.715 MB |

These are historical planning distances, not fresh paired Go acceptance.
Memory is closer, but all final CPU and memory gates remain open. Component
tradeoffs remain admissible within the measured combined experiment; the memory
result alone does not promote this combination or excuse the CPU regression.

Validation includes 126 AST, 26 parser, 28 binder, 12 compiler and 26 arena
library tests; 29 AST and five encoder integration tests; 13 AST and eight arena
doctests; workspace Clippy and declared Rust 1.96 checking; pinned generated
source/client-byte checks; and formatting. Strict-provenance Miri passes all
26 arena tests, nine selected auxiliary tests and seven parent tests.
AddressSanitizer passes all 26 arena and 126 AST library tests with the existing
macOS leak-detection setting. These scoped runs do not claim fresh complete
ownership-producer evidence. Independent implementation and arithmetic reviews
found no actionable issue.

The [review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-auxiliary-parent/README.md)
preserves 467 files: frozen source/binaries, graph streams, all raw samples,
receipt, exact generator inputs, successful and failed development checks,
instrumentation commands/results, and auxiliary accounting. Every archive member
was read back and hash-verified. Unchanged CP1/helpers and the earlier physical
census remain pinned through their respective prior archives.

Continue with the reviewed [parser list construction plan](S07-bis-parser-list-plan.md):
remove short temporary buffers while keeping recursive edge publication and
custom/lazy dispatch unchanged. Its conditional opportunity is roughly 100 MB
of requests across represented successful backings, with actual coverage and CPU
effect unmeasured. This does not close the remaining gates by itself. Build it
into the coherent candidate and measure once; keep the row policy, avoid another
traversal experiment, and record the remaining CPU deficit explicitly.

### Inline parser-list combined screen: request traffic lower, CPU still slower

Frozen revision `03f9aa3`, manifest
`c4eb05e07d2df5aafabd3e8849a0da36dabecad49586763878f980e25fc8cb88`,
adds inline buffers for short eager-parser lists to the preceding combination.
Normal executable SHA:
`ef4149e8bee81dc5a40d182bb0dc8d1a11e731f08415c24abca41c254d862236`.
All 13,094 graphs match Go in both worker modes, with 13,094 in-place binds and
zero fallbacks. All eight warmups and 56 measured runs complete. Receipt replay
and independent raw-observation/statistics verification pass.

| Metric | One worker: candidate / CP1 | Ratio | Eight workers: candidate / CP1 | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Wall | 5.386969 / 4.747851 s | 1.1346 | 1.204690 / 1.136668 s | 1.0598 |
| Allocated | 2.244182 / 4.642569 GB | 0.4834 | 2.244184 / 4.642571 GB | 0.4834 |
| Peak RSS | 2.318565 / 4.506714 GB | 0.5145 | 2.321908 / 4.509204 GB | 0.5149 |

CPU upper 95% ratios are **1.1623 / 1.1460**, with lower bounds 1.1116 / 1.0293.
Timing relative MAD is 0.78% / 1.18% for the candidate and 1.63% / 2.01% for
CP1. The whole combination saves 2.398 GB of requests and about 2.188 GB RSS
against its same-screen control, while adding 639 ms / 68 ms wall time.
The runner records `regressing_or_uncertain`; both timing intervals remain
above parity. The combination remains **experimental, not promoted**; CP1 is
still the retained control.

Allocation is about 86.59 MB below the preceding auxiliary capture, consistent
with removing short temporary buffers. This is a cross-capture difference,
not an isolated component measurement. Peak RSS is essentially unchanged.
Separate candidate wall medians do not establish the list change's CPU effect.
The measured combination still contains substantial CPU and memory tradeoffs;
they are retained for further work without changing any final gate.

| Remaining distance to historical Go-derived limits | One worker | Eight workers |
| --- | ---: | ---: |
| Wall above limit | 2.449 s (1.834× limit) | 0.571 s (1.900× limit) |
| Allocated above limit | 208.956 MB | 208.417 MB |
| Peak RSS above limit | 109.617 MB | 104.863 MB |

These are historical planning distances, not fresh paired Go acceptance.
Both CPU modes and both memory gates remain open. The request reduction does
not by itself qualify the combination, nor can savings from separate screens
be added to forecast another result.

Validation passes 128 AST, 34 parser, 28 binder and 12 compiler library tests;
29 AST and five encoder integration tests; 13 AST doctests; workspace Clippy,
declared Rust 1.96 checking and formatting. Generated source and emitters are
unchanged. Scoped strict-provenance Miri passes two borrowed-edge AST tests and
the eight new parser tests; AddressSanitizer passes all 128 AST tests and those
eight parser tests. The initially broad full-parser Miri run was deliberately
stopped at its unchanged 6,000-level native stack-growth stress test; its log and
interruption reason are retained, and it is not claimed as passing. Independent
plan, implementation and screen reviews found no actionable issue. Scoped
instrumentation is not fresh complete ownership-producer evidence.

The [review archive](../tools/s07/performance-experiments/results/2026-09-09-compact-parser-lists/README.md)
retains 458 files, including frozen sources/binaries, graphs, every raw sample,
receipt, generator inputs and successful/failed development checks. Every member
was read back and hash-verified. Unchanged CP1/helpers and the earlier census
remain pinned through their prior archives.

The subsequent [current-cost diagnostic](S07-bis-current-costs.md) uses this exact
frozen source and a small driver patch. One normal run measures 2.749 s parsing
and 2.742 s binding/publication; a separate allocation run observes 2.244 GB of
pipeline requests, 1.910 GB net live growth and 333.733 MB freed or superseded
requests. These are diagnostic observations, not paired performance estimates,
physical storage domains or RSS attribution. All work counts and input digests
match. An exact-binary native profile then identifies shared keyword comparison
and string-slice validity checking as the next bounded CPU candidate. The
[implementation plan](S07-bis-text-processing-plan.md) records its correctness
checks and the build-isolation correction needed before its timing screen.
The obsolete census driver is not used: reconstructing semantic nodes would add
observer allocations and change the publication path.


### Shared keyword and slice screen: near CP1 CPU, memory reduction retained

The [shared text candidate](S07-bis-text-processing-plan.md) keeps the compact
layout and replaces repeated keyword binary searches with generated byte
matches. Valid UTF-8 slices inherit validity only when both boundaries prove
it; all other cases keep classification. Full string/scanner parity and affected
compiler tests pass. The first build accidentally reused a phase diagnostic's
Cargo artifacts; it was rejected before timing, retained, and followed by a
reviewed/tested isolation fix for final and intermediate build directories.

The corrected `8f7236e` freeze passes all 13,094 graphs in both worker modes,
with 13,094 in-place bindings and zero fallback each. Its fixed eight warmups
and 56 samples compare the complete combination with the same retained CP1:

| Metric | One worker: CP1 → candidate | Eight workers: CP1 → candidate |
| --- | ---: | ---: |
| Wall | 5.078450 → 5.180391 s | 1.087606 → 1.079982 s |
| Allocated bytes | 4.642568 → 2.244183 GB | 4.642573 → 2.244184 GB |
| Peak RSS | 4.506599 → 2.318565 GB | 4.509237 → 2.321842 GB |

The same-batch CPU deltas are +101.942 ms and −7.623 ms. Ratios are 1.020073 and
0.992991, with upper 95% bounds 1.055861 and 1.006322. One-worker nonregression
is not established; neither mode demonstrates a CPU win. The combination still
saves approximately 2.398 GB requested and 2.188 GB RSS against its paired
control. It remains experimental, and CP1 remains the retained control. The
previous batch cannot establish the text change's isolated effect; do not add
or subtract independently captured component savings. No timing retry occurred.

| Remaining distance to historical Go-derived limits | One worker | Eight workers |
| --- | ---: | ---: |
| Wall | +2.242764 s (1.763461× limit) | +0.446020 s (1.703542× limit) |
| Allocated bytes | +208.956 MB (1.102670× limit) | +208.417 MB (1.102378× limit) |
| Peak RSS | +109.617 MB (1.049624× limit) | +104.797 MB (1.047269× limit) |

All final gates remain open. This is a recorded combined tradeoff, not a memory
win promoted despite an unresolved CPU screen. End keyword/slice tuning here;
any next change needs a named current cost, rather than more variants of this
small surface. The remaining 333.733 MB of freed/superseded requests from the
separate diagnostic is not automatically removable. A current storage family
must be counted before its total bytes can be treated as a redesign opportunity.

The [complete archive](../tools/s07/performance-experiments/results/2026-09-09-compact-text-processing/README.md)
retains the failed build attempt, corrected frozen source and executables,
complete graphs/raw timings, receipt and validation/review logs. Scoped Miri,
ASan, E4/scanner parity, affected tests, Clippy and Rust 1.96 checks are recorded
in the implementation plan; fresh complete ownership and Go acceptance
producers remain prerequisites for final promotion.
