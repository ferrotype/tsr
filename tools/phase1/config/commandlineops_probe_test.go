package tsoptions_test

// Phase 1 F3a, access only: the COMMAND-LINE and RESULT half of
// `internal/tsoptions` -- `parsedcommandline.go`, `commandlineparser.go`,
// `errors.go`, `commandlineoption.go`, `namemap.go`, `wildcarddirectories.go`,
// `enummaps.go`, `diagnostics.go` and `parsedbuildcommandline.go`.
//
// This is the operation-level surface. It does not overlap the
// `commandLineBaseline` subject, which owns the 80 frozen
// `tsoptions/commandLineParsing` OUTPUTS and renders them through the pinned
// `formatNewBaseline`: nothing here reads a baseline file, and no case here
// claims `tsoptions.parseCommandLineBaseline`.
//
// Shape. Every request is a trace: an ordered list of `actions`, one elementary
// pinned call each, answered in request order under `ordered`. Ordered results
// -- an option map's insertion order, an enum map's key order, a file-name list
// -- travel as arrays of arrays, never as JSON objects, because the comparison
// canonicalises with sorted keys.
//
// What is recorded and what is not. Diagnostics are recorded as their numeric
// code followed by their `MessageArgs()`, never as rendered text: the code and
// the arguments are the pinned contract, the English sentence is message data
// that a rewording may change without changing behaviour. The same rule governs
// the two refusals this surface has -- `parseOptionValue`'s
// `panic("listOrElement not supported here")` (commandlineparser.go:328) and
// `ParseListTypeOption`'s `panic("List of ... is not yet supported.")` (:378) --
// which are recorded as `refused: true` and not as their wording. No host path,
// temporary directory or timestamp enters an observation: every filesystem in
// this file is a `vfstest` map built from the request's own absolute paths.
//
// Inputs are never read back from an expected value. The only pinned data this
// file reads are the option declaration tables themselves
// (`tsoptions.OptionsDeclarations`, `OptionsForWatch`, `BuildOpts`,
// `OptionsForBuild`, and the root map through the in-package companion), which
// are the subject under test, not an expectation.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"os"
	"reflect"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

const commandLineOpsSubject = "commandLine"

type commandLineOpsAction struct {
	Op string `json:"op"`
	// Argument vectors for the two parser entry points.
	Args []string `json:"args"`
	// Which declaration table an option-level action reads.
	Table string `json:"table"`
	Name  string `json:"name"`
	Value string `json:"value"`
	// Lookup control for the name-map actions.
	AllowShort bool `json:"allowShort"`
	// Spec lists for the wildcard-directory actions.
	Include []string `json:"include"`
	Exclude []string `json:"exclude"`
	// A raw `core.ScriptTarget` for the default-lib action. The integer is an
	// input: the pinned constants are `ScriptTarget(int32)` values
	// (core/compileroptions.go:506-527) and the port's are `ScriptTarget(pub
	// i32)` (crates/tsr_core/src/lib.rs:114), so the same integer names the
	// same target on both sides without either side restating the table.
	Target int32 `json:"target"`
	// Declarations for the worker-diagnostics action, built from exported
	// fields only.
	Declarations []commandLineOpsDeclaration `json:"declarations"`
	// Which `ParsedCommandLine` accessor a `parsed_config` action drives.
	Probe string `json:"probe"`
	// Compiler option names to read off a parse result, by their JSON name.
	Options []string `json:"options"`
}

type commandLineOpsDeclaration struct {
	Name              string `json:"name"`
	ShortName         string `json:"shortName"`
	Kind              string `json:"kind"`
	IsFilePath        bool   `json:"isFilePath"`
	IsTSConfigOnly    bool   `json:"isTSConfigOnly"`
	IsCommandLineOnly bool   `json:"isCommandLineOnly"`
}

type commandLineOpsRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// The filesystem every parse in this request sees, and the directory it is
	// rooted at. Absolute paths only: the pinned `vfstest` filesystem refuses a
	// relative path by panicking, which F3a already recorded for the host group.
	Files            map[string]string      `json:"files"`
	CurrentDirectory string                 `json:"currentDirectory"`
	CaseSensitive    bool                   `json:"caseSensitive"`
	JSONText         string                 `json:"jsonText"`
	Actions          []commandLineOpsAction `json:"actions"`
}

// ---------------------------------------------------------------------------
// Rendering helpers. Everything below turns a pinned result into scalars,
// arrays of scalars, or arrays of arrays. No JSON object is ever used to carry
// ordered data.
// ---------------------------------------------------------------------------

// commandLineOpsValue renders a value that crossed the parser's `any`
// boundary. The parser stores exactly five shapes: nil, bool, int, string and
// `[]any` (commandlineparser.go:249-338). Anything else is reported as an
// unrecognised shape rather than guessed at.
func commandLineOpsValue(value any) any {
	switch typed := value.(type) {
	case nil:
		return nil
	case bool:
		return typed
	case int:
		return typed
	case string:
		return typed
	case []string:
		rendered := make([]any, 0, len(typed))
		for _, item := range typed {
			rendered = append(rendered, item)
		}
		return rendered
	case []any:
		rendered := make([]any, 0, len(typed))
		for _, item := range typed {
			rendered = append(rendered, commandLineOpsValue(item))
		}
		return rendered
	default:
		// `convertJsonOptionOfEnumType` stores the enum map's own value, which
		// is a named `core` type over an int or a string
		// (commandlineparser.go:406-408). Its underlying kind is read here for
		// the same reason as in commandLineOpsEnumEntries: a `MarshalJSON` on
		// the named type must not rename what the parser stored.
		reflected := reflect.ValueOf(value)
		switch reflected.Kind() {
		case reflect.Bool:
			return reflected.Bool()
		case reflect.String:
			return reflected.String()
		case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
			return reflected.Int()
		default:
			return "unrecognised-value-shape:" + reflect.TypeOf(value).String()
		}
	}
}

// commandLineOpsDiagnostic renders one diagnostic as its numeric code, its text
// range and then its arguments. The rendered sentence is deliberately absent:
// the code, the range and the arguments are the pinned contract, the English
// wording is message data. The range is what separates a diagnostic built by
// CreateDiagnosticForNodeInSourceFile (errors.go:92), which skips trivia to the
// node it names, from a compiler diagnostic, which both the pin
// (ast/diagnostic.go:240, core.UndefinedTextRange) and the port
// (crates/tsr_ast/src/diagnostic.rs:85, TextRange::new(-1, -1)) place at -1/-1.
func commandLineOpsDiagnostic(diagnostic *ast.Diagnostic) []any {
	row := []any{int(diagnostic.Code()), diagnostic.Pos(), diagnostic.End()}
	for _, argument := range diagnostic.MessageArgs() {
		row = append(row, argument)
	}
	return row
}

func commandLineOpsDiagnostics(diagnostics []*ast.Diagnostic) []any {
	rows := make([]any, 0, len(diagnostics))
	for _, diagnostic := range diagnostics {
		rows = append(rows, commandLineOpsDiagnostic(diagnostic))
	}
	return rows
}

func commandLineOpsStrings(values []string) []any {
	rendered := make([]any, 0, len(values))
	for _, value := range values {
		rendered = append(rendered, value)
	}
	return rendered
}

// commandLineOpsEnumEntries renders an option's enum map in the pin's own key
// order. The value is read through reflection on its underlying kind so that no
// `MarshalJSON` on a `core` enum can rename it: the port carries the same table
// as `(&str, EnumValue)` pairs of a string or an i32
// (crates/tsr_tsoptions/src/option_declarations.rs:26-29), and these are the
// two shapes compared.
func commandLineOpsEnumEntries(option *tsoptions.CommandLineOption) []any {
	rows := []any{}
	enumMap := option.EnumMap()
	if enumMap == nil {
		// The pin returns nil for a non-enum option (commandlineoption.go:86-91)
		// and the port carries an empty `enum_values` slice
		// (crates/tsr_tsoptions/src/option_declarations.rs:38). Neither side can
		// see the other's distinction, so both render as an empty list and the
		// one pinned consumer of the nil test, formatEnumTypeKeys (errors.go:22),
		// is compared directly in its own action instead.
		return rows
	}
	for key, value := range enumMap.Entries() {
		reflected := reflect.ValueOf(value)
		var rendered any
		switch reflected.Kind() {
		case reflect.String:
			rendered = reflected.String()
		case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
			rendered = reflected.Int()
		default:
			rendered = "unrecognised-enum-value-shape:" + reflected.Kind().String()
		}
		rows = append(rows, []any{key, rendered})
	}
	return rows
}

