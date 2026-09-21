package tsoptions_test

// Phase 1 F3a, access only: the config-parsing half of `internal/tsoptions` --
// tsconfigparsing.go and parsinghelpers.go, plus wildcarddirectories.go.
//
// Subject "configParse". One request one action; the observation is a plain
// value tree of primitives so the comparison can canonicalise it. Two rules
// shape everything below.
//
//   * NOTHING PINNED IS REIMPLEMENTED. Every observation is the return value of
//     a pinned call, decomposed into JSON. Where the pinned entry point is
//     unexported, the companion in-package overlay (`helper` in
//     scripts/phase1_capture.py::run_probe, the worked example being
//     tools/phase1/filesystem/matchfiles_inpackage_test.go) forwards to it in
//     one line and this file calls the forwarder. The pin is untouched: both
//     files are supplied through `go test -overlay`.
//
//   * NO PINNED WORDING IS FROZEN. A diagnostic travels as its numeric code,
//     its already-stringified arguments, its position and whether it carries a
//     file -- never its rendered sentence, and never a Go library error string.
//     A pinned refusal is recorded as the FACT of the refusal.
//
// This probe compiles into the pinned `tsoptions_test` package so it can reuse
// that package's own `memoCache` (tsconfigparsing_test.go:1754), the pinned
// minimal ExtendedConfigCache, rather than restating a cache of its own. That
// package links internal/testutil/baseline, whose init calls
// repo.TestDataPath(), and repo panics under -trimpath, so this probe must be
// registered with "trimpath": False as the other two tsoptions probes are.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"os"
	"runtime"
	"sort"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/contentmapper"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

const configParseSubject = "configParse"

type configParseRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	Action    string `json:"action"`

	// Host construction, shared by every action that needs a filesystem.
	Files            map[string]string `json:"files"`
	CurrentDirectory string            `json:"currentDirectory"`
	CaseSensitive    bool              `json:"caseSensitive"`

	// Config identification and text.
	ConfigFileName string `json:"configFileName"`
	BasePath       string `json:"basePath"`
	JSONText       string `json:"jsonText"`

	// What the observation should carry. An empty list means "everything the
	// action offers"; naming sections keeps a case discriminating.
	Report      []string `json:"report"`
	OptionNames []string `json:"optionNames"`

	// The four unexported option parsers (parsinghelpers.go:198-266). `kind` is
	// one of compiler/watch/typeAcquisition/build; `entries` are the ordered
	// key/value pairs ParseOption is handed, one at a time.
	ParserKind    string        `json:"parserKind"`
	ParserEntries []parserEntry `json:"parserEntries"`

	// The option path GetContentMapperOptionDiagnosticLocation walks into a
	// mapper's syntax, one segment per step.
	OptionPath []pathSegment `json:"optionPath"`

	// Repeat the parse with a shared pinned memoCache, so a case can see what a
	// cache hit re-emits. `parseAlso` names a second config parsed first
	// through the same cache.
	UseCache   bool   `json:"useCache"`
	ParseTwice bool   `json:"parseTwice"`
	ParseAlso  string `json:"parseAlso"`

	// Leaf-call inputs.
	Helper     string     `json:"helper"`
	Target     string     `json:"target"`
	Key        string     `json:"key"`
	Name       string     `json:"name"`
	Spec       string     `json:"spec"`
	Specs      []string   `json:"specs"`
	Disallow   bool       `json:"disallow"`
	Include    []string   `json:"include"`
	Exclude    []string   `json:"exclude"`
	Value      []any      `json:"value"`
	Entries    [][]any    `json:"entries"`
	Extra      []string   `json:"extra"`
	Query      []string   `json:"query"`
	Index      int        `json:"index"`
	Extensions [][]string `json:"extensions"`
	Present    []string   `json:"present"`

	// Pre-validated spec components, for the file-name expansion action.
	ValidatedFiles    []string `json:"validatedFiles"`
	ValidatedIncludes []string `json:"validatedIncludes"`
	ValidatedExcludes []string `json:"validatedExcludes"`
	FilesBefore       []string `json:"filesBefore"`
	IncludesBefore    []string `json:"includesBefore"`
	IsDefaultInclude  bool     `json:"isDefaultInclude"`
}

// --- tagged value encoding ---------------------------------------------------
//
// The pinned config surface is written against Go's `any`, and which concrete
// Go type an `any` holds is itself part of the contract: parseNumber accepts
// int and float64 and refuses everything else; ParseStringArray accepts []any
// and refuses []string; normalizeJsonValue turns a map[string]any into a
// key-sorted OrderedMap but leaves an OrderedMap's order alone. So a request
// states the concrete type with a tag, and an observation reports the concrete
// type it got back. A bare JSON value could not express either.

// parserEntry is one key/value pair handed to a parser's ParseOption, in the
// order the request gives them. The value carries its Go type as a tag, like
// every other value in this probe.
// pathSegment is one step of a content-mapper option path: a named member, or
// an array index when IsIndex is set.
type pathSegment struct {
	Name    string `json:"name"`
	Index   int    `json:"index"`
	IsIndex bool   `json:"isIndex"`
}

type parserEntry struct {
	Key   string `json:"key"`
	Value []any  `json:"value"`
}

// orderedEntries turns the request's pairs into the pinned OrderedMap the
// unexported parsers and convertMapToOptions take, preserving request order.
func orderedEntries(entries []parserEntry) (*collections.OrderedMap[string, any], error) {
	ordered := &collections.OrderedMap[string, any]{}
	for _, entry := range entries {
		decoded, err := decodeValue(entry.Value)
		if err != nil {
			return nil, err
		}
		ordered.Set(entry.Key, decoded)
	}
	return ordered, nil
}

