package tsoptions

// Phase 1 F3a, access only: the in-package companion of
// `commandlineops_probe_test.go`, overlaid into the pinned `tsoptions` package
// as `phase1_probe_inpackage_export_test.go` while the probe itself is overlaid
// into `tsoptions_test`. Nothing under `upstream/` is modified: both files are
// overlay-only test sources, and `run_probe` re-checks the tree afterwards.
//
// Why it exists. Eight of this step's 65 operations are unexported and have no
// exported caller that exposes their result on its own:
//
//   - `getCompilerOptionValueTypeString` and `formatEnumTypeKeys` appear only
//     as *arguments* of a diagnostic raised by the command-line parser
//     (commandlineparser.go:271, errors.go:16-18). The port has both as named
//     functions -- `OptionDeclaration::value_type_name`
//     (crates/tsr_tsoptions/src/option_declarations.rs:50) and
//     `OptionDeclaration::enum_names` (:62) -- so reaching them only through a
//     parser the port does not have would report a gap where there is none.
//   - `createDiagnosticForInvalidEnumType` (errors.go:14) is likewise only
//     reachable through the parser.
//   - `extraKeyDiagnostics` / `extraKeyDidYouMeanDiagnostics` (errors.go:103,
//     :118) are pure name -> message maps behind the tsconfig parse.
//   - `getParseCommandLineWorkerDiagnostics` (diagnostics.go:30) builds a value
//     whose fields are unexported.
//   - `getWildcardDirectories`, `getWildcardDirectoryFromSpec` and
//     `toCanonicalKey` (wildcarddirectories.go:10, :99, :85) are reachable only
//     through `ParsedCommandLine.WildcardDirectories()`, which needs a
//     `ConfigFile` and hands back an unordered `map[string]bool`.
//   - `getInputOptionName` (commandlineparser.go:164) is one line inside
//     `parseStrings`.
//
// What this file does NOT do. It restates nothing. Every function below is a
// single call to the pinned function with the arguments the request supplies,
// or a read of a pinned struct's unexported field. No control flow between
// pinned steps is reproduced here, and no expected value is read.

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// Phase1RootOption reads a declaration out of the pinned tsconfig root option
// map (tsconfigparsing.go:57). It exists so the `listOrElement` path of
// `ParseListTypeOption` can be driven with the pin's own `extends` declaration
// instead of a synthesised stand-in: `extends` is the only `listOrElement`
// declaration at this pin, and it lives in an unexported map.
func Phase1RootOption(name string) *CommandLineOption {
	return tsconfigRootOptionsMap.ElementOptions.Get(name)
}

// Phase1CompilerOptionValueTypeString is errors.go:28.
func Phase1CompilerOptionValueTypeString(option *CommandLineOption) string {
	return getCompilerOptionValueTypeString(option)
}

// Phase1FormatEnumTypeKeys is errors.go:21. The keys come from the caller so
// the deprecated-key filter can be driven directly; the probe passes the
// option's own `EnumMap().Keys()`, which is the only thing the pin ever passes.
func Phase1FormatEnumTypeKeys(option *CommandLineOption, keys []string) string {
	return formatEnumTypeKeys(option, keys)
}

// Phase1InvalidEnumTypeDiagnostic is errors.go:14 with the nil source file and
// nil node the command-line parser passes (commandlineparser.go:275, :410).
func Phase1InvalidEnumTypeDiagnostic(option *CommandLineOption) *ast.Diagnostic {
	return createDiagnosticForInvalidEnumType(option, nil, nil)
}

// Phase1ExtraKeyDiagnostics is errors.go:103 and errors.go:118 for one parent
// option name. A nil result is the pin's own answer for a name neither switch
// covers, and is reported as such rather than turned into an error.
func Phase1ExtraKeyDiagnostics(parent string) (unknown *diagnostics.Message, didYouMean *diagnostics.Message) {
	return extraKeyDiagnostics(parent), extraKeyDidYouMeanDiagnostics(parent)
}

// Phase1WorkerDiagnostics calls diagnostics.go:30 and reads the value it built.
// Every field below is read, never recomputed.
func Phase1WorkerDiagnostics(declarations []*CommandLineOption) (
	typeMismatch *diagnostics.Message,
	unknownOption *diagnostics.Message,
	unknownDidYouMean *diagnostics.Message,
	alternateMode *diagnostics.Message,
	alternateModeHasNameMap bool,
	declarationCount int,
) {
	worker := getParseCommandLineWorkerDiagnostics(declarations)
	if worker.didYouMean.alternateMode != nil {
		alternateMode = worker.didYouMean.alternateMode.diagnostic
		alternateModeHasNameMap = worker.didYouMean.alternateMode.optionsNameMap != nil
	}
	return worker.OptionTypeMismatchDiagnostic,
		worker.didYouMean.UnknownOptionDiagnostic,
		worker.didYouMean.UnknownDidYouMeanDiagnostic,
		alternateMode,
		alternateModeHasNameMap,
		len(worker.didYouMean.OptionDeclarations)
}

// Phase1ToCanonicalKey is wildcarddirectories.go:85.
func Phase1ToCanonicalKey(path string, useCaseSensitiveFileNames bool) string {
	return toCanonicalKey(path, useCaseSensitiveFileNames)
}

// Phase1WildcardDirectoryFromSpec is wildcarddirectories.go:99. The pinned
// result is a pointer to an unexported struct, so its three fields are returned
// separately with a `matched` flag standing for the nil pointer.
func Phase1WildcardDirectoryFromSpec(spec string, useCaseSensitiveFileNames bool) (
	key string, path string, recursive bool, matched bool,
) {
	match := getWildcardDirectoryFromSpec(spec, useCaseSensitiveFileNames)
	if match == nil {
		return "", "", false, false
	}
	return match.Key, match.Path, match.Recursive, true
}

// Phase1GetWildcardDirectories is wildcarddirectories.go:10.
func Phase1GetWildcardDirectories(
	include []string,
	exclude []string,
	comparePathsOptions tspath.ComparePathsOptions,
) map[string]bool {
	return getWildcardDirectories(include, exclude, comparePathsOptions)
}

// Phase1GetInputOptionName is commandlineparser.go:164.
func Phase1GetInputOptionName(input string) string {
	return getInputOptionName(input)
}
