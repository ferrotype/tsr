# C0 harness regression fixtures

`corpus.json.gz` contains seven real rows from the initial C0 native and Rust
captures: five S08 controls, a production panic, and a content-mapper refusal.
Each row retains its request, native observation, original Rust completion
record, and exact raw stdout/stderr/observation bytes (hex encoded). The fixture
records the upstream pin, native observation digest, Rust capture digest and
executable digest. `phase2_fixtures.load()` checks the bundle digest, the original
request digests and every raw artifact digest.

The bundle is 104 KB compressed. No compiler executable, full corpus or native
checkout is required to replay these rows or recompute their comparisons. To
exercise capture validation, tests bind copies of the original completion rows
to a temporary miniature capture with inert executable bytes and a fixture-only
source snapshot. These are **test inputs, not acceptance evidence**; they cannot
pass the producer's real build-source check. The original completion metadata
remains in the committed bundle for provenance.

Do not replace these fixtures to make a failing mutation test pass. New compiler
results belong in separately captured evidence. The current comparator is run
against the native/Rust observations on every test run; stored comparison
verdicts are not used as assertions.
