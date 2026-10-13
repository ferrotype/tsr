# Go consumer for production embedding budget verification

Status: shared production operation with native/wasm scenario correctness
comparisons. Build identities describe measurement contract 2; a successful
build alone does not establish a passing budget. [CONTRACT.md](CONTRACT.md)
records the agreed first-slice wire and lifetime contract and remaining gaps.

The consumer runs the pinned compiler through thin native and `GOOS=js`
front ends sharing one operation: parse configuration from an explicit memory
snapshot, load a fresh program, check with one checker, optionally emit into
memory, retain the program and results through an explicit observation
checkpoint, then release them. It does not call the command-line driver,
incremental/watch machinery, acceptance executor or baseline type walker.

The corresponding Rust authorities must be a native public `tsr_embed`
consumer and production `tsr_wasm::MemoryHost/WasmSession`. The test-only
compiler-wasm acceptance artifact is ineligible for these budgets.

## Pinned authority and source layout

The build must use `data/upstream.json` and the existing pinned Go environment,
export a verified upstream tree, and place the consumer beneath its `tsc`
module. This is necessary for Go's `internal` import boundary; editing the
upstream checkout or creating a replacement compiler module is unnecessary.
The build command is `python3 tools/phase7/budget/go/build.py --output DIR`.
`DIR` must be new and outside every `target` ancestor. Native and wasm
artifacts record the same shared consumer source and upstream revision. Both
are ordinary optimized Go builds with `-trimpath -ldflags="-s -w"`; the test-only
compiler acceptance executable is not used. The record binds the source guard,
unit-test/build success and explicit pre-operation/checkpoint GC policy.

The shared package calls these production APIs:

- `tsoptions.ParseCommandLine` for explicit argument overrides and
  `GetParsedCommandLineOfConfigFile` for the config, its dependencies, includes
  and project references, using only the snapshot host.
- `compiler.NewCompilerHost`, `compiler.NewProgram` and
  `ProgramOptions.SingleThreaded = core.TSTrue` for a fresh program and one
  checker. Verify the actual checker association rather than assuming this
  option alone proves work equivalence.
- The program's config, program, syntactic, global and per-file semantic
  diagnostic methods, plus per-file declarations when declaration/composite
  is enabled, followed by production sort/deduplication.
- `Program.Emit` with an in-memory `WriteFile` callback, retaining callback
  writes and the `EmitResult` in their original order.
- `GetSourceFiles`, `TypeCount` and owned diagnostic/output values for work
  observations. No extra type/symbol walk is permitted to manufacture work.

`compiler.GetDiagnosticsOfAnyProgram` conditionally skips later phases after
errors. It cannot replace the agreed schedule without establishing that the
Rust authorities perform the same work. Production wasm retained checking and
emit now use one compiler checker pool; focused tests verify identity.

The split is a shared operation package, a native NDJSON front end,
and a `syscall/js` front end. Byte arrays cross inside JSON integer arrays;
the shared operation does not import `syscall/js`. The JS front end currently
exposes one synchronous `phase7BudgetGo(jsonCommand)` function. Errors are
terminal; discard the instance after a failure.

## Verification before measurement

First test a tiny configuration with one source file and production bundled
libraries. Add focused controls for config `extends`/include, missing libraries,
symlink/case behavior, malformed config, and optional emit writes. An
unsupported host operation or input shape must fail explicitly. A missing
supplied physical file may produce the compiler's ordinary diagnostic, but
must never cause a read from disk. Builtin bundled libraries are deliberately
the production wrapper on both sides; explicit library transport is excluded
until a real scenario requires it.

Then compare the same frozen tiny input against public Rust consumers before
provisioning the five scenarios. Existing scenario obligations are a starting
point: their recorded checker counts and type counts cannot be copied into a
new one-checker authority as accepted expectations without recomputation.

Keep ordinary timing/size artifacts separate from allocator instrumentation.
Go `runtime.ReadMemStats` and a declared `runtime.GC`/`runtime.KeepAlive`
checkpoint can observe retention; they do not define requested allocation
traffic or process RSS. The eventual host reader must report these quantities
separately and reject acceptance-only artifacts.

Version 2 streams full metadata and content, then returns bounded observation
headers and source/write rows after checkpoint; see CONTRACT.md for exact
fields. The current Go build passes twelve focused body tests. Both native
and wasm use the same operation and explicitly collect the Go runtime after
successful preparation and at checkpoint. Node adapters collect V8 after
preparation and at checkpoint; execution acknowledgements include neither
collection nor observation serialization. Native process peaks use getrusage,
and Node outer metrics label their own authority separately from Go MemStats.
Actual depth probes and Xstate passes establish bounded correctness, not
accepted timing, size or memory budgets.
