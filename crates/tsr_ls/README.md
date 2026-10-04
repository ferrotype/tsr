# tsr_ls

Read-only language services over retained compiler snapshots: hover and classified
symbol display, references/highlights, definitions/implementations/source maps,
symbols, signature help, inlay hints, semantic tokens, call hierarchy, code lenses,
selection ranges, folding and linked editing. It also owns LSP coordinate and
diagnostic conversion.

Callers retain the program and checker lease for each request. Navigation-only
implementation files keep private owners, and source-definition's `QueryChecker`
callback acquires a checker only when semantic lookup is necessary. Results are
converted using the negotiated UTF-8/UTF-16 encoding and content-map projections.

See [the L3 development checks](../../tools/phase5/lsp/README.md#l3-read-only-features)
for native comparisons and the remaining cross-project/mapper integration work.
