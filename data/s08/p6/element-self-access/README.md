# Self-type access for private member references

Five strict, no-lib programs with `noUnusedLocals` witness the `isSelfTypeAccess`
argument of pinned Go `markPropertyAsReferenced`. Each class has a private
method `m` that is referenced only from inside a method. The receivers are a
local instance, a static instance, `this`, a `this`-typed alias and the class
itself for a static method, each through element and property access. Every
case appears once in exported classes and once in module-local classes. A fifth
program places a static self access under a function type annotation and reads
a static member through a static alias of the class.

Element access passes the apparent object type's symbol
(`getPropertyTypeForIndexType`); property access passes the receiver's resolved
symbol (`checkPropertyAccessExpressionOrQualifiedName`). The self-access skip
only matters when the referenced member symbol is the enclosing method's own
declaration symbol:
- `const self = this; self["m"]` references `m`;
- `self.m` does not;
- `StaticSelf["m"]` references `m` only when the class is exported. The class
  name resolves to the binder's local export-value symbol, while `typeof` the
  class carries the export symbol.
- `AliasSelf.alias["m"]` does not reference `m`, because the type symbol is the
  class; `AliasSelf.alias.m` does, because the receiver resolves to `alias`.
- The containing method is the nearest function-like declaration, so a
  `typeof TypeQuerySelf.m` annotation inside `m` does not reference `m`.

Before the fix, Rust passed the receiver's resolved symbol at the element-access
site too. It reported TS6133 for `self["m"]` and exported `StaticSelf["m"]`,
and omitted it for `AliasSelf.alias["m"]`. Its containing-method search also
stopped at function types and signatures, omitting TS6133 for `TypeQuerySelf`.

The driver is `tools/s08/p6/semantic_diagnostics_oracle_test.go`. The native
capture is `target/s08/p6-element-self-access-native-04`. Regenerate with:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import run_overlay, strict_json_loads, ROOT
request = strict_json_loads((ROOT/'tools/s08/p6/element-self-access-requests.json').read_bytes())
driver = (ROOT/'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text()
run_overlay(ROOT/'target/s08/p6-element-self-access-native-new', 'checker', driver, request, 'TestS08P6SemanticDiagnostics')
PY
cargo test --locked -p ts_compiler --test checker_semantics self_type_access
```

This supplemental comparison does not certify E2 acceptance.
