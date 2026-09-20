# Inline binding layout and base inventory audit

This diagnostic checks Fable's layout/count claims against actual compiled Rust
payloads, the generator rule, normalized SchemaAPI fields and physical Go base
embeddings. It changes no production code and performs no benchmark or full
workspace build.

## Compiled sizes

The probe imports the existing production `tsr_ast` rlib's public payload types.
Its copied enum preserves every variant's order and current boxing choice; its
copied Node frame preserves the actual fields, types and order. Assertions check
the unchanged copies' size/alignment against real `NodeData` and `Node` before
observing the alternate enum.

On the recorded Rust **1.97.1**, macOS arm64 compilation:

| Type | Size | Alignment |
| --- | ---: | ---: |
| Actual `NodeData` | 40 | 8 |
| Actual `Node` | 80 | 8 |
| Actual `IdentifierData` | 32 | 8 |
| Identifier plus `Option<FlowId>` | 40 | 8 |
| Enum keeping that enlarged Identifier inline | 48 | 8 |
| Node frame with that enum | 88 | 8 |

The arithmetic **8 × 19,593,488 = 156,747,904 bytes** is correct for occupied
core node records. It does not include spare page slots or price replacement
binding storage, payload boxes, directory changes, allocation traffic or RSS.
It is not a claim that every possible inline-binding implementation costs this
amount: this experiment retains the current boxing decisions while enlarging
Identifier only.

There are **21**, not 14, nonboxed payloads whose actual size is 32 bytes:

- Identifier, PrivateIdentifier, ForStatement, ForInOrOfStatement,
  VariableDeclaration, BindingElement, TypeAliasDeclaration;
- EnumMember, ImportDeclaration, ExportAssignment, CallSignatureDeclaration,
  ConstructSignatureDeclaration, CallExpression, TaggedTemplateExpression;
- ConditionalTypeNode, NamedTupleMember, JSDocTemplateTag, JSDocCallbackTag,
  JSDocTypedefTag, JSDocSignature, ImportEqualsDeclaration.

**16** of these have binding fields. PrivateIdentifier, TaggedTemplateExpression,
JSDocTemplateTag, JSDocCallbackTag and JSDocTypedefTag do not. The named examples
Identifier, CallExpression, BindingElement, EnumMember and ImportDeclaration
are all 32 bytes and inline as claimed.

[The generator](../../../../xtask/src/gen/ast.rs) first removes deferred fields
(`deferred`, `fields`), then applies a conservative field budget: JsString 32,
raw slices 16, other currently supported fields 8; budgets above 32 are boxed.
The diagnostic checks that this rule reproduces all 192 current boxing choices.
Adding binding fields therefore needs an explicit generator policy. Reboxing
enlarged payloads could preserve the enum ceiling, but would allocate payloads
separately; those costs are not modeled here. Small scalar fields also mean a
conservative budget is not interchangeable with `size_of`.

## Base groups and the missing direct field

The reported **83/109** partition is reproducible with **locals taking
precedence**:

| Disjoint group | Shapes |
| --- | ---: |
| Flow, excluding declaration and locals | 23 |
| Declaration, excluding flow and locals | 24 |
| Both flow and declaration, excluding locals | 9 |
| Locals containers, including function-like shapes | 27 |
| None of those three bases | 109 |

These are not independent total base counts. Across all shapes, FlowNodeBase
occurs **44** times, DeclarationBase **54**, and LocalsContainerBase **27**;
**18** shapes have both flow and declaration, including those grouped under
locals. FunctionLikeBase covers **14** of the 27 locals shapes. The exact names
and transitive paths are retained in the result.

