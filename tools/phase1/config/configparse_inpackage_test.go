package tsoptions

// Phase 1 F3a, access only: the in-package companion of the `configParse`
// probe. It is overlaid into the pinned `tsoptions` package as a second
// `_test.go` file (run_probe's `helper` parameter; the worked example is
// tools/phase1/filesystem/matchfiles_inpackage_test.go), because most of
// tsconfigparsing.go and parsinghelpers.go is unexported and a probe living in
// `tsoptions_test` cannot reach it.
//
// Every function here is a one-line forwarder. Nothing in this file decides
// anything, reformats a result or repairs an input: each Phase1* wrapper calls
// exactly one pinned entry point (or, where the pinned type is unexported,
// builds the pinned struct from components and calls one method on it) and
// hands the pinned result straight back. The pin itself is untouched: the file
// is supplied through `go test -overlay` and never written into upstream/.

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
)

// --- file-spec validation (tsconfigparsing.go:1546-1611) ---------------------

// Phase1SpecToDiagnostic forwards to specToDiagnostic. The pinned message
// POINTER is not reported: only whether a message was returned and, if so, its
// numeric code, so a reworded message does not fail a case.
func Phase1SpecToDiagnostic(spec string, disallowTrailingRecursion bool) (int32, bool) {
	message := specToDiagnostic(spec, disallowTrailingRecursion)
	if message == nil {
		return 0, false
	}
	return message.Code(), true
}

// invalidTrailingRecursion and invalidDotDotAfterRecursiveWildcard get no
// forwarder. specToDiagnostic is their only caller, and both are inlined into
// the Rust counterpart (crates/tsr_tsoptions/src/convert_options.rs:236-257),
// so a separate entry point would compare the harnesses rather than the port.
// The cases reach them through specToDiagnostic and say so in
// operation_actions.

// --- ${configDir} (tsconfigparsing.go:434-442, 1803-1821) --------------------

func Phase1StartsWithConfigDirTemplate(value any) bool {
	return startsWithConfigDirTemplate(value)
}

func Phase1SubstitutedPath(value string, basePath string) string {
	return getSubstitutedPathWithConfigDirTemplate(value, basePath)
}

func Phase1SubstitutedStringArray(list []string, basePath string) []string {
	return getSubstitutedStringArrayWithConfigDirTemplate(list, basePath)
}

// --- wildcard directories (wildcarddirectories.go) ---------------------------

func Phase1GetWildcardDirectories(include []string, exclude []string, options tspath.ComparePathsOptions) map[string]bool {
	return getWildcardDirectories(include, exclude, options)
}

// --- defaults (tsconfigparsing.go:927-947) -----------------------------------

func Phase1DefaultCompilerOptions(configFileName string) *core.CompilerOptions {
	return getDefaultCompilerOptions(configFileName)
}

func Phase1DefaultTypeAcquisition(configFileName string) *core.TypeAcquisition {
	return getDefaultTypeAcquisition(configFileName)
}

// --- raw JSON conversion (tsconfigparsing.go:882-925) ------------------------

func Phase1ConvertToObject(sourceFile *ast.SourceFile) (any, []*ast.Diagnostic) {
	return convertToObject(sourceFile)
}

func Phase1NormalizeJsonValue(value any) any {
	return normalizeJsonValue(value)
}

func Phase1IsStringValue(value any) bool {
	return isStringValue(value)
}

// Phase1IsDoubleQuotedString forwards to isDoubleQuotedString
// (tsconfigparsing.go:814). The caller supplies the node.
func Phase1IsDoubleQuotedString(node *ast.Node) bool {
	return isDoubleQuotedString(node)
}

// Phase1IsCompilerOptionsValue resolves the named compiler option through the
// pinned CommandLineCompilerOptionsMap and forwards to isCompilerOptionsValue.
// A name with no declaration yields (false, false) so the caller can tell an
// unknown option from a rejected value.
func Phase1IsCompilerOptionsValue(optionName string, value any) (bool, bool) {
	option := CommandLineCompilerOptionsMap.Get(optionName)
	if option == nil {
		return false, false
	}
	return isCompilerOptionsValue(option, value), true
}

// --- parsing helpers with unexported names (parsinghelpers.go) ---------------

func Phase1ParseStringMap(value any) *collections.OrderedMap[string, []string] {
	return parseStringMap(value)
}