func commandLineOpsEnumKeys(option *tsoptions.CommandLineOption) []string {
	enumMap := option.EnumMap()
	if enumMap == nil {
		return nil
	}
	return slices.Collect(enumMap.Keys())
}

// commandLineOpsDeprecated reports the deprecated keys of an option as a sorted
// list. `DeprecatedKeys` returns a `*collections.Set[string]`, which has no
// order, so sorting is the only faithful rendering, and a nil set renders as an
// empty list for the reason given at commandLineOpsEnumEntries.
func commandLineOpsDeprecated(option *tsoptions.CommandLineOption) []any {
	deprecated := option.DeprecatedKeys()
	if deprecated == nil {
		return []any{}
	}
	collected := make([]string, 0, deprecated.Len())
	for key := range deprecated.Keys() {
		collected = append(collected, key)
	}
	slices.Sort(collected)
	return commandLineOpsStrings(collected)
}

func commandLineOpsMessageCode(message any) any {
	// A nil `*diagnostics.Message` arrives here as a typed nil, so the value is
	// checked through reflection rather than against the untyped nil.
	reflected := reflect.ValueOf(message)
	if !reflected.IsValid() || reflected.IsNil() {
		return nil
	}
	type coded interface{ Code() int32 }
	if typed, ok := message.(coded); ok {
		return int(typed.Code())
	}
	return nil
}

// ---------------------------------------------------------------------------
// Declaration tables.
// ---------------------------------------------------------------------------

// commandLineOpsTable answers with a pinned declaration slice. `root` is not a
// slice at the pin -- the root options live in an unexported
// `CommandLineOptionNameMap` -- so it is served by name through the in-package
// companion instead.
func commandLineOpsTable(name string) ([]*tsoptions.CommandLineOption, bool) {
	switch name {
	case "compiler":
		return tsoptions.OptionsDeclarations, true
	case "watch":
		return tsoptions.OptionsForWatch, true
	case "build":
		return tsoptions.BuildOpts, true
	case "buildOnly":
		return tsoptions.OptionsForBuild, true
	default:
		return nil, false
	}
}

// commandLineOpsOption resolves one declaration for an option-level action.
func commandLineOpsOption(action commandLineOpsAction) (*tsoptions.CommandLineOption, error) {
	if action.Table == "root" {
		option := tsoptions.Phase1RootOption(action.Name)
		if option == nil {
			return nil, fmt.Errorf("no pinned root option named %q", action.Name)
		}
		return option, nil
	}
	declarations, known := commandLineOpsTable(action.Table)
	if !known {
		return nil, fmt.Errorf("unknown declaration table %q", action.Table)
	}
	// The pin's own lookup rule, so a duplicated lowercase name resolves the
	// way the parser would resolve it (namemap.go:15-28, :35-37).
	option := tsoptions.GetNameMapFromList(declarations).Get(action.Name)
	if option == nil {
		return nil, fmt.Errorf("no option named %q in table %q", action.Name, action.Table)
	}
	return option, nil
}

// ---------------------------------------------------------------------------
// The per-request parse-config cache.
// ---------------------------------------------------------------------------

type commandLineOpsSession struct {
	request commandLineOpsRequest
	parsed  *tsoptions.ParsedCommandLine
}

func (session *commandLineOpsSession) host() *tsoptionstest.VfsParseConfigHost {
	return tsoptionstest.NewVFSParseConfigHost(
		session.request.Files, session.request.CurrentDirectory, session.request.CaseSensitive)
}

