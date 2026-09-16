# Missing identifier name resolution

Eight programs place parser-inserted, zero-width identifiers in value and type
positions: an arrow body followed by `||`, a missing initializer, binary operand,
call argument, type name, qualified right side, property name and element access.
They were selected from the E2 differences where Rust added TS2304
`Cannot find name '(Missing)'`.

The native syntactic diagnostics equal Rust's in every program, so parser
recovery is not the cause. Pinned Go `Checker.getResolvedSymbol` skips name
resolution when `ast.NodeIsMissing(node)`, so native reports no TS2304 there.
Before the fix, Rust added one in the arrow, initializer, binary-operand and
element-access programs.

The native capture is `target/s08/p6-missing-identifier-native-01`. Regenerate
and compare with:

```sh
python3 scripts/s08_p2.py capture --spec tools/s08/p6/missing-identifier-requests.json --output target/s08/p6-missing-identifier-native-new
cargo run -p ts_compiler --example p2_checker -- target/s08/p6-missing-identifier-native-new/requests.json target/s08/p6-missing-identifier-rust.json
python3 scripts/s08_p2.py compare --native target/s08/p6-missing-identifier-native-new --actual target/s08/p6-missing-identifier-rust.json --output target/s08/p6-missing-identifier-comparison.json
cargo test --locked -p ts_compiler --test checker_semantics missing_identifier
```

This supplemental comparison does not certify E2 acceptance.
