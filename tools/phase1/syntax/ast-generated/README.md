# Exact generated AST witnesses

The fixture has 26 typed factory scenarios (QualifiedName, Block and the
dynamic JSDocParameterOrPropertyTag) and all 232 generated predicates. The
predicate schedule includes every declared kind, immediately adjacent invalid
values and signed 16-bit extremes. The fixture generator writes inputs and
call dispatch only; all answers come from pinned Go and production Rust.

The typed scenarios record constructor fields, nil versus empty lists,
per-field child identities, nonzero locations and flags, create/update/clone
hook ordering, unchanged/changed update identity, shallow clone list identity,
full and early-exit child traversal, and unchanged/replacement visitors. The
JSDoc cases exercise both name-first orders. The second sentinel is
`AwaitExpression(ThisKeyword)`, so its subtree facts differ from the identifier
sentinels: missing that edge changes the observed mask. Facts are requested
through the production cache/dispatch on QualifiedName and Block.

The Go probe is an access-only overlay in package `ast`. Its ordinary package
tests import `testutil/fixtures`, whose init needs a real repository path;
therefore this probe must run with `trimpath: false`. The first development
attempt with trimpath enabled is retained as a failed run, not counted as a
product failure.

```sh
python3 tools/phase1/syntax/ast-generated/generate.py --check
python3 -m pytest scripts/tests/test_phase1_generated_ast.py -q
```

The source fingerprints and normal syntax-family capture/replay must include
this directory. Production code is neither modified nor linked to expected
observations. The shared Phase 1 driver serves `generatedAst` and
`generatedPredicate`; every exact claimed operation has a generated dispatch
or a named action. Passing these cases grants no credit to operations omitted from their exact
request links or to private parser/binder helpers.

Handwritten SourceFile/SyntheticExpression behavior and visitor-specific
fields are explicit in the expansion below. Existing S06 constructor and
accessor witnesses retain their own precise coverage.

The expanded generator `generate_shapes.py` adds four input modes for each of
190 ordinary shapes. It emits actual New/Update calls and payload getters,
with a separate update action per field. Runtime observations compare those
results, clone hooks, child and visitor order and identity, names and generated
facts. SourceFile is covered by an explicit factory/owner-metadata case;
SyntheticExpression by its cast and child traversal only. Its four semantic
Type-payload operations remain outside this fixture's claims.

The complete schedule is 1,026 rows and 1,380 distinct operation identities.
A native panic or unsupported action fails the capture; it never supplies
coverage for later actions. Generated request actions are checked by both
children before executing their fixed trace.