func Phase1ParseNumber(value any) *int {
	return parseNumber(value)
}

func Phase1ParseStringArrayStrict(value any) ([]string, bool) {
	return parseStringArrayStrict(value)
}

// Phase1ParseProjectReference forwards to parseProjectReference and returns the
// unexported result struct's fields, which a `tsoptions_test` caller cannot
// name. `present` is false when the pinned function returned nil.
func Phase1ParseProjectReference(value any) (present bool, path string, circular bool, hasPath bool, pathValid bool, hasCircular bool, circularValid bool) {
	result := parseProjectReference(value)
	if result == nil {
		return false, "", false, false, false, false, false
	}
	return true, result.reference.Path, result.reference.Circular,
		result.hasPath, result.pathValid, result.hasCircular, result.circularValid
}

// Phase1ParseContentMapper forwards to parseContentMapper, decomposing the
// *contentmapper.Mapper so the caller needs no contentmapper import.
func Phase1ParseContentMapper(value any) (present bool, pkg string, extensions []string, options []byte, errors []*ast.Diagnostic) {
	mapper, errs := parseContentMapper(value)
	if mapper == nil {
		return false, "", nil, nil, errs
	}
	return true, mapper.Package, mapper.Definition.Extensions, mapper.Options, errs
}

// --- option name maps (tsconfigparsing.go:596-624) ---------------------------

// Phase1CommandLineOptionsToMap forwards to commandLineOptionsToMap over a list
// of synthesised declarations with the supplied names, and returns the keys the
// pinned builder produced. The keys travel as a slice so the caller decides how
// to order them; the pinned result is a Go map and has no order of its own.
func Phase1CommandLineOptionsToMap(names []string) []string {
	declarations := make([]*CommandLineOption, 0, len(names))
	for _, name := range names {
		declarations = append(declarations, &CommandLineOption{Name: name, Kind: CommandLineOptionTypeBoolean})
	}
	built := commandLineOptionsToMap(declarations)
	keys := make([]string, 0, len(built))
	for key := range built {
		keys = append(keys, key)
	}
	return keys
}

// --- absolute-path conversion (parsinghelpers.go:696-738) --------------------

func Phase1ConvertToOptionsWithAbsolutePaths(options *collections.OrderedMap[string, any], cwd string) *collections.OrderedMap[string, any] {
	return convertToOptionsWithAbsolutePaths(options, CommandLineCompilerOptionsMap, cwd)
}

// --- config file specs (tsconfigparsing.go:94-151, 1934-2024) ----------------

// Phase1Specs carries the components of the unexported configFileSpecs struct.
// `build` assembles the pinned struct in its declared field order; nothing else
// here interprets the components.
type Phase1Specs struct {
	Files             []any
	Includes          []any
	Excludes          []any
	ValidatedFiles    []string
	ValidatedIncludes []string
	ValidatedExcludes []string
	FilesBefore       []string
	IncludesBefore    []string
	IsDefaultInclude  bool
}

func (s Phase1Specs) build() configFileSpecs {
	return configFileSpecs{
		filesSpecs:                              s.Files,
		includeSpecs:                            s.Includes,
		excludeSpecs:                            s.Excludes,
		validatedFilesSpec:                      s.ValidatedFiles,
		validatedIncludeSpecs:                   s.ValidatedIncludes,
		validatedExcludeSpecs:                   s.ValidatedExcludes,
		validatedFilesSpecBeforeSubstitution:    s.FilesBefore,
		validatedIncludeSpecsBeforeSubstitution: s.IncludesBefore,
		isDefaultIncludeSpec:                    s.IsDefaultInclude,
	}
}

func Phase1FileNamesFromConfigSpecs(specs Phase1Specs, basePath string, options *core.CompilerOptions, host vfs.FS, extraExtensions []string) ([]string, int) {
	built := specs.build()
	return getFileNamesFromConfigSpecs(built, basePath, options, host, extraExtensions)
}

func Phase1MatchesExclude(specs Phase1Specs, fileName string, options tspath.ComparePathsOptions) bool {
	built := specs.build()
	return built.matchesExclude(fileName, options)
}

func Phase1MatchedIncludeSpec(specs Phase1Specs, fileName string, options tspath.ComparePathsOptions) string {
	built := specs.build()
	return built.getMatchedIncludeSpec(fileName, options)
}

