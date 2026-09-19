# P4 native exponentiation endpoint

`../jsnum_test.go` is the access-only overlay. The files here record its Go 1.27.1
run against the pinned source on darwin/arm64. Every entry compares exact output
bits with the Rust companion, except NaN payload/sign. These files are a native
ARM observation, not an x86 observation.

Run `cargo build -p ts_jsnum --example p4_arithmetic --locked`, then
`python3 scripts/s08_p4_jsnum.py --actual target/debug/examples/p4_arithmetic
--output target/s08/p4-arithmetic-native-<fresh-id>` on each native CI runner.
The test executes Go's actual `Number.Exponentiate` and compares Rust on that
same architecture. It records the native int64 conversion alongside each power.

At the pin, float64(math.MaxInt64) is 2^63, so the exact positive endpoint enters
the integer fast path. ARM's float-to-int conversion saturates at MaxInt64;
x86 uses MinInt64 for overflow. Rust's float cast saturates on both, requiring
an explicit x86 endpoint branch. The negative odd power consequence is upstream
behavior, not ECMAScript idealization. The four-target run is required to verify
the x86 branch; the committed ARM capture does not establish it.
