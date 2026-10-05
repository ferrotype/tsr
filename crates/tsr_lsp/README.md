# tsr_lsp

Production language-server dispatch over immutable project sessions. Includes
initialization, document synchronization, inferred options and configuration,
pull and push diagnostics, cancellation, progress, logging, watcher registration
and a native watcher fallback. `tsrust --lsp --stdio` uses this library.

Language services include navigation, completion, editing, cross-project
references/rename/implementation and incoming calls. Sessions integrate project
reference source redirects, automatic type acquisition and content-mapper
processes. Phase 5 L7 still owns full fourslash/replay coverage and latency work;
the advertised capabilities are not a claim that those remaining checks pass.

Cancellation targets requests once dispatch starts, as in the pin; queued IDs
are still reserved. Watch events from either transport schedule one debounced,
capability-gated diagnostic refresh for relevant files or directories.

Two deliberate lifecycle safeguards are retained with owner approval
(2026-10-04): duplicate in-flight IDs are rejected under ADR 0019, and requests
after shutdown are rejected with `InvalidRequest: server is shut down` instead
of re-entering a closed project session. Go overwrites duplicate IDs and has no
shutdown admission state. `exit` still requires initialization and valid absent
parameters, and remains available after shutdown.