“The other 109 get nothing” is incorrect. `CaseOrDefaultClause` belongs to that
group but directly declares `FallthroughFlowNode` in
[ast_generated.go:1318](../../../../upstream/tsc/internal/ast/ast_generated.go#L1318).
The binder writes it when a nonfinal clause can fall through in
[binder.go:2143](../../../../upstream/tsc/internal/binder/binder.go#L2143); the current
Rust binder has the corresponding write in
[statements.rs:385](../../../../crates/tsr_binder/src/statements.rs#L385).
The base fields plus direct fields cover **84 shapes**, leaving **108** without
these binding fields. This is a schema capacity inventory, not evidence that
every field is written on the frozen workload.

| Field | Shapes carrying it | Storage origin / actual binder path |
| --- | ---: | --- |
| Symbol | 54 | DeclarationBase; `addDeclarationToSymbol` |
| FlowNode | 44 | FlowNodeBase; `setFlowNode` and direct expression writes |
| Locals | 27 | LocalsContainerBase; `GetLocals` returns the field for declaration-table mutation |
| NextContainer | 27 | LocalsContainerBase; `addToContainerChain` |
| LocalSymbol | 14 | ExportableBase; exported declaration binding |
| EndFlowNode | 8 | BodyBase; control-flow-container binding |
| ReturnFlowNode | 4 | Direct fields on ConstructorDeclaration, FunctionDeclaration, FunctionExpression and ClassStaticBlockDeclaration; `setReturnFlowNode` |
| FallthroughFlowNode | 1 | Direct CaseOrDefaultClause field; switch binding |

The four direct ReturnFlowNode shapes are already in the locals group. No other
shape among the 109 declares the audited symbol, flow, locals or container
fields. Header flags and facts caches are different fields and are not claimed
absent from those shapes.

This checks transitive embedding, not a string count of `FlowNodeBase` lines.
[The Go generator](../../../../upstream/tools/scripts/tsc/generate-go-ast.ts#L143)
rejects duplicate inheritance paths, embeds bases recursively and puts empty
marker bases first to avoid trailing-zero-size padding. The diagnostic compares
every concrete shape's direct Go embeddings and every transitive base's actual
embeddings to the normalized schema, then checks the physical binding fields
and their types. The handwritten SourceFile is included.
[The Rust adapter](../../../../tools/s03/ast-export.mts#L43) flattens inherited
storage fields and concrete refinements before the Rust emitter defers runtime
fields. A replacement rule must use this complete field inventory, not only
membership in three bases.

The denominator is **192 payload shapes**, not 351 known kinds or 34 kind aliases.
Seven shapes admit multiple kinds; for example, ForInOrOfStatement and
CaseOrDefaultClause each cover two. Token also overlaps kinds having other
payload shapes. Kind aliases do not introduce additional stored payload structs.

## Reproduction and limits

The retained [result](result.json.gz) includes all 192 actual size/alignment
observations, group members, field origins, source hashes, compiler version,
command and linked rlib hash. [Observed stdout](observed-layouts.txt) and the
[compiled source](probe-source.rs.gz) preserve the small probe's exact output
and generated definitions. No native binary is committed.

The successful capture reused
`target/release/deps/libts_ast-f7392d082e67be11.rlib`, compiled by Rust 1.97.1.
This is an observation of that recorded rlib, not a claim that production was
rebuilt from current sources. Rust rejects a different compiler version. Thin
LTO lets Rust consume the release rlib's bitcode rather than asking Apple's
older linker LTO reader to decode it. Reproduction can name another existing
matching rlib; any new observations get their own source/version/binary hashes.

```sh
python3 tools/s07/performance-experiments/binding-layout-audit/audit.py \
  --rlib target/release/deps/libts_ast-f7392d082e67be11.rlib \
  --toolchain 1.97.1 --output target/s07-bis/binding-layout-audit
python3 -m unittest discover \
  -s tools/s07/performance-experiments/binding-layout-audit -p 'test_*.py' -v
```

Seven focused Python regressions pass. They reject an omitted direct field,
missing transitive base, changed physical embedding, duplicate inheritance path,
changed boxing choice and unrecognized enum form; the positive inventory also
checks the disjoint versus overlapping counts. The compiled probe checks the
real/copy enum and Node frame identities. CPU, allocations, owner lifetime and
binding behavior remain outside this diagnostic.
