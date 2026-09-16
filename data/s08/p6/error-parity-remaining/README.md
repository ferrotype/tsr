# Native diagnostic regression witnesses

`requests.json` contains the 26 variants that still differed on errors in
`target/s08/p6-error-parity-tail-01`. Original loading inputs, options and
diagnostic phases are preserved. Type/symbol and public-display walks are
disabled for this diagnostic-only test.

`observations.json` contains their native `error_pre_diagnostics` arrays from
that authenticated capture, renamed to `diagnostics`. The Go pin, source
capture digest and file digests are recorded in `provenance.json`. Expected
diagnostics are measured Go output, not Rust snapshots. The test compares
codes, ranges, arguments, chains, related information and tags exactly.

```sh
cargo test -p ts_compiler --test checker_error_tail
```

Before the fixes, all 26 witnesses differed. After the fixes, all match. The
separate [449-case comparison](../../../../tools/s08/p6/error-parity-remaining/README.md)
also checks full error rendering and type/symbol/public-display parity against
the native observations, including 300 sampled error-matching controls.
