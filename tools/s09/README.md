# Request printing observations

`printing_test.go` is an access-only overlay in the pinned upstream
`internal/printer` external test package. It constructs a small frozen syntax inventory,
calls `EncodeNode`, records those exact protocol bytes, then calls `DecodeNodes`
and `Printer.Emit` with no source file. Malformed inputs use the frozen bytes
directly. It records native Go outcomes, including native output for explicitly
unsupported Rust requests.

Run `python3 scripts/s09_printing.py --output <new-directory> --freeze` to propose
new observations. The new directory keeps the exact request, overlay, native
output, provenance and test stdout. The committed observations bind the request,
overlay source, capture script, overlay helper, upstream pin and local toolchain.
`s09_printing.verify_frozen(root)` checks that closure and the exact row inventory,
options and failure classes without invoking Go. Ownership evidence calls that
offline validator before measuring the Rust requests, so editing the frozen
request tree without recapturing its native result fails preparation.

The API session's print handler delegates to the same decoder and printer. This
overlay exercises those production functions without creating an API session or
using base64 transport. Only the three `PrintNodeParams` flags are accepted;
newlines use the native handler's default. Upstream `EncodeNode(nil, nil)`
dereferences the root, so the nil-root case instead changes an encoded
identifier's root kind to the node-list sentinel. The decoder produces a nil
root and the printer's nil-pointer panic is recorded. Another explicit mutation
changes a type-literal root to `SyntheticExpression` and its data to the
child-data tag, and records that decoder's contract panic. Its existing children
are decoded first, ensuring the failing request has populated scratch storage.
Both recipes
start from real native encoded bytes and are frozen with the other requests.

The nil-root capture corrected an earlier source-only assumption that printing
a decoded nil root returned empty text. The native call panics. Observations
keep that exact payload and the explicit `nil-root` class, separately from
`synthetic-expression`; Rust tests must compare their corresponding specific
contract failures rather than accepting any panic.

These observations cover printing, including named unsupported Rust syntax and
options. They do not cover insertion formatting, session scratch disposal,
generation retirement, or a complete S09-3 result. Rust ownership tests must
exercise request lifetime separately. No upstream source is modified.

# Formatter observations

`format_oracle/main.go` is step F0 of the [formatter plan](../../docs/S09-3-formatter-plan.md).
It is a persistent process, built as a `main` package inside a fresh
`git archive` export of the pinned tree, because it imports internal packages.
No upstream source is modified and the submodule is never touched. It reads one
JSON request per line and answers one JSON observation per line.

The denominator is the frozen S06 parser inventory, reduced to distinct parser
inputs: the same source, file name, script kind and module-indicator options
parse to the same tree, so formatting them twice proves nothing. Parse parity on
these inputs is E1's, which is why they are reused rather than invented.

Operations, with the call shapes the formatter itself uses:

- `nav`: at up to 512 evenly spaced byte offsets plus the end of text,
  `GetTokenAtPosition`, `FindPrecedingToken`, `FindPrecedingTokenEx` excluding
  JSDoc, `GetStartOfNode` with and without JSDoc, `FindChildOfKind` for the six
  bracket kinds, and `FindNextToken` under the token's parent and under the file.
- `indent`: `GetIndentation` at every line start under both assumptions, and at
  every eighth navigation offset, under the `default`, `tabs` and `two` settings.
- `format`: the `FormatDocument` edit list under `default`, `tabs`, `two`,
  `dense` and `terse`. The last two flip the rule options away from their
  defaults, including semicolon insertion and removal.

Each stream is published as a row count, a count of native failure rows and the
SHA-256 of its rows. The row grammar is line based, not JSON, because the Rust
side reproduces it byte for byte: `T|<offset>|<kind>,<pos>,<end>`, `-` for no
node, `E|<pos>|<end>|<hex of new text>` for an edit. A native panic inside one
call becomes that row's value, prefixed with `!`, so one failing position does
not hide the rest of the file and the port is held to the failure too.

Two native facts the observations record rather than hide. The pinned navigation
asserts on one input (`taggedTemplatesWithTypeArguments2.ts`). And under
semicolon removal the formatter emits overlapping edits on about 1,500 inputs,
which `ApplyBulkEdits` cannot apply; the edit list is the observation, and the
failure to apply it is recorded beside it as `text_panic`.

```sh
python3 scripts/s09_format.py observe --output <new directory>            # whole inventory, about 4 minutes
python3 scripts/s09_format.py observe --output <dir> --prefix compiler/a  # a diagnostic subset
python3 scripts/s09_format.py detail  --output <dir> --id <request id>    # literal rows of one input
python3 scripts/s09_format.py freeze  --output <new directory>            # rewrites data/s09/format-probes.json
python3 scripts/s09_format.py verify  --output <new directory>            # reproduces the frozen file or fails
```

`data/s09/format-probes.json` binds the pin, the local Go toolchain, the oracle
and script hashes, the inventory digest, the operation and variant lists, and
the native totals with the digest of the whole observation stream. `verify`
repeats the complete observation and requires the frozen file byte for byte; a
second complete run reproduced it. The corpus comparison is a live differential, as
E1's is: Go and Rust answer the same requests and are compared request by
request, so no per-file observation is committed.
