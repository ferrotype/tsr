# S12 evidence

Owner acceptance: **2026-09-20**, [ADR 0020](adr/0020-phase-0-gate-decision.md).
These captures were recorded on **2026-09-19**, the day before closure. The owner
confirmed they remain applicable after the crate rename. Their age is not a
problem: the tracker fingerprint changed with the crate names. S12 reuses the
measurements and retains their original provenance.

## Immutable records

Each linked filename is the complete SHA-256 of the record bytes. Every record
has a successful exit, `valid_capture: true`, matching stdout/stderr hashes and
the same upstream pin `1f70213d4922b434345f639b441681e470c7cfc1`.
Original source/input hashes, environment, host and toolchain remain inside the
records; no record or `.latest` pointer was rewritten for this acceptance.
Dates below are the original recording times, in UTC.

| Producer | Record SHA-256 (link to original) | Recorded revision | Recorded at (UTC) | Frozen cases passed |
| --- | --- | --- | --- | --- |
| binder | [`14b49da69e6c`](../status/evidence/14b49da69e6c1cc5b0a9a9778ffd14dbf7c6c606d33371c6a91267b74f4aa38f.json) | `098941fd9e7a` | 2026-09-19T15:47:35+00:00 | 12,829/12,829 |
| bindworkload | [`85a54b34aa18`](../status/evidence/85a54b34aa18160150c0b7a723fb88013bb297dc05cf53d4fb665400d4ce15d9.json) | `b5aca216ab99` | 2026-09-19T00:55:03+00:00 | — |
| checkerbench | [`5c877c889f36`](../status/evidence/5c877c889f3630fa74103cc142b2b07234e3bb68d28fbc875789b74e9d507c97.json) | `b5aca216ab99` | 2026-09-19T04:03:06+00:00 | — |
| checkertext | [`e19d93b1b2e7`](../status/evidence/e19d93b1b2e77cd0a2bda635f9e50aecd53ca78e5e4a4c27d45adc3e2ed2dc70.json) | `098941fd9e7a` | 2026-09-19T15:04:26+00:00 | 4/4 |
| clippy | [`2d09f2332746`](../status/evidence/2d09f23327466d969f3802fe696309740929a8d87f3cb4d2186d88f87cb1ba70.json) | `098941fd9e7a` | 2026-09-19T14:03:09+00:00 | — |
| deny | [`16dfddf0b840`](../status/evidence/16dfddf0b840ad0d1a124a290ff5598d6009f125cf62d7ae1ad3b6a5c7fc753c.json) | `098941fd9e7a` | 2026-09-19T14:03:14+00:00 | — |
| e1 | [`79c4514b8421`](../status/evidence/79c4514b84217208c97c6f02053dd215f7c941dd1a139d25c362bdf4334b2ea2.json) | `098941fd9e7a` | 2026-09-19T15:22:16+00:00 | 12,829/12,829 |
| e2 | [`b5b93a54faa5`](../status/evidence/b5b93a54faa5b610971750c4a6278925f04e94b3f58c5c1a971925f43d5a04f1.json) | `b5aca216ab99` | 2026-09-19T02:04:47+00:00 | — |
| e3 | [`e360fcedd982`](../status/evidence/e360fcedd982cc66cf6b3e9bbd5eefc299699b4fe1cc50d064741d4f1ead151d.json) | `098941fd9e7a` | 2026-09-19T15:03:16+00:00 | 7/7 |
| e4 | [`e725f3e08b00`](../status/evidence/e725f3e08b00343292e002e98e2f1b1bf62ccf4fe56c6a92077c37259717577d.json) | `098941fd9e7a` | 2026-09-19T15:03:49+00:00 | 401/401 |
| e5 | [`0bf06ff8e9e2`](../status/evidence/0bf06ff8e9e2bcc77da389d9bc86c167c2ffe777ad666f3f6258f3a519963f92.json) | `b5aca216ab99` | 2026-09-19T04:07:09+00:00 | — |
| e6 | [`66ab8b7c8e50`](../status/evidence/66ab8b7c8e5067d93ee54f115b5f9f1aacc9c66645f50992a47ac5831c39e7eb.json) | `b5aca216ab99` | 2026-09-19T00:59:09+00:00 | — |
| e7 | [`17a5f52a9809`](../status/evidence/17a5f52a9809d5a6c148572c189148b42273c84e53005eccd24ab700dad8604b.json) | `09e6016839ad` | 2026-09-19T09:02:46+00:00 | — |
| e8 | [`556276951ed1`](../status/evidence/556276951ed1b6a50a1a7a17946129a76c78ce6659f311d05e5050b1a54e9d67.json) | `09e6016839ad` | 2026-09-19T09:04:20+00:00 | — |
| fmt | [`af9d3221d4a1`](../status/evidence/af9d3221d4a11c5c873b319aee931ee99cf374bda56af8e5f2a75898ded0075d.json) | `098941fd9e7a` | 2026-09-19T14:02:35+00:00 | — |
| gen | [`18df800a71b6`](../status/evidence/18df800a71b64b7255a58d25907b6ed0a29fad807c9886d7573099299d810af1.json) | `098941fd9e7a` | 2026-09-19T15:06:59+00:00 | — |
| oracle | [`db9b5b2ec271`](../status/evidence/db9b5b2ec2719b6b57419acc4180632ba0a86b2ed29beb40ba799a720668ac7f.json) | `098941fd9e7a` | 2026-09-19T14:02:28+00:00 | — |
| program | [`561874e0c6df`](../status/evidence/561874e0c6df08b856dc4e595da900079fc4be78dde91ba0c27a3016ace9f10b.json) | `098941fd9e7a` | 2026-09-19T16:18:51+00:00 | — |
| relater | [`981fff12bacd`](../status/evidence/981fff12bacd54eab8b02804bdb78919731df5cccb1ad6a4c0890017a369c642.json) | `b5aca216ab99` | 2026-09-19T04:06:42+00:00 | — |
| scanner | [`a088df9f7c39`](../status/evidence/a088df9f7c39827fc505dd973140afd89b8b19d7607a76be4497377fb2271394.json) | `098941fd9e7a` | 2026-09-19T15:12:48+00:00 | 33,332/33,332 |
| selftest | [`d6d5116f5f80`](../status/evidence/d6d5116f5f8058eae1f0be73e6cba00599e35e637e3a1777c1d3fb9f01c29b75.json) | `098941fd9e7a` | 2026-09-19T13:37:57+00:00 | — |
| workspace | [`0cf9cc951740`](../status/evidence/0cf9cc95174085613ffed90cbfee4b7240359ce12fa3c6c9d7971ce66f92dd33.json) | `098941fd9e7a` | 2026-09-19T13:38:18+00:00 | — |

