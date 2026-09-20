# Numeric dependency slice

S05 uses Number.String, FromString and ParsePseudoBigInt from the pinned Go
jsnum package. Number arithmetic, rounding helpers, comparisons and the
PseudoBigInt value wrapper belong to later compiler work.

The parser accepts byte slices, checks Go's grammar, then uses Rust's float
parser or num-bigint's exact integer conversion. Invalid UTF-8 remains invalid
numeric input; whitespace uses the pinned Go Zs set and the ECMA additions,
not the scanner's broader whitespace predicate. Go's empty-digit helper results
and negative-zero sign are preserved.

Dependencies: ryu-js 1.0.3 provides ECMAScript decimal formatting; num-bigint
0.5.1 with num-traits 0.2.19 supplies exact radix integers and ties-to-even
float conversion. These are separately justified capabilities, not a new
compiler numeric representation. Cargo.lock pins transitive versions; policy
and MSRV checks apply. S05's direct numeric oracle requests verify these paths
as well as scanner literal integration.
