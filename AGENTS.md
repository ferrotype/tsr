# Rust implementation guidance

For Codex/Astra, read [the Rust guide](docs/CODEX-RUST-GUIDELINES.md) once per
task, only when:

- Preparing a concrete Rust implementation or code change, including a Go-to-Rust port.
- Implementing or modifying Rust code.
- Reviewing Rust code.

Decide from the requested work, not incidental source inspection.

Do not load it for status or evidence questions, interpreting memory/CPU profiles
or benchmarks, roadmap discussion, general explanations, or documentation-only
work. Mentioning Rust or Go does not trigger it. An explicit request to inspect
or edit the guide itself is an exception.

These loading conditions also apply when another document links to the guide.

# Finishing changes and recording evidence

Before recording evidence or preparing a commit, follow
[the finish procedure](docs/TRACKING.md#finish-procedure).

- Run focused checks for the changed code. Do not rerun every producer or the
  full `selftest` merely to commit; CI runs the full suite.
- A stale historical capture is not a failing compiler test. Refresh only the
  evidence required by the current task; preserve earlier results and approvals.
- Finish source edits before captures, render status once at the end, and stage
  the referenced artifacts with their pointers. Do not regenerate inventories
  unless their actual inputs or classifications changed.
- Check each command's exit status. Never commit after a failed staging or
  validation command. Use new commits and normal pushes for published branches.
