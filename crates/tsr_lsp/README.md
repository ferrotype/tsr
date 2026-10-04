# tsr_lsp

Production language-server dispatch over immutable project sessions. Includes
initialization, document synchronization, inferred options and configuration,
pull and push diagnostics, cancellation, progress, logging, watcher registration
and a native watcher fallback. `tsrust --lsp --stdio` uses this library.

The advertised pinned capabilities are the final protocol contract. Feature
handlers assigned to Phase 5 L3–L6 currently return a named method-not-implemented
error. This is not yet a complete editor language server.
