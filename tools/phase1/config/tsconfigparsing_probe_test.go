package tsoptions_test

// Test-only bridge for the 87 pinned config baselines. Inputs are exported
// directly from the native tables, except the 16 function-local acquisition
// cases whose input-only Fs prefix is recovered as in F3a. Native witnesses
// authenticate those inputs and the exact envelope bytes. Rust supplies every
// result field and diagnostic byte to the same pure envelope function.
import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

const (
	tsconfigParsingOperation = "tsoptions.tsconfigParsingBaseline"
	tsconfigFsHead           = "Fs::\n"
	tsconfigNameHead         = "\nconfigFileName:: "
	tsconfigEntryMark        = "//// ["
	typeAcquisitionBasePath  = "/apath"
)

type configInput struct {
	ConfigFileName string            `json:"config_file_name"`
	BasePath       string            `json:"base_path"`
	JSONText       string            `json:"json_text"`
	AllFileList    map[string]string `json:"all_file_list"`
	Locale         string            `json:"locale,omitempty"`
}
type tsconfigParsingRequest struct {
	Case                   string        `json:"case"`
	Operation              string        `json:"operation"`
	Baseline               string        `json:"baseline"`
	API                    string        `json:"api"`
	Source                 string        `json:"source"`
	Title                  string        `json:"title"`
	IncludeCompilerOptions bool          `json:"include_compiler_options"`
	Inputs                 []configInput `json:"inputs"`
}

func configInputs(request tsconfigParsingRequest, expected string) (bool, []configInput, error) {
	if request.API == "jsonParse" {
		texts, ok := parseConfigFileTextToJsonInputs(request.Title)
		if !ok {
			return false, nil, fmt.Errorf("missing pinned title %s", request.Title)
		}
		result := []configInput{}
		for _, text := range texts {
			result = append(result, configInput{JSONText: text})
		}
		return false, result, nil
	}
	var configs []testConfig
	var include bool
	switch request.Source {
	case "table":
		entry, ok := tsconfigParsingEntry(request.Title)
		if !ok {
			return false, nil, fmt.Errorf("missing pinned title %s", request.Title)
		}
		configs = entry.input
		include = entry.includeCompilerOptions
	case "recovered":
		config, err := recoverTypeAcquisitionInput(expected)
		if err != nil {
			return false, nil, err
		}
		configs = []testConfig{config}
		include = true
	default:
		return false, nil, fmt.Errorf("unknown input source")
	}
	result := []configInput{}
	for _, c := range configs {
		// None of these 87 frozen inputs passes existing options. Refuse a
		// pin change rather than silently eliding a new input.
		if c.existingOptions != nil {
			return false, nil, fmt.Errorf("new existingOptions input needs transport")
		}
		result = append(result, configInput{ConfigFileName: c.configFileName, BasePath: c.basePath, JSONText: c.jsonText, AllFileList: c.allFileList})
	}
	return include, result, nil
}
func configHost(input configInput) (string, tsoptions.ParseConfigHost) {
	base := input.BasePath
	if base == "" {
		base = tspath.GetNormalizedAbsolutePath(tspath.GetDirectoryPath(input.ConfigFileName), "")
	}
	files := map[string]string{}
	maps.Copy(files, input.AllFileList)
	files[tspath.CombinePaths(base, input.ConfigFileName)] = input.JSONText
	return base, tsoptionstest.NewVFSParseConfigHost(files, input.BasePath, true)
}
func configDiagnostics(errors []*ast.Diagnostic) []any {
	result := []any{}
	for _, d := range errors {
		var file any
		if d.File() != nil {
			file = hex.EncodeToString([]byte(d.File().FileName()))
		}
		result = append(result, map[string]any{
			"code": d.Code(), "pos": d.Pos(), "end": d.End(), "category": d.Category(), "file": file,
			"args":  bridgeWire(append([]string{}, d.MessageArgs()...)),
			"chain": configDiagnostics(d.MessageChain()), "related": configDiagnostics(d.RelatedInformation()),
		})
	}
	return result
}
func configObserve(request tsconfigParsingRequest) []map[string]any {
	result := []map[string]any{}
	for _, input := range request.Inputs {
		var errors []*ast.Diagnostic
		row := map[string]any{}
		format := &diagnosticwriter.FormattingOptions{NewLine: "\n", ComparePathsOptions: tspath.ComparePathsOptions{CurrentDirectory: "/", UseCaseSensitiveFileNames: true}}
		if request.API == "jsonParse" {
			var value any
			value, errors = tsoptions.ParseConfigFileTextToJson("/apath/tsconfig.json", "/apath", input.JSONText)
			row["raw"] = bridgeWire(value)
		} else {
			base, host := configHost(input)
			config := testConfig{configFileName: input.ConfigFileName, basePath: input.BasePath, jsonText: input.JSONText, allFileList: input.AllFileList}
			if input.Locale != "" {
				config.existingOptions = &core.CompilerOptions{Locale: input.Locale}
			}
			var parsed *tsoptions.ParsedCommandLine
			switch request.API {
			case "json":
				parsed = getParsedWithJsonApi(config, host, base)
			case "jsonSourceFile":
				parsed = getParsedWithJsonSourceFileApi(config, host, base)
			default:
				panic("unknown config API")
			}
			row["compiler"] = bridgeWire(parsed.ParsedConfig.CompilerOptions)
			row["acquisition"] = bridgeWire(parsed.ParsedConfig.TypeAcquisition)
			// A filename result is a sequence, unlike option values whose
			// nil/empty presence states affect serialization.
			row["files"] = bridgeWire(append([]string{}, parsed.ParsedConfig.FileNames...))
			row["raw"] = bridgeWire(parsed.Raw)
			errors = parsed.Errors
			format.NewLine = "\r\n"
			format.CurrentDirectory = base
			if input.Locale != "" {
				format.Locale = parsed.Locale()
			}
		}
		var text strings.Builder
		diagnosticwriter.FormatDiagnosticsWithColorAndContext(&text, diagnosticwriter.FromASTDiagnostics(errors), format)
		row["errors"] = hex.EncodeToString([]byte(text.String()))
		row["diagnostics"] = configDiagnostics(errors)
		result = append(result, row)
	}
	return result
}

