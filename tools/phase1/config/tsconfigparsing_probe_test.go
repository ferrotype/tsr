package tsoptions_test

// Phase 1 F3a, access only: the 87 `config/tsconfigParsing` reference outputs.
//
// Like the command-line group and unlike F2a's carried `config/matchFiles`
// envelope, these have a real pinned producer. Unlike the command-line group,
// its assembly is not a reusable function: `baselineParseConfigWith`
// (tsconfigparsing_test.go:1503) ends in `baseline.Run` (:1554), and
// `TestParseConfigFileTextToJson` (:130) assembles its sections inline in the
// test body. Calling either as-is is not an option:
//
//   * `baseline.Run` -> `writeComparison` (baseline.go:41-80) writes under
//     `repo.TestDataPath()/baselines/local`, which is inside the pinned
//     submodule, and calls `t.Errorf` on any difference (:79). A Rust-fed
//     render that diverged would make `go test` exit non-zero and abort the
//     whole capture instead of recording a `different` row -- the comparison
//     would destroy the evidence it exists to produce.
//
// So this file carries the two assemblies, which the plan authorises for
// exactly this case: "Where assembly is inline, carry a minimal reviewed
// test-source patch exposing the same assembly over supplied observations"
// (docs/PHASE1-implementation-plan.md:739-741). What is carried is the section
// *sequencing* and nothing else. Every value in it comes from a pinned call:
//
//   * `Fs::`               -- the pinned `printFS` (:1657), called directly
//   * `configFileName::`   -- the config's own raw file name
//   * `CompilerOptions::`  -- the pinned `internal/json.MarshalIndentWrite`
//   * `TypeAcquisition::`  -- the same, under the pinned nil gate (:1531)
//   * `FileNames::`        -- `strings.Join` of the parse's own file names
//   * `Errors::`           -- the pinned
//                             `diagnosticwriter.FormatDiagnosticsWithColorAndContext`
//   * `Input::`/`Config::` -- the pinned `ParseConfigFileTextToJson` and
//                             `writeJsonReadableText` (:1557)
//
// and the host is the pinned `tsoptionstest.NewVFSParseConfigHost`. The two
// entry points are the pinned `getParsedWithJsonApi` (:952) and
// `getParsedWithJsonSourceFileApi` (:1481), called rather than reimplemented,
// which is why this file compiles into `tsoptions_test`.
//
// The carried sequencing is held to the only standard that makes carrying it
// safe: it must reproduce the untouched pinned bytes. Each row reports
// `rendered_sha256` against the frozen file's `expected_sha256`, and all 87
// are required to be exact -- these outputs are `rendering_verified: true` in
// data/phase1/config-baselines.json, so `exception_problems` refuses an
// exception on any of them.
//
// WHERE THE INPUTS COME FROM. 71 of the 87 read the pinned tables directly:
// `parseJsonConfigFileTests` (:168) and `parseConfigFileTextToJsonTests` (:40)
// are package-level vars in `tsoptions_test`, so the overlay reads the pinned
// data itself and nothing is transcribed. The other 16 cannot be read that
// way: `TestParseTypeAcquisition`'s table is a function-local (:1563), so the
// overlay recovers each one's config text from the baseline's own `Fs::` and
// `configFileName::` sections -- both INPUT sections -- and takes the rest of
// that test's input shape from the pinned literal at :1634-1644, which is
// constant across all eight: base path `/apath`, `allFileList` of
// `/apath/a.ts` and `/apath/b.ts`, one input, `includeCompilerOptions` true.
// A wrong recovery cannot pass silently, because `Fs::` is rendered back out
// of the reconstructed host and compared with the rest of the bytes.
//
// The probe never reads `CompilerOptions::`, `TypeAcquisition::`,
// `FileNames::`, `Config::` or `Errors::` -- the result sections -- to build a
// request, and it never edits, repairs or special-cases a baseline.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

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
	// TestParseTypeAcquisition's function-local input shape
	// (tsconfigparsing_test.go:1634-1644), constant across all eight cases.
	typeAcquisitionBasePath = "/apath"
)

type tsconfigParsingRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Baseline  string `json:"baseline"`
	// "json", "jsonSourceFile" or "jsonParse": which pinned entry point the
	// output is written from.
	API string `json:"api"`
	// "table" when the input is read from a pinned package-level table,
	// "recovered" when it is rebuilt from the baseline's own input sections.
	Source string `json:"source"`
	// The pinned test title, which is the table key and the output-name stem.
	Title string `json:"title"`
}

// phase1RenderParseConfig is `baselineParseConfigWith`
// (tsconfigparsing_test.go:1503-1555) with its final `baseline.Run` removed and
// the assembled bytes returned instead. Nothing else is changed: the loop, the
// host construction, the section order, the two `WriteString("\n")` separators,
// the `TypeAcquisition` nil gate and both newline settings are the pinned
// ones, and every value is produced by the pinned call named in the file
// comment above.
func phase1RenderParseConfig(
	includeCompilerOptions bool,
	input []testConfig,
	getParsed func(config testConfig, host tsoptions.ParseConfigHost, basePath string) *tsoptions.ParsedCommandLine,
) (string, error) {
	var baselineContent strings.Builder
	for i, config := range input {
		basePath := config.basePath
		if basePath == "" {
			basePath = tspath.GetNormalizedAbsolutePath(tspath.GetDirectoryPath(config.configFileName), "")
		}
		configFileName := tspath.CombinePaths(basePath, config.configFileName)
		allFileLists := make(map[string]string, len(config.allFileList)+1)
		maps.Copy(allFileLists, config.allFileList)
		allFileLists[configFileName] = config.jsonText
		host := tsoptionstest.NewVFSParseConfigHost(allFileLists, config.basePath, true /*useCaseSensitiveFileNames*/)
		parsedConfigFileContent := getParsed(config, host, basePath)

		baselineContent.WriteString("Fs::\n")
		if err := printFS(&baselineContent, host.FS(), "/"); err != nil {
			return "", err
		}
		baselineContent.WriteString("\n")
		baselineContent.WriteString("configFileName:: ")
		baselineContent.WriteString(config.configFileName)
		baselineContent.WriteString("\n")
		if includeCompilerOptions {
			baselineContent.WriteString("CompilerOptions::\n")
			if err := json.MarshalIndentWrite(&baselineContent, parsedConfigFileContent.ParsedConfig.CompilerOptions, "", "  "); err != nil {
				return "", err
			}
			baselineContent.WriteString("\n")
			baselineContent.WriteString("\n")

			if parsedConfigFileContent.ParsedConfig.TypeAcquisition != nil {
				baselineContent.WriteString("TypeAcquisition::\n")
				if err := json.MarshalIndentWrite(&baselineContent, parsedConfigFileContent.ParsedConfig.TypeAcquisition, "", "  "); err != nil {
					return "", err
				}
				baselineContent.WriteString("\n")
				baselineContent.WriteString("\n")
			}
		}
		baselineContent.WriteString("FileNames::\n")
		baselineContent.WriteString(strings.Join(parsedConfigFileContent.ParsedConfig.FileNames, ","))
		baselineContent.WriteString("\n")
		baselineContent.WriteString("Errors::\n")
		diagnosticwriter.FormatDiagnosticsWithColorAndContext(&baselineContent, diagnosticwriter.FromASTDiagnostics(parsedConfigFileContent.Errors), &diagnosticwriter.FormattingOptions{
			NewLine: "\r\n",
			ComparePathsOptions: tspath.ComparePathsOptions{
				CurrentDirectory:          basePath,
				UseCaseSensitiveFileNames: true,
			},
		})
		baselineContent.WriteString("\n")
		if i != len(input)-1 {
			baselineContent.WriteString("\n")
		}
	}
	return baselineContent.String(), nil
}