func Phase1MatchedFileSpec(specs Phase1Specs, fileName string, options tspath.ComparePathsOptions) string {
	built := specs.build()
	return built.getMatchedFileSpec(fileName, options)
}

func Phase1HasFileWithHigherPriorityExtension(file string, extensions [][]string, present []string) bool {
	has := make(map[string]struct{}, len(present))
	for _, name := range present {
		has[name] = struct{}{}
	}
	return hasFileWithHigherPriorityExtension(file, extensions, func(fileName string) bool {
		_, found := has[fileName]
		return found
	})
}

// Phase1RemoveWildcardFilesWithLowerPriorityExtension forwards to the pinned
// remover over an ordered map built from `existing`, and returns the surviving
// values in the pinned map's own order.
func Phase1RemoveWildcardFilesWithLowerPriorityExtension(file string, existing []string, extensions [][]string) []string {
	var wildcardFiles collections.OrderedMap[string, string]
	for _, name := range existing {
		wildcardFiles.Set(name, name)
	}
	removeWildcardFilesWithLowerPriorityExtension(file, &wildcardFiles, extensions, func(value string) string { return value })
	survivors := make([]string, 0, wildcardFiles.Size())
	for value := range wildcardFiles.Values() {
		survivors = append(survivors, value)
	}
	return survivors
}

// --- config syntax lookup (tsconfigparsing.go:1613-1801) ---------------------

func Phase1GetTsConfigObjectLiteralExpression(sourceFile *ast.SourceFile) *ast.ObjectLiteralExpression {
	return getTsConfigObjectLiteralExpression(sourceFile)
}

// --- the four option parsers (parsinghelpers.go:198-266) ---------------------
//
// `optionParser` and its four implementations are unexported, and
// `convertMapToOptions` (tsconfigparsing.go:626-632) is the generic that
// dispatches through the interface. These forwarders build the pinned struct
// over a caller-supplied target and call exactly one method on it, so a case
// can observe each implementation's answer separately instead of inferring it
// from a whole parse.

// Phase1ParserDiagnosticCodes returns the two unknown-option message codes the
// named parser answers with. The pinned message POINTER is not reported, only
// its numeric code, so a reworded message does not fail a case.
func Phase1ParserDiagnosticCodes(kind string) (unknown int32, didYouMean int32, ok bool) {
	var parser optionParser
	switch kind {
	case "compiler":
		parser = &compilerOptionsParser{&core.CompilerOptions{}}
	case "watch":
		parser = &watchOptionsParser{&core.WatchOptions{}}
	case "typeAcquisition":
		parser = &typeAcquisitionParser{&core.TypeAcquisition{}}
	case "build":
		parser = &buildOptionsParser{&core.BuildOptions{}}
	default:
		return 0, 0, false
	}
	return parser.UnknownOptionDiagnostic().Code(), parser.UnknownDidYouMeanDiagnostic().Code(), true
}

// Phase1ParseOptionInto runs one parser's ParseOption over the supplied
// key/value pairs, in order, and reports the diagnostic codes it produced. The
// pairs travel as a pinned OrderedMap so their order is the caller's.
func Phase1ParseOptionInto(kind string, entries *collections.OrderedMap[string, any]) (codes []int32, ok bool) {
	var parser optionParser
	switch kind {
	case "compiler":
		parser = &compilerOptionsParser{&core.CompilerOptions{}}
	case "watch":
		parser = &watchOptionsParser{&core.WatchOptions{}}
	case "typeAcquisition":
		parser = &typeAcquisitionParser{&core.TypeAcquisition{}}
	case "build":
		parser = &buildOptionsParser{&core.BuildOptions{}}
	default:
		return nil, false
	}
	codes = []int32{}
	for key, value := range entries.Entries() {
		for _, diagnostic := range parser.ParseOption(key, value) {
			codes = append(codes, diagnostic.Code())
		}
	}
	return codes, true
}

// Phase1ConvertMapToOptions forwards to the pinned generic, which is the only
// caller of ParseOption on the config path. It reports the named fields of the
// compiler options it filled, so a case sees what the dispatch produced rather
// than only that it ran.
func Phase1ConvertMapToOptions(entries *collections.OrderedMap[string, any]) *core.CompilerOptions {
	return convertMapToOptions(entries, &compilerOptionsParser{&core.CompilerOptions{}}).CompilerOptions
}
