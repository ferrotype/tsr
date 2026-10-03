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

# Finishing changes

Parity with the pin is an expectation file per suite, `status/parity/<suite>.json`,
that CI recomputes on every pull request ([docs/EVIDENCE-plan.md](docs/EVIDENCE-plan.md)).

- Run focused checks for the changed code: the crate's tests and clippy, and
  `python3 scripts/parity.py run <suite> --output DIR --id <variant>` for the
  tests a change touches. CI runs the full suites.
- A change that fixes or breaks a test shows up as a `check` difference; run
  `parity.py accept` so the expectation file is exact for the commit, and give
  every entry you add a reason. Approvals (`approved`) are the owner's.
- `cargo xtask validate` must pass: every `// port:` marker names a function of
  the pinned inventory. Do not regenerate the ledger unless the pin moved.
- Check each command's exit status. Never commit after a failed validation
  command. Use new commits and normal pushes for published branches.