// phase1RenderParseConfigText is the inline assembly of
// `TestParseConfigFileTextToJson` (tsconfigparsing_test.go:130-160), lifted out
// of the test body unchanged. Note its `NewLine` is "\n", not the "\r\n" the
// other renderer uses -- a real difference between the two, preserved here.
func phase1RenderParseConfigText(inputs []string) string {
	var baselineContent strings.Builder
	for i, jsonText := range inputs {
		baselineContent.WriteString("Input::\n")
		baselineContent.WriteString(jsonText)
		baselineContent.WriteString("\n")
		parsed, errors := tsoptions.ParseConfigFileTextToJson("/apath/tsconfig.json", "/apath", jsonText)
		baselineContent.WriteString("Config::\n")
		if err := writeJsonReadableText(&baselineContent, parsed); err != nil {
			panic(err)
		}
		baselineContent.WriteString("\n")
		baselineContent.WriteString("Errors::\n")
		diagnosticwriter.FormatDiagnosticsWithColorAndContext(&baselineContent, diagnosticwriter.FromASTDiagnostics(errors), &diagnosticwriter.FormattingOptions{
			NewLine: "\n",
			ComparePathsOptions: tspath.ComparePathsOptions{
				CurrentDirectory:          "/",
				UseCaseSensitiveFileNames: true,
			},
		})
		baselineContent.WriteString("\n")
		if i != len(inputs)-1 {
			baselineContent.WriteString("\n")
		}
	}
	return baselineContent.String()
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

func TestPhase1ConfigTsconfigParsing(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []tsconfigParsingRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Operation != tsconfigParsingOperation {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the tsconfigParsing baseline probe"
			observations = append(observations, row)
			continue
		}
		path := filepath.Join(repo.TestDataPath(), "baselines", "reference", filepath.FromSlash(request.Baseline))
		expected, readErr := os.ReadFile(path)
		if readErr != nil {
			t.Fatalf("%s: %v", request.Case, readErr)
		}
		rendered, inputs, renderErr := renderTsconfigParsing(request, string(expected))
		if renderErr != nil {
			row["result"] = "harness_failed"
			row["error"] = fmt.Sprintf("cannot render %s: %v", request.Baseline, renderErr)
			observations = append(observations, row)
			continue
		}
		renderedSum := sha256.Sum256([]byte(rendered))
		expectedSum := sha256.Sum256(expected)
		row["result"] = "observed"
		row["observation"] = map[string]any{
			"baseline":        request.Baseline,
			"api":             request.API,
			"input_source":    request.Source,
			"inputs":          inputs,
			"rendered":        rendered,
			"rendered_sha256": hex.EncodeToString(renderedSum[:]),
			"expected_sha256": hex.EncodeToString(expectedSum[:]),
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

// renderTsconfigParsing resolves one request's inputs and renders it, returning
// the bytes and the inputs it used so a differing row records both.
func renderTsconfigParsing(request tsconfigParsingRequest, expected string) (string, []map[string]any, error) {
	describe := func(input []testConfig) []map[string]any {
		rows := make([]map[string]any, 0, len(input))
		for _, config := range input {
			rows = append(rows, map[string]any{
				"config_file_name": config.configFileName,
				"base_path":        config.basePath,
				"json_text":        config.jsonText,
				"all_file_list":    config.allFileList,
			})
		}
		return rows
	}
	switch request.API {
	case "jsonParse":
		inputs, found := parseConfigFileTextToJsonInputs(request.Title)
		if !found {
			return "", nil, fmt.Errorf("no pinned parseConfigFileTextToJsonTests entry titled %q", request.Title)
		}
		texts := make([]map[string]any, 0, len(inputs))
		for _, text := range inputs {
			texts = append(texts, map[string]any{"json_text": text})
		}
		return phase1RenderParseConfigText(inputs), texts, nil
	case "json", "jsonSourceFile":
		getParsed := getParsedWithJsonApi
		if request.API == "jsonSourceFile" {
			getParsed = getParsedWithJsonSourceFileApi
		}
		switch request.Source {
		case "table":
			rec, found := tsconfigParsingEntry(request.Title)
			if !found {
				return "", nil, fmt.Errorf("no pinned parseJsonConfigFileTests entry titled %q", request.Title)
			}
			rendered, err := phase1RenderParseConfig(rec.includeCompilerOptions, rec.input, getParsed)
			return rendered, describe(rec.input), err
		case "recovered":
			config, err := recoverTypeAcquisitionInput(expected)
			if err != nil {
				return "", nil, err
			}
			input := []testConfig{config}
			rendered, err := phase1RenderParseConfig(true, input, getParsed)
			return rendered, describe(input), err
		default:
			return "", nil, fmt.Errorf("unknown input source %q", request.Source)
		}
	default:
		return "", nil, fmt.Errorf("unknown tsconfigParsing api %q", request.API)
	}
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
