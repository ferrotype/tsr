# ADR 0011: Deep recursion: reserved stacks and growth guards

Status: Accepted (2026-09-05; implementation recorded 2026-10-11)
Plan: section 6, decision 6; section 13, item 11

## Context

Go stacks grow on demand up to 1 GB. Corsa removed the trampolines the TypeScript-in-TypeScript compiler used: the binder, `checkBinaryLikeExpression`, the transforms and the printer recurse on the left operand of binary expressions. Rust threads have fixed stacks and a stack overflow aborts the process.

## Decision

Run parsing, checking and emit on threads created with large reserved stacks (256 MB to 1 GB, virtual, committed lazily) and add growth guards in the deepest recursive paths. Stress tests cover the full parse, bind, check, transform and emit path, left- and right-associative chains, and deep nesting of parentheses, JSX and conditional types; native and `wasm32` stack strategies are validated separately because WebAssembly stacks do not grow.

## Consequences

The four sites use the strategies below. This records the implementation of
section 13, item 11; it does not claim unlimited nesting or waive an observed
stack failure. A 100,000-term chain is one failure shape among several.

| Site | Native strategy | WebAssembly strategy |
| --- | --- | --- |
| Binder binary expressions | Existing explicit continuation frames, with guards for other recursive syntax | The same continuation frames |
| Checker binary expressions | Reserved worker stack and growth guards | Heap frames for binary and parenthesized expression entry/exit; ordinary workers retain operator semantics and postchecks |
| Transforms | Reserved stack and existing visitor guards | Explicit expression-spine frames only for transforms whose ordinary binary/transparent-node behavior permits it (ES modules and const-enum inlining); specialized hooks continue through their normal visitors |
| Printer binary expressions | Reserved stack and growth guards | Heap frames with the ordinary comment, source-map, precedence and enter/exit operations |

AST subtree-fact calculation also uses an iterative postorder walk. It is a
prerequisite to reaching these sites on long expressions, not a substitute for
their semantic checks. The continuations preserve left/right order, repeated
visits, factory updates and failure cleanup. They do not bypass specialized
visitor hooks or apply a generic transform to every expression kind.

Remaining recursive paths include printer parentheses, checker destructuring
fallbacks and specialized transforms. Native full check/emit matches Go on
100,000-term left/right chains, 5,000 parentheses, 600 JSX elements, 600 nested
conditional types and 1,600 nested statements. Production wasm matches the
5,000-term left/right chains with Node's `--stack-size=4096`; 5,000 parentheses
still trap and remain a recorded limit under investigation. The browser smoke
tests 5,001 binary operands with 64 wrappers on each required engine. A pass at
one of these bounds makes no claim about the others.

## Evidence

`tsc/internal/core/core.go` (`ApplyDebugStackLimit`), `tsc/internal/binder/binder.go` (binary expression binding), `tsc/internal/checker/checker.go` (`checkBinaryLikeExpression`), `tsc/internal/printer/printer.go` (`emitBinaryExpression`).

[R5](../PHASE7-R5.md) retains the full-operation native and wasm observations,
artifact identities and limitations. The implementation lives in
`tsr_binder`'s binary continuations, `tsr_checker/src/expression_stack.rs`,
`tsr_transformers/src/expression_spine.rs`,
`tsr_printer/src/printer_expression_stack.rs` and
`tsr_ast/src/subtree_facts.rs`.
