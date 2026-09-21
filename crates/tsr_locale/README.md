# tsr_locale

Pinned TypeScript locale parsing and diagnostic language matching. Uses generated registry and CLDR data from golang.org/x/text v0.38.0; no host locale or process environment is consulted.

`Locale::parse` returns the recovered locale and a success flag;
`parse_detailed` retains the native failure category. An absent context locale
and an explicitly supplied default locale remain distinct. Diagnostic selection
uses the pinned fourteen-language roster (English plus thirteen translations),
not a general-purpose matcher for arbitrary application rosters.

Regenerate the registry, aliases, likely subtags, matcher index and native
recovery tests with `python3 scripts/generate_locale_tables.py --write`.
`--check` repeats the native export and verifies the recorded manifest. The
translation text tables themselves belong to `tsr_diagnostics` and are generated
by `cargo xtask gen` from the upstream embedded assets.

Requires Rust 1.96 or newer. Licensed under Apache-2.0; the generated x/text
material retains its BSD attribution in NOTICE and licenses/GO-BSD-3-Clause.txt.
