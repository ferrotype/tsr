# L4 implementation record

L4 is in progress on top of L3. This checkpoint adds completion and resolution
routing, contextual/member/literal/JSX completions, JSDoc snippets and tags,
closing-tag insertion, module/reference paths, and a project-owned export index
with import edits. Published indexes retain names and source identities, never
checker-local handles. Resolve reacquires the current program's checker.

`tools/phase5/lsp/completions.py` compares real Go/Rust server responses, including
resolved items and resulting contents after applying import edits. The first
checkpoint matches 163 responses in each of four UTF-8/UTF-16 and minimal/rich
capability combinations. It preserves all response fields, sorting completion
lists by the pinned fourslash comparator. This is a bounded development sample,
not the complete L4 family or the L7 replay acceptance.

Focused autoimport, language-service, format, project and LSP tests pass, as do
the changed crates' clippy checks, package policy and `cargo xtask validate`.
The existing native watcher test requires FSEvents access outside the sandbox.
There is no LSP adapter in `parity.py` yet; its replay supervisor belongs to L7.

Remaining L4 work includes package discovery and entrypoint filtering, complete
path-map completion, remaining completion/snippet contexts, preference and
cache-invalidation cases, and expansion against the assigned native families.
Multi-project discovery and ATA remain L6 dependencies as in the Phase 5 plan.
