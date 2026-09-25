# F5b filesystem contract audit

Audited against upstream `1f70213d4922b434345f639b441681e470c7cfc1`
and the frozen Go 1.27.1 observations. The owner approved retaining the closed
typed APIs for these two foreign-input differences on 2026-09-22. Neither is a
missing glob or filesystem algorithm.

## Exact differences

| Frozen case | Action (zero-based) | Native observation | Rust observation |
| --- | ---: | --- | --- |
| `filesystem/glob/match-group-branch-buffer` | 28, `match_elems("x")` after a foreign element is installed | `result: null`, `panic: "unimplemented_segment_type"` | `result: null`, `panic: "unrepresentable_element"` |
| `filesystem/vfstest/from-map-rejects-malformed-maps` | 4, `from_map_guarded` with an integer file value | `["guarded", "pinned:invalid file type"]` | `["guarded", "unrepresentable_input"]` |

The first trace has 42 actions, the second 10. Their other 50 actions agree,
including group branch reuse, nil/empty groups, separator bounds panics,
non-rooted and non-normalized paths, mixed path styles, invalid symlink targets,
and subsequent valid filesystem construction. The driver regression
`foreign_go_values_remain_explicit_without_hiding_representable_actions`
replays both full requests and checks each action against the frozen native
observation. It requires the two unequal actions to remain unequal and does not
normalize the comparison. `data/phase1/approved-differences.json` records the
owner qualification separately, bound to both complete observations.

## Why these operands cannot reach Rust production code

Pinned `internal/glob/glob.go:184` declares `element` as `fmt.Stringer`.
Its parser creates only the seven concrete element types listed there. The
default panic at line 327 can only be reached by injecting another dynamic
type; the native probe declares its own `phase1Foreign` solely for that purpose.
The Rust `tsr_glob::Element` enum has exactly the seven supported variants.
It cannot hold that eighth probe-local type. All real group and matcher paths
in this request still execute the production matcher.

Pinned `internal/vfs/vfstest/vfstest.go:70` accepts `map[string]File` with
`File any`. Its documented inputs are strings, byte slices and map files;
`FromMapWithClock` switches over those three kinds and panics on any other
dynamic value at line 119. The probe supplies integer `1234`. Rust's
`tsr_vfs::vfstest::InputFile` represents the three documented kinds as `Text`,
`Bytes` and `File`; an integer is not an input value. Path validation still
executes in production and retains the native errors for malformed paths.

These are typed input boundaries, not the previously approved mutable-alias
differences. The compiler-options, retained-snapshot and package-`Parseable`
approvals do not cover either case.

## Decision

Retain the closed typed APIs. Approval covers only the two exact foreign-operand
differences above, bound to their current pin, request and complete native/Rust
observation pairs. Both raw rows stay `different`; the native observations and
every representable action stay in the acceptance schedule. A changed panic,
return value or any other action must remain a failure.

The alternative is a real dynamic input API with rejection of arbitrary caller
types, which broadens production APIs solely for defensive Go branches. Adding
a manufactured `Foreign` production variant, or making the driver claim that
production raised Go's panic, would provide no such implementation and is not a
valid fix.

The Linux-only `filesystem/osvfs/nativepath-realpath-linux-procfs` observation
is separate. It still needs an applicable Linux run and is not covered by this
decision.

## Unix retry corrections

The audit also found a real Linux port gap. The pinned `Realpath` passes both
`unix.Open` and `unix.Readlink` through `ignoringEINTR`; Rust retried `openat`
but called `std::fs::read_link` once. Rust 1.97.1's Unix implementation uses
`cvt` for `readlink`, so it does not supply the missing retry implicitly.

Both calls now use one private `ignoring_eintr` helper. Its deterministic test
checks repeated raw EINTR followed by success, a different raw errno returned
on the first call, and a wrapped EINTR returned without retry. The wrapper case
preserves the pinned helper's deliberate raw-error comparison.

The helper is not Linux-only. On every Unix target it wraps each syscall that
Go 1.27.1's `os` package retries and Rust 1.97.1's std does not: `stat` and
`lstat` (`native::metadata`, `native::symlink_metadata`), `readlink`, the
`openat` of the Linux `Realpath`, `unlink` and `rmdir` on the `RemoveAll` fast
path (the descriptor walk uses rustix's `retry_on_intr`), every `mkdir` of
`ensureDirectoryExists`, and the directory open of `DirFS.ReadDir`. Calls whose
std implementation already retries (`open` through `cvt_r`, and `read`/`write`
through the `Interrupted` loops of `read_to_end` and `write_all`) are left alone.

Two of those were added after the review (I30). `os.MkdirAll` makes one level
at a time and `os.Mkdir` retries each `mkdir(2)`, while std's `create_dir_all`
calls `cvt` once per level; `ensure_directory` now runs `native::mkdir_all`, a
transcription of the Go recursion with the stat fast path and the lstat
double-check. As in Go, a failure is reported for the level that failed, so
`mkdir <root>/afile: not a directory` for a request under the regular file
`afile`, not for the requested path (checked against a native Go probe). The
open in `os.ReadDir` (`openDirNolog`) retries `open(2)`, while std's `read_dir`
calls `opendir(3)` once; `Dir::read_dir` now opens through `native::read_dir`.
The unit tests inject a raw EINTR into the first `mkdir` of every level and
into the first directory open, and check that the tree is created, the call
sequence, and that another errno is returned once for the level that failed.
Two more go through the production entry points: `OsFs::ensure_directory` and
`write_file` under the regular file `afile` must report
`mkdir <root>/afile: not a directory` (the pinned tsgo `osvfs.FS().WriteFile`
gives that for `afile/x.txt`, `afile/sub/x.txt` and `afile/sub/deeper/x.txt`),
and a test-only hook interrupts the raw `mkdir` and directory open under
`ensure_directory` and `entries`, so a call site that bypasses the retrying
helpers fails. The family harness cannot see either: it records a PathError's
op and errno but not its path. They are source-derived contract witnesses: a
native EINTR comparison is not possible. The Windows build keeps `create_dir_all`, where the helper is the
identity.