// The section sequence is the pinned baselineParseConfigWith and the inline
// TestParseConfigFileTextToJson assembly. Only result acquisition is separated:
// no call below parses a config or formats a diagnostic.
func configRender(request tsconfigParsingRequest, typed map[string]any) map[string]any {
	if len(typed) != 1 {
		panic("unknown config renderer fields")
	}
	rows, ok := typed["rows"].([]any)
	if !ok || len(rows) != len(request.Inputs) {
		panic("config renderer schedule")
	}
	var b strings.Builder
	for i, input := range request.Inputs {
		row, ok := rows[i].(map[string]any)
		if !ok {
			panic("invalid config result")
		}
		keys := []string{"raw", "errors", "diagnostics"}
		if request.API != "jsonParse" {
			keys = append(keys, "compiler", "acquisition", "files")
		}
		if len(row) != len(keys) {
			panic("wrong config renderer field count")
		}
		for _, k := range keys {
			if _, ok := row[k]; !ok {
				panic("missing config renderer field " + k)
			}
		}
		if request.API == "jsonParse" {
			b.WriteString("Input::\n")
			b.WriteString(input.JSONText)
			b.WriteString("\nConfig::\n")
			if err := writeJsonReadableText(&b, bridgeDecode(row["raw"])); err != nil {
				panic(err)
			}
			b.WriteString("\n")
		} else {
			_, host := configHost(input)
			b.WriteString("Fs::\n")
			if err := printFS(&b, host.FS(), "/"); err != nil {
				panic(err)
			}
			b.WriteString("\nconfigFileName:: ")
			b.WriteString(input.ConfigFileName)
			b.WriteString("\n")
			var options core.CompilerOptions
			bridgeAssign(reflect.ValueOf(&options).Elem(), bridgeDecode(row["compiler"]))
			var acquisition *core.TypeAcquisition
			bridgeAssign(reflect.ValueOf(&acquisition).Elem(), bridgeDecode(row["acquisition"]))
			if request.IncludeCompilerOptions {
				b.WriteString("CompilerOptions::\n")
				if err := json.MarshalIndentWrite(&b, &options, "", "  "); err != nil {
					panic(err)
				}
				b.WriteString("\n\n")
				if acquisition != nil {
					b.WriteString("TypeAcquisition::\n")
					if err := json.MarshalIndentWrite(&b, acquisition, "", "  "); err != nil {
						panic(err)
					}
					b.WriteString("\n\n")
				}
			}
			var files []string
			bridgeAssign(reflect.ValueOf(&files).Elem(), bridgeDecode(row["files"]))
			b.WriteString("FileNames::\n")
			b.WriteString(strings.Join(files, ","))
			b.WriteString("\n")
		}
		b.WriteString("Errors::\n")
		b.Write(bridgeBytes(row["errors"]))
		b.WriteString("\n")
		if i != len(rows)-1 {
			b.WriteString("\n")
		}
	}
	rendered := b.String()
	sum := sha256.Sum256([]byte(rendered))
	return map[string]any{"baseline": request.Baseline, "typed": typed, "rendered": rendered, "rendered_sha256": hex.EncodeToString(sum[:])}
}
func init() {
	bridgeRenderers[tsconfigParsingOperation] = func(raw stdjson.RawMessage, typed map[string]any) map[string]any {
		var r tsconfigParsingRequest
		if err := stdjson.Unmarshal(raw, &r); err != nil {
			panic(err)
		}
		return configRender(r, typed)
	}
}

