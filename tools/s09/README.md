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
