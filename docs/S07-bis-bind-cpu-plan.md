# S07-bis: narrow flow writes and immutable source facts

Status: incorporated into [the local-view binder plan](S07-bis-local-bind-plan.md).
The standalone small-candidate sequence below is retained as its historical
reference. Its flow-write and source-fact contracts still apply, but there will
be no separate prerequisite screen before the architectural CPU candidate.

## Candidate and effort boundary

Combine two mechanisms in the existing compact implementation: avoid decoding
a payload enum and repeating generic dispatch when writing its flow field; read
immutable source metadata once per binder. Implement and review both before
screening the complete combination. Component checks prove semantics and explain
the diff; they are not separate performance promotion gates.

The profile identifies these operations, not their removable fractions. There
is no predicted percentage or claim that this candidate closes the CPU gap.
Keep the work confined to these paths, their generator and counterexamples.
If it requires a new general borrowing API, owner cache, storage representation,
unchecked access or altered error semantics, stop and revise the scope before
expanding it. Do not launch another profiler or capture/replay experiment.

## 1. Make flow capability and writes narrow

`Binder::set_flow_node` currently calls `has_flow_node_data(&self.n(node))`.
That helper decodes `NodeDataRead` to test a concrete-payload interface. The
public setter then validates the owner and flow, checks compatibility records,
and routes through `BindingWrite::store`, which separately finds the generated
field key and dispatches to its setter. The concrete implementation steps are:

1. Generate flow-field capability from the same pinned embedding inventory used
   by `xtask/src/gen/ast_compact.rs::binding_fields`. Provide a narrow read through
   `NodeRead` or its existing data-source facade. Compact core/transaction reads
   inspect the actual shape; owned and lazy nodes inspect their concrete variant.
   Replace the binder's full-enum capability test. Keep capability independent
   of the open `SyntaxKind`: factory nodes can deliberately disagree with it.
2. Generate a private flow-word resolver returning the checked row field and its
   `FieldKey` in one shape dispatch. Use the existing payload/context split in
   `CoreStore` to encode and store the flow without decoding other payload
   fields. Keep the existing full-reference escape encoding and clearing rules.
   Other binding fields can continue through their current path; do not widen
   this into a general setter rewrite.
3. Add a bounded exclusive-core flow-write path through `BindBuilder`,
   `ParsedFile` and `compact/binding.rs`, using the existing safe
   `node_store_and_source_mut` split. Reuse a checked target resolution where
   borrowing permits it without changing validation order. If resolution reuse
   needs a general capability migration, retain the duplicate check in this
   candidate and record that limitation. No unrestricted `builder_mut` or
   `node_mut`: flow writes cannot invalidate the syntax completion proof.
4. Preserve the public `set_node_flow` contract. Check target owner/slot before
   the referenced flow and validate before mutating. Existing materialized
   bindings override inline fields; flow-only records remain distinct from
   materialized empty records. Unsupported shapes still accept the public
   setter through its existing fallback. The binder's separate capability test
   retains its no-op behavior on those shapes, including not validating an
   unused flow. Published, lazy, imported and mapped-source paths retain their
   current owner checks and behavior.

Expected handwritten files are `crates/tsr_binder/src/containers.rs`,
`crates/tsr_ast/src/node_read.rs` (or the existing generated data-source facade),
`bind_result.rs`, `storage.rs` and `compact/binding.rs`. Regenerate through
`cargo xtask gen`; do not edit generated output or add a second shape inventory.
No extra per-node state or unbounded cache is part of this change.

Counterexamples must cover every generated flow-capable shape and an unsupported
shape, kind/payload disagreement, core and owned/lazy reads, `Some -> None`,
flow-only versus materialized-empty presence, existing compatibility overrides,
invalid target/flow error precedence and no mutation on failure, foreign and
mapped-sibling writes, and a full-reference escape overwritten by local/None.
Reuse existing generator and binding fixtures rather than reproducing the
implementation in tests. Verify that narrow writes leave the completion proof
valid and do not reintroduce a full syntax validation scan.

