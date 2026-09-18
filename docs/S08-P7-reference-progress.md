# S08 reference relater: parity record

Branch `codex/s08-p7`. The measurement reference constructs its types from the
retained AST and binder owners, compiler options and module-loader facts. It
does not pre-resolve through the production checker. It owns lazy type links,
native initialization, generic references and signature instantiation,
tuple/mapped/conditional/template construction, relation caches and structured
diagnostic rendering. `Description` remains a unit-test API only.

## Status (2026-09-18)

Over the frozen inventory, 21 fixtures in all five modes, compared by
`scripts/s08_relater.py`'s strict comparator against the frozen native
observations:

| implementation | strict groups | behavioral groups | unsupported |
| --- | --- | --- | --- |
| reference (bound program) | **105/105** | 105/105 | 0 |
| production ID relater | 105/105 | 105/105 | 0 |

Strict means identical results, top-level `checkTypeRelatedTo` outcomes,
diagnostics, and identical `types_created`, `signatures_created`,
`instantiations` and relation-cache entries and flags before lookup, after
lookup and around every action. The checkpoint this work started from measured
75/105 strict and 82/105 behavioral with three unsupported groups.

Validation: 59 prototype unit tests, the 11 Python relater contract tests,
`python3 scripts/checks.py clippy` and `fmt` clean over the workspace (the
package had 92 clippy findings left from the bound-program rewrite; they are
fixed). No measurement capture was run and no evidence or status view changed:
`scripts/s08_relater.py capture` and `cargo xtask run relater` are the owner's.
With both implementations at full parity the producer's `same_work` condition
holds, so a capture now yields the three ratio metrics instead of withholding
them.

## Method

Totals cannot locate a construction difference. Both implementations were
traced event by event (temporary, env-gated, removed before commit): every type
creation with its flags, every counted instantiation with the source type, every
relation-cache write and every top-level relation result, each with a filtered
backtrace naming the responsible function. The production ID relater matches the
native counters on all 105 groups, so its trace stands for the pinned checker's
sequence; diffing it against the reference's trace names the Go function behind
each missing or extra event. Every port below was read against the pinned
source before it was written.

## Ports, by the fixture that exposed them

`deferred-generic-members` (first relation 19/8/23 against native 21/8/31):

- `instantiateSymbol`: an instantiation of an instantiated property or parameter
  symbol returns to the root symbol with `combineTypeMappers(links.mapper,
  mapper)`, and a symbol whose resolved type holds no type variables is returned
  as is. The reference instantiated the already instantiated type. Return types
  stay chained, as in `getReturnTypeOfSignature`. This was most of the gap.
- `getNormalizedType`: a deferred (node-backed) reference normalizes to the
  ordinary reference of its target and resolved arguments.
- `createMarkerType`: marker types run the counted
  `instantiateTypes(typeParameters, mapper)` before creating the reference.
- `isInstantiatedGenericParameter` (with a new `Signature.target`): resolves the
  target signature's parameter before a callback comparison.
- `isRelatedToEx`: a type parameter related to exactly its constraint is `True`
  before any recursion or cache entry; an unconstrained one relates through
  `unknown`, with the second, this-argument attempt. This fixed a cache entry
  recorded as succeeded where native records failed.
- `propertiesRelatedTo`: `sourceProp == targetProp` is symbol identity (one type
  link), not a comparison of resolved types, which resolved the source first.

`optional-rest-tuples`: `createNormalizedTupleType` takes a variadic array's
element from `getIndexTypeOfType(t, numberType)`, which resolves the array
reference's members during lookup rather than at the first relation.

`flow-return-inference`, `jsdoc-module`:

- `getObjectTypeInstantiation` filters outer type parameters for every
  `SymbolFlagsTypeLiteral` symbol, which the binder also gives function types,
  constructor types and mapped types, and for methods. The reference filtered
  type literals and methods only, so a function type in an extends clause was
  re-created instead of being returned by the identity entry. This is the filter
  the rejected counting experiment (recorded below) pointed at.
- A function declaration's anonymous type records its syntax provenance, so it
  could contain type variables and takes the counted instantiation path
  (permissive and restrictive instantiation of the check type).
- `inferFromSignature`/`applyToParameterTypes`: parameter types are resolved and
  the source's rest tuple is built by `getRestTypeAtPosition` (signatures now
  carry their parameter declarations for the tuple labels). Contravariant
  candidates stay a named refusal.
- `getNonNullableType` in the callback check of `compareSignaturesRelated`,
  through a checker-level hook for `getGlobalNonNullableTypeInstantiation`
  registered under strictNullChecks.
- An unaliased empty type literal is the shared `emptyTypeLiteralType`.
- An instantiated intersection applies `getIntersectionType`'s absorbing
  reductions (`any`, `never`, `unknown` removal, one constituent). A result that
  needs a new intersection type stays the named unsupported branch.
- The `Substitution`/`IndexedAccess`/`Conditional` type-flag bits were permuted
  relative to the pin (24/25/26); corrected, with a unit test. Inside the
  prototype the names were used consistently, so no frozen fixture was affected.
  It was a latent ordering defect: `CompareTypes` sorts union constituents by
  increasing flag value first, so a union holding two of these kinds (an indexed
  access and a conditional, say) would have been ordered differently from the
  pin. The comparator does not compare type flags today.

