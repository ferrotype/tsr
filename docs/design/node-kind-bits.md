# Kind hints on node edges

A proposal, with the measurements that size it. Nothing here is implemented.
The first draft put the kind inside `NodeId`; the review of #115 showed why
that layout cannot work, and this amended draft moves the hint out of the
identity and into the storage edges, under an explicit lifecycle. The
review's findings are listed at the end with where each one lands.

## The case

The October profiling pass counted the check phase's node reads on the sixty
heaviest checker variants (`tsr_ast` feature `access-stats`, split per phase
by the checkerbench clocks): 195M reads, of which 92M (47%) served only a
kind check, 9M (5%) nothing but validation, 43M (22%) a parent walk and 46M
(23%) decoded the node's data. A read costs about 4 ns (the directory, a
bounds check, the header load). The pin reads a node's kind through a
pointer for well under a nanosecond.

Those counts predate the flow-reference shape of #115, which removed a share
of exactly these reads; the first step of the work is to count again on the
current head. If the kind-only share holds, a child whose kind arrives with
its edge makes `is_identifier(child)`, `kind == K::X` on a child, and the kind
tests of every ancestor walk bit arithmetic on a word already in hand: an
estimated 8 to 10% of the check phase, and the binder's kind reads as well
(the binder is the parse-bind gap at 1.8x; see the profiling notes). The
number is a sizing hypothesis until a bounded prototype measures it.

## Layout

**`NodeId` does not change.** It stays `arena: u32 | slot: u32` in a
`NonZeroU64`, with its derived equality, hashing and ordering, its `bits()`,
and `from_parts`. There is no second encoding of an identity, so the
consumers that hash `bits()` directly (`checker/key.rs`, `checker/flow.rs`)
and every map keyed by `NodeId` keep one canonical key. Identity and hint
are different things and live in different places.

**The hint lives in the child word of a sealed file.** A compact child edge
is a `u32` today: 0 for a nil edge, `u32::MAX` for an escaped full identity
(a foreign owner, resolved through the escape table), otherwise the slot.
A sealed file's words become `kind: 9 | slot: 23`, kind 0 meaning "no
hint", the two sentinels unchanged. The word does not grow, the escape table
does not change, and nothing is allocated.

- **A `ChildWord` newtype** replaces the raw `u32` wherever a word is read or
  written: the compact lists (`local_word`, `encode`, `get`), the generated
  readers, the parent and list readers, the encoder's and decoder's edge
  handling, and the local binder, which today treats each word as a slot
  (`BindContext::node` wraps the word, `LocalBind::node` hands it to
  `resolve_slot`, `node_id` reconstructs from it). Its only decoders are
  `slot()`, `hint()` and the sentinel tests, and they take the file's format
  from the storage, so an omitted conversion is a type error rather than a
  silently wrong integer. Auxiliary, symbol and flow words keep their own
  formats and types.
- **Readers hand out a `Child` handle**, the id with its hint, produced only
  by a successful read on a view. Predicates on children consult the hint;
  anything that needs a key, a map entry or a comparison takes `.id()`. A
  `NodeId` arriving from anywhere else carries no hint and takes the
  checked read it takes today.
- **Open kinds.** `NodeKind` is an `i16` and the factory, the encoder and the
  decoder preserve kinds outside the 351 known ones (`-1`, `512`, `32767`
  round-trip today). A hint is written only for a raw kind in `1..=511`;
  every other kind is stored unhinted and the header keeps the full value.
  Nothing is truncated or rejected.
- **Large files.** A slot beyond 23 bits (8.4M nodes in one arena, reachable
  with about 8 MiB of `a;`) does not fit the hinted word. The file's format
  is chosen at seal: a storage whose arena is within the range gets hinted
  words, a larger one keeps the plain 32-bit slot words it has now and never
  carries hints. There is no refusal, no capacity error and no change to the
  infallible allocation paths; the limit bounds where the optimization
  applies, not what the parser accepts. The core, lazy (JSDoc) and synthetic
  owners each carry their own format flag.

## Lifecycle: when a hint may be trusted

A hint is a copy of a header's kind, and kinds do change after an id is
minted: `FactoryHooks::on_create` may replace a node's whole record through
`node_mut` (a test in `storage/parent_tests.rs` does exactly that, and the
saved id then reads the new kind), the transformers edit completed files
through `ast_builder_mut`, a bind result's `node_mut` routes into the parsed
builder, and `ParsedFile::builder_mut` reopens a completed parse. The rule
is therefore not "kinds never change" but a sealed/unsealed lifecycle that
the storage enforces:

