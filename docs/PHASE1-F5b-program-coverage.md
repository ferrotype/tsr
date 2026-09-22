# F5b program-loader witness audit

This audit adds seven exact compiler operation links, supported by two named
witnesses in `data/phase1/cases.json`. It also found and fixed two loader
boundaries that the original 48 program observations did not exercise. The pin
remains `1f70213d4922b434345f639b441681e470c7cfc1`.

The original `program-requests.json`, `program-observations.json`,
`program-manifest.json` and `tools/s07/program/export_test.go` remain byte-for-byte
unchanged. Four supplementary native observations live in
`data/s07/program-boundary-{requests,observations,manifest}.json`. They are
produced by `scripts/s07_program_boundaries.py`, using the original adapter's
observation types and the separate access-only test at
`tools/s07/program/boundaries/export_test.go`.

## Production corrections

### Skip module-resolution metadata

Pinned `fileLoader.loadSourceFileMetaData` immediately returns filename-derived
implied format when `SkipModuleResolution` is true. Rust skipped dependency
loading but still asked the resolver for package scope when constructing file
metadata. That could retain a package directory/type and turn an ordinary
`.ts` source into an ES module while resolution was explicitly disabled.

`metadata::load` now receives the loader's skip flag and bypasses package scope.
The existing library shortcut remains CommonJS. The new
`skip-resolution-metadata` request supplies `type: "module"` both in `/src` and
in a package under `node_modules`. Native and Rust both report empty package
directory/type for all three roots, CommonJS for the two `.ts` roots and ESNext
for the `.mts` root. Full file observations compare, including source identity,
contents, imports and external-module indicators.

### Custom default-library paths

Go normalizes the host's default-library directory against the current directory
when constructing its loader, then removes a trailing separator before the
library-priority prefix check. Rust previously retained the input verbatim and
used it directly for that check. A trailing separator therefore assigned all
libraries the unknown-library priority; a relative directory also lost the
normalized-prefix invariant.

Rust now normalizes the directory once and strips its trailing separator in
`lib_priority`. Three requests use `/custom/`, `/custom` and `../custom/` from
`/src`, with the explicit libraries supplied in reverse priority order. All
produce `/custom/lib.es5.d.ts`, then `/custom/lib.es2015.promise.d.ts`, then the
root source, with identical library flags and metadata. The separator-free
request is the control. No library contents are replaced by harness logic.

## Exact new witnesses

All operation IDs below have prefix `tsc/internal/compiler/`. The production
annotations name existing equivalent code, with the two corrections above.

| Witness and literal request | Operations newly linked | Observable result and necessary path |
| --- | --- | --- |
| `witness/s07-program-boundaries-rust`; `skip-resolution-metadata` | `fileloader.go:fileLoader.loadSourceFileMetaData`, `program.go:Program.GetSourceFiles` | The native test calls `processAllProgramFiles`, then `Program.GetSourceFiles`; Rust reads `Program::files`. The three complete file records distinguish package metadata from filename-only metadata and preserve source order. |
| Same witness; `library-path-trailing-separator`, `library-path-normalized-control`, `library-path-relative` | `fileloader.go:fileLoader.getDefaultLibFilePriority`, `program.go:Program.GetSourceFiles` | Two explicit libraries require the priority comparison. The returned file order reverses the configured list identically on both runtimes, independent of host path spelling. |
| `witness/s07-include-helper-paths-rust`; `singleton-import`, `references`, `type-reference-package-zero`, `library-directive` | `fileInclude.go:FileIncludeReason.isReferencedFile`, `FileIncludeReason.computeReferenceFileRelatedInfo`, `referenceFileLocation.text`, `program.go:Program.GetSourceFileByPath` | Existing include-reason tests compare complete absolute/relative diagnostics and related information. The four reference forms produce related codes 1399, 1401, 1404 and 1406 at the source ranges. The source-file lookup supplies the observed text/range; it is not inferred from a similarly named checker-host method. |
| Same witness; `synthetic-helper`, `synthetic-jsx`, `invalid-trivia-bytes` | Same four include operations | Synthetic imports retain quoted names but produce no related source location. A real import after invalid-byte comment trivia retains exactly `"./dep"`. Repeated calls compare the cached results and stable identities. |

`singleton-import` also observes a non-reference root and empty/missing include
queries. This witnesses the non-reference classification branch; it does not
claim an invalid-path test for `GetSourceFileByPath`. Rust resolves and retains
reference text while constructing `ReferenceLocation`, and inlines Go's small
`computeReferenceFileRelatedInfo` dispatch inside its existing related-info
method. The annotations and witness describe those exact equivalences.

## Boundaries left pending

- Rust's task scheduling, host construction and postorder collector are
  organized differently from Go's `filesParser`/`parseTask`. Matching a whole
  loader graph does not establish each private scheduler operation. Their
  identities are not assigned blanket corpus credit.
- The `Program` trait methods in `checker_host.rs` are not credited simply
  because the loader probe reads corresponding fields through its own methods.
  These distinct entry points need their own observation or a reviewed phase
  destination.
- `moduleResolutionSupportsPackageJsonExportsAndImports` remains pending.
  Existing `options/37/{command,syntax}` observations report only removed-option
  TS5108, so they do not discriminate the package-map helper's outcome. No new
  witness is fabricated from the options' names.
- Getter casts for Go's tagged include-reason data, compiler-host tracing,
  checker, emitter, project-reference execution and watcher behavior receive no
  new attribution in this pass.

## Validation

- Four finalized native rows captured through the new access-only adapter; its
  manifest authenticates requests, both adapter files, pinned compiler sources
  and output bytes. Request and adapter changes during capture fail the run.
- `cargo test -p tsr_compiler --lib tests::actual_go_loader -- --nocapture`:
  two tests pass, covering all 48 existing rows and the four supplementary rows.
  The final native output equals the exact observation bytes used by that run.
- The new Rust test and native `--check` command are listed in
  `data/s07/program-helper-tests.json`, so subsequent program producers execute
  them. Their paths already belong to the producer's source closure.
- Rust formatting was applied only to the changed compiler/helper files. No
  corpus, benchmark, broad test suite or historical-evidence rewrite was run.

The initial unrestricted native command could not use the external Go cache,
and automatic review rejected its escalation because it would overwrite the
original frozen capture. The replacement was explicitly authorized as a separate
four-row output under `target/`; only its new supplementary artifact was
installed. An initial transitional Rust invocation encountered a 52-versus-48
request count while the supplement was being separated; the final original
request inventory is unchanged and both complete comparisons pass.