func configDocument(t *testing.T) ([]byte, []tsconfigParsingRequest) {
	t.Helper()
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var d struct {
		Requests []tsconfigParsingRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &d); err != nil {
		t.Fatal(err)
	}
	return input, d.Requests
}
func configExpected(t *testing.T, r tsconfigParsingRequest) []byte {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(repo.TestDataPath(), "baselines", "reference", filepath.FromSlash(r.Baseline)))
	if err != nil {
		t.Fatal(err)
	}
	return b
}
func TestPhase1ConfigInputs(t *testing.T) {
	input, requests := configDocument(t)
	for i, r := range requests {
		include, inputs, err := configInputs(r, string(configExpected(t, r)))
		if err != nil {
			t.Fatal(err)
		}
		requests[i].Inputs = inputs
		requests[i].IncludeCompilerOptions = include
	}
	sum := sha256.Sum256(input)
	b, err := stdjson.MarshalIndent(map[string]any{"requests": requests, "request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), b, 0600); err != nil {
		t.Fatal(err)
	}
}
func TestPhase1ConfigTsconfigParsing(t *testing.T) {
	input, requests := configDocument(t)
	rows := []map[string]any{}
	for _, r := range requests {
		row := map[string]any{"case": r.Case, "operation": r.Operation}
		if r.Operation != tsconfigParsingOperation {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the tsconfigParsing baseline probe"
			rows = append(rows, row)
			continue
		}
		expected := configExpected(t, r)
		include, inputs, err := configInputs(r, string(expected))
		if err != nil {
			t.Fatal(err)
		}
		if include != r.IncludeCompilerOptions || !reflect.DeepEqual(inputs, r.Inputs) {
			t.Fatalf("%s: frozen inputs differ from pinned test", r.Case)
		}
		typed := bridgeRoundtrip(map[string]any{"rows": configObserve(r)}).(map[string]any)
		observation := configRender(r, typed)
		if observation["rendered"] != string(expected) {
			t.Fatalf("%s: native renderer changed frozen bytes", r.Case)
		}
		sum := sha256.Sum256(expected)
		row["result"] = "observed"
		row["observation"] = observation
		row["metadata"] = map[string]any{"expected_sha256": hex.EncodeToString(sum[:])}
		rows = append(rows, row)
	}
	sum := sha256.Sum256(input)
	b, err := stdjson.MarshalIndent(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "observations": rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), b, 0600); err != nil {
		t.Fatal(err)
	}
}

