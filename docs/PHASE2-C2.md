# C2 implementation record

## Starting point

C1 review fixes are committed as `cc03e64`. The full reviewed-source capture
at `target/phase2/c1-reviewed` is the authenticated C2 baseline, retained in
`data/phase2/c2-baseline.json.gz`: 13,432 executed variants, no harness errors,
12,459 full-domain matches, and all 9,367 S08 regression rows matching.
The source inputs were stable. The native capture is reused unchanged.

The original `target/phase2/rust` capture is preserved as
`target/phase2/rust-c1-historical-0ab79c6`. The default capture path now points
to the reviewed C1 capture. No capture was deleted or rewritten to absorb a
code change. Its raw snapshots remain usable after C2 edits make it historical.

The C2-start worksheet contains all 191 C2-owned rows with an open domain.
It records the baseline outcomes and exact row reproduction commands. The
declaration-transform panic has a traced C5 handoff from C1. The remaining
rows stay open until their attribution and validation are completed; the
worksheet is not a complete C2 function audit or an exit claim.

## Plan review

- C1's variance measurement handoff is explicit and remains C2 work.
- B03 and B08 need node-builder algorithms over type state that already
  exists. A storage redesign requires a demonstrated missing fact.
- B08 preserves distributivity when an instantiated conditional check is no
  longer a type parameter; its guard is not merely a name collision.
- B10's native template cross-product limit is distinct from the union
  reduction limit.
- Claims and exit accounting must not hide unclassified failures or ignore
  a regressed row previously marked closed.
- Corpus and benchmark binaries are different artifacts. Measurement joins
  their shared production source identity and pin, while authenticating each
  binary's own build. It must not require equal executable hashes.
- Intermediate validation remains the frozen 300-row sample plus affected
  rows. The full function audit grows with the implementation; an attributed
  small refusal fix can proceed before every unrelated row is understood.

## First implementation slice

B04, `Checker.extractRedundantTemplateLiterals`, is independently attributed:
the intersection constructor returns `Unsupported` at the exact pinned call
site. Thirty C2 rows expose it. The port uses the existing subtype relation
and pattern-literal predicate, preserving reverse removal order and the
position before `any` handling. No new type representation is needed.

Direct native probes cover matching templates, incompatible patterns,
`Lowercase<string>`, generic templates/mappings, retained constituent order,
and incompatible patterns combined with `any`. Implementation and sampled
results are recorded below once observed. Later C2 operators, the full audit,
creation-trace contracts, producer wiring and quiet-host measurement remain
open.
