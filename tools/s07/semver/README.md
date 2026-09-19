# Pinned semver observations

`python3 scripts/s07_semver.py` checks fresh observations from the pinned Go
`version.go` and `version_range.go`, exported with only their module metadata and
package sources. `--write` explicitly replaces reviewed evidence. The producer
also executes the original package tests and verifies the pinned Go toolchain.

Requests are every string literal in both upstream test files, with original
line origins, plus the reviewed `supplemental.json` byte cases. No Rust result
selects requests. The checked-in request and observation files preserve exact
input bytes, partial Version fields after errors, error classes/messages,
canonical formatting, nil ordering, and complete comparison/range-test matrices.
The manifest fingerprints source, producer inputs, requests and observations.

`cargo test -p tsr_semver` compares the public Rust API with these observations.
This establishes the helper operations covered by the frozen inputs; it does not
certify module-resolution behavior or the full compiler.