`mapped-conditional-infer`:

- `prependTypeMapping`/`appendTypeMapping`: merged mappers, which map the first
  result through the second where a composite instantiates it. A homomorphic
  mapped type is instantiated with the variable's mapping prepended, and a
  mapped property's template with `appendTypeMapping(mappedType.mapper,
  typeParameter, key)`, where the instantiated mapped type's mapper is the
  composite of its cloned iteration parameter and the outer mapper.
- `isTypeParameterPossiblyReferenced` begins with a precondition the reference
  lacked: a type parameter whose symbol does not have exactly one declaration is
  always possibly referenced. `Promise<T>` and its `this` type are declared in
  several lib files.
- Class and interface type parameters are members of the merged symbol: the
  same-named parameter of every merged declaration is one type. The reference
  created one per declaration, and with it a second `Promise<T>`. Environments
  may be keyed by any of the merged declarations, so the mapper lookup consults
  all of them; without that, a member declared in a secondary declaration was
  left uninstantiated and the marker comparison returned `True` where native
  returns `Maybe`.

`instantiation-limit` (a constant 4 types and 4 instantiations short, the same
on `Build<3>` as on `Build<1001>`, so all on the generic declaration path):

- `resolveTypeReferenceMembers` pads the type arguments with the type itself as
  `this`, for a tuple target too: the empty tuple `[]` is its own target and its
  base is the array type with that this argument.
- `getTupleBaseType`: a variadic element contributes `T[number]`, a real indexed
  access type, to the base array's element union.
- A generic (variadic) tuple defers every indexed access that is not a fixed
  element index, after resolving the object's members as
  `isStringIndexSignatureOnlyType` does.
- `newConditionalType` instantiates the root's check and extends types afresh
  (the extends type without the inferences) before the alias is instantiated.
  The deferred conditional previously stored the inferred extends type.

Earlier in the same effort, before this pass: the pin's conditional case in
`getOuterTypeParameters` (a conditional contributes its `infer` parameters to
every node below it, without which `ReturnType<typeof f>` resolved to `any`);
draining the `checkTypeRelatedTo` observer after lookup so lookup-time relations
are not charged to the first action; and `getUndefinedStrippedTargetIfNeeded`
with the `filterType`/`extractTypesOfKind` path behind it.

A negative result worth keeping: routing `instantiate_source_type`'s outer
parameter mapping through the counted `instantiate` overshot native
(`deferred-generic-members` 23 to 37 against 31). The counting site was right;
the environment was too wide, because the possibly-referenced filter did not
apply to function types. That is the first `flow-return-inference` port above.

## Semantic fixes and work parity

The reference is not the production checker and nothing in `crates/` changed.
It is the second implementation the E2 relater criteria compare the production
ID relater against, and the frozen contract (`data/s08/relater-fixtures.json`)
forbids "sealing/pre-resolving lazy graphs to bypass relation allocations": a
reference that skips work the pinned algorithm performs would win the
throughput and allocation comparison for the wrong reason. The counters are how
that is checked.

Most ports above changed what the reference computes, not only how much: the
merged type parameter (two `T`s, a second `Promise<T>`, and `True` where native
returns `Maybe`), the cache entry recorded as succeeded instead of failed for an
unconstrained type parameter, `T` instead of `T[number]` in a variadic tuple's
base, `length` of a generic tuple resolved instead of deferred, a deferred
conditional that stored the inferred extends type, a function type re-created
instead of returned by the identity entry, the shared empty type literal,
deferred references left unnormalized, and the intersection reductions.

A few are work parity only, with an unchanged result on these fixtures: the
counted `instantiateTypes` in `createMarkerType`, the counted instantiation of a
function declaration's type that returns it unchanged, the source rest tuple
built for an `any` rest target, `NonNullable<any>`, and the alias-argument
instantiation of a deferred conditional (which predates this pass).

One consequence for production is worth stating: `relater_prototype_parity`
requires both implementations to match under the strict comparator, so a
production optimization that avoids creating a type or an instantiation on one
of these 21 fixtures' paths would lower that metric even with identical results.
That is the comparator's rule, not the frozen contract's wording.

## Regression tests

- `generic_member_relation_performs_the_native_construction`: the frozen
  `deferred-generic-members` program, which needs no lib, must create 21 types
  and 8 signatures, run 31 instantiations and leave the native cache flags for
  the first identity relation, and nothing on a repeat. Verified to fail (25
  against 31) with the root-returning symbol instantiation disabled.
- `merged_interface_declarations_share_their_type_parameter`: verified to fail
  with the canonical type parameter disabled.
- `an_unaliased_empty_type_literal_is_the_shared_empty_type`,
  `type_flags_match_the_pinned_enumeration`, and the three tests from the
  earlier fixes.
- For this pass: `an_optional_mapped_type_makes_its_template_optional_once`,
  `mapped_modifiers_come_through_a_constrained_key_parameter`,
  `keyof_any_and_unknown_follow_the_pinned_results`,
  `an_intersection_reduces_over_unions_disjoint_domains_and_supertypes` and
  `a_contravariant_position_infers_its_own_candidate`. 64 unit tests in all.

Lib-dependent rules (Promise, tuples, `NonNullable`) are covered by the frozen
fixtures, not by unit tests: the unit fixtures bind without the default library.

## Beyond the frozen inventory (development check, not evidence)

Twenty-six programs outside the inventory were run through both
implementations and compared group by group (`extra.py` in the session
scratchpad; the frozen 105 remain the only evidence). **105 of 130 groups agree
exactly**, and no disagreement is silent: every remaining one is a named
refusal. Own conditional and mapped types, two-parameter and callback generics,
contravariance, optional and rest tuples, array-holding generics,
`Promise<'x'>` against `Promise<string>`, and `Partial`, `Pick`, `Record`,
`Required`, `Readonly`, `NonNullable`, `ReturnType`, `keyof`, a distributed
indexed access, `Build<3>` and the modifier-carrying `Pick` all match.

Closed in this pass (each had been a silent drift, which the frozen contract
forbids as much as a wrong result):

- `getTemplateTypeFromMappedType` applies a mapped type's `?` modifier to the
  template once, so a property instantiates `X | undefined` rather than gaining
  `undefined` afterwards, and `getTypeOfMappedSymbol` adds nothing when the
  property type can already be undefined or void (`Partial`).
- `getConstraintOfTypeParameter` resolves the base constraint behind a declared
  constraint, which is where `keyof T` is created for `P in K` with
  `K extends keyof T`, and `getModifiersTypeFromMappedType` finds the modifiers
  type through that constraint, while the keys still come from the constraint
  itself (`Pick`).
- `getIndexTypeEx`: `keyof unknown` is `never` and `keyof any` is the shared
  `string | number | symbol` (`Record`).
- A distributed indexed access passes its alias to the union it builds
  (`getUnionTypeEx(propTypes, ..., alias, nil)`), and a boolean index is not
  distributed.
- `getIntersectionType` is ported: flattening with the `includes` mask, the
  never/nullable/disjoint-domain/any reductions, `removeRedundantSupertypes`,
  the shared empty type literal, the intersection cache, and union
  distribution (paired nullable peeling, divide and conquer, cross product).
  Both the intersection type node and instantiation now go through it
  (`NonNullable`). The reductions that call back into relations, and two or
  more unions of primitive types, are refused by name.
- Contravariant inference candidates: a conditional's inference now carries
  upstream's `contravariant`/`bivariant` state, keeps `contraCandidates` apart
  and resolves through `getTypeFromInference`, so an `infer` parameter in a
  parameter position collects its own candidate.

What still refuses, by name, one program each:

| program | named refusal |
| --- | --- |
| `Awaited<Promise<string>>` | generic signature inference through compound types |
| `Parameters<typeof f>` | implicit `unknown[]` constraint of an infer parameter in a rest position |
| `Extract<'a' \| 'b' \| 1, string>` | substitution type in a conditional true branch |
| `Omit<{...}, 'y'>` | base constraint of an indexed access or conditional type |
| `G<T> = { [K in keyof T as ...]: ... }` | mapped nonliteral key/index signature |

The two the owner asked for moved but did not close. `Awaited` needed
`getIntersectionType`, which is now ported, and now stops at inference through
an intersection's members. `Parameters` needed contravariant candidates, which
are now ported, and now stops at `getInferredTypeParameterConstraint`: an
`infer P` in a rest position is implicitly constrained to `unknown[]`, which
creates an array type during lookup. That rule is refused precisely where it
would produce a constraint, so `Promise<infer U>` and the other frozen infer
forms are unaffected. `Extract` needs substitution types, which remain a named
unsupported kind.

## Known differences that do not affect the comparison

Creation order within a phase differs in places: the reference creates an
interface's or alias's type parameters before the declared type, where the pin
creates the declared type first, and it creates a deferred conditional after its
alias arguments are instantiated rather than before. Counts and states are
identical; type ids inside a phase are not. Nothing compared depends on ids
today. Union constituent order does depend on type ids in the pin, so a fixture
that unions a type parameter with its own declaring type could expose this.

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

Named unsupported branches still include contravariant conditional inference
candidates, inference between generic signatures, non-array variadic rest
slicing, some mapped union/array cases, an instantiated intersection that needs a
new intersection type, the non-nullable form of `unknown` and of instantiable
types, the genericity of mapped and template parameter types, a type parameter
constraint that needs a this argument, string mapping/substitution and some
diagnostic display forms. Do not silently substitute success or adjust counters
to close these gaps. Several thousand lines of reference code still need
independent review. Prototype unit tests do not establish native fixture parity;
the frozen comparison does.

## Remaining

The six fixture families are closed. What is left is the owner's: the full
normal/allocation capture (`scripts/s08_relater.py capture`), `cargo xtask run
relater`, and the status regeneration. Until that capture is recorded the four E2
relater criteria remain unmet in the ledger, although the parity the first one
asks for now holds on the development host.
