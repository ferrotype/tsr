# Test-only public embedding wasm consumer

Owner decision 2026-10-11 permits this independent acceptance crate for
`compiler-wasm`. Compilation enters public `tsr_embed`; the same executor,
codecs, DTOs, mapper callbacks and pinned walker files are included by path in
the native and wasm clients. Test schedules remain outside production crates.
This artifact is excluded from production wasm size and performance budgets.
Browser and single-file transpile acceptance continue using `tsr_wasm`.

Build with the repository-pinned wasm-bindgen 0.2.126 CLI and Rust toolchain:

```sh
python3 tools/phase7/portable/wasm-compiler/build.py \
  --wasm-bindgen /absolute/path/to/wasm-bindgen \
  --output /absolute/persistent/new-compiler-artifact-directory
```

Output must be new and outside every `target` ancestor. Cargo intermediates
use the repository target directory, while the wasm, generated binding factory
and build/hash manifest plus the exact Node transport/shim are retained directly
in the requested output. The
build rejects source changes during compilation. No tool downloads occur.

Set `TSR_WASM_COMPILER_ARTIFACT_DIR` to that directory, then pass
`tools/phase7/portable/wasm-consumer` as the explicit `compiler-wasm` consumer.
The host starts it with `--compiler`. The Node transport reads raw NDJSON
synchronously, including callback replies during suspended Rust operations.
Owners remain in wasm until Release; failure frames and traps terminate the
child. A new variant creates a new process/instance, with no native fallback.
`TSR_WASM_ARTIFACT_DIR` selects the separate production transpile artifact.

Focused transport checks:

```sh
node --test tools/phase7/portable/test-compiler-wasm.mjs
CARGO_TARGET_DIR="$PWD/target" cargo test --locked \
  --manifest-path tools/phase7/portable/wasm-compiler/Cargo.toml --lib
```
