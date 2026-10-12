# Live native watch resource probe

This bounded probe is for Phase 7 R5 item 3. Its standalone release build,
warnings-denied clippy, isolation controls and fifty-cycle tiny native control
have passed. A fifty-cycle canonical MUI docs capture also passed with its
16,766-file roster, exact tracked-count plateau, zero tracked teardown and
unchanged original inventory. Evidence must be retained separately. These live
controls exercised macOS FSEvents; the Linux measurement path remains untested.
It requires a provisioned project root, its config, and an included editable
TypeScript file. The owner's largest project is a required separate capture;
a small synthetic control does not certify that project.

`run.py` copies the whole project into a new retained capture directory. It
preserves contained symlinks by remapping them into the copy, rejects links
outside the supplied root and special files, and never creates hardlinks.
Contained dangling links to unbuilt workspace packages are preserved. On macOS,
`--copy-mode clone` requires APFS `clonefile` copies with independent inodes and
copy-on-write storage; it never falls back to hardlinks or ordinary copying.
Only the copy's selected TS file is edited. The project is watched with
`--noEmit --incremental false --composite false --runExternalCode false`; no external mapper is
executed and compiler output is disabled. `.git` is excluded. The source
inventory is compared again after a successful run. Concurrent original-project
changes invalidate the capture without implying the runner caused them.

Each edit appends the same-width comment with a generation number, preserving
UTF-8 (with or without BOM) or BOM-marked UTF-16 LE/BE encoding. For UTF-16, an
incomplete final byte stays at the end where the pinned file decoder ignores it.
The controller and probe compare decoded compiler text, not raw encoded file
bytes; arbitrary non-BOM UTF-8 bytes stay unchanged. The source's
semantic work and file roster stay equivalent; source bytes and source identity
change. The probe requires real native filesystem callbacks, observes the new
bytes through the actual compiler program, and checks that the superseded edited
`ProgramFile`'s weak reference has expired after each cycle. It retains counters
and one weak current-file reference, rather than old program roots.

The standalone tooling workspace source-includes the production CLI's
`OsSystem`, process support, local-time support and counting mimalloc allocator.
It calls public `tsr_execute::command_line` with a passive ownership observer and
the actual default `FSWatchBackend`. The observer makes the command return a
watcher and enables the existing incremental testing mode. That mode uses full
source texts as versions and retains the previous snapshot, so all memory
observations here are **testing-mode watch memory**, not ordinary CLI `--watch`
memory. The probe controls
`do_cycle` timing after real native callbacks; it does not exercise the CLI's
blocking `WatchManager::run_loop` scheduler. No events are injected or mocked.
Native backend errors/timeouts and returned compiler failures remain failures.
Ordinary compiler diagnostics remain in `compiler.log`; a watch command's
startup status does not encode the current diagnostic count.

After five warmup edits, the default fifty-cycle capture requires exact bounded
tracked owner/allocation counts, release of superseded edited files, and zero
tracked owners/storage after watcher close. These arena counts exclude cache
bookkeeping and general allocator bytes. Native descriptor counts are measured
by `/proc/PID/fd` on Linux or numeric `lsof` descriptors on macOS, with a default
steady-state spread bound of two. The before-watch and after-close descriptor
difference is retained separately; it is not equated to the arena teardown gate.
Testing-mode watch RSS comes from `ps` and is reported as samples and a fitted
trend separately. New captures carry an explicit `memory_scope` label.
The macOS descriptor measurement counts numeric file descriptors, not Mach
ports or every native backend resource.
Allocator high-water RSS never becomes a leak assertion or zero-byte teardown
requirement. The source inventory, compiler log, raw probe records, binary hash,
all samples and gate outcomes remain in the capture.

## Provision and build separately

Dependencies include the repository's pinned Rust toolchain and cached Cargo
dependencies, native filesystem watcher support, `ps`, and `lsof` on macOS.
The manifest has its own `[workspace]` and lockfile, so building it does not edit
the production workspace or the S10 consumer lockfiles. The production CLI's
reserved compiler-thread stack and release allocator are preserved. Build only
after coordinating with other captures; no build is implicit in `run.py`.

```sh
cargo build --offline --release \
  --manifest-path tools/phase7/watch/Cargo.toml \
  --target-dir target/phase7/watch \
  --message-format=json > /absolute/evidence/watch-build.jsonl
```

Use the matching `compiler-artifact.executable` from Cargo's JSON output as
`--probe`; do not assume Cargo's configured executable path. Retain that build
output and the compiler identity alongside the capture. The script hashes the
selected binary but does not independently certify how it was built.

Inspect an isolated copy without starting any compiler:

```sh
python3 tools/phase7/watch/run.py prepare \
  --project /absolute/provisioned/workspace \
  --config relative/project/tsconfig.json \
  --edit-file relative/project/src/included.ts \
  --output /absolute/evidence/watch-prepare
```

Capture with a new directory:

```sh
python3 tools/phase7/watch/run.py run \
  --project /absolute/provisioned/workspace \
  --config relative/project/tsconfig.json \
  --edit-file relative/project/src/included.ts \
  --probe /absolute/cargo/artifact/executable \
  --output /absolute/evidence/watch-new
```

The first control should use `fixtures/project`: two real source modules and
minimal `noLib: true` global interfaces, so the runner's event, omission, plateau
and close checks can be verified quickly. Select `index.ts` as the edited file.
The capture timeout applies to every startup/cycle/close phase. A failure keeps
its raw log and partial observations. A failed run is never a passing resource
measurement.

Run isolation controls with `python3 -B tools/phase7/watch/test_isolation.py
--probe /absolute/executable`. They check external-link refusal, copied internal
links and independent inodes for ordinary/APFS copies, UTF-8 BOM and UTF-16 LE/BE
edit/rebuild cycles, and a short live watch
whose config points emission/build-info paths outside the copy. The external
directory and original fixture must remain unchanged.

For a canonical workload, retain its provision marker and scenario beside the
capture, use the entire provisioned case as `--project`, and pass its frozen
roster as `--expected-program-files`. For `mui-docs`, the config is
`docs/tsconfig.json`, `docs/next.config.ts` is explicitly included, and the
frozen roster has 16,766 files. A different observed roster refuses a pass.