The correctness/quality records come from the [S11 closure](S11-closure.md),
CI run `35446072791`, test merge `098941fd9e7a723bc606748bc85de5ad06d910c3`.
Cases were checked against the run specs and manifests at its tested branch
head `20a4348`. The full E2 capture is used here, rather than the S11 CI E2
record that reports only the frozen denominator. The benchmark and S10 records
retain their own source revisions and execution contexts.

## Closure evaluation

The collection satisfies all **55** experiment criteria in the approved
`status/experiments.toml`: E1 2/2, E2 12/12, E3 18/18, E4 11/11, E5 3/3,
E6 2/2, E7 4/4 and E8 3/3. The numerical results and workload limits are in
[ADR 0020](adr/0020-phase-0-gate-decision.md#evidence-available-for-the-decision).

The S01–S10 exit predicates and required items also evaluate true against this
collection. Cases-based parity was derived from the exact recorded manifests,
with no missing, failed or skipped cases. Static provenance, mapping counts and
accepted contract states came from the committed S11 report at `8729daff`.
This is an evaluation of the declared evidence collection, not a replacement
for the live report's single-context freshness evaluation. S11 is independent
of the S12 gate and its closure is recorded separately.

## S10 amended-limit confidence check

The unchanged [initial S10 report](../data/s10/initial-acceptance.json) preserves
21 samples per runtime for each timing comparison. Its original `stable: false`
results and former thresholds remain as measured history. With the owner's
amended limits, the recorded 95% bootstrap bounds are on the passing side:

| Comparison | Recorded elapsed ratio | 95% upper bound | Approved elapsed-ratio ceiling |
| --- | --- | --- | --- |
| WASM parser Rust/Go | 0.596334038 | 0.602314627 | 1 / 1.5 = 0.666666667 |
| Node in-process/socket | 0.341848024 | 0.346077886 | 0.40 |

Both captures are complete; the largest recorded relative MAD is 0.013349.
The artifact-size ratio is 0.268019059 against the approved 0.30 limit.
These are re-evaluations of the existing observations under the accepted
policy, not new measurements or edits to the raw captures.
