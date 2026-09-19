# Named S08 P2 native fixtures

`requests.json` fixes six real programs, 29 type queries and four merge modes.
Options explicitly set `strict: true` and `noLib: true`; the original checker
applies the effective strict defaults. No library declarations or missing-global
diagnostic filters are injected.

```sh
python3 scripts/s08_p2.py capture --output target/s08/p2-native-programs
cargo run -p ts_compiler --example p2_checker -- target/s08/p2-native-programs/requests.json target/s08/p2-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p2-native-programs --actual target/s08/p2-rust.json --output target/s08/p2-comparison.json
```

The native overlay adds a Go test driver and a read-only existing-name-type bridge. Programs use the original compiler
host, parser, binder and checker. The sharing host caches the original parsed
SourceFile object, so independently constructed programs share the same bound
base file. It does not replace type checking, merging, display or diagnostics.

- `single-checker`: one checker merges base + A + B.
- `repeated`: the same checker queries base/A/B, then B/A/base; merged symbol and
  type identities must be reused.
- `separate-checker`: independently retained A and B programs share base; query
  A/B, reverse B/A, release A and query B again.
- `concurrent`: start both independent checker constructions and queries from a
  two-worker barrier, then perform the same reverse/release sequence.

Native symbol identity is normalized by original bound-file traversal and by
checker-local discovery. Raw addresses and global numeric ids are not portable
expectations. Symbol graph snapshots retain declaration order, value declarations,
flags, member/export tables (including nil versus empty), parents and export
symbols. Private symbol names replace only their process-local class number with
`class:<hex file>:<kind>:<pos>:<end>`, retaining the `@#name` suffix. Both symbol
names and table keys use this representation; tables sort by the normalized
bytes. The declaring class comes from the value declaration (or first declared
node), so separate and nested classes retain distinct private identities. A
private key without that provenance is a harness error. Unique-symbol property
names similarly replace only their numeric suffix with the unique symbol's
`symbol:<hex file>:<kind>:<pos>:<end>` declaration identity. That identity comes
from the existing `nameType` link, via a read-only overlay bridge and the Rust
operation's validated identity read; snapshots never resolve types or create
links. Well-known names without a numeric suffix retain their bytes. Missing or
malformed unique-name provenance is a harness error; distinct namespace keys
remain distinct even when both spell `key`. Separate checks observe
pointer identity, shared-file identity, repeated result identity and the
before/after bound-file snapshot.

Every query retains native default and InTypeAlias display bytes and properties.
Both display calls use no enclosing declaration. Rust supplies the two default
flags used by Go's `TypeToString`; alias expansion passes only `InTypeAlias` on
both sides. This fixture does not cover contextual name qualification or source
annotation reuse. The `annotation_name` selector queries a reference's name
separately from its full annotation, preserving the pinned error-type result.
The additional programs cover readonly/const grammar short-circuiting, hex and
quoted display spellings, and contextual object initialization.
Diagnostics retain all structured fields and config/program/syntactic/bind/
semantic/global phases, their sorted combined payload, and original error-baseline
bytes. Source-language errors are observations; `unsupported` is a distinct
operation state with a required operation name and reason. The Rust endpoint
reports it separately for a program, query, diagnostic phase, baseline or merge
mode. Incomplete semantic checking keeps completed phases, including missing
global diagnostics, and marks combined diagnostics and the baseline unavailable.
Any unsupported state prevents a matched comparison.

The Rust adapter calls production `Program` and `CheckerOwner` operations and
uses a shared `FileCache` for all merge modes. Its plain ASCII error-baseline
decorator ports the upstream harness; unported related-information, library,
config, content-mapper, duplicate-file and non-ASCII branches are named
unsupported outcomes. It never inserts frozen diagnostic or display bytes.
The separate `rust_ownership` envelope checks that a retained type keeps its
Program alive after caller handles are released, remains queryable, and releases
the Program when dropped. These checks have no native ownership equivalence
claim; false or missing checks fail validation.

Captures remain under `target` with source snapshots, commands, logs, hashes and
runtime identity. This fixture does not change P0 archives or certify the full
S08 acceptance denominator.