func decodeValue(value []any) (any, error) {
	if len(value) == 0 {
		return nil, fmt.Errorf("a tagged value needs at least a tag")
	}
	tag, ok := value[0].(string)
	if !ok {
		return nil, fmt.Errorf("a tagged value's first element must be the tag")
	}
	payload := func() any {
		if len(value) > 1 {
			return value[1]
		}
		return nil
	}()
	switch tag {
	case "null":
		return nil, nil
	case "emptyStruct":
		return struct{}{}, nil
	case "bool", "number", "string":
		return payload, nil
	case "int":
		number, ok := payload.(float64)
		if !ok {
			return nil, fmt.Errorf("int payload is not a JSON number")
		}
		return int(number), nil
	case "nilarray":
		var nilSlice []any
		return nilSlice, nil
	case "array":
		elements, ok := payload.([]any)
		if !ok {
			return nil, fmt.Errorf("array payload is not a JSON array")
		}
		result := make([]any, 0, len(elements))
		for _, element := range elements {
			nested, ok := element.([]any)
			if !ok {
				return nil, fmt.Errorf("array element is not a tagged value")
			}
			decoded, err := decodeValue(nested)
			if err != nil {
				return nil, err
			}
			result = append(result, decoded)
		}
		return result, nil
	case "strings":
		elements, ok := payload.([]any)
		if !ok {
			return nil, fmt.Errorf("strings payload is not a JSON array")
		}
		result := make([]string, 0, len(elements))
		for _, element := range elements {
			text, ok := element.(string)
			if !ok {
				return nil, fmt.Errorf("strings element is not a string")
			}
			result = append(result, text)
		}
		return result, nil
	case "object", "map":
		entries, ok := payload.([]any)
		if !ok {
			return nil, fmt.Errorf("%s payload is not a JSON array of pairs", tag)
		}
		ordered := &collections.OrderedMap[string, any]{}
		plain := map[string]any{}
		for _, entry := range entries {
			pair, ok := entry.([]any)
			if !ok || len(pair) != 2 {
				return nil, fmt.Errorf("%s entry is not a [key, value] pair", tag)
			}
			key, ok := pair[0].(string)
			if !ok {
				return nil, fmt.Errorf("%s entry key is not a string", tag)
			}
			nested, ok := pair[1].([]any)
			if !ok {
				return nil, fmt.Errorf("%s entry value is not a tagged value", tag)
			}
			decoded, err := decodeValue(nested)
			if err != nil {
				return nil, err
			}
			if tag == "object" {
				ordered.Set(key, decoded)
			} else {
				plain[key] = decoded
			}
		}
		if tag == "object" {
			return ordered, nil
		}
		return plain, nil
	default:
		return nil, fmt.Errorf("unknown value tag %q", tag)
	}
}

// renderNumber writes a float64 the way both sides can agree on. Go's
// encoding/json writes 1.0 as `1` while a Rust f64 writes `1.0`, and the
// comparison canonicalises the PARSED documents, so a bare number would make an
// agreeing pair differ on nothing but its spelling. The text is the shared
// representation.
func renderNumber(value float64) string {
	if value == float64(int64(value)) && value < 1e15 && value > -1e15 {
		return fmt.Sprintf("%d", int64(value))
	}
	return fmt.Sprintf("%v", value)
}

