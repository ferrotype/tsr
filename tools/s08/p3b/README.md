# S08 P3b source intersection cases

The frozen request contains 73 queries in 24 programs, including every P3a
request and all four shared-file merge modes. It reuses the existing observer
and validator; the Rust endpoint is the production checker and printer.

```sh
python3 scripts/s08_p2.py capture --spec tools/s08/p3b/requests.json --output target/s08/p3b-native
cargo run -p tsr_compiler --example p2_checker -- target/s08/p3b-native/requests.json target/s08/p3b-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p3b-native --actual target/s08/p3b-rust.json --output target/s08/p3b-comparison.json
```

Use fresh output paths. Twenty complete programs (68 queries) and the merge
modes match. Four named programs remain incomplete: `union-duplicate-properties`,
`union-missing-type`, `intersection-invalid-constituent` and
`intersection-readonly`. Their unported diagnostic/grammar paths remain explicit;
the full comparison exits 1 and records `matched: false`. Readonly property query
graphs match independently, but the grammar failure is not a passing baseline.
No other mismatch is expected or accepted.

Replay the archived observation without rebuilding or rerunning Go:

```sh
mkdir -p target/s08/p3b-review-replay
tar -xJf data/s08/p3b/review.tar.xz -C target/s08/p3b-review-replay
python3 scripts/s08_p2.py compare --native target/s08/p3b-review-replay/native --actual target/s08/p3b-review-replay/rust/observations.json --output target/s08/p3b-review-replay/verified.json
```

Replay retains the same four failures and exits 1. See
[the implementation record](../../../docs/S08-P3.md) for scope and validation.
