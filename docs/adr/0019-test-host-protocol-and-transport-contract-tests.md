# ADR 0019: Test-host protocol and transport contract tests

Status: Accepted (2026-09-19)
Plan: section 8 (oracle seams), Phase 1/5 boundaries and section 14, step 6
Sprint: S11

## Context

The pinned Go fourslash harness injects a filesystem, case sensitivity, symlinks,
a parse cache, plugin spawners, inferred-project options and an initialization
signal. Phase 5 retains those executable assertions and connects them to the
Rust project system and language service. S11 establishes the transport seams;
it does not implement that integration.

The owner accepted this revised design after reviewing the first implementation.
Wire version 2 replaces the configuration callback and mapper method proxy. The
version-1 captures remain historical evidence, not evidence for this contract.

## Decision

1. Keep the separate `ts_testhost --stdio` prototype and reusable session API.
   The endpoint executes no external code, performs no disk fallback and makes
   no claim to execute fourslash semantic assertions.

2. Use Content-Length JSON-RPC framing compatible with pinned `baseproto.go`,
   with bounded frames, unique object keys and bounded nesting. Malformed input
   terminates the connection. Client request IDs follow the LSP domain: strings
   or signed 32-bit integers, including zero and negatives. Reject duplicate
   in-flight IDs; allow reuse after the final response. String and numeric IDs
   remain distinct. Canceled callback tombstones cannot affect reused IDs.
   Outbound size admission includes the actual request ID.

3. Use a bounded, single-threaded router. It continues processing input while
   callbacks await responses or streams await credit. S11 does **not** prove
   that a worker blocked in a synchronous filesystem call can issue a reverse
   request while the router keeps pumping. That bridge and its blocked-worker
   cancellation/progress tests belong explicitly to Phase 5; PLAN.md reflects
   the same boundary. A peer must continue draining the connection's output.

4. Use pinned callbackFS semantics for the five read operations, with only the
   injected immutable filesystem as fallback. Preserve null/delegate versus
   missing/empty, including `{}` as a missing readFile result. Record restricted
   or projected behavior in [S11](../S11.md).

5. JSON strings are strict Unicode; reject invalid UTF8 and lone surrogates.
   This leaves a known gap against the repository's arbitrary-byte source-text
   contract: those files cannot cross the filesystem JSON transport. Do not
   replace bytes silently. The base64 mapper tunnel can carry arbitrary bytes.
   Opaque JSON options, progress and error data retain their numeric tokens.

6. Options travel client to server in `test/initialize` and `test/setOptions`.
   Completion is an internal endpoint hook: an immediate stub in S11 and the
   server's project system in Phase 5. The request response is the completion
   barrier; `testhost/initialized` mirrors `InitComplete`. Refuse new filesystem,
   spawn and stream-write work while an update is pending. Reject size failures
   before invoking the hook. A failed hook guarantees no applied change; an
   applied update returns success even if cancellation raced with commitment.
   Keep the barrier until the outcome is known. Remove `testhost/configuration`.
   This does not remove LSP `workspace/configuration`: Go's real initialization
   signal includes that separate exchange, to be connected in Phase 5.

7. Tunnel the raw byte stream returned by `contentmapper.Spawner`. The server
   asks the client to spawn a registered plugin and receives a stream identity.
   Ordered bytes move unchanged in both directions in bounded base64 frames,
   with separate stderr, bounded credit, EOF, close and exit reporting. Exit
   status may be unavailable for an in-process spawner; never invent success.
   Either side can close; the client owns process cleanup, including disconnect
   and late results after canceled spawning. Cancellation retires the name.
   There is no limit of one mapper request per plugin: mapper frames and their
   concurrency belong to the actual mapper host, not to this tunnel. S11 driver
   operations exercise the transport; Phase 5 attaches the real mapper host.

8. Carry a patch against the pinned Go fourslash harness, not a fork. Retain Go
   executable assertions and patch transport only. S11's small access-only
   oracle overlays are separate from that future harness patch. Replay the five
   actual Go test-spawner sequences through the tunnel and compare concatenated
   bytes in each direction. Read/write chunk boundaries are not semantic.
   Report transport-only stress/failure cases separately from Go observations.

Phase 5 LSP shares the same connection. `test/` and `testhost/` names stay
separate from LSP methods. Server requests use never-reused `callback:<n>` IDs;
the two directions have independent request identity spaces, so a client may
also use such a string. `$/cancelRequest` retains LSP cancellation semantics;
`testhost/progress` is separate from LSP `$/progress`. Parse-cache injection is
not represented by S11 and remains a Phase 5 requirement.

## Alternatives considered

**Wire configuration callback.** Rejected. The client supplied the options, but
was then asked to apply and acknowledge them back to the server. Go's setter
updates server-owned state; the Rust project system will own application too.
The extra callback assigned responsibility to the wrong party and introduced
size-admission and canceled-update consistency failures. An internal hook puts
publication and response commitment at the owning endpoint.

**Six-method plugin proxy.** Rejected. Forwarding initialize/open/transform/close
methods implements a second, test-only mapper host. It bypasses the real host's
framing, concurrency and process failure handling. The one-outstanding-method
restriction also contradicts pinned `projectLease.Transform`, which releases
the host lock before invoking the mapper. A raw stream preserves the spawner
boundary so the production host can be exercised in Phase 5.

## Consequences

* Version 1 peers must migrate; accepting the ADR alone does not certify the
  revised implementation. Fresh S11 evidence must cover version 2.
* Base64 expands bytes by roughly one third. Credits, chunk limits and lifecycle
  states add protocol complexity, but bound memory and avoid waiting inside the
  router for a plugin consumer. The shared connection still has head-of-line
  effects, and a client that stops reading can block the synchronous writer.
* Opaque transport permits concurrent mapper requests and preserves malformed
  mapper bytes for the real host to diagnose. The endpoint cannot validate
  mapper semantics; byte equality is a transport result only.
* Strict Unicode limits filesystem inputs. Parse-cache injection, synchronous
  worker bridging, project-system application, actual mapper-host integration
  and full LSP/fourslash behavior remain explicit Phase 5 work.
* The carried harness patch must be maintained at pin updates, while keeping
  the executable assertions unchanged. No parallel test-language fork is added.

## Evidence

The authorities at the pin are `internal/api/callbackfs.go`,
`internal/jsonrpc/baseproto.go`, `internal/jsonrpc/jsonrpc.go`,
`internal/contentmapper/hostimpl.go` (`Spawner`, `projectLease.Transform`),
`internal/fourslash/fourslash.go`, and `internal/lsp/server.go`
(`SetCompilerOptionsForInferredProjects`, `InitComplete`).

The frozen inventories live in `data/s11/`; access-only bridges in `tools/s11/`;
the producer is `cargo xtask run testhost`. See [S11](../S11.md) for the exact
wire schema, bounds, hook contract and validation scope.
