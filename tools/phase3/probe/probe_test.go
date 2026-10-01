package testrunner

// The Phase 3 transform probe driver. Each request is a compiler test source
// (the `// @option` and `// @filename` format, one configuration); the driver
// compiles it as the compiler runner does and, for every source file that is
// neither a default library nor a declaration file, runs each requested chain
// of transformers and records the printed text.
import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"runtime"
	"slices"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type phase3Probe struct {
	ID        string     `json:"id"`
	Name      string     `json:"name"`
	SourceHex string     `json:"source_hex"`
	Chains    [][]string `json:"chains"`
}

func phase3ProbeChain(t *testing.T, program *compiler.Program, name string, chain []string) (result map[string]any) {
	result = map[string]any{"chain": chain}
	defer func() {
		if value := recover(); value != nil {
			result["state"] = "panic"
			result["message"] = fmt.Sprint(value)
		}
	}()
	var text string
	if len(chain) == 1 && chain[0] == "declarations" {
		var diagnostics []*ast.Diagnostic
		text, diagnostics = compiler.Phase3Declarations(t.Context(), program, program.GetSourceFile(name))
		reported := []map[string]any{}
		for _, d := range diagnostics {
			reported = append(reported, map[string]any{"code": d.Code(), "pos": d.Pos(), "end": d.End(), "message_hex": hex.EncodeToString([]byte(d.String()))})
		}
		result["diagnostics"] = reported
	} else {
		text = compiler.Phase3Transform(t.Context(), program, program.GetSourceFile(name), chain)
	}
	result["state"] = "printed"
	result["text_hex"] = hex.EncodeToString([]byte(text))
	return result
}

func TestPhase3Probe(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("PHASE3_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var probes []phase3Probe
	if err := decoder.Decode(&probes); err != nil {
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
	for _, probe := range probes {
		if probe.ID == "" || seen[probe.ID] || probe.Name == "" || len(probe.Chains) == 0 {
			t.Fatal("empty, duplicate or chainless probe: " + probe.ID)
		}
		seen[probe.ID] = true
		row := map[string]any{"id": probe.ID, "state": "failed"}
		t.Run(probe.ID, func(t *testing.T) {
			defer func() {
				if value := recover(); value != nil {
					row["message"] = fmt.Sprint(value)
					t.Error("probe panic", value)
				}
			}()
			source, err := hex.DecodeString(probe.SourceHex)
			if err != nil {
				t.Fatal(err)
			}
			path := "/probe/" + probe.Name
			settings := extractCompilerSettings(string(source))
			configurations := harnessutil.GetFileBasedTestConfigurations(t, settings, compilerVaryBy)
			var configuration *harnessutil.NamedTestConfiguration
			switch len(configurations) {
			case 0:
			case 1:
				configuration = configurations[0]
			default:
				t.Fatal("a probe has one configuration; split the variations into probes")
			}
			payload := makeUnitsFromTest(string(source), path)
			c := newCompilerTest(t, probe.ID, path, &payload, configuration)
			harnessutil.SkipUnsupportedCompilerOptions(t, c.options)
			program := c.result.Program.Program()
			row["options"] = c.options
			row["diagnostics"] = len(c.result.Diagnostics)
			// The program's inputs in the S07 loading-request shape, so a Rust
			// test loads the same program without porting the test-file format.
			inputs, roots, symlinks := map[string]string{}, []string{}, map[string]string{}
			for _, unit := range slices.Concat(c.toBeCompiled, c.otherFiles) {
				inputs[tspath.GetNormalizedAbsolutePath(unit.UnitName, c.currentDirectory)] = hex.EncodeToString([]byte(unit.Content))
			}
			for _, unit := range c.toBeCompiled {
				name := tspath.GetNormalizedAbsolutePath(unit.UnitName, c.currentDirectory)
				if !tspath.FileExtensionIs(name, tspath.ExtensionJson) && !tspath.FileExtensionIs(name, tspath.ExtensionTsBuildInfo) {
					roots = append(roots, name)
				}
			}
			for from, to := range c.result.Symlinks {
				symlinks[tspath.GetNormalizedAbsolutePath(from, c.currentDirectory)] = tspath.GetNormalizedAbsolutePath(to, c.currentDirectory)
			}
			if len(c.tsConfigFiles) != 0 {
				t.Fatal("a probe is configured by settings, not by a tsconfig unit")
			}
			row["loading"] = map[string]any{"id": probe.ID, "cwd": c.currentDirectory, "case_sensitive": c.harnessOptions.UseCaseSensitiveFileNames,
				"files": inputs, "symlinks": symlinks, "roots": roots, "options": c.options, "skip_module_resolution": false}
			files := []map[string]any{}
			for _, file := range program.GetSourceFiles() {
				if program.IsSourceFileDefaultLibrary(file.Path()) || file.IsDeclarationFile {
					continue
				}
				chains := []map[string]any{}
				for _, chain := range probe.Chains {
					chains = append(chains, phase3ProbeChain(t, program, file.FileName(), chain))
				}
				files = append(files, map[string]any{"name_hex": hex.EncodeToString([]byte(file.FileName())), "chains": chains})
			}
			row["files"] = files
			row["state"] = "executed"
		})
		if row["state"] != "executed" && row["message"] == nil {
			row["message"] = "test assertion failed or skipped; see go.stdout"
		}
		if err := encoder.Encode(row); err != nil {
			t.Fatal(err)
		}
	}
	summary, err := json.Marshal(map[string]any{"rows": len(probes), "transformers": compiler.Phase3TransformerNames(), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE3_SUMMARY"), summary, 0o600); err != nil {
		t.Fatal(err)
	}
}
