# S08 reference relater — stopped work record

Stopped at the owner's request on 2026-09-18. Branch `codex/s08-p7`, based on
`f9ec1d1`. This commit preserves an **incomplete implementation checkpoint**. All delegated
edits are finished; no measurement or build is left running. No acceptance
evidence, thresholds or generated status views were changed.

The measurement reference now constructs types from retained AST/binder owners,
compiler options and module-loader facts. It no longer pre-resolves through the
production checker. It owns lazy type links, native initialization, generic
references and signature instantiation, tuple/mapped/conditional/template
construction, relation caches, and structured diagnostic rendering. The producer
requires exact native states and diagnostics and stops a full capture before
measurement when parity fails. `Description` remains a unit-test API only.

Validation at this stopping point:

- `cargo test -p s08_relater_prototype --lib`: **53 passed**.
- `cargo build -p ts_compiler --example p7_relater --features relation-probe`: passed.
- `PYTHONPATH=scripts python3 -m unittest test_s08_measurement.RelaterContract test_s08_relater_sources`: **11 passed**.
- Formatting applied to the prototype and compiler packages.
- Bounded development comparison: **75/100 groups strictly match** the frozen
  native observations. This ran 20 fixtures, each in all five modes; the
  `instantiation-limit` fixture was deliberately excluded from the final check.
  Results are under `target/s08/reference-development/`. These are development
  observations, not a capture accepted by the producer.
- Workspace/release/MSRV/clippy checks and the full relater measurement have not
  been run for this implementation.

## Remaining acceptance work

| Fixture | Observed remaining issue |
| --- | --- |
| `optional-rest-tuples` | Three modes stop at `undefined-stripped union target`. Lookup is 102 types / 4 signatures / 0 instantiations versus native 105 / 5 / 1. Identity first-action state is 115 / 5 / 11 versus 116 / 5 / 12. Finish native tuple normalization/setup timing and optional-element relation handling. |
| `deferred-generic-members` | All actions execute, but relation-cache flags and construction work differ. First action is 109 types / 12 signatures / 23 instantiations versus native 111 / 12 / 31. Continue canonical signature/mapper/variance investigation. |
| `mapped-conditional-infer` | All actions execute, but lookup is one instantiation short; the first action has an extra nested ternary and reaches 196 / 33 / 140 versus native 186 / 30 / 155. Audit conditional inference, mapper composition and signature instantiation against the pin. |
| `flow-return-inference` | Setup does not resolve the expected `number` identity. Identity relations return false; lookup is 95 / 8 / 6 versus native 103 / 8 / 11. This is a semantic issue, not merely accounting. |
| `jsdoc-module` | Same ReturnType/inference identity problem: 91 / 7 / 6 versus native 99 / 7 / 11. The earlier rest-signature unsupported branch is fixed. |
| `instantiation-limit` | Native 1000-step conditional tail loop is implemented and a small `Build<8>` test passes. Recheck the frozen `Build<1001>` case, its diagnostic range, and its actual 507,609-type / 507,513-instantiation state. No current full-fixture result is claimed. |

The other fifteen checked fixtures match all five modes, including exact
lookup/action counts, cache flags and structured diagnostics. This includes
template literals, byte/numeric literals, recursive objects, union/intersection
error elaboration and all four deep-relation fixtures.

## Code map and boundaries

- `tools/s08/relater-prototype/src/bound_input.rs`: retained AST, binder symbols,
  lexical/module lookup.
- `src/bound.rs` and `src/bound/`: native initialization, source construction,
  generic/signature/mapped/conditional/value resolution and instantiation.
- `src/{type_link,generics,tuples,template,signatures,relation_keys}.rs`: graph
  lifetime rules, relation algorithms and actual key-type construction.
- `src/{diagnostics,display}.rs`: structured chains and lazy type display.
- `crates/ts_compiler/examples/p7_relater/reference.rs`: independent adapter.
- `scripts/s08_relater.py`: strict comparison, source fingerprint and capture
  precondition; associated Python tests reject false equivalence.

Named unsupported branches still include advanced conditional/signature
inference, non-array variadic rest slicing, some mapped union/array cases,
intersection re-instantiation, string mapping/substitution and some diagnostic
display forms. Do not silently substitute success or adjust counters to close
these gaps. Several thousand lines of new reference code still need independent
review. Prototype unit tests do not establish native fixture parity.

After fixing the six fixture families: run the full 105-group normal/allocation
comparison, required code checks, then the frozen complete measurement batch.
Only then record `run.relater.*` evidence and regenerate status. Until then the
four E2 relater criteria remain unmet.