## 2. Cache only immutable facts for the logical source

In `Binder::new`, resolve `builder.source()` once and copy two booleans:
whether its parse diagnostics are empty and whether its parsed external-module
indicator is present. The binding entry has already validated this logical
source, including a source-file read during result construction. Review that
precondition at both consuming and published entries before moving these reads.

Use those values in contextual/private identifier diagnostics and strict-mode
message selection, preserving the existing order of node, parent and keyword
checks. Do not move keyword classification ahead of `is_identifier_name`.
Do not cache `seen_parse_error`, live node flags, accumulated binding diagnostics,
CommonJS detection or a physical owner's first source. Mapped siblings and
sequential files must each get their own immutable facts. This adds two booleans
per binder, not retained metadata for every node.

Expected files are `crates/tsr_binder/src/state.rs`, `diagnostics.rs` and existing
exclusive/published binding tests. Compare full diagnostic output for ordinary
and escaped reserved names, ambient and JSDoc nodes, parse-error suppression,
script/external-module/CommonJS contexts and private `#constructor`. Include
sequential files and mapped siblings with different metadata. Keep node flags
mutable in these checks: only the parsed source facts are immutable.

## 3. Review and correctness before timing

Review the completed diff against the pinned Go operations and these contracts.
Concentrate on kind/shape separation, field-presence semantics, validation order,
logical-source identity, reference escapes, and the safe borrow boundary. A
generated capability must agree with both inline storage and owned variants.
No fresh independent-agent review is claimed by this plan; use one only if
separately requested. Record any scope changes before observing timing results.

Run affected AST/binder debug and release tests, generated-source drift,
formatting, Clippy and the declared minimum Rust checks. Exercise the affected
ownership/reference cases under the existing Miri and ASan harnesses, preserving
their scope in the record. Check published/consuming graph parity, factory
updates, lazy nodes and deep binding with existing tests. Stack growth itself
remains unchanged. Retain failed checks and corrected attempts.

Freeze the completed source plus normal/allocation binaries using fresh,
isolated Cargo output and intermediate directories. Verify the build has no
phase/profiling adapter or diagnostic feature enabled. Run all 13,094 Go/Rust
binding graphs at both worker counts with the existing producer before the
screen. Record full work, loaded digest, path selection and hashes. The complete
producer and cross-platform acceptance requirements in the primary plan still
apply before promotion; a focused test pass cannot substitute for them.

## 4. One fixed combined screen, then a decision

Retain the original accepted CP1 control, manifest
`3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931`.
The current compact/shared-text freeze (normal executable
`3b375719ca8c3334c443f77b934c1388eec42ec1a94d12971586bc792056e8c2`)
is the source starting point and remains archived as an experimental reference.
Do not relabel it accepted under the infrastructure exception: its prior
one-worker upper ratio fails that condition.

Use the existing single fixed screen: eight warmups and 56 samples, seven
alternating control/candidate pairs for normal/allocation binaries at one and
eight workers, over the full workload. Compare CP1 with the complete compact +
shared-text + flow/source-fact candidate. Do not run component screens or add
an extra baseline matrix. Keep all results, failures, stderr and binary hashes.
The prior compact screen is contextual evidence, not a paired estimate of this
candidate's incremental saving.

Report medians, CPU confidence bounds, noise, allocation and RSS, plus all four
historical Go-relative distances. Judge the coherent result under section 6 of
the primary plan, including its combined-tradeoff and reviewed-small-change
rules. The 5% threshold prioritizes investment; it does not erase a clean small
win. A memory win alone does not prove CPU readiness, nor is a component's CPU
regression an automatic veto before the combination is measured.

End this direction after that fixed result. If it is weak, noisy or regressing,
record the outcome and the implementation's disposition; do not repeat the same
candidate until a favorable batch appears, add individual historical savings,
or open another diagnostic loop. Further work needs a new concrete mechanism.
Fresh Go comparison and the unchanged final S07 gates remain the acceptance
test, regardless of this screen's result.
