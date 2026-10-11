# tsr_embed

The Rust embedding entry point for [tsr](https://github.com/ferrotype/tsr),
a Rust port of the TypeScript compiler. It provides parsing, owned program
sessions, diagnostics, scoped type queries and single-file transpilation
without requiring a compiler process.

Requires **Rust 1.96 or newer**. The project is under development; its APIs and
supported compiler behavior are not stable. See the repository's status and
sprint records for current coverage. The `tsr_*` crates are released together at
one version.

## Check an in-memory file

The default `checker` feature enables `Session`. The host explicitly supplies
file contents; this example reports TS2322 for assigning a number to a string.
It uses `no_lib` to keep the example self-contained. For standard library types,
compose the host with `tsr_bundled::BundledFs`.

```toml
[dependencies]
tsr_embed = "0.4.0"
tsr_arena = "0.4.0"
tsr_core = "0.4.0"
tsr_jsstring = "0.4.0"
tsr_tsoptions = "0.4.0"
tsr_vfs = "0.4.0"
```

```rust
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_core::{CompilerOptions, Tristate};
use tsr_embed::{FileCache, ProgramOptions, Session};
use tsr_jsstring::JsString;

fn main() {
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/main.ts", b"const value: string = 1;".as_slice());
    let options = ProgramOptions {
        config: tsr_tsoptions::ParsedCommandLine::new(
            CompilerOptions {
                strict: Tristate::TRUE,
                no_lib: Tristate::TRUE,
                ..Default::default()
            },
            vec![JsString::from_bytes(b"/main.ts".as_slice())],
        ),
        host: Arc::new(host.finish()),
        current_directory: JsString::from_bytes(b"/".as_slice()),
        default_library_path: JsString::from_bytes(b"/lib".as_slice()),
        skip_module_resolution: false,
        single_threaded: Tristate::UNKNOWN,
    };
    let mut cache = FileCache::new();
    let counters = Counters::new();
    let session = Session::load(options, &mut cache, &counters).unwrap();
    let file = session.program().file(b"/main.ts").expect("loaded file");
    let mut operation = session.operation().unwrap();
    let diagnostics = session.program()
        .semantic_diagnostics_with_checker(&mut operation, file)
        .unwrap();
    assert_eq!(diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(), [2322]);
}
```

A query borrows its operation scope. Use `retain_type`, `retain_symbol` or
`retain_signature` for a result that must survive that scope; it then keeps its
program alive. `retire()` prevents subsequent queries, including through retained
results. See the [embedding guide](https://github.com/ferrotype/tsr/blob/main/docs/S10.md)
for lifetime and host details.

## Emit in memory

`Session::emit` runs the compiler's emit with an in-memory write callback and
returns every written file's name and bytes, in the compiler's emitted-files
order, with the skip flag and the emit diagnostics. Nothing is written through
the host. `EmitOptions` selects target files of the session's program, the
outputs (`EmitOnly`) and forced emit. This entry point carries no acceptance
claim yet.

`writes` preserves the actual write-callback order and each callback's
`EmitWriteData` (source identity, source-map URL position, diagnostics and
build-info metadata). `source_maps` preserves the emitter's records, including
inline maps and their input-source lookup names. `files` keeps its existing
emitted-files order. Each callback refers to that array by `emitted_file_index`;
`written_file` resolves a record originating from that output without copying
bytes; listed records resolve by index without a foreign-record ownership check.
A callback the compiler
does not list keeps its bytes in `unlisted_file()`. The output's
`program()` retains the source owners needed by diagnostics and write metadata
after the session is dropped; release the output to release that retention.
`emit_with_request` accepts a `SessionRequest` and returns `None` when the
compiler's emit cancellation gate stops the request.

The retained program makes `EmitOutput` construction private: code that
previously constructed a struct literal should receive it from an emit operation
or start with `EmitOutput::default()` and assign fields. Destructuring should use
`..` for the private retention field. A default output has no retained program.

```rust,ignore
let output = session.emit(&tsr_embed::EmitOptions::default()).unwrap();
for file in &output.files {
    println!("{}: {} bytes", String::from_utf8_lossy(file.name.as_bytes()), file.text.len());
}
```

`Session::diagnostics` selects a `DiagnosticKind` and uses the same compiler
checker pool as emission. It accepts a `SessionRequest` and an optional target
file, and applies the production phase's filtering, sorting and cancellation
rules. Its returned diagnostic identities require the session's program to
remain alive. `with_compiler_operations` holds that pool's operations together
for a callback; `for_file` chooses the compiler checker associated with a
session file, and `iter` visits every checker in pool order. Successive type and
symbol queries in that callback therefore observe the state used by emission.
The independent `operation()` API retains its separate interactive checker.
Reentry through the session is rejected before waiting, retirement invalidates
both pools, and a panic in a compiler callback retires the session.

## Configured incremental and mapped programs

`Session::load` now honors `incremental: true`: diagnostics and emission use the
production incremental program, read previous build info through the supplied
filesystem and may emit `.tsbuildinfo`, including with `noEmit`. The in-memory
writer captures build info alongside other outputs; it does not persist it.
Supply those output bytes in the host of a later load to reuse them. The
production reader validates compiler version and content-mapper identities.
There is no test-specific version normalization in the facade.

`Session::load_with_services` additionally takes `SessionServices`, with an
optional caller-owned content-mapper project and borrowed `BuildInfoReader`.
The reader is used only during incremental construction and is not retained.
`BuildInfo` is the public value carried by build-info write metadata. The
`build_info_reader(program)` helper supplies the production filesystem reader
for callers that need to decorate it. `from_program` remains plain compatible
adoption; `from_program_with_build_info_reader` is the explicit fallible
adoption method that honors incremental configuration.

The `content_mapper` module exposes production `new_host` and the minimum
host, project, spawner and transform values needed to supply runtime services.
Create a host with a runtime-specific `Spawner`, a `Context` and a `Locale`,
open its `ProjectSpec`, and pass the resulting project in `SessionServices`.
Program construction uses that project for canonical and supplemental virtual
sources. The caller owns project and host shutdown and can share both across
loads; keep them open while any subsequent load or incremental operation needs
them. No test mapper or process implementation is selected by the facade.

`BundledFs::with_libraries` supplies an explicit ordered snapshot of already
loaded library bytes through the production bundled filesystem policy. It uses
exact, case-sensitive bare names under `bundled:///libs`, preserves loaded BOM
bytes, and never substitutes built-in or inner-host files for missing entries.
An empty snapshot has no library files. Immutable inner hosts receive a fresh
overlay cache identity; live inner hosts remain uncached.

## Transpile a single file

The default `checker` feature also exposes `transpile_module` and
`transpile_declaration`. Both call the production single-file transpiler with
its own in-memory host; supply decoded input bytes, a `TranspileRequest` and
`TranspileOptions`. No filesystem callbacks are needed. Compiler options are
copied and overridden as required by the pinned transpile API: for example,
`noEmit` does not suppress transpilation, and module transpilation disables
declaration output. Declaration transpilation enables isolated declarations
and loads the pin's minimal library. These operations can produce different
output and diagnostics from `Session::emit` over a complete program.

```rust
use tsr_embed::{
    TranspileOptions, TranspileRequest, transpile_declaration, transpile_module,
};

let request = TranspileRequest::default();
let options = TranspileOptions::default();
let input = b"export const x: number = 1;\n";
let module = transpile_module(&request, input, &options).unwrap().unwrap();
assert_eq!(module.output_text.as_bytes(), b"export const x = 1;\n");
let declaration = transpile_declaration(&request, input, &options).unwrap().unwrap();
assert_eq!(declaration.output_text.as_bytes(), b"export declare const x: number;\n");
```

`TranspileOptions` contains optional borrowed `CompilerOptions`, a filename
(empty selects `module.ts`, or `module.tsx` when JSX is enabled), and
`report_diagnostics`. The latter adds syntactic and option diagnostics; emit
diagnostics are always returned. `TranspileOutput` owns `output_text`,
`source_map_text` (empty when absent), `diagnostics` and a retained `program`.
Input and options can be dropped immediately after the call. Keep the program
alongside diagnostics whose file identities you need to resolve; copying a
diagnostic alone does not retain its source file.

Both functions return `Result<Option<TranspileOutput>, TranspileError>`.
An already canceled request returns `Ok(None)` from module transpilation.
Declaration transpilation forces emit and skips the pin's cancellation gate,
so the same request still produces declarations. The public types alias the
production request, options, output and compiler error types; compilation calls
can remain entirely under `tsr_embed`.

## Parser only

Use `tsr_embed = { version = "0.4.0", default-features = false }` to omit the
checker. `parse` returns an immutable syntax tree; `parse_and_encode` returns
owned protocol-8 bytes. Both take loaded `SourceText`, a `ScriptKind` and explicit
`SourceFileParseOptions`. Physical-file BOM and encoding conversion belong to
the host. This feature selection also omits program sessions and transpilation.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
