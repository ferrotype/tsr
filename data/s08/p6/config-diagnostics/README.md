# Original config diagnostics in the native baseline executor

The six `deprecatedCompilerOptions` fixtures each produce seven TS5023 unknown
option diagnostics and one TS6046 invalid-target diagnostic at the pin. These
are config-parse errors, not checker deprecation warnings. `requests.json` and
`observations.json` preserve their original inputs and native pre-emit payloads
from the full E2 capture `224d2fa6` at `827c353`. `provenance.json` records hashes.

The native runner removes the main config from compiler inputs after parsing
it. The old Rust executor only used the resulting options/roots, dropping both
config errors and the config AST needed to locate later option diagnostics.
The original config is still present in the frozen baseline `error_inputs`.
The new helper reparses those inputs through production tsoptions and the
shared S07 config resolver host. It does not infer expected diagnostics from
native observations or change the frozen compiler inputs/options.

Native authority at the pin:

- `testrunner/test_case_parser.go:parseTestCaseContentWithSettings`: parse the
  original root test unit directly; build the config VFS case-sensitively and
  resolve inherited configs through that VFS.
- `testutil/harnessutil/harnessutil.go:CompileFilesEx`: preserve the config AST,
  errors and content mappers alongside the final options/root list. Its fresh
  ParsedCommandLine leaves `comparePathsOptions` zero-valued; the adapter
  preserves this detail rather than using the config parser's context.
- `tsoptions/parsedcommandline.go:GetConfigFileParsingDiagnostics`: concatenate
  root syntax diagnostics followed by config conversion/option errors.

Rust also retains inherited config owners when sorting, formatting and
serializing diagnostics. Missing original config input becomes a named load
failure. The earlier P4 inventory, which predates original baseline inputs,
retains its existing behavior; P5/E2 requests carry the necessary bytes.

Five compiler regressions cover the six exact native diagnostic sequences,
inherited config ownership, original BOM retention, explicit missing-input
failure, and `tsconfigRootdirInclude`. The latter's frozen native expectation
includes both the missing-comma TS1005 and TS6059's root-file explanation.
It catches omission of config specs, incorrect matching context, and missing
root syntax diagnostics. The inherited/BOM cases are direct API regressions
derived from the pinned contracts, not additional recorded Go observations.

The bounded corpus selection and reproduction commands are in
`../static-index-prototype/README.md`. No full-corpus ratios are inferred from
that selection, and no divergence or denominator change is made.
