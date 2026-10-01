package testrunner

// The Phase 3 native emit oracle (docs/PHASE3-plan.md, T0). An access-only
// driver over the pinned compiler runner: it compiles each requested variant
// as the runner does and records what the runner's `output`, `sourcemap` and
// `sourcemap record` sub-tests would baseline, the emitted files behind them,
// and the pinned printer's reprint of every non-library source file. Nothing
// here changes what the pin computes; baseline.Phase3Observe only diverts the
// composed text from the file comparison.
import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/outputpaths"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/testutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/baseline"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/tsbaseline"
	"github.com/microsoft/TypeScript/tsc/internal/transpile"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
)

type phase3Request struct {
	ID                string            `json:"id"`
	Path              string            `json:"path"`
	RawSHA256         string            `json:"raw_sha256"`
	LoadedSHA256      string            `json:"loaded_sha256"`
	Settings          map[string]string `json:"settings"`
	ConfigurationName string            `json:"configuration_name"`
	ConfiguredName    string            `json:"configured_name"`
	AcceptanceTier    string            `json:"acceptance_tier"`
}

type phase3Captured struct{ name, value string }

func phase3Hash(raw []byte) string { value := sha256.Sum256(raw); return hex.EncodeToString(value[:]) }

func phase3Hex(value string) string { return hex.EncodeToString([]byte(value)) }

func phase3Failure(row map[string]any, reason, message string) {
	stage := row["execution_stage"].(string)
	state := "harness_failed"
	if strings.HasPrefix(stage, "native_") {
		state = "upstream_failed"
		if reason == "panic" || reason == "assertion" {
			reason = "native_" + reason
		}
	}
	row["state"] = state
	row["failure"] = map[string]string{"stage": stage, "reason": reason, "message": message}
}

// Bytes are kept exact: file names, message text and arguments are hex.
func phase3Diagnostics(values []*ast.Diagnostic) []map[string]any {
	result := make([]map[string]any, 0, len(values))
	for _, d := range values {
		var file any
		if d.File() != nil {
			file = phase3Hex(d.File().FileName())
		}
		args := []string{}
		for _, arg := range d.MessageArgs() {
			args = append(args, phase3Hex(arg))
		}
		result = append(result, map[string]any{
			"file_hex": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(),
			"key_hex": phase3Hex(string(d.MessageKey())), "text_hex": phase3Hex(d.MessageText()), "args_hex": args,
			"chain": phase3Diagnostics(d.MessageChain()), "related": phase3Diagnostics(d.RelatedInformation()),
		})
	}
	return result
}

func phase3Baseline(captured phase3Captured) map[string]any {
	if captured.value == baseline.NoContent {
		return map[string]any{"state": "no_content", "name": captured.name}
	}
	return map[string]any{"state": "content", "name": captured.name, "text_hex": phase3Hex(captured.value)}
}

func phase3Files(files *collections.OrderedMap[string, *harnessutil.TestFile], texts bool) []map[string]any {
	result := []map[string]any{}
	for file := range files.Values() {
		entry := map[string]any{"name_hex": phase3Hex(file.UnitName), "sha256": phase3Hash([]byte(file.Content)), "bytes": len(file.Content)}
		if texts {
			entry["text_hex"] = phase3Hex(file.Content)
		}
		result = append(result, entry)
	}
	return result
}

func phase3Print(file *ast.SourceFile, removeComments bool, texts bool) (result map[string]any) {
	defer func() {
		if value := recover(); value != nil {
			result = map[string]any{"state": "panic", "message": fmt.Sprint(value)}
		}
	}()
	p := printer.NewPrinter(printer.PrinterOptions{RemoveComments: removeComments, NewLine: core.NewLineKindLF}, printer.PrintHandlers{}, printer.NewEmitContext())
	text := p.EmitSourceFile(file)
	result = map[string]any{"state": "printed", "sha256": phase3Hash([]byte(text)), "bytes": len(text)}
	if texts {
		result["text_hex"] = phase3Hex(text)
	}
	return result
}