func (session *commandLineOpsSession) comparePathsOptions() tspath.ComparePathsOptions {
	return tspath.ComparePathsOptions{
		UseCaseSensitiveFileNames: session.request.CaseSensitive,
		CurrentDirectory:          session.request.CurrentDirectory,
	}
}

// configParse builds the request's `ParsedCommandLine` once and keeps it, so a
// trace can set state with one action and read it back with the next.
func (session *commandLineOpsSession) configParse(t *testing.T) *tsoptions.ParsedCommandLine {
	t.Helper()
	if session.parsed == nil {
		session.parsed = tsoptionstest.GetParsedCommandLine(
			t,
			session.request.JSONText,
			session.request.Files,
			session.request.CurrentDirectory,
			session.request.CaseSensitive,
		)
	}
	return session.parsed
}

// ---------------------------------------------------------------------------
// Actions.
// ---------------------------------------------------------------------------

func commandLineOpsRun(t *testing.T, session *commandLineOpsSession, action commandLineOpsAction) (row map[string]any, err error) {
	row = map[string]any{"op": action.Op}
	switch action.Op {

	case "input_option_name":
		// commandlineparser.go:164.
		row["input"] = action.Value
		row["option_name"] = tsoptions.Phase1GetInputOptionName(action.Value)

	case "option_declaration":
		// commandlineoption.go:79-102, read through the pinned accessors.
		option, resolveErr := commandLineOpsOption(action)
		if resolveErr != nil {
			return nil, resolveErr
		}
		row["name"] = option.Name
		row["short_name"] = option.ShortName
		row["kind"] = string(option.Kind)
		row["is_file_path"] = option.IsFilePath
		row["is_tsconfig_only"] = option.IsTSConfigOnly
		row["is_command_line_only"] = option.IsCommandLineOnly
		row["disallow_null_or_undefined"] = option.DisallowNullOrUndefined()
		element := option.Elements()
		if element == nil {
			row["element_name"] = nil
			row["element_kind"] = nil
			row["element_is_file_path"] = nil
		} else {
			row["element_name"] = element.Name
			row["element_kind"] = string(element.Kind)
			row["element_is_file_path"] = element.IsFilePath
		}
		row["enum_entries"] = commandLineOpsEnumEntries(option)
		row["deprecated_keys"] = commandLineOpsDeprecated(option)

	case "option_value_type_string":
		// errors.go:28.
		option, resolveErr := commandLineOpsOption(action)
		if resolveErr != nil {
			return nil, resolveErr
		}
		row["name"] = option.Name
		row["value_type_string"] = tsoptions.Phase1CompilerOptionValueTypeString(option)

	case "format_enum_type_keys":
		// errors.go:21, given the option's own key order.
		option, resolveErr := commandLineOpsOption(action)
		if resolveErr != nil {
			return nil, resolveErr
		}
		row["name"] = option.Name
		row["formatted"] = tsoptions.Phase1FormatEnumTypeKeys(option, commandLineOpsEnumKeys(option))

	case "invalid_enum_type_diagnostic":
		// errors.go:14 with the nil source file and node the parser passes.
		option, resolveErr := commandLineOpsOption(action)
		if resolveErr != nil {
			return nil, resolveErr
		}
		row["name"] = option.Name
		row["diagnostic"] = commandLineOpsDiagnostic(tsoptions.Phase1InvalidEnumTypeDiagnostic(option))

	case "extra_key_diagnostics":
		// errors.go:103 and :118.
		unknown, didYouMean := tsoptions.Phase1ExtraKeyDiagnostics(action.Value)
		row["parent"] = action.Value
		row["unknown_code"] = commandLineOpsMessageCode(unknown)
		row["did_you_mean_code"] = commandLineOpsMessageCode(didYouMean)

	case "worker_diagnostics":
		// diagnostics.go:30.
		declarations := make([]*tsoptions.CommandLineOption, 0, len(action.Declarations))
		for _, declared := range action.Declarations {
			declarations = append(declarations, &tsoptions.CommandLineOption{
				Name:              declared.Name,
				ShortName:         declared.ShortName,
				Kind:              tsoptions.CommandLineOptionKind(declared.Kind),
				IsFilePath:        declared.IsFilePath,
				IsTSConfigOnly:    declared.IsTSConfigOnly,
				IsCommandLineOnly: declared.IsCommandLineOnly,
			})
		}
		mismatch, unknown, didYouMean, alternate, alternateHasMap, count :=
			tsoptions.Phase1WorkerDiagnostics(declarations)
		row["option_type_mismatch_code"] = commandLineOpsMessageCode(mismatch)
		row["unknown_option_code"] = commandLineOpsMessageCode(unknown)
		row["unknown_did_you_mean_code"] = commandLineOpsMessageCode(didYouMean)
		row["alternate_mode_code"] = commandLineOpsMessageCode(alternate)
		row["alternate_mode_has_name_map"] = alternateHasMap
		row["declaration_count"] = count

	case "name_map":
		// namemap.go:15, :35 and :48 over a pinned table.
		declarations, known := commandLineOpsTable(action.Table)
		if !known {
			return nil, fmt.Errorf("unknown declaration table %q", action.Table)
		}
		nameMap := tsoptions.GetNameMapFromList(declarations)
		row["lookup"] = action.Name
		row["allow_short"] = action.AllowShort
		if found := nameMap.Get(action.Name); found != nil {
			row["get"] = found.Name
		} else {
			row["get"] = nil
		}
		if found := nameMap.GetOptionDeclarationFromName(action.Name, action.AllowShort); found != nil {
			row["get_option_declaration_from_name"] = found.Name
		} else {
			row["get_option_declaration_from_name"] = nil
		}

	case "parse_list_type_option":
		// commandlineparser.go:347.
		option, resolveErr := commandLineOpsOption(action)
		if resolveErr != nil {
			return nil, resolveErr
		}
		row["name"] = option.Name
		row["value"] = action.Value
		func() {
			defer func() {
				if recovered := recover(); recovered != nil {
					// The FACT of the refusal, not its wording.
					row["refused"] = true
					row["result"] = nil
					row["errors"] = nil
				}
			}()
			row["refused"] = false
			result, errors := tsoptions.ParseListTypeOption(option, action.Value)
			row["result"] = commandLineOpsValue(result)
			row["result_is_nil"] = result == nil
			row["errors"] = commandLineOpsDiagnostics(errors)
		}()

	case "lib_file_name":
		// enummaps.go:132.
		name, ok := tsoptions.GetLibFileName(action.Value)
		row["input"] = action.Value
		row["found"] = ok
		if ok {
			row["file_name"] = name
		} else {
			row["file_name"] = nil
		}

	case "default_lib_file_name":
		// enummaps.go:226.
		row["target"] = int(action.Target)
		row["file_name"] = tsoptions.GetDefaultLibFileName(
			&core.CompilerOptions{Target: core.ScriptTarget(action.Target)})

	case "canonical_key":
		// wildcarddirectories.go:85.
		row["input"] = action.Value
		row["key"] = tsoptions.Phase1ToCanonicalKey(action.Value, session.request.CaseSensitive)

	case "wildcard_directory_from_spec":
		// wildcarddirectories.go:99.
		key, path, recursive, matched := tsoptions.Phase1WildcardDirectoryFromSpec(
			action.Value, session.request.CaseSensitive)
		row["spec"] = action.Value
		row["matched"] = matched
		row["key"] = key
		row["path"] = path
		row["recursive"] = recursive

	case "wildcard_directories":
		// wildcarddirectories.go:10. The pinned result is a Go map, which has
		// no order, so the entries are sorted by directory. The insertion order
		// the TypeScript object carried is not recoverable from the returned
		// value and is not invented here.
		directories := tsoptions.Phase1GetWildcardDirectories(
			action.Include, action.Exclude, session.comparePathsOptions())
		row["include"] = commandLineOpsStrings(action.Include)
		row["exclude"] = commandLineOpsStrings(action.Exclude)
		row["is_nil"] = directories == nil
		names := make([]string, 0, len(directories))
		for name := range directories {
			names = append(names, name)
		}
		slices.Sort(names)
		entries := []any{}
		for _, name := range names {
			entries = append(entries, []any{name, directories[name]})
		}
		row["directories"] = entries

	case "parse_command_line":
		// commandlineparser.go:43.
		parsed := tsoptions.ParseCommandLine(action.Args, session.host())
		row["args"] = commandLineOpsStrings(action.Args)
		row["file_names"] = commandLineOpsStrings(parsed.FileNames())
		row["errors"] = commandLineOpsDiagnostics(parsed.Errors)
		raw, ok := parsed.Raw.(*collections.OrderedMap[string, any])
		if !ok {
			return nil, fmt.Errorf("ParseCommandLine returned an unrecognised Raw shape")
		}
		entries := []any{}
		for key, value := range raw.Entries() {
			entries = append(entries, []any{key, commandLineOpsValue(value)})
		}
		row["raw"] = entries
		selected, selectErr := commandLineOpsSelectOptions(parsed.CompilerOptions(), action.Options)
		if selectErr != nil {
			return nil, selectErr
		}
		row["compiler_options"] = selected
		row["current_directory"] = parsed.GetCurrentDirectory()
		row["use_case_sensitive_file_names"] = parsed.UseCaseSensitiveFileNames()

	case "parse_build_command_line":
		// commandlineparser.go:64.
		parsed := tsoptions.ParseBuildCommandLine(action.Args, session.host())
		row["args"] = commandLineOpsStrings(action.Args)
		row["projects"] = commandLineOpsStrings(parsed.Projects)
		row["resolvedProjects"] = commandLineOpsStrings(parsed.ResolvedProjectPaths())
		row["errors"] = commandLineOpsDiagnostics(parsed.Errors)
		raw, ok := parsed.Raw.(*collections.OrderedMap[string, any])
		if !ok {
			return nil, fmt.Errorf("ParseBuildCommandLine returned an unrecognised Raw shape")
		}
		entries := []any{}
		for key, value := range raw.Entries() {
			entries = append(entries, []any{key, commandLineOpsValue(value)})
		}
		row["raw"] = entries
		selected, selectErr := commandLineOpsSelectOptions(parsed.CompilerOptions, action.Options)
		if selectErr != nil {
			return nil, selectErr
		}
		row["compiler_options"] = selected
		// parsedbuildcommandline.go:40. Only whether the locale parsed to the
		// package default is recorded: the tag's canonical spelling belongs to
		// golang.org/x/text, not to this port.
		row["locale_is_default"] = parsed.Locale() == locale.Default

	case "parsed_config":
		return commandLineOpsConfigProbe(t, session, action)

	default:
		return nil, fmt.Errorf("unknown command-line action %q", action.Op)
	}
	return row, nil
}

