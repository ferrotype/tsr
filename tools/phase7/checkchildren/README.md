# Rust checkchildren prototype

This compiler-private analyzer follows the dispatch roster and lexical successor
walk in `upstream/tools/customlint/checkchildren.go`. It inspects explicit
non-expanded returns in methods directly called on `self` by
`check_source_element_worker` and `check_expression_worker`. Nested closures and
items have separate control flow and are excluded. The lint has a dedicated CI job using the tooling nightly. Phase 7 R5 remains
incomplete; this check has the explicit scope and limitations below.

Run `python3 tools/phase7/checkchildren/run.py fixtures` for its controls, or
`python3 tools/phase7/checkchildren/run.py check --output /absolute/new/log`
for the checker crate. Logs must be outside `target/`. The script builds with
the nightly in `data/s04/toolchains.toml`; that toolchain needs `rustc-dev`.
Compiler-private dependencies remain outside the production workspace crates.

Child entry points follow the upstream `checkExpression*`,
`checkSourceElement*`, and `resolveCall` naming families. One Rust spelling is
corrected explicitly: `check_expression_related_with_elaboration` ports
`checkTypeRelatedToAndOptionallyElaborate`, which relates already checked types.
It is not a child traversal. This correction is not a handler exemption.

The Rust-specific absence rules recognize standard `Option::is_none` /
`is_some` guards on the same expression, and `let Some(child) = ... else`
guards for the same bound child (including `Some(&child)`). The binding and
standard Option variant are resolved through type checking. A return of the
standard `Result::Err` constructor is excluded because it aborts the fallible
operation; the constructor is resolved by its compiler language item, so an
unrelated function named `Err` cannot bypass the lint and an import alias of the
real constructor is handled. This does not promise diagnostics after an arena,
cancellation, or other operation failure. Expanded returns, including the
lowering of `?`, are outside this explicit-return analysis.

The fixture has eight planted omissions, a repaired control, unrelated and
custom guard controls, standard/aliased Err controls, and an empty dispatch
roster refusal. The repaired `let-else` control uses `panic!()` solely to retain
Rust's required divergence after removing its planted return. Two isolated
Cargo compilations verify summary replay on a `Fresh` cached artifact. The
summary is a rustc diagnostic, so a valid cached compile retains its result.
The wrapper executable's name includes a hash of the tool source and exact
compiler identity, which Cargo includes in its workspace-wrapper artifact
identity. A repeated invocation can therefore use cached evidence; it is not
claimed as a fresh compiler analysis. Missing summaries fail the script.

## Remaining analysis limitations

The initial checker run found six warnings in 104 dispatched methods. After
reviewing each against the pin, six per-return annotations now identify these
analysis limitations. The current run passes with two dispatch workers, 104
dispatched methods, zero findings, six reviewed allowances and no invalid reasons.
The local log is retained at
`../ts-rust-benchmarks/2026-10-11/r5-checkchildren-reviewed.log`.
Source inspection identifies these control-flow/semantic limitations; they
remain visible and are not evidence of six production bugs:

| Handler | Why the lexical successor walk cannot establish safety |
| --- | --- |
| `check_type_declaration` | An intrinsic keyword is a leaf type node. The return skips the general annotation traversal. |
| `check_function_declaration` | The constructor branch delegates to `check_constructor_body`, which checks the signature and body. |
| `check_array_tuple_syntax` (keyof) | The returned expression itself checks the operand. The walk still sees the later readonly-specific operand check. |
| `check_array_tuple_syntax` (unique) | The unique branch delegates to `check_unique_type_operator`; the later check belongs to the readonly branch. |
| `check_binary_expression` | Destructuring checks the right operand and delegates the left to assignment checking instead of ordinary expression checking. |
| `check_import_call_expression` | `args.first() == None` also makes the later `args.iter().skip(2)` loop empty. Matching only the bound specifier does not prove that collection fact. |

The analyzer does not prove branch predicates, child identity across delegated
methods, collection cardinality, or leaf-node facts. It also does not audit
handlers with no recognized direct child-check calls, matching the upstream
scope. A clean result would establish only this scoped explicit-return check,
not complete child traversal. These limitations can be recorded with the per-return allowances below, or
resolved through focused analysis with counterexample fixtures. Broad method-name
exemptions or production behavior changes to satisfy the tool are not appropriate.


## Reviewed return allowances

A reviewed limitation can be recorded immediately before the specific return:

```rust
// checkchildren-allow: returned check_source_element checks the keyof operand; later check is readonly-only
return self.check_source_element(operand);
```

The comment must be on the immediately preceding line, and the return must
begin its line after indentation. An allowance applies to that return only.
Empty reasons fail the run. Applied allowances emit a diagnostic containing the
location and reason, and the summary reports both the remaining findings and
the allowance count. Allowances are source-level lint annotations; they do not
approve or modify parity expectations. The fixture proves that another return
in the same method is still checked and that an empty reason fails. A summary
with zero findings and nonzero allowances is a scoped lint result with reviewed
limitations, not a proof of complete child traversal.
