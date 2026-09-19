# S08 P2 checkpoint evidence

`review-fixes.tar.xz` is the current 29-query, six-program capture; its provenance
is in `review-fixes.json`. The original 23-query capture and `record.json` remain
as the initial delivery record.

Each archive contains the original native capture with its source snapshot,
requests, observations, command and logs, plus the Rust observation and exact
comparison. The corresponding record identifies the implementation revision and archive
hash. These are named checkpoint results, not E2/E3/E4 or performance evidence.

Replay without running Go again:

```sh
mkdir -p target/s08/p2-replay
tar -xJf data/s08/p2/review-fixes.tar.xz -C target/s08/p2-replay
python3 scripts/s08_p2.py compare --native target/s08/p2-replay/native --actual target/s08/p2-replay/rust/observations.json --output target/s08/p2-replay/verified.json
```

To run the current Rust implementation against those original observations:

```sh
cargo run -p tsr_compiler --example p2_checker -- target/s08/p2-replay/native/requests.json target/s08/p2-replay/current-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p2-replay/native --actual target/s08/p2-replay/current-rust.json --output target/s08/p2-replay/current-comparison.json
```

The follow-up observer aligns both display calls to no enclosing declaration and
adds named diagnostic/reference/display regressions. Its native capture was
rerun; the initial archive remains unchanged. Context-sensitive display and
full-corpus parity remain pending.