// commandLineOpsSelectOptions reads the named compiler options off a parse
// result. The names are the pinned struct's own JSON names, so the selection
// carries no restated mapping.
func commandLineOpsSelectOptions(options *core.CompilerOptions, names []string) ([]any, error) {
	if len(names) == 0 {
		return []any{}, nil
	}
	encoded, err := stdjson.Marshal(options)
	if err != nil {
		return nil, err
	}
	var decoded map[string]any
	if err := stdjson.Unmarshal(encoded, &decoded); err != nil {
		return nil, err
	}
	rows := []any{}
	for _, name := range names {
		value, present := decoded[name]
		if !present {
			rows = append(rows, []any{name, nil, false})
			continue
		}
		rows = append(rows, []any{name, value, true})
	}
	return rows, nil
}

// commandLineOpsConfigProbe drives one accessor of the request's
// `ParsedCommandLine`, built from the request's tsconfig text.
func commandLineOpsConfigProbe(t *testing.T, session *commandLineOpsSession, action commandLineOpsAction) (map[string]any, error) {
	t.Helper()
	parsed := session.configParse(t)
	row := map[string]any{"op": action.Op, "probe": action.Probe}
	probe, argument, _ := strings.Cut(action.Probe, ":")
	switch probe {

	case "config_name":
		// parsedcommandline.go:120.
		row["config_name"] = parsed.ConfigName()

	case "file_names":
		row["file_names"] = commandLineOpsStrings(parsed.FileNames())

	case "file_names_by_path":
		// parsedcommandline.go:334. A map, so the entries are sorted by path.
		byPath := parsed.FileNamesByPath()
		keys := make([]string, 0, len(byPath))
		for key := range byPath {
			keys = append(keys, string(key))
		}
		slices.Sort(keys)
		entries := []any{}
		for _, key := range keys {
			entries = append(entries, []any{key, byPath[tspath.Path(key)]})
		}
		row["file_names_by_path"] = entries

	case "config_file_parsing_diagnostics":
		// parsedcommandline.go:393.
		row["diagnostics"] = commandLineOpsDiagnostics(parsed.GetConfigFileParsingDiagnostics())

	case "current_directory":
		// parsedcommandline.go:189 and :193.
		row["current_directory"] = parsed.GetCurrentDirectory()
		row["use_case_sensitive_file_names"] = parsed.UseCaseSensitiveFileNames()

	case "matched_file_spec":
		// parsedcommandline.go:449.
		row["file"] = argument
		row["matched"] = parsed.GetMatchedFileSpec(argument)

	case "matched_include_spec":
		// parsedcommandline.go:453.
		spec, isDefault := parsed.GetMatchedIncludeSpec(argument)
		row["file"] = argument
		row["matched"] = spec
		row["is_default_include"] = isDefault

	case "wildcard_directories":
		// parsedcommandline.go:258, which is the only pinned caller of
		// getWildcardDirectories. Sorted, for the reason given above.
		directories := parsed.WildcardDirectories()
		row["is_nil"] = directories == nil
		names := make([]string, 0, len(directories))
		for name := range directories {
			names = append(names, name)
		}
		slices.Sort(names)
		entries := []any{}
		for _, name := range names {
			entries = append(entries, []any{name, directories[name]})
		}
		row["directories"] = entries

	case "wildcard_directory_globs":
		// parsedcommandline.go:276, whose only job beyond the accessor above is
		// to run fileGlobPatterns (:31) and parse the result. The glob objects
		// are opaque, so their count and their sorted source patterns are what
		// is recorded.
		globs := parsed.WildcardDirectoryGlobs()
		patterns := make([]string, 0, len(globs))
		for _, parsedGlob := range globs {
			patterns = append(patterns, fmt.Sprint(parsedGlob))
		}
		slices.Sort(patterns)
		row["glob_count"] = len(globs)
		row["patterns"] = commandLineOpsStrings(patterns)

	case "extended_source_files":
		// parsedcommandline.go:386.
		row["extended_source_files"] = commandLineOpsStrings(parsed.ExtendedSourceFiles())

	case "project_references":
		// parsedcommandline.go:345 and :379.
		references := parsed.ProjectReferences()
		rows := []any{}
		for _, reference := range references {
			rows = append(rows, []any{reference.Path, reference.OriginalPath, reference.Circular})
		}
		row["project_references"] = rows
		row["resolved_paths"] = commandLineOpsStrings(parsed.ResolvedProjectReferencePaths())

	case "common_source_directory":
		// parsedcommandline.go:157, which runs checkSourceFilesBelongToPath
		// (:176) whenever a rootDir or a config file path is present. That
		// check appends to `Errors`, so the error list is read afterwards and
		// the growth is reported.
		before := len(parsed.Errors)
		row["common_source_directory"] = parsed.CommonSourceDirectory()
		row["errors_added"] = len(parsed.Errors) - before
		row["errors"] = commandLineOpsDiagnostics(parsed.Errors[before:])

	case "build_info_file_name":
		// parsedcommandline.go:253.
		row["build_info_file_name"] = parsed.GetBuildInfoFileName()

	case "input_output_names":
		// parsedcommandline.go:135, which runs
		// getOutputDeclarationAndSourceFileNames (:197) and fills both maps.
		parsed.ParseInputOutputNames()
		row["source_to_project_reference"] = commandLineOpsReferenceMap(parsed.SourceToProjectReference())
		row["output_dts_to_project_reference"] = commandLineOpsReferenceMap(parsed.OutputDtsToProjectReference())

	case "content_mappers":
		// parsedcommandline.go:349, :358 and :366.
		mappers := parsed.ContentMappers()
		row["mapper_count"] = len(mappers)
		row["extensions"] = commandLineOpsStrings(parsed.ContentMapperExtensions())
		row["mapper_for_file"] = argument
		row["has_mapper_for_file"] = argument != "" && parsed.GetContentMapperForFileName(argument) != nil

	case "type_acquisition":
		// parsedcommandline.go:321 and :325.
		parsed.SetTypeAcquisition(&core.TypeAcquisition{Enable: core.TSTrue})
		acquisition := parsed.TypeAcquisition()
		row["set_then_read_enable"] = acquisition != nil && acquisition.Enable == core.TSTrue

	case "set_compiler_options":
		// parsedcommandline.go:310 and :489. The locale is supplied by the
		// request and only its default-ness is recorded, for the reason given
		// at the build action.
		parsed.SetCompilerOptions(&core.CompilerOptions{Locale: argument})
		row["locale_input"] = argument
		row["locale_is_default"] = parsed.Locale() == locale.Default
		row["file_names_survived"] = len(parsed.FileNames())

	case "set_parsed_options":
		// parsedcommandline.go:306, with a wholly new ParsedOptions.
		parsed.SetParsedOptions(&tsoptions.ParsedOptions{
			CompilerOptions: &core.CompilerOptions{},
			FileNames:       []string{argument},
		})
		row["file_names"] = commandLineOpsStrings(parsed.FileNames())

	default:
		return nil, fmt.Errorf("unknown parsed-config probe %q", action.Probe)
	}
	return row, nil
}

