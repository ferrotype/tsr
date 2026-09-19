# S08 P3a native cases

`requests.json` fixes 30 source queries over 13 programs, with the four P2
shared-file merge modes retained. Both endpoints use the production checker and
printer. The same P2 observer/protocol accepts a selected request inventory;
no checker algorithm is copied into the harness.

```sh
python3 scripts/s08_p2.py capture --spec tools/s08/p3/requests.json --output target/s08/p3a-native
cargo run -p tsr_compiler --example p2_checker -- target/s08/p3a-native/requests.json target/s08/p3a-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p3a-native --actual target/s08/p3a-rust.json --output target/s08/p3a-comparison.json
```

The full comparison currently exits 1 and records `matched: false` for
`union-duplicate-properties` and `union-missing-type`. Their native errors are
retained; Rust explicitly reports unsupported diagnostic paths. The other 11
programs (28 queries) and four merge modes match. Do not interpret these two
failures as parity or accept any other mismatch.

Use new output paths. The capture hashes and snapshots the selected request
file, shared observer and producer inputs. Both display calls use no enclosing
declaration, with matching explicit flags. Missing-global diagnostics remain in
the result. Full P3, generic libraries, context-sensitive display, acceptance and
performance are not certified by these named cases.

Replay the delivery archive without rerunning Go:

```sh
mkdir -p target/s08/p3a-replay
tar -xJf data/s08/p3a/capture.tar.xz -C target/s08/p3a-replay
python3 scripts/s08_p2.py compare --native target/s08/p3a-replay/native --actual target/s08/p3a-replay/rust/observations.json --output target/s08/p3a-replay/verified.json
```

Archive replay has the same two named failures and exits 1.
