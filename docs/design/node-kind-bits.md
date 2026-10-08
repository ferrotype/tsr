# Kind bits in `NodeId`

A proposal, with the measurements that size it. Nothing here is implemented.

## The case

The October profiling pass counted the check phase's node reads on the sixty
heaviest checker variants (`tsr_ast` feature `access-stats`, split per phase
by the checkerbench clocks): 195M reads, of which 92M (47%) serve only a
kind check, 9M (5%) serve nothing but validation, 43M (22%) a parent walk and
46M (23%) decode the node's data. A read costs about 4 ns (the directory, a
bounds check, the header load). The pin reads a node's kind through a
pointer for well under a nanosecond.

If a `NodeId` carried its node's kind, the 92M kind-only reads would not
happen: `is_identifier(id)`, `kind == K::X` on a child, and the kind tests of
every ancestor walk would be bit arithmetic on the id. That is 8 to 10% of
the check phase; nothing else in the node-access area is left (flat pages
are done at 1.4%, the directory measured negative, parent reads are about
1%). The walks also get cheaper: a walk reads a node for its parent id and
tests the parent's kind without reading the parent.

## Layout

`NodeId` is a `NonZeroU64` packing `arena: u32 | slot: u32`, both nonzero,
shared by `SymbolId` and `AuxId` through `packed_id!`. There are 351 node
kinds (`NodeKind` is an `i16`), so a kind needs 9 bits.

Proposed: `arena: 32 | slot: 23 | kind: 9`, kind in the low bits.

- Ordering is unchanged in effect: ids compare by arena, then slot, and the
  kind is a function of the two, so no map or sort keyed by `NodeId` changes
  its order.
- The arena keeps its 32 bits: arena ids are a process-global sequence
  (`NEXT_ARENA`), and a long editor session mints several per parse.
- The slot drops to 23 bits: 8.4M nodes per file. The largest library files
  are a few hundred thousand nodes; a generated file of several hundred MB
  could exceed it, and the parser would refuse it with a clear error rather
  than wrap. That limit is the one real cost of the layout.
- `SymbolId` and `AuxId` keep the current layout; the macro grows a variant.

## Where ids are made

Sixty-four sites construct ids from parts, most in tests. The ones that
matter:

- `StorageBuilder::push_node` (`tsr_arena/src/file.rs`): the parser's and the
  decoder's allocation. The stored node is in hand, so the kind is known.
- Child edges in the compact payload (`CompactContext::decode_node`,
  `compact/lists.rs`): a child is stored as a `u32` word, 0 for none,
  `u32::MAX` for an escaped full reference, otherwise the slot. The parent
  header's `parent: u32` is the same encoding. Reconstructing the child's id
  here must not read the child's header, or kind-only reads come back
  through the edges. The word has room: `kind: 9 | slot: 23` fits in 32 bits
  with the two sentinels kept (slot 0 and all ones), so edges carry the kind
  at no memory cost. The encoder writes it from the child's header at
  construction.
- Declaration lists (`declaration_lists.rs`): symbol declarations stored as
  compact (arena, slot) rows and reconstructed on read. The checker tests
  declarations' kinds constantly; the rows should carry the kind too (9 bits
  of a row word), or those reads stay.
- API node handles (`index.kind.path`): the kind is already in the handle.
- Everything else (`from_parts` in tests, token caches, the binder's local
  reads): a kind of 0 means "unknown", and `NodeId::kind` falls back to the
  header read. Correctness never depends on the bits; they are a cache of
  the header's kind.

## What must hold

- **A node's kind never changes after its id is embedded anywhere.** Kinds
  are written in two places: `NodeMut`'s whole-node write
  (`node_mut.rs`, `header.kind = value.kind`) and the auxiliary record kind
  (`auxiliary.rs`, a list kind, not a node kind). The parser's reparser
  mutates node data through `node_mut(..).data_mut()` and creates new nodes
  for reparsed JSDoc types; it is not known to change an existing node's
  kind, but that is the first thing to verify: a debug assertion in the
  whole-node write that the kind is unchanged, run over the corpus suites.
  If a site does change kinds before the parent edge is encoded, nothing is
  wrong; if one changes kinds after, that site must re-encode the parent's
  edge, or the design falls back to headers for that node class.
- **Every read validates.** `NodeRead` keeps validating the arena and slot;
  the kind bits add a debug assertion against the header's kind, so a stale
  bit is caught in debug builds and the corpus suites.
- **The decoder and the API client's encoded trees** (`tsr_encoder`) mint
  ids through the same builder; the wire format is unchanged (indices, not
  ids).

## Cost and order

Three to five days: the id layout and macro (half a day), the edge word
format with its encoder and decoder (a day), `NodeKind` from the id with
the header fallback and the debug assertions (half a day), the hot
predicates and walks switched to id kinds (a day), then the full validation
(unit tests, the seven parity suites, the mutation assertion over the
corpus, memory census unchanged by construction). Measured the same way as
the rest of the pass: concurrent pairs on the top-60 and on the deep and
flow groups.

Expected: 8 to 10% of the check phase on the heavy variants, more on the
deep-expression ones where ancestor walks dominate, and a visible share of
the LSP token search. Not expected: a change in the ratio's order of
magnitude; the remaining checker gap is elsewhere (see the profiling notes).

## Decision needed

Whether the 8.4M-nodes-per-file limit is acceptable, and whether to spend
the week now or after the flow-analysis work, which measured larger.