- **Hints are written at seal.** `Arena::seal` already moves a completed
  file's pages into one vector when `AstBuilder::complete` runs; the same
  pass writes each word's kind from the child's final header and marks the
  storage hinted. Before the seal every word is a plain slot and every read
  consults the header, so construction, hooks and the decoder's generated
  creation need no care.
- **Mutable access unseals.** Every entry to mutation of a sealed storage
  (`builder_mut`, `node_mut`, the transformers' `ast_builder_mut`, the bind
  result's routing) passes through one `unseal`, which clears the trust
  flag in constant time: readers then mask the slot out of the word and
  ignore the kind bits until the storage is sealed again, when the kinds are
  rewritten from the headers in one pass. A flags-only edit
  (`set_node_flags` on a completed file) does not change kinds and stays
  outside the unseal path.
- **Validation of the fast path.** A `Child` handle exists only because a
  validated read (owner, slot, publication) on a view produced it, and its
  hint is authoritative for that view's sealed, trusted storage. Wrong
  owners, invalid slots and unpublished or failed files never produce a
  handle, because the read fails first; a bind overlay reads the parsed
  words and adds binding data without changing kinds, and its mutation path
  unseals like any other. Release builds keep every check they have today;
  the optimization removes the header load from reads that already hold a
  handle, not the validation of imported ids.
- **Debug assertions and corpus runs** compare every consulted hint with the
  header's kind. They check the lifecycle; they are not what enforces it.

## Declaration lists

Symbol declarations (`declaration_lists.rs`) are stored as compact
`(arena, slot)` rows and reconstructed on read, and the checker tests their
kinds constantly. A row has room for the same nine bits under the same
rule: written when the bind result is completed, trusted while it is
immutable. This is a second step after the edges, measured separately.

## Cost and order

1. **Count again** on the current head (`access-stats`, per phase): the
   eligible kind-only share after the flow-reference shape. Half a day.
2. **`ChildWord`** and its decoders, with every word consumer converted
   (compact lists, generated readers, parent and list readers, encoder and
   decoder, the local binder). A day; the compiler finds the omissions.
3. **The seal lifecycle**: the kind pass in `seal`, `unseal` on the mutation
   entries, the per-storage format and trust flags, the debug assertions.
   Tests: hinted and unhinted files side by side, open kinds (`-1`, `512`,
   `32767`), nil and escape words, the largest hinted slot, a file past
   2^23 nodes in the plain format, unseal and re-seal around a transformer
   edit, a hook that changes a kind during construction, overlays. A day.
4. **A bounded prototype**: `Child` handles from the view's child accessors
   and the ten hottest kind predicates and ancestor walks in the checker
   switched to the hint, measured in concurrent pairs on the top-60 and the
   deep and flow groups, including the seal pass and the handle plumbing;
   the memory census must be unchanged by construction (same word size, no
   new allocation). A day. The result decides whether the rest of the
   checker and the binder follow.
5. **Validation**: the unit tests, the seven parity suites, the mutation
   assertion over the corpus. Half a day.

## What the review of #115 found, and where it lands

1. *Kind bits inside `NodeId` with an unknown-kind alternative break
   identity: derived equality, hashing and ordering cover the whole word,
   and `checker/key.rs` and `checker/flow.rs` hash `bits()` directly.*
   `NodeId` is unchanged; the hint is in the edge, never in the identity.
2. *Kind immutability was an assumption to verify; hooks, `node_mut`,
   `builder_mut`, transformers and overlays change kinds after ids escape,
   and repairing one parent cannot reach copied ids.* The sealed/unsealed
   lifecycle above: hints exist only in sealed, trusted storage, and every
   mutation entry unseals.
3. *Kind-only reads must not bypass the validation boundary.* The fast path
   consumes handles that only validated reads produce; raw ids keep the
   checked read.
4. *Nine bits are the known kinds, not the kind domain.* Kinds outside
   `1..=511` are unhinted; the header keeps the `i16`.
5. *The local binder consumes edge words as slots and would silently
   misread packed words.* The `ChildWord` newtype makes every consumer,
   the binder included, decode through one place.
6. *The 23-bit limit is reachable with about 8 MiB of source and the
   allocation paths are infallible.* No limit on what parses: a large
   arena keeps the plain format and carries no hints.

And the gain is to be re-counted and then measured on a bounded prototype
before the full migration, as the order above says.

## Decision needed

When to spend the four days: now, or after the binder pass, which the
parse-bind attribution put next and which would use the same hints.
