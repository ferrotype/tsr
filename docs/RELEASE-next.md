# Next Rust release (unreleased)

The 0.2.0 packages are already published. [RELEASE-0.2.0.md](RELEASE-0.2.0.md)
is their historical record; new crates and facade exports must not be added to
that release's package count or publish order.

Phase 5 adds `tsr_lsproto` and `tsr_lsp`; the facade exports the protocol
library. These are
unreleased changes for the next lockstep release, whose version is not yet
selected. The development manifests still say 0.2.0; that does not identify
this working tree with the immutable published 0.2.0 archives. In particular,
the modified facade cannot be published again as `tsr` 0.2.0.

Before preparing the next registry upload:

1. Choose a version greater than 0.2.0 and bump every public package and its
   sibling dependency requirements together. Refresh workspace and standalone
   consumer lockfiles as part of that release change.
2. Verify registry availability and ownership for new package names. Include
   the Phase 5 crates present in the final source tree; `tsr_lsproto` belongs
   after `tsr_jsonrpc` and before its consumers and the facade. Use the current
   [package policy and dependency order](../tools/packaging/README.md), not the
   historical 0.2.0 list.
3. Run package asset and archive verification on the final versioned source,
   then write the new release's package count and publish instructions from
   that result.

This record authorizes no registry publication or package-name reservation.
