package checker_test

// Supplied checker-host state, not a claim of ordinary resolver reachability.
import (
	"context"
	"encoding/json"
	"os"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/module"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase7AlternateProgram struct {
	*compiler.Program
	result *module.ResolvedModule
}

func (p *phase7AlternateProgram) GetResolvedModule(_ ast.HasFileName, _ string, _ core.ResolutionMode) *module.ResolvedModule {
	return p.result
}

type phase7Diagnostic struct {
	Code       int32                         `json:"code"`
	Pos        int                           `json:"pos"`
	End        int                           `json:"end"`
	Args       []string                      `json:"args"`
	Chain      []phase7Diagnostic            `json:"chain"`
	Repopulate *ast.RepopulateDiagnosticInfo `json:"repopulate"`
}

func phase7Freeze(d *ast.Diagnostic) phase7Diagnostic {
	r := phase7Diagnostic{d.Code(), d.Pos(), d.End(), d.MessageArgs(), []phase7Diagnostic{}, d.RepopulateInfo()}
	for _, c := range d.MessageChain() {
		r.Chain = append(r.Chain, phase7Freeze(c))
	}
	return r
}
func TestPhase7AlternateResolution(t *testing.T) {
	opts := &core.CompilerOptions{NoLib: core.TSTrue, Module: core.ModuleKindESNext, ModuleResolution: core.ModuleResolutionKindBundler}
	fs := vfstest.FromMap(map[string]string{"/p/main.ts": "import {value} from \"@scope/pkg\";\n"}, true)
	p := compiler.NewProgram(compiler.ProgramOptions{Host: compiler.NewCompilerHost("/p", fs, "/lib", nil, nil, nil), Config: &tsoptions.ParsedCommandLine{ParsedConfig: &tsoptions.ParsedOptions{FileNames: []string{"/p/main.ts"}, CompilerOptions: opts}}, SingleThreaded: core.TSTrue})
	rows := []phase7Diagnostic{}
	for _, alternate := range []string{"/p/node_modules/@scope/pkg/index.d.ts", "/p/node_modules/@types/scope__pkg/index.d.ts"} {
		supplied := &phase7AlternateProgram{p, &module.ResolvedModule{AlternateResult: alternate}}
		c, _ := checker.NewChecker(supplied, nil)
		diags := c.GetDiagnostics(context.Background(), p.GetSourceFiles()[0])
		if len(diags) != 1 || diags[0].Code() != 2307 || len(diags[0].MessageChain()) != 1 {
			t.Fatalf("unexpected diagnostics: %v", diags)
		}
		if diags[0].File() != p.GetSourceFiles()[0] || diags[0].MessageChain()[0].File() != p.GetSourceFiles()[0] {
			t.Fatal("diagnostic source file lost")
		}
		rows = append(rows, phase7Freeze(diags[0]))
	}
	raw, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_ALTERNATE_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