func renderValue(value any) []any {
	switch typed := value.(type) {
	case nil:
		return []any{"null"}
	case struct{}:
		return []any{"emptyStruct"}
	case bool:
		return []any{"bool", typed}
	case float64:
		return []any{"number", renderNumber(typed)}
	case int:
		return []any{"int", typed}
	case string:
		return []any{"string", typed}
	case core.Tristate:
		return []any{"tristate", int(typed)}
	case []any:
		if typed == nil {
			return []any{"nilarray"}
		}
		elements := make([]any, 0, len(typed))
		for _, element := range typed {
			elements = append(elements, renderValue(element))
		}
		return []any{"array", elements}
	case []string:
		if typed == nil {
			return []any{"nilarray"}
		}
		elements := make([]any, 0, len(typed))
		for _, element := range typed {
			elements = append(elements, element)
		}
		return []any{"strings", elements}
	case *collections.OrderedMap[string, any]:
		if typed == nil {
			return []any{"null"}
		}
		entries := make([]any, 0, typed.Size())
		for key, nested := range typed.Entries() {
			entries = append(entries, []any{key, renderValue(nested)})
		}
		return []any{"object", entries}
	case *collections.OrderedMap[string, []string]:
		if typed == nil {
			return []any{"null"}
		}
		entries := make([]any, 0, typed.Size())
		for key, nested := range typed.Entries() {
			entries = append(entries, []any{key, renderValue(nested)})
		}
		return []any{"object", entries}
	case map[string]any:
		keys := make([]string, 0, len(typed))
		for key := range typed {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		entries := make([]any, 0, len(keys))
		for _, key := range keys {
			entries = append(entries, []any{key, renderValue(typed[key])})
		}
		return []any{"map", entries}
	default:
		// The concrete type is part of the contract, so an unexpected one is
		// reported as itself rather than coerced into something comparable.
		return []any{"other", fmt.Sprintf("%T", value)}
	}
}

// renderDiagnostic reports a diagnostic as identity plus location: the numeric
// code, the already-stringified arguments, the span, and whether a file is
// attached. The rendered sentence is deliberately absent -- freezing it would
// make a case fail on a rewording that changes no behaviour -- and so is the
// file NAME, because the Rust side's diagnostic carries a node identity rather
// than a file name and a name comparison would be comparing the harnesses.
func renderDiagnostic(diagnostic *ast.Diagnostic) map[string]any {
	args := make([]any, 0, len(diagnostic.MessageArgs()))
	for _, arg := range diagnostic.MessageArgs() {
		args = append(args, arg)
	}
	return map[string]any{
		"code":     int(diagnostic.Code()),
		"args":     args,
		"pos":      diagnostic.Pos(),
		"end":      diagnostic.End(),
		"has_file": diagnostic.File() != nil,
	}
}

func renderDiagnostics(list []*ast.Diagnostic) []any {
	rows := make([]any, 0, len(list))
	for _, diagnostic := range list {
		rows = append(rows, renderDiagnostic(diagnostic))
	}
	return rows
}

// renderOptions projects the named compiler options. A whole-struct dump would
// be a Go serializer's output rather than a port contract, so a case names the
// options its behaviour is about and gets exactly those.
func renderOptions(options *core.CompilerOptions, names []string) ([]any, error) {
	if options == nil {
		return nil, nil
	}
	rows := make([]any, 0, len(names))
	for _, name := range names {
		var rendered []any
		switch name {
		case "outDir":
			rendered = []any{"string", options.OutDir}
		case "outFile":
			rendered = []any{"string", options.OutFile}
		case "rootDir":
			rendered = []any{"string", options.RootDir}
		case "declarationDir":
			rendered = []any{"string", options.DeclarationDir}
		case "baseUrl":
			rendered = []any{"string", options.BaseUrl}
		case "tsBuildInfoFile":
			rendered = []any{"string", options.TsBuildInfoFile}
		case "generateCpuProfile":
			rendered = []any{"string", options.GenerateCpuProfile}
		case "generateTrace":
			rendered = []any{"string", options.GenerateTrace}
		case "configFilePath":
			rendered = []any{"string", options.ConfigFilePath}
		case "pathsBasePath":
			rendered = []any{"string", options.PathsBasePath}
		case "rootDirs":
			rendered = renderValue(options.RootDirs)
		case "typeRoots":
			rendered = renderValue(options.TypeRoots)
		case "types":
			rendered = renderValue(options.Types)
		case "lib":
			rendered = renderValue(options.Lib)
		case "moduleSuffixes":
			rendered = renderValue(options.ModuleSuffixes)
		case "customConditions":
			rendered = renderValue(options.CustomConditions)
		case "paths":
			rendered = renderValue(options.Paths)
		case "strict":
			rendered = []any{"tristate", int(options.Strict)}
		case "allowJs":
			rendered = []any{"tristate", int(options.AllowJs)}
		case "noEmit":
			rendered = []any{"tristate", int(options.NoEmit)}
		case "skipLibCheck":
			rendered = []any{"tristate", int(options.SkipLibCheck)}
		case "composite":
			rendered = []any{"tristate", int(options.Composite)}
		case "declaration":
			rendered = []any{"tristate", int(options.Declaration)}
		case "resolveJsonModule":
			rendered = []any{"tristate", int(options.ResolveJsonModule)}
		case "runExternalCode":
			rendered = []any{"tristate", int(options.RunExternalCode)}
		case "maxNodeModuleJsDepth":
			if options.MaxNodeModuleJsDepth == nil {
				rendered = []any{"null"}
			} else {
				rendered = []any{"int", *options.MaxNodeModuleJsDepth}
			}
		case "target":
			rendered = []any{"int", int(options.Target)}
		case "module":
			rendered = []any{"int", int(options.Module)}
		case "moduleResolution":
			rendered = []any{"int", int(options.ModuleResolution)}
		case "moduleDetection":
			rendered = []any{"int", int(options.ModuleDetection)}
		case "jsx":
			rendered = []any{"int", int(options.Jsx)}
		case "newLine":
			rendered = []any{"int", int(options.NewLine)}
		default:
			return nil, fmt.Errorf("no projection for compiler option %q", name)
		}
		rows = append(rows, []any{name, rendered})
	}
	return rows, nil
}

func renderTypeAcquisition(types *core.TypeAcquisition) any {
	if types == nil {
		return nil
	}
	return []any{
		[]any{"enable", []any{"tristate", int(types.Enable)}},
		[]any{"include", renderValue(types.Include)},
		[]any{"exclude", renderValue(types.Exclude)},
		[]any{"disableFilenameBasedTypeAcquisition", []any{"tristate", int(types.DisableFilenameBasedTypeAcquisition)}},
	}
}

func comparePathsOptions(request configParseRequest) tspath.ComparePathsOptions {
	return tspath.ComparePathsOptions{
		CurrentDirectory:          request.CurrentDirectory,
		UseCaseSensitiveFileNames: request.CaseSensitive,
	}
}

func (r configParseRequest) base() string {
	if r.BasePath != "" {
		return r.BasePath
	}
	return r.CurrentDirectory
}

func (r configParseRequest) wants(section string) bool {
	if len(r.Report) == 0 {
		return true
	}
	for _, name := range r.Report {
		if name == section {
			return true
		}
	}
	return false
}

func (r configParseRequest) specs() tsoptions.Phase1Specs {
	toAny := func(values []string) []any {
		if values == nil {
			return nil
		}
		result := make([]any, 0, len(values))
		for _, value := range values {
			result = append(result, value)
		}
		return result
	}
	return tsoptions.Phase1Specs{
		Files:             toAny(r.FilesBefore),
		Includes:          toAny(r.IncludesBefore),
		Excludes:          toAny(r.ValidatedExcludes),
		ValidatedFiles:    r.ValidatedFiles,
		ValidatedIncludes: r.ValidatedIncludes,
		ValidatedExcludes: r.ValidatedExcludes,
		FilesBefore:       r.FilesBefore,
		IncludesBefore:    r.IncludesBefore,
		IsDefaultInclude:  r.IsDefaultInclude,
	}
}

// describeParsed projects one ParsedCommandLine into the sections a request
// asked for. Every value is read straight off the pinned result.
func describeParsed(request configParseRequest, parsed *tsoptions.ParsedCommandLine) (map[string]any, error) {
	observation := map[string]any{}
	if request.wants("file_names") {
		names := make([]any, 0, len(parsed.ParsedConfig.FileNames))
		for _, name := range parsed.ParsedConfig.FileNames {
			names = append(names, name)
		}
		observation["file_names"] = names
	}
	if request.wants("errors") {
		observation["errors"] = renderDiagnostics(parsed.Errors)
	}
	if request.wants("error_codes") {
		codes := make([]any, 0, len(parsed.Errors))
		for _, diagnostic := range parsed.Errors {
			codes = append(codes, int(diagnostic.Code()))
		}
		observation["error_codes"] = codes
	}
	if request.wants("options") {
		rendered, err := renderOptions(parsed.ParsedConfig.CompilerOptions, request.OptionNames)
		if err != nil {
			return nil, err
		}
		observation["options"] = rendered
	}
	if request.wants("type_acquisition") {
		observation["type_acquisition"] = renderTypeAcquisition(parsed.ParsedConfig.TypeAcquisition)
	}
	if request.wants("raw") {
		observation["raw"] = renderValue(parsed.Raw)
	}
	if request.wants("raw_keys") {
		keys := []any{}
		if raw, ok := parsed.Raw.(*collections.OrderedMap[string, any]); ok {
			for key := range raw.Keys() {
				keys = append(keys, key)
			}
		}
		observation["raw_keys"] = keys
	}
	if request.wants("ordered") {
		// The same top-level key list as `raw_keys`, under the name an
		// order-sensitive request must use: the comparison canonicalises with
		// sorted keys, so JSON member order survives only inside an array.
		keys := []any{}
		if raw, ok := parsed.Raw.(*collections.OrderedMap[string, any]); ok {
			for key := range raw.Keys() {
				keys = append(keys, key)
			}
		}
		observation["ordered"] = keys
	}
	if request.wants("syntax_diagnostics") {
		// The config source file's OWN parser diagnostics, which the pinned
		// GetConfigFileParsingDiagnostics reports alongside parsed.Errors.
		var syntax []*ast.Diagnostic
		if parsed.ConfigFile != nil {
			syntax = parsed.ConfigFile.SourceFile.Diagnostics()
		}
		observation["syntax_diagnostics"] = renderDiagnostics(syntax)
	}
	if request.wants("compile_on_save") {
		if parsed.CompileOnSave == nil {
			observation["compile_on_save"] = nil
		} else {
			observation["compile_on_save"] = *parsed.CompileOnSave
		}
	}
	if request.wants("extended_source_files") {
		names := []any{}
		if parsed.ConfigFile != nil {
			for _, name := range parsed.ConfigFile.ExtendedSourceFiles {
				names = append(names, name)
			}
		}
		observation["extended_source_files"] = names
	}
	if request.wants("project_references") {
		rows := []any{}
		for _, reference := range parsed.ParsedConfig.ProjectReferences {
			rows = append(rows, []any{reference.Path, reference.OriginalPath, reference.Circular})
		}
		observation["project_references"] = rows
	}
	if request.wants("content_mappers") {
		rows := []any{}
		for _, mapper := range parsed.ParsedConfig.ContentMappers {
			extensions := make([]any, 0, len(mapper.Definition.Extensions))
			for _, extension := range mapper.Definition.Extensions {
				extensions = append(extensions, extension)
			}
			rows = append(rows, []any{mapper.Package, extensions, string(mapper.Options)})
		}
		observation["content_mappers"] = rows
	}
	return observation, nil
}

func buildHost(request configParseRequest) *tsoptionstest.VfsParseConfigHost {
	return tsoptionstest.NewVFSParseConfigHost(request.Files, request.CurrentDirectory, request.CaseSensitive)
}

func parseSourceFileConfig(request configParseRequest, host tsoptions.ParseConfigHost, configFileName string, text string, cache tsoptions.ExtendedConfigCache) *tsoptions.ParsedCommandLine {
	path := tspath.ToPath(configFileName, request.CurrentDirectory, request.CaseSensitive)
	sourceFile := tsoptions.NewTsconfigSourceFileFromFilePath(configFileName, path, text)
	return tsoptions.ParseJsonSourceFileConfigFileContent(
		sourceFile,
		host,
		request.base(),
		nil, /*existingOptions*/
		nil, /*existingOptionsRaw*/
		configFileName,
		nil, /*resolutionStack*/
		cache,
	)
}

// observeConfigParse answers one request. It returns the observation, or an
// error, which the caller records as a harness failure: a harness failure is
// never a semantic result.
func observeConfigParse(t *testing.T, request configParseRequest) (map[string]any, error) {
	switch request.Action {

	// ---- the four option parsers --------------------------------------------

	case "parser_diagnostics":
		unknown, didYouMean, ok := tsoptions.Phase1ParserDiagnosticCodes(request.ParserKind)
		if !ok {
			return nil, fmt.Errorf("no pinned parser named %q", request.ParserKind)
		}
		return map[string]any{
			"kind":                      request.ParserKind,
			"unknown_option_code":       unknown,
			"unknown_did_you_mean_code": didYouMean,
		}, nil

	case "option_parser_parse_option":
		entries, err := orderedEntries(request.ParserEntries)
		if err != nil {
			return nil, err
		}
		codes, ok := tsoptions.Phase1ParseOptionInto(request.ParserKind, entries)
		if !ok {
			return nil, fmt.Errorf("no pinned parser named %q", request.ParserKind)
		}
		return map[string]any{"kind": request.ParserKind, "diagnostic_codes": codes}, nil

	case "convert_map_to_options":
		entries, err := orderedEntries(request.ParserEntries)
		if err != nil {
			return nil, err
		}
		options := tsoptions.Phase1ConvertMapToOptions(entries)
		rendered, err := renderOptions(options, request.OptionNames)
		if err != nil {
			return nil, err
		}
		return map[string]any{"options": rendered}, nil

	case "content_mapper_diagnostic_location":
		// GetContentMapperOptionDiagnosticLocation (tsconfigparsing.go:1706) is
		// exported, but it takes a *ParsedCommandLine and a mapper from that
		// parse, so the case has to run the parse first and then ask where a
		// named option path inside the mapper's syntax lives.
		host := buildHost(request)
		configFileName := tspath.CombinePaths(request.BasePath, request.ConfigFileName)
		source := tsoptions.NewTsconfigSourceFileFromFilePath(
			configFileName, tspath.ToPath(configFileName, request.BasePath, request.CaseSensitive), request.JSONText)
		parsed := tsoptions.ParseJsonSourceFileConfigFileContent(
			source, host, request.BasePath, nil, nil, configFileName, nil, nil)
		mappers := parsed.ContentMappers()
		if len(mappers) == 0 {
			return map[string]any{"mappers": 0, "has_file": false, "pos": -1, "end": -1}, nil
		}
		segments := make([]contentmapper.OptionPathSegment, 0, len(request.OptionPath))
		for _, segment := range request.OptionPath {
			segments = append(segments, contentmapper.OptionPathSegment{
				Property: segment.Name, Index: segment.Index, IsIndex: segment.IsIndex,
			})
		}
		file, span := tsoptions.GetContentMapperOptionDiagnosticLocation(parsed, mappers[0], segments)
		return map[string]any{
			"mappers":  len(mappers),
			"has_file": file != nil,
			"pos":      span.Pos(),
			"end":      span.End(),
		}, nil

	// ---- whole-parse actions ------------------------------------------------

	case "parse_source_file":
		host := buildHost(request)
		var cache tsoptions.ExtendedConfigCache
		if request.UseCache {
			cache = &memoCache{}
		}
		if request.ParseAlso != "" {
			text, ok := request.Files[request.ParseAlso]
			if !ok {
				return nil, fmt.Errorf("parseAlso names %q, which is not in files", request.ParseAlso)
			}
			parseSourceFileConfig(request, host, request.ParseAlso, text, cache)
		}
		parsed := parseSourceFileConfig(request, host, request.ConfigFileName, request.JSONText, cache)
		if request.ParseTwice {
			parsed = parseSourceFileConfig(request, host, request.ConfigFileName, request.JSONText, cache)
		}
		return describeParsed(request, parsed)

	case "read_config_file":
		// GetParsedCommandLineOfConfigFile reads the text itself, so the config
		// comes from the declared filesystem rather than from jsonText.
		host := buildHost(request)
		var cache tsoptions.ExtendedConfigCache
		if request.UseCache {
			cache = &memoCache{}
		}
		if request.ParseAlso != "" {
			if _, errors := tsoptions.GetParsedCommandLineOfConfigFile(request.ParseAlso, nil, nil, host, cache); len(errors) > 0 {
				return nil, fmt.Errorf("the priming parse of %q reported read errors", request.ParseAlso)
			}
		}
		parsed, readErrors := tsoptions.GetParsedCommandLineOfConfigFile(request.ConfigFileName, nil, nil, host, cache)
		if parsed == nil {
			return map[string]any{
				"read_failed": true,
				"errors":      renderDiagnostics(readErrors),
			}, nil
		}
		observation, err := describeParsed(request, parsed)
		if err != nil {
			return nil, err
		}
		observation["read_failed"] = false
		return observation, nil

	case "parse_json_api":
		// The JSON API: the text is converted to a value first, and the value,
		// not the source file, is what ParseJsonConfigFileContent consumes.
		host := buildHost(request)
		configFileName := request.ConfigFileName
		path := tspath.ToPath(configFileName, request.base(), request.CaseSensitive)
		value, _ := tsoptions.ParseConfigFileTextToJson(configFileName, path, request.JSONText)
		parsed := tsoptions.ParseJsonConfigFileContent(
			value, host, request.base(), nil /*existingOptions*/, configFileName,
			nil /*resolutionStack*/, nil /*extendedConfigCache*/)
		return describeParsed(request, parsed)

	case "parse_json_api_value":
		// The same API reached with a hand-built Go value, so the case can put
		// a plain map or a typed slice across the `any` boundary.
		host := buildHost(request)
		value, err := decodeValue(request.Value)
		if err != nil {
			return nil, err
		}
		parsed := tsoptions.ParseJsonConfigFileContent(
			value, host, request.base(), nil /*existingOptions*/, request.ConfigFileName,
			nil /*resolutionStack*/, nil /*extendedConfigCache*/)
		return describeParsed(request, parsed)

	case "parse_config_text":
		configFileName := request.ConfigFileName
		path := tspath.ToPath(configFileName, request.base(), request.CaseSensitive)
		value, errors := tsoptions.ParseConfigFileTextToJson(configFileName, path, request.JSONText)
		return map[string]any{
			"value":  renderValue(value),
			"errors": renderDiagnostics(errors),
		}, nil

	case "convert_to_object":
		// convertToObject is the circularity branch's converter: unlike
		// convertConfigFileToObject it does NOT require an object root.
		configFileName := request.ConfigFileName
		path := tspath.ToPath(configFileName, request.base(), request.CaseSensitive)
		sourceFile := tsoptions.NewTsconfigSourceFileFromFilePath(configFileName, path, request.JSONText)
		value, errors := tsoptions.Phase1ConvertToObject(sourceFile.SourceFile)
		return map[string]any{
			"value":  renderValue(value),
			"errors": renderDiagnostics(errors),
		}, nil

	case "extended_config":
		// ParseExtendedConfig on its own: the cache entry, not the parse that
		// would normally call it.
		host := buildHost(request)
		path := tspath.ToPath(request.ConfigFileName, request.CurrentDirectory, request.CaseSensitive)
		entry := tsoptions.ParseExtendedConfig(request.ConfigFileName, path, nil, host, nil)
		names := []any{}
		for _, name := range entry.ExtendedFileNames() {
			names = append(names, name)
		}
		return map[string]any{
			"extended_file_names": names,
			"has_entry":           entry != nil,
		}, nil

	// ---- leaf actions -------------------------------------------------------

	case "spec_diagnostic":
		// specToDiagnostic is the only entry point reported. Its two predicates
		// (invalidTrailingRecursion, invalidDotDotAfterRecursiveWildcard) are
		// inlined on the Rust side, so reporting them separately would compare
		// the harnesses rather than the port; each case's operation_actions
		// names whichever of the two its spec and flag actually reach.
		code, present := tsoptions.Phase1SpecToDiagnostic(request.Spec, request.Disallow)
		return map[string]any{"code": int(code), "reported": present}, nil

	case "config_dir":
		switch request.Helper {
		case "value":
			// The guard and the replacement are asymmetric on purpose: the
			// predicate lowercases, strings.Replace does not. A case supplies
			// the spelling that separates them.
			rows := make([]any, 0, len(request.Specs))
			for _, value := range request.Specs {
				rows = append(rows, []any{
					value,
					tsoptions.Phase1StartsWithConfigDirTemplate(value),
					tsoptions.Phase1SubstitutedPath(value, request.base()),
				})
			}
			return map[string]any{"per_value": rows}, nil
		case "array":
			// getSubstitutedStringArrayWithConfigDirTemplate returns nil when
			// no element matched, which every pinned caller collapses back to
			// the input list. Both the nil signal and the list are reported.
			substituted := tsoptions.Phase1SubstitutedStringArray(request.Specs, request.base())
			return map[string]any{
				"substituted": substituted != nil,
				"array":       renderValue(substituted),
			}, nil
		default:
			return nil, fmt.Errorf("unknown config_dir helper %q", request.Helper)
		}

	case "supported_extensions":
		options := &core.CompilerOptions{}
		for _, entry := range request.Entries {
			if len(entry) != 2 {
				return nil, fmt.Errorf("an option entry is not a [key, value] pair")
			}
			key, ok := entry[0].(string)
			if !ok {
				return nil, fmt.Errorf("an option entry key is not a string")
			}
			tagged, ok := entry[1].([]any)
			if !ok {
				return nil, fmt.Errorf("an option entry value is not a tagged value")
			}
			value, err := decodeValue(tagged)
			if err != nil {
				return nil, err
			}
			tsoptions.ParseCompilerOptions(key, value, options)
		}
		supported := tsoptions.GetSupportedExtensions(options, request.Extra)
		withJson := tsoptions.GetSupportedExtensionsWithJsonIfResolveJsonModule(options, supported)
		render := func(groups [][]string) []any {
			rows := make([]any, 0, len(groups))
			for _, group := range groups {
				elements := make([]any, 0, len(group))
				for _, extension := range group {
					elements = append(elements, extension)
				}
				rows = append(rows, elements)
			}
			return rows
		}
		return map[string]any{
			"supported": render(supported),
			"with_json": render(withJson),
		}, nil

	case "parse_value":
		value, err := decodeValue(request.Value)
		if err != nil {
			return nil, err
		}
		switch request.Helper {
		case "tristate":
			return map[string]any{"result": []any{"tristate", int(tsoptions.ParseTristate(value))}}, nil
		case "string":
			return map[string]any{"result": []any{"string", tsoptions.ParseString(value)}}, nil
		case "string_array":
			return map[string]any{"result": renderValue(tsoptions.ParseStringArray(value))}, nil
		case "string_map":
			return map[string]any{"result": renderValue(tsoptions.Phase1ParseStringMap(value))}, nil
		case "number":
			number := tsoptions.Phase1ParseNumber(value)
			if number == nil {
				return map[string]any{"result": []any{"null"}}, nil
			}
			return map[string]any{"result": []any{"int", *number}}, nil
		case "string_array_strict":
			result, ok := tsoptions.Phase1ParseStringArrayStrict(value)
			return map[string]any{"result": renderValue(result), "accepted": ok}, nil
		case "is_string_value":
			return map[string]any{"result": tsoptions.Phase1IsStringValue(value)}, nil
		case "is_option_value":
			accepted, known := tsoptions.Phase1IsCompilerOptionsValue(request.Name, value)
			return map[string]any{"result": accepted, "known_option": known}, nil
		case "project_reference":
			present, path, circular, hasPath, pathValid, hasCircular, circularValid :=
				tsoptions.Phase1ParseProjectReference(value)
			return map[string]any{
				"present": present, "path": path, "circular": circular,
				"has_path": hasPath, "path_valid": pathValid,
				"has_circular": hasCircular, "circular_valid": circularValid,
			}, nil
		case "content_mapper":
			present, pkg, extensions, options, errors := tsoptions.Phase1ParseContentMapper(value)
			return map[string]any{
				"present":    present,
				"package":    pkg,
				"extensions": renderValue(extensions),
				"options":    string(options),
				"errors":     renderDiagnostics(errors),
			}, nil
		case "normalize_json_value":
			return map[string]any{"result": renderValue(tsoptions.Phase1NormalizeJsonValue(value))}, nil
		default:
			return nil, fmt.Errorf("unknown parse_value helper %q", request.Helper)
		}

	case "parse_option":
		value, err := decodeValue(request.Value)
		if err != nil {
			return nil, err
		}
		switch request.Target {
		case "compiler":
			options := tsoptions.Phase1DefaultCompilerOptions(request.ConfigFileName)
			errors := tsoptions.ParseCompilerOptions(request.Key, value, options)
			rendered, err := renderOptions(options, request.OptionNames)
			if err != nil {
				return nil, err
			}
			return map[string]any{"options": rendered, "errors": renderDiagnostics(errors)}, nil
		case "type_acquisition":
			types := tsoptions.Phase1DefaultTypeAcquisition(request.ConfigFileName)
			errors := tsoptions.ParseTypeAcquisition(request.Key, value, types)
			return map[string]any{
				"type_acquisition": renderTypeAcquisition(types),
				"errors":           renderDiagnostics(errors),
			}, nil
		case "watch":
			options := &core.WatchOptions{}
			errors := tsoptions.ParseWatchOptions(request.Key, value, options)
			interval := []any{"null"}
			if options.Interval != nil {
				interval = []any{"int", *options.Interval}
			}
			return map[string]any{
				"watch": []any{
					[]any{"watchInterval", interval},
					[]any{"watchFile", []any{"int", int(options.FileKind)}},
					[]any{"watchDirectory", []any{"int", int(options.DirectoryKind)}},
					[]any{"fallbackPolling", []any{"int", int(options.FallbackPolling)}},
					[]any{"synchronousWatchDirectory", []any{"tristate", int(options.SyncWatchDir)}},
					[]any{"excludeDirectories", renderValue(options.ExcludeDir)},
					[]any{"excludeFiles", renderValue(options.ExcludeFiles)},
				},
				"errors": renderDiagnostics(errors),
			}, nil
		case "build":
			options := &core.BuildOptions{}
			errors := tsoptions.ParseBuildOptions(request.Key, value, options)
			builders := []any{"null"}
			if options.Builders != nil {
				builders = []any{"int", *options.Builders}
			}
			return map[string]any{
				"build": []any{
					[]any{"clean", []any{"tristate", int(options.Clean)}},
					[]any{"dry", []any{"tristate", int(options.Dry)}},
					[]any{"force", []any{"tristate", int(options.Force)}},
					[]any{"builders", builders},
					[]any{"stopBuildOnErrors", []any{"tristate", int(options.StopBuildOnErrors)}},
					[]any{"verbose", []any{"tristate", int(options.Verbose)}},
				},
				"errors": renderDiagnostics(errors),
			}, nil
		default:
			return nil, fmt.Errorf("unknown parse_option target %q", request.Target)
		}

	case "default_options":
		// The two pinned defaults are separate entry points, so a case asks for
		// one of them by naming the section it reports.
		observation := map[string]any{}
		if request.wants("options") {
			options := tsoptions.Phase1DefaultCompilerOptions(request.ConfigFileName)
			rendered, err := renderOptions(options, request.OptionNames)
			if err != nil {
				return nil, err
			}
			observation["options"] = rendered
		}
		if request.wants("type_acquisition") {
			observation["type_acquisition"] = renderTypeAcquisition(
				tsoptions.Phase1DefaultTypeAcquisition(request.ConfigFileName))
		}
		return observation, nil

	case "option_absolute_path":
		switch request.Helper {
		case "one":
			value, err := decodeValue(request.Value)
			if err != nil {
				return nil, err
			}
			converted, ok := tsoptions.ConvertOptionToAbsolutePath(
				request.Name, value, tsoptions.CommandLineCompilerOptionsMap, request.CurrentDirectory)
			return map[string]any{"converted": ok, "result": renderValue(converted)}, nil
		case "all":
			options := &collections.OrderedMap[string, any]{}
			for _, entry := range request.Entries {
				if len(entry) != 2 {
					return nil, fmt.Errorf("an option entry is not a [key, value] pair")
				}
				key, ok := entry[0].(string)
				if !ok {
					return nil, fmt.Errorf("an option entry key is not a string")
				}
				tagged, ok := entry[1].([]any)
				if !ok {
					return nil, fmt.Errorf("an option entry value is not a tagged value")
				}
				value, err := decodeValue(tagged)
				if err != nil {
					return nil, err
				}
				options.Set(key, value)
			}
			return map[string]any{
				"result": renderValue(tsoptions.Phase1ConvertToOptionsWithAbsolutePaths(options, request.CurrentDirectory)),
			}, nil
		default:
			return nil, fmt.Errorf("unknown option_absolute_path helper %q", request.Helper)
		}

	case "option_name_map":
		switch request.Helper {
		case "get":
			rows := make([]any, 0, len(request.Query))
			for _, name := range request.Query {
				found := ""
				if option := tsoptions.CommandLineCompilerOptionsMap.Get(name); option != nil {
					found = option.Name
				}
				rows = append(rows, []any{name, found})
			}
			return map[string]any{"resolved": rows}, nil
		case "spelling":
			rows := make([]any, 0, len(request.Query))
			for _, name := range request.Query {
				found := ""
				if option := tsoptions.CommandLineCompilerOptionsMap.GetSpellingSuggestion(name); option != nil {
					found = option.Name
				}
				rows = append(rows, []any{name, found})
			}
			return map[string]any{"suggested": rows}, nil
		case "build_map":
			keys := tsoptions.Phase1CommandLineOptionsToMap(request.Query)
			sort.Strings(keys)
			rows := make([]any, 0, len(keys))
			for _, key := range keys {
				rows = append(rows, key)
			}
			return map[string]any{"keys": rows}, nil
		default:
			return nil, fmt.Errorf("unknown option_name_map helper %q", request.Helper)
		}

	case "wildcard_directories":
		directories := tsoptions.Phase1GetWildcardDirectories(request.Include, request.Exclude, comparePathsOptions(request))
		keys := make([]string, 0, len(directories))
		for key := range directories {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		rows := make([]any, 0, len(keys))
		for _, key := range keys {
			rows = append(rows, []any{key, directories[key]})
		}
		return map[string]any{"directories": rows, "nil_result": directories == nil}, nil

	case "config_specs":
		specs := request.specs()
		switch request.Helper {
		case "file_names":
			options := &core.CompilerOptions{}
			for _, entry := range request.Entries {
				if len(entry) != 2 {
					return nil, fmt.Errorf("an option entry is not a [key, value] pair")
				}
				key, ok := entry[0].(string)
				if !ok {
					return nil, fmt.Errorf("an option entry key is not a string")
				}
				tagged, ok := entry[1].([]any)
				if !ok {
					return nil, fmt.Errorf("an option entry value is not a tagged value")
				}
				value, err := decodeValue(tagged)
				if err != nil {
					return nil, err
				}
				tsoptions.ParseCompilerOptions(key, value, options)
			}
			host := buildHost(request)
			names, literal := tsoptions.Phase1FileNamesFromConfigSpecs(
				specs, request.base(), options, host.FS(), request.Extra)
			rows := make([]any, 0, len(names))
			for _, name := range names {
				rows = append(rows, name)
			}
			return map[string]any{"file_names": rows, "literal_len": literal}, nil
		case "match":
			rows := make([]any, 0, len(request.Query))
			for _, name := range request.Query {
				rows = append(rows, []any{
					name,
					tsoptions.Phase1MatchesExclude(specs, name, comparePathsOptions(request)),
					tsoptions.Phase1MatchedIncludeSpec(specs, name, comparePathsOptions(request)),
					tsoptions.Phase1MatchedFileSpec(specs, name, comparePathsOptions(request)),
				})
			}
			return map[string]any{"matches": rows}, nil
		case "extension_priority":
			return map[string]any{
				"higher_priority": tsoptions.Phase1HasFileWithHigherPriorityExtension(
					request.Spec, request.Extensions, request.Present),
				"survivors": renderValue(tsoptions.Phase1RemoveWildcardFilesWithLowerPriorityExtension(
					request.Spec, request.Present, request.Extensions)),
			}, nil
		default:
			return nil, fmt.Errorf("unknown config_specs helper %q", request.Helper)
		}

	case "syntax_element":
		configFileName := request.ConfigFileName
		path := tspath.ToPath(configFileName, request.base(), request.CaseSensitive)
		sourceFile := tsoptions.NewTsconfigSourceFileFromFilePath(configFileName, path, request.JSONText)
		describe := func(node *ast.Node) any {
			if node == nil {
				return nil
			}
			return []any{int(node.Kind), node.Pos(), node.End()}
		}
		switch request.Helper {
		case "prop_array_element":
			element := tsoptions.GetTsConfigPropArrayElementValue(sourceFile.SourceFile, request.Key, request.Spec)
			if element == nil {
				return map[string]any{"node": nil}, nil
			}
			return map[string]any{"node": describe(element.AsNode()), "text": element.Text}, nil
		case "options_syntax":
			objectLiteral := tsoptions.Phase1GetTsConfigObjectLiteralExpression(sourceFile.SourceFile)
			if objectLiteral == nil {
				return map[string]any{"node": nil, "has_object": false}, nil
			}
			node := tsoptions.GetOptionsSyntaxByArrayElementValue(objectLiteral, request.Key, request.Spec)
			return map[string]any{"node": describe(node), "has_object": true}, nil
		case "double_quoted":
			element := tsoptions.GetTsConfigPropArrayElementValue(sourceFile.SourceFile, request.Key, request.Spec)
			if element == nil {
				return map[string]any{"found": false}, nil
			}
			return map[string]any{
				"found":         true,
				"double_quoted": tsoptions.Phase1IsDoubleQuotedString(element.AsNode()),
			}, nil
		default:
			return nil, fmt.Errorf("unknown syntax_element helper %q", request.Helper)
		}

	case "reference_syntax":
		host := buildHost(request)
		parsed := parseSourceFileConfig(request, host, request.ConfigFileName, request.JSONText, nil)
		diagnostic := tsoptions.CreateDiagnosticAtReferenceSyntax(
			parsed, request.Index, diagnostics.Compiler_option_0_cannot_be_given_an_empty_string, "reference.path")
		if diagnostic == nil {
			return map[string]any{"reported": false}, nil
		}
		return map[string]any{"reported": true, "diagnostic": renderDiagnostic(diagnostic)}, nil

	default:
		return nil, fmt.Errorf("unknown configParse action %q", request.Action)
	}
}

func TestPhase1ConfigParse(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []configParseRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != configParseSubject {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the config-parsing probe"
			observations = append(observations, row)
			continue
		}
		observation, err := observeConfigParse(t, request)
		if err != nil {
			row["result"] = "harness_failed"
			row["error"] = fmt.Sprintf("%s: %v", request.Case, err)
			observations = append(observations, row)
			continue
		}
		row["result"] = "observed"
		row["observation"] = observation
		observations = append(observations, row)
	}

	hash := sha256.Sum256(input)
	output := map[string]any{
		"request_sha256": hex.EncodeToString(hash[:]),
		"go":             runtime.Version(),
		"goos":           runtime.GOOS,
		"goarch":         runtime.GOARCH,
		"version":        1,
		"observations":   observations,
	}
	encoded, err := stdjson.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(encoded, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
