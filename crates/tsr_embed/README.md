# tsr_embed

The Rust embedding entry point for [tsr](https://github.com/ferrotype/tsr),
a Rust port of the TypeScript compiler. It provides parsing, owned program
sessions, diagnostics and scoped type queries without requiring a compiler process.

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
tsr_embed = "0.3.0"
tsr_arena = "0.3.0"
tsr_core = "0.3.0"
tsr_jsstring = "0.3.0"
tsr_tsoptions = "0.3.0"
tsr_vfs = "0.3.0"
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

```rust,ignore
let output = session.emit(&tsr_embed::EmitOptions::default()).unwrap();
for file in &output.files {
    println!("{}: {} bytes", String::from_utf8_lossy(file.name.as_bytes()), file.text.len());
}
```

## Parser only

Use `tsr_embed = { version = "0.3.0", default-features = false }` to omit the
checker. `parse` returns an immutable syntax tree; `parse_and_encode` returns
owned protocol-8 bytes. Both take loaded `SourceText`, a `ScriptKind` and explicit
`SourceFileParseOptions`. Physical-file BOM and encoding conversion belong to
the host.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
