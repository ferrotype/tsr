package testrunner

// The Phase 3 build-info witness driver (an access-only overlay). Each
// request is a compiler test source (the `// @option` and `// @filename`
// format, one configuration) and a list of steps. The source is set up as
// the compiler runner sets it up (`newCompilerTest`); every step then runs
// one compilation as the harness's post-emit compilation runs it
// (`harnessutil.Phase3IncrementalStep`): a fresh file system holding the
// previous step's files and everything it wrote, with the step's edits, and
// the harness's `createProgram`, which reads the build info with the
// harness's test reader and wraps the program in `incremental.NewProgram`.
// The row records each step's loading request (the S07 shape), its emit
// results, its diagnostics and every file it wrote with its text.
import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"maps"
	"os"
	"runtime"
	"slices"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase3IncrementalStep struct {
	// Absolute name to new text, or null to delete the file.
	Edits map[string]*string `json:"edits"`
	// The program's root names, when they change.
	Roots []string `json:"roots"`
	// Compiler options to set (ParseCompilerOptions values).
	Options map[string]any `json:"options"`
	Actions []string       `json:"actions"`
}

type phase3IncrementalCase struct {
	ID        string                  `json:"id"`
	Name      string                  `json:"name"`
	SourceHex string                  `json:"source_hex"`
	Steps     []phase3IncrementalStep `json:"steps"`
}

func phase3Hex(text string) string {
	return hex.EncodeToString([]byte(text))
}

func phase3Diagnostic(d *ast.Diagnostic) map[string]any {
	file := ""
	if d.File() != nil {
		file = d.File().FileName()
	}
	return map[string]any{"file": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(),
		"message_hex": phase3Hex(d.Localize(locale.Default))}
}

func phase3Diagnostics(diagnostics []*ast.Diagnostic) []map[string]any {
	result := []map[string]any{}
	for _, d := range diagnostics {
		result = append(result, phase3Diagnostic(d))
	}
	return result
}

