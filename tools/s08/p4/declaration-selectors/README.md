# Native declaration diagnostic selectors

`observations.json` records 504 calls to the pinned declaration transformer's
actual private diagnostic selectors over `source.ts`: 63 source contexts, both
name/type context choices, and all four combinations of private/external module
name and accessibility. Each observation retains the diagnostic code and exact
error-node/type-name identities. `report.json` records the pin and input hashes.

`oracle_test.go` is an access-only Go test overlay under
`internal/transformers/declarations/s08_selectors_test.go`. Set
`S08_SELECTOR_SOURCE` to `source.ts` and `S08_SELECTOR_OUTPUT` to a fresh output;
run the command recorded in `report.json` with a local overlay mapping to the test
file and the project's pinned Go environment. The Rust test
`selector_codes_and_locations_match_native_declaration_contexts` re-parses the
same source and compares every observation.

This fixture verifies diagnostic selection and locations. It does not certify
that the declaration transformer or checker emitted every required event, nor
substitute for the full declaration-phase program comparisons.