// recoverTypeAcquisitionInput rebuilds one `TestParseTypeAcquisition` input
// from the baseline's own input sections. `configFileName::` carries the raw
// config name and the `Fs::` block carries every file the host was built from,
// including the config text itself; the rest of the shape is the pinned
// literal at tsconfigparsing_test.go:1634-1644. It refuses anything it does
// not recognise rather than guessing.
func recoverTypeAcquisitionInput(content string) (testConfig, error) {
	nameAt := strings.Index(content, tsconfigNameHead)
	if !strings.HasPrefix(content, tsconfigFsHead) || nameAt < 0 {
		return testConfig{}, fmt.Errorf("baseline has no Fs:: and configFileName:: input sections")
	}
	configName, _, found := strings.Cut(content[nameAt+len(tsconfigNameHead):], "\n")
	if !found || configName == "" {
		return testConfig{}, fmt.Errorf("configFileName:: section is empty")
	}
	files := map[string]string{}
	body := content[len(tsconfigFsHead):nameAt]
	for _, block := range strings.Split(body, tsconfigEntryMark)[1:] {
		path, rest, ok := strings.Cut(block, "]\r\n")
		if !ok {
			return testConfig{}, fmt.Errorf("Fs:: entry %q has no terminated header", block)
		}
		files[path] = strings.TrimSuffix(rest, "\r\n\r\n")
	}
	configPath := tspath.CombinePaths(typeAcquisitionBasePath, configName)
	jsonText, ok := files[configPath]
	if !ok {
		return testConfig{}, fmt.Errorf("Fs:: does not carry the config file %q", configPath)
	}
	delete(files, configPath)
	return testConfig{
		jsonText:       jsonText,
		configFileName: configName,
		basePath:       typeAcquisitionBasePath,
		allFileList:    files,
	}, nil
}

func tsconfigParsingEntry(title string) (parseJsonConfigTestCase, bool) {
	for _, rec := range parseJsonConfigFileTests {
		if rec.title == title {
			return rec, true
		}
	}
	return parseJsonConfigTestCase{}, false
}

// parseConfigFileTextToJsonInputs reads the pinned package-level
// `parseConfigFileTextToJsonTests` table (tsconfigparsing_test.go:40) by title.
// Its element type is an anonymous struct, so only the input slice is returned.
func parseConfigFileTextToJsonInputs(title string) ([]string, bool) {
	for _, rec := range parseConfigFileTextToJsonTests {
		if rec.title == title {
			return rec.input, true
		}
	}
	return nil, false
}

// F5b integration inputs are separate from the 309 frozen baseline outputs.
// Both runtimes use configRender; all diagnostic bytes come from their own
// production parser and writer. No expected file or translated catalog is read.
func TestPhase1LocalizedConfig(t *testing.T) {
	input, requests := configDocument(t)
	rows := []map[string]any{}
	for _, r := range requests {
		if r.Operation != tsconfigParsingOperation || r.Source != "integration" || (r.API != "json" && r.API != "jsonSourceFile") || len(r.Inputs) != 1 || r.Inputs[0].Locale == "" {
			t.Fatalf("invalid localized integration request %s", r.Case)
		}
		typed := bridgeRoundtrip(map[string]any{"rows": configObserve(r)}).(map[string]any)
		rows = append(rows, map[string]any{"case": r.Case, "operation": r.Operation, "result": "observed", "observation": configRender(r, typed)})
	}
	sum := sha256.Sum256(input)
	b, err := stdjson.MarshalIndent(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "observations": rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), b, 0600); err != nil {
		t.Fatal(err)
	}
}
