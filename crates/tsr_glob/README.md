# tsr_glob

The language-server glob grammar of the pinned TypeScript compiler
(`internal/glob`), for [tsr](https://github.com/ferrotype/tsr): parse a pattern,
render it back and match a path against it. Patterns and inputs are bytes.

This is not the `tsconfig.json` include/exclude matcher, which is a different
grammar and lives in `tsr_tsoptions`. Here `*` spans one path segment, `**` may
only sit next to `/`, `{a,b}` alternates and `[a-z]` is a rune range. The pinned
source describes itself as intended for testing; it is ported as it is. A
negated range stores its flag and ignores it, and matching panics when a
separator element consumes the rest of the input.

Requires Rust 1.96 or newer. Licensed under Apache-2.0; the grammar derives from
Go source under the BSD 3-Clause license in licenses/GO-BSD-3-Clause.txt.