// The pinned printer over every source file of the program that is not a
// default library file: once with comments and once with RemoveComments.
func phase3Reprint(c *compilerTest, texts bool) []map[string]any {
	result := []map[string]any{}
	for _, file := range c.result.Program.GetSourceFiles() {
		if c.result.Program.IsSourceFileDefaultLibrary(file.Path()) {
			continue
		}
		result = append(result, map[string]any{
			"name_hex": phase3Hex(file.FileName()), "source_sha256": phase3Hash([]byte(file.Text())),
			"script_kind": int(file.ScriptKind), "language_variant": int(file.LanguageVariant),
			"comments": phase3Print(file, false, texts), "no_comments": phase3Print(file, true, texts),
		})
	}
	return result
}

func phase3ReadRequests(t *testing.T) ([]byte, []phase3Request) {
	raw, err := os.ReadFile(os.Getenv("PHASE3_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var requests []phase3Request
	if err := decoder.Decode(&requests); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatalf("trailing input: %v", err)
	}
	seen := map[string]bool{}
	for _, request := range requests {
		if request.ID == "" || seen[request.ID] || request.AcceptanceTier != "executed" {
			t.Fatal("empty/duplicate request")
		}
		seen[request.ID] = true
	}
	return raw, requests
}

func TestPhase3Emit(t *testing.T) {
	raw, requests := phase3ReadRequests(t)
	texts := os.Getenv("PHASE3_TEXTS") == "1"
	output, err := os.Create(os.Getenv("PHASE3_OUTPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	encoder := json.NewEncoder(output)
	for _, request := range requests {
		row := map[string]any{"id": request.ID, "acceptance_tier": request.AcceptanceTier, "state": "not_executed", "execution_stage": "harness_input"}
		complete := false
		t.Run(request.ID, func(t *testing.T) {
			defer func() {
				if value := recover(); value != nil {
					row["panic"] = fmt.Sprint(value)
					phase3Failure(row, "panic", fmt.Sprint(value))
					t.Error("emit oracle panic", value)
				}
				if !complete && row["failure"] == nil {
					if t.Skipped() && row["execution_stage"] == "native_option_guard" {
						row["state"] = "upstream_skipped"
					} else if t.Failed() {
						phase3Failure(row, "assertion", "test assertion failed; see go.stdout")
					} else {
						row["execution_stage"] = "harness_incomplete"
						phase3Failure(row, "incomplete", "emit observation exited before completion")
					}
				}
				harnessutil.S08ObserveDiagnostics = nil
				harnessutil.S08ObserveStage = nil
				harnessutil.S08ObserveOptionRejection = nil
				baseline.Phase3Observe = nil
			}()
			stage := func(next string) func() {
				previous := row["execution_stage"]
				row["execution_stage"] = next
				return func() { row["execution_stage"] = previous }
			}
			fail := func(reason string, values ...any) {
				phase3Failure(row, reason, fmt.Sprint(values...))
				t.Fatal(values...)
			}
			harnessutil.S08ObserveStage = stage
			harnessutil.S08ObserveOptionRejection = func(message string) {
				phase3Failure(row, "option_rejected", message)
			}
			path := filepath.Join(repo.RootPath(), strings.TrimPrefix(request.Path, "tsc/"))
			original, err := os.ReadFile(path)
			if err != nil {
				fail("source_read", err)
			}
			if phase3Hash(original) != request.RawSHA256 {
				fail("source_digest", "physical source digest differs")
			}
			loaded, ok := osvfs.FS().ReadFile(path)
			if !ok || phase3Hash([]byte(loaded)) != request.LoadedSHA256 {
				fail("source_digest", "loaded source digest differs")
			}
			row["raw_sha256"] = phase3Hash(original)
			row["loaded_sha256"] = phase3Hash([]byte(loaded))
			harnessutil.S08ObserveDiagnostics = func(pre, post []*ast.Diagnostic) {
				restore := harnessutil.S08EnterObservation()
				row["pre_diagnostics"] = len(pre)
				row["post_diagnostics"] = len(post)
				restore()
			}
			stage("native_parse")
			payload := makeUnitsFromTest(loaded, path)
			configuration := &harnessutil.NamedTestConfiguration{Config: request.Settings, Name: request.ConfigurationName}
			stage("native_setup")
			c := newCompilerTest(t, request.ID, path, &payload, configuration)
			stage("harness_identity")
			if c.configuredName != request.ConfiguredName {
				fail("configured_name", "configured name drift", c.configuredName)
			}
			row["options"] = c.options
			row["harness_options"] = c.harnessOptions
			stage("native_option_guard")
			harnessutil.SkipUnsupportedCompilerOptions(t, c.options)
			stage("harness_observation")
			row["has_non_dts_files"] = c.hasNonDtsFiles
			row["diagnostics"] = len(c.result.Diagnostics)
			emit := map[string]any{"state": "absent"}
			if c.result.Result != nil {
				emitted := []string{}
				for _, name := range c.result.Result.EmittedFiles {
					emitted = append(emitted, phase3Hex(name))
				}
				emit = map[string]any{"state": "executed", "emit_skipped": c.result.Result.EmitSkipped, "emitted_files_hex": emitted,
					"diagnostics": phase3Diagnostics(c.result.Result.Diagnostics), "source_maps": len(c.result.Result.SourceMaps)}
			}
			row["emit"] = emit
			row["outputs"] = map[string]any{"js": phase3Files(&c.result.JS, texts), "dts": phase3Files(&c.result.DTS, texts), "maps": phase3Files(&c.result.Maps, texts)}
			suite := "compiler"
			if strings.Contains(request.Path, "/tests/cases/conformance/") {
				suite = "conformance"
			}
			header := tspath.GetPathFromPathComponents(tspath.GetPathComponentsRelativeTo(repo.TestDataPath(), path, tspath.ComparePathsOptions{}))
			// Each of the runner's sub-tests is its own t.Run there; a failure in
			// one does not stop the others, and it is that domain's outcome here.
			domain := func(name, stageName string, body func(t *testing.T)) map[string]any {
				var captured []phase3Captured
				result := map[string]any{"state": "not_baselined"}
				restore := stage(stageName)
				passed := t.Run(name, func(t *testing.T) {
					defer func() {
						baseline.Phase3Observe = nil
						if value := recover(); value != nil {
							result = map[string]any{"state": "failed", "reason": "native_panic", "message": fmt.Sprint(value)}
						}
					}()
					baseline.Phase3Observe = func(fileName string, actual string, opts baseline.Options) bool {
						captured = append(captured, phase3Captured{filepath.ToSlash(filepath.Join(opts.Subfolder, fileName)), actual})
						return true
					}
					body(t)
				})
				restore()
				if result["state"] == "failed" {
					return result
				}
				if !passed {
					return map[string]any{"state": "failed", "reason": "native_assertion", "message": "sub-test failed; see go.stdout"}
				}
				switch len(captured) {
				case 0:
					return result
				case 1:
					return phase3Baseline(captured[0])
				}
				return map[string]any{"state": "failed", "reason": "harness", "message": "more than one baseline in one sub-test"}
			}
			if !c.hasNonDtsFiles {
				row["output"] = map[string]any{"state": "disabled", "reason": "no input file other than declaration files"}
			} else if message, skipped := skippedEmitTests[c.basename]; skipped {
				row["output"] = map[string]any{"state": "disabled", "reason": message}
			} else {
				row["output"] = domain("output", "native_output", func(t *testing.T) {
					tsbaseline.DoJSEmitBaseline(t, c.configuredName, header, c.options, c.result, c.tsConfigFiles, c.toBeCompiled, c.otherFiles, c.harnessOptions, baseline.Options{Subfolder: suite})
				})
			}
			row["sourcemap"] = domain("sourcemap", "native_sourcemap", func(t *testing.T) {
				tsbaseline.DoSourcemapBaseline(t, c.configuredName, header, c.options, c.result, c.harnessOptions, baseline.Options{Subfolder: suite})
			})
			row["sourcemap_record"] = domain("sourcemap record", "native_sourcemap_record", func(t *testing.T) {
				tsbaseline.DoSourcemapRecordBaseline(t, c.configuredName, header, c.options, c.result, c.harnessOptions, baseline.Options{Subfolder: suite})
			})
			stage("native_reprint")
			row["reprint"] = phase3Reprint(c, texts)
			stage("complete")
			row["state"] = "executed"
			complete = true
		})
		if err := encoder.Encode(row); err != nil {
			t.Fatal(err)
		}
	}
	summary, err := json.Marshal(map[string]any{"request_sha256": phase3Hash(raw), "rows": len(requests), "single_threaded": testutil.TestProgramIsSingleThreaded(), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE3_SUMMARY"), summary, 0o600); err != nil {
		t.Fatal(err)
	}
}

// The pinned transpile runner, configuration by configuration: the inputs it
// derives from each test file, each unit's TranspileModule and
// TranspileDeclaration result, and the baseline text runKind composes.
func TestPhase3Transpile(t *testing.T) {
	output, err := os.Create(os.Getenv("PHASE3_OUTPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	encoder := json.NewEncoder(output)
	r := NewTranspileBaselineRunner()
	rows := 0
	for _, fileName := range r.EnumerateTestFiles() {
		content, ok := osvfs.FS().ReadFile(fileName)
		if !ok {
			t.Fatal("could not read transpile test file: " + fileName)
		}
		settings := extractCompilerSettings(content)
		configurations := harnessutil.GetFileBasedTestConfigurations(t, settings, transpileVaryBy)
		if len(configurations) == 0 {
			configurations = []*harnessutil.NamedTestConfiguration{{Config: settings}}
		}
		extension := tspath.GetAnyExtensionFromPath(fileName, nil, false)
		baseName := tspath.GetBaseFileName(fileName)
		justName := strings.TrimSuffix(baseName, extension)
		units := makeUnitsFromTest(content, baseName).testUnitData
		for _, configuration := range configurations {
			configuredName := justName
			if configuration.Name != "" {
				configuredName += "(" + formatTranspileConfigurationName(configuration.Name) + ")"
			}
			inputs := []map[string]string{}
			for _, unit := range units {
				inputs = append(inputs, map[string]string{"name_hex": phase3Hex(unit.name), "content_hex": phase3Hex(unit.content)})
			}
			row := map[string]any{"id": "transpile/" + baseName + "#" + configuration.Name, "file": "transpile/" + baseName, "source_sha256": phase3Hash([]byte(content)),
				"configuration_name": configuration.Name, "configured_name": configuredName, "settings": configuration.Config, "units": inputs, "state": "not_executed"}
			runs := []map[string]any{}
			passed := t.Run(configuredName, func(t *testing.T) {
				defer func() { baseline.Phase3Observe = nil }()
				options := &core.CompilerOptions{}
				harnessOptions := &harnessutil.HarnessOptions{}
				harnessutil.SetOptionsFromTestConfig(t, configuration.Config, options, harnessOptions, srcFolder, false)
				row["options"] = options
				row["harness_options"] = harnessOptions
				run := func(declaration bool) {
					var captured []phase3Captured
					baseline.Phase3Observe = func(fileName string, actual string, opts baseline.Options) bool {
						captured = append(captured, phase3Captured{filepath.ToSlash(filepath.Join(opts.Subfolder, fileName)), actual})
						return true
					}
					r.runKind(t, configuredName, extension, units, options, harnessOptions, declaration)
					baseline.Phase3Observe = nil
					if len(captured) != 1 {
						t.Fatal("transpile run composed no single baseline")
					}
					results := []map[string]any{}
					for _, unit := range units {
						transpileOptions := transpile.Options{CompilerOptions: options, FileName: unit.name, ReportDiagnostics: harnessOptions.ReportDiagnostics}
						var result *transpile.Output
						if declaration {
							result = transpile.TranspileDeclaration(t.Context(), unit.content, transpileOptions)
						} else {
							result = transpile.TranspileModule(t.Context(), unit.content, transpileOptions)
						}
						outputExtension := outputpaths.GetOutputExtension(unit.name, options.Jsx)
						if declaration {
							outputExtension = tspath.GetDeclarationEmitExtensionForPath(unit.name)
						}
						results = append(results, map[string]any{"unit_hex": phase3Hex(unit.name), "output_name_hex": phase3Hex(tspath.ChangeExtension(unit.name, outputExtension)),
							"output_hex": phase3Hex(result.OutputText), "source_map_hex": phase3Hex(result.SourceMapText), "diagnostics": phase3Diagnostics(result.Diagnostics)})
					}
					runs = append(runs, map[string]any{"declaration": declaration, "baseline": phase3Baseline(captured[0]), "units": results})
				}
				if !options.EmitDeclarationOnly.IsTrue() {
					run(false)
				}
				if options.Declaration.IsTrue() {
					run(true)
				}
			})
			row["runs"] = runs
			if passed {
				row["state"] = "executed"
			} else {
				row["state"] = "upstream_failed"
			}
			if err := encoder.Encode(row); err != nil {
				t.Fatal(err)
			}
			rows++
		}
	}
	summary, err := json.Marshal(map[string]any{"rows": rows, "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE3_SUMMARY"), summary, 0o600); err != nil {
		t.Fatal(err)
	}
}
