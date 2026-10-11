# tsr_vfs

Virtual filesystem and host contracts for tsr.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).

The default-off `harness` feature exposes the Go test-mock recorder, probe
counters and retained walk callbacks for differential tests. Production hosts
use the default borrowed `FileSystem::walk_dir` contract.

`MemoryBuilder::insert_unavailable_file(path, raw_size)` records a known regular
file without its content. File existence, stat size, directory enumeration and
symlink resolution remain available. Reading that file returns a structured,
path-bearing error saying its content is unavailable; it is distinct from a
missing file or a readable zero-byte file. Call `insert_physical` to replace the
placeholder with raw bytes before `finish`. A frozen snapshot never reads from
an external host to fill missing content.
