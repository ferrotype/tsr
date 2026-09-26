# Three diagnostic differences caused by JavaScript emit order

Pin: `1f70213d4922b434345f639b441681e470c7cfc1`.

The native harness creates two separate programs. It checks the first before
emit. On the second it calls `Program.Emit` **before** semantic diagnostics
(`internal/testutil/harnessutil/harnessutil.go:640–689`). The differences below
are first-resolution effects in the second checker; native does not relocate or
amend the first program's diagnostics.

| Case | Exact errors-domain difference | Native operation first resolving the type |
| --- | --- | --- |
| `compiler/incorrectRecursiveMappedTypeConstraint.ts#configuration=0` | The two TS2313 diagnostics gain TS2751 related information at `v`, `[108,109)`. | `ConstEnumInliningTransformer.visit` asks `EmitResolver.GetConstantValue` about `v[k]`. |
| `compiler/typeParameterWithInvalidConstraintType.ts#configuration=0` | TS2313 gains TS2751 related information at `x`, `[70,71)`. | `ImportElisionTransformer.visit` marks linked references and checks the receiver of `x.foo`. |
| `conformance/types/mapped/recursiveMappedTypes.ts#configuration=0` | TS2615 is first reported at `x`, `[1705,1706)`, instead of the `Child<ListWidget>` annotation `[1621,1638)`. | The same import-reference pass checks the receiver of `x.type` before declaration/semantic checking. |

The first two requests do not enable declaration emit. The third does, but its
TS2615 stack still originates in **JavaScript** import elision, before the
subsequent declaration transform. These are not declaration-builder defects.

## Observed call paths

The retained `stacks.json` contains all 14 diagnostic-production stacks from the
three source requests, labelled `PRE`, `EMIT`, or `POST`. Addresses and argument
values are omitted from stack frames; function names and source lines remain.
The `site` preserves the current and constraint node kinds/ranges. AST positions
include leading trivia; diagnostic ranges above use native normalized positions.
Stack source lines refer to the executed overlay (the two inserted checker hooks
shift later checker lines by one or two); the citations below use the pin.

For the first case, the decisive emit stack is:

```
Program.Emit
  emitter.emitJSFile -> runScriptTransformers
  ConstEnumInliningTransformer.visit       inliners/constenum.go:40
  EmitResolver.GetConstantValue            checker/emitresolver.go:1162
  Checker.GetConstantValue                 checker/services.go:876
  checkExpressionCached -> checkIndexedAccess
  checkNonNullExpression -> checkIdentifier(v)
  getNarrowableTypeForReference -> getBaseConstraintOfType
  getResolvedBaseConstraint                checker/checker.go:27823
```

The last two cases have this common caller path:

```
Program.Emit
  emitter.emitJSFile -> runScriptTransformers
  ImportElisionTransformer.visit           tstransforms/importelision.go:29
  EmitResolver.MarkLinkedReferencesRecursively
  markLinkedReferences -> markPropertyAliasReferenced
  checkExpressionCached(receiver)          checker/checker.go:28791
```

For the invalid constraint, the receiver lookup reaches `getResolvedBaseConstraint`
and TS2313. For the recursive mapped type, it reaches `getTypeOfMappedSymbol`
and TS2615 (`checker.go:21332`). The full intervening instantiation and alias
resolution stack is retained.

## Rust boundary and follow-up

Rust's existing exact tests in `c2_constraint_gaps.rs` match all three native
**pre-emit** diagnostic inventories and their type/symbol/display domains. The
Rust corpus executor explicitly records `error_baseline.emit = not_executed`.
`tsr_transformers` currently exposes declaration transformation; it has neither
the JavaScript import-elision nor constant-enum-inlining transform caller.
`Program::declaration_diagnostics_with_checker` cannot supply either caller.

The C5 follow-up is to execute the actual JavaScript transform/resolver schedule
on a fresh program before obtaining post-emit diagnostics, including its normal
source selection, option guards, and checker ownership. Then compare these
three native post-emit inventories and error-baseline bytes. Adding extra
constraint queries to semantic checking, changing error locations, or attaching
related information by case name would change the pre-emit API and would not
port the missing operation.

This handoff is restricted to these three rows and the errors domain. It is not
a claim that arbitrary emit-dependent differences are explained or accepted.
No Rust source was changed for this investigation.

## Reproduction and retained evidence

Run from the repository root:

```
python3 data/phase2/c2-emit-handoffs/regenerate.py
```

The script uses the pinned native harness through a separate Go overlay under
`target/phase2/c2-emit-handoffs`; it does not edit canonical native tools,
upstream files, or capture directories. `observer.go` reads the Go stack and
node kind/range only at the existing TS2313/TS2615 production sites. It introduces
no checker queries and changes no counters, guards, or diagnostics.

`requests.json` freezes the three source/configuration requests.
`observations.json` retains their complete pre/post diagnostic trees and error
baseline bytes. Regeneration asserts these against the previously frozen native
observations in `fixtures/c2/constraint_gaps`, not against Rust output.
`provenance.json` binds the pin, Go runtime, single-threaded harness mode,
observer/reproducer/request/output hashes, overlay source hashes, and canonical
producer input hashes. Raw trace/output hashes refer to files retained in the
separate target directory. Root integration binds the final Rust capture and
claims; those are deliberately not edited here.
