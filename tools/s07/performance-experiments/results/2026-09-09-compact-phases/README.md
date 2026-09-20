# CP1 versus CP4: matched elapsed phases

The same existing phase adapter, compiled against each frozen production source,
ran the consuming path at one worker with normal mimalloc, Rust 1.97.1 and
`ts_ast/layout-profile`. One warmup and three alternating measured runs per
revision completed; all eight children match the full workload/digest and bind
all 13,094 files in place with zero fallbacks.

| Median | CP1 | CP4 | Difference |
| --- | ---: | ---: | ---: |
| Parse | 2.568171 s | 3.509063 s | +0.940892 s |
| Binding and publication | 2.137196 s | 2.971233 s | +0.834037 s |
| Diagnostic pipeline | 4.720425 s | 6.495556 s | +1.775131 s |

Both phases regress. This does not isolate inline binding fields from the other
changes. Per-file clocks include descheduling and timer overhead; binding includes
the entire publication/validation transition. These are attribution observations,
not acceptance statistics or comparable replacements for normal-binary screens.

The archive contains 862 verified files: both frozen phase builds and sources,
identical adapter code, helper snapshots, build/lock provenance and failures,
every raw observation, and supplementary CPU/memory audit notes. The native CPU
raw exports themselves are in the [CP4 archive](../2026-09-09-compact-binding/README.md).

- Archive bytes: 11,086,156.
- SHA-256: `e96dac4e602f1b40dfb0c84f0dc9bee4d4b5c81eb16d4ca4cd4306f5f69e0131`.
- All members were read back and verified against [archive.json](archive.json).
- CP1 build manifest: `5fcb73a45da7251760c44ff5c5b73bec96d8dda847797f578771097ab9347866`.
- CP4 build manifest: `0b4eeea8ddba565507b2f60b4165a5477003b0aa08598c5dd585f59b94dd57e0`.
- Capture manifest: `38ed43a40a8ef2dd8eaa61c64f428d3fd4f4815193980cba76c4162d4a872f6d`.

Replay needs the original workload paths, registry/toolchain prerequisites and
immutable permissions. No production sources or existing phase tools were edited
for this diagnostic. See the [implementation record](../../../../../docs/S07-bis-compact-storage.md)
for the resulting decision and unchanged final gates.