func commandLineOpsReferenceMap(references map[tspath.Path]*tsoptions.SourceOutputAndProjectReference) []any {
	keys := make([]string, 0, len(references))
	for key := range references {
		keys = append(keys, string(key))
	}
	slices.Sort(keys)
	rows := []any{}
	for _, key := range keys {
		reference := references[tspath.Path(key)]
		rows = append(rows, []any{key, reference.Source, reference.OutputDts})
	}
	return rows
}

// ---------------------------------------------------------------------------
// Driver.
// ---------------------------------------------------------------------------

func TestPhase1ConfigCommandLineOps(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	// Decode the schedule one request at a time, and decode a request's BODY
	// only once its subject says it is ours. Every probe in a family sees the
	// whole schedule, so decoding all of it up front makes one group's action
	// shape able to break another group's probe -- which is exactly what
	// happened here: `configParse` carries `actions[].options` as an object and
	// this group declares it as []string, so the unmarshal failed on a request
	// this probe never intended to answer, before the subject check could
	// decline it.
	var document struct {
		Requests []stdjson.RawMessage `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, raw := range document.Requests {
		var header struct {
			Case      string `json:"case"`
			Operation string `json:"operation"`
			Subject   string `json:"subject"`
		}
		if err := stdjson.Unmarshal(raw, &header); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": header.Case, "operation": header.Operation}
		if header.Subject != commandLineOpsSubject {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the command-line operations probe"
			observations = append(observations, row)
			continue
		}
		var request commandLineOpsRequest
		if err := stdjson.Unmarshal(raw, &request); err != nil {
			t.Fatalf("%s: %v", header.Case, err)
		}
		session := &commandLineOpsSession{request: request}
		trace := make([]any, 0, len(request.Actions))
		var failure error
		for _, action := range request.Actions {
			answered, actionErr := commandLineOpsRun(t, session, action)
			if actionErr != nil {
				failure = fmt.Errorf("%s: %w", action.Op, actionErr)
				break
			}
			trace = append(trace, answered)
		}
		if failure != nil {
			row["result"] = "harness_failed"
			row["error"] = failure.Error()
		} else {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": trace}
		}
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