func TestPhase3Incremental(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("PHASE3_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var cases []phase3IncrementalCase
	if err := decoder.Decode(&cases); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatalf("trailing input: %v", err)
	}
	output, err := os.Create(os.Getenv("PHASE3_OUTPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	encoder := json.NewEncoder(output)
	seen := map[string]bool{}
	for _, request := range cases {
		if request.ID == "" || seen[request.ID] || request.Name == "" || len(request.Steps) == 0 {
			t.Fatal("empty, duplicate or stepless case: " + request.ID)
		}
		seen[request.ID] = true
		row := map[string]any{"id": request.ID, "state": "failed"}
		t.Run(request.ID, func(t *testing.T) {
			defer func() {
				if value := recover(); value != nil {
					row["message"] = fmt.Sprint(value)
					t.Error("case panic", value)
				}
			}()
			source, err := hex.DecodeString(request.SourceHex)
			if err != nil {
				t.Fatal(err)
			}
			path := "/probe/" + request.Name
			settings := extractCompilerSettings(string(source))
			configurations := harnessutil.GetFileBasedTestConfigurations(t, settings, compilerVaryBy)
			var configuration *harnessutil.NamedTestConfiguration
			switch len(configurations) {
			case 0:
			case 1:
				configuration = configurations[0]
			default:
				t.Fatal("a case has one configuration")
			}
			payload := makeUnitsFromTest(string(source), path)
			c := newCompilerTest(t, request.ID, path, &payload, configuration)
			harnessutil.SkipUnsupportedCompilerOptions(t, c.options)
			config := c.result.Program.Program().CommandLine()
			errorInputs := []map[string]any{}
			files := map[string]string{}
			for _, unit := range slices.Concat(c.tsConfigFiles, c.toBeCompiled, c.otherFiles) {
				errorInputs = append(errorInputs, map[string]any{"name_hex": phase3Hex(unit.UnitName), "content_hex": phase3Hex(unit.Content)})
			}
			for _, unit := range slices.Concat(c.toBeCompiled, c.otherFiles) {
				files[tspath.GetNormalizedAbsolutePath(unit.UnitName, c.currentDirectory)] = unit.Content
			}
			symlinks := map[string]string{}
			for from, to := range c.result.Symlinks {
				symlinks[tspath.GetNormalizedAbsolutePath(from, c.currentDirectory)] = tspath.GetNormalizedAbsolutePath(to, c.currentDirectory)
			}
			row["error_inputs"] = errorInputs
			steps := []map[string]any{}
			for index, step := range request.Steps {
				for name, text := range step.Edits {
					if text == nil {
						if _, ok := files[name]; !ok {
							t.Fatalf("step %d deletes %s, which does not exist", index, name)
						}
						delete(files, name)
					} else {
						files[name] = *text
					}
				}
				if step.Roots != nil || step.Options != nil {
					options := config.CompilerOptions().Clone()
					for _, name := range slices.Sorted(maps.Keys(step.Options)) {
						if errors := tsoptions.ParseCompilerOptions(name, step.Options[name], options); len(errors) != 0 {
							t.Fatalf("step %d: option %s: %v", index, name, errors)
						}
					}
					roots := config.FileNames()
					if step.Roots != nil {
						roots = step.Roots
					}
					config = &tsoptions.ParsedCommandLine{
						ParsedConfig: &tsoptions.ParsedOptions{
							CompilerOptions: options,
							FileNames:       roots,
							ContentMappers:  config.ContentMappers(),
						},
						ConfigFile: config.ConfigFile,
						Errors:     config.Errors,
					}
				}
				inputs := map[string]string{}
				for name, text := range files {
					inputs[name] = phase3Hex(text)
				}
				loading := map[string]any{"id": request.ID, "cwd": c.currentDirectory, "case_sensitive": c.harnessOptions.UseCaseSensitiveFileNames,
					"files": inputs, "symlinks": symlinks, "roots": config.FileNames(), "options": config.CompilerOptions(), "skip_module_resolution": false}
				result := harnessutil.Phase3IncrementalStep(files, symlinks, c.harnessOptions.UseCaseSensitiveFileNames, c.currentDirectory, config, step.Actions)
				emits := []map[string]any{}
				for _, emit := range result.Emits {
					if emit == nil {
						emits = append(emits, nil)
						continue
					}
					emitted := emit.EmittedFiles
					if emitted == nil {
						emitted = []string{}
					}
					emits = append(emits, map[string]any{"emit_skipped": emit.EmitSkipped, "emitted_files": emitted, "diagnostics": phase3Diagnostics(emit.Diagnostics)})
				}
				diagnostics := [][]map[string]any{}
				for _, list := range result.Diagnostics {
					diagnostics = append(diagnostics, phase3Diagnostics(list))
				}
				outputs := []map[string]any{}
				for _, file := range result.Outputs {
					outputs = append(outputs, map[string]any{"name": file.UnitName, "text_hex": phase3Hex(file.Content)})
					files[file.UnitName] = file.Content
				}
				steps = append(steps, map[string]any{"loading": loading, "actions": step.Actions, "emits": emits, "diagnostics": diagnostics, "outputs": outputs})
			}
			row["steps"] = steps
			row["state"] = "executed"
		})
		if row["state"] != "executed" && row["message"] == nil {
			row["message"] = "test assertion failed or skipped; see the test output"
		}
		if err := encoder.Encode(row); err != nil {
			t.Fatal(err)
		}
	}
	// The declarations whose values the build info records (the
	// `AffectsBuildInfo` filter of `setCompilerOptions`), in declaration order.
	affectsBuildInfo := []string{}
	for _, option := range tsoptions.OptionsDeclarations {
		if option.AffectsBuildInfo {
			affectsBuildInfo = append(affectsBuildInfo, option.Name)
		}
	}
	summary, err := json.Marshal(map[string]any{"rows": len(cases), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "version": core.Version(), "affects_build_info": affectsBuildInfo})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE3_SUMMARY"), summary, 0o600); err != nil {
		t.Fatal(err)
	}
}
