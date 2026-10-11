package checker_test

import (
	"context"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"testing"
)

type phase7AccessorDiagnostic struct {
	Code    int32    `json:"code"`
	FileNil bool     `json:"file_nil"`
	Pos     int      `json:"pos"`
	End     int      `json:"end"`
	Args    []string `json:"args"`
}

func phase7AccessorFreeze(ds []*ast.Diagnostic) []phase7AccessorDiagnostic {
	r := []phase7AccessorDiagnostic{}
	for _, d := range ds {
		r = append(r, phase7AccessorDiagnostic{d.Code(), d.File() == nil, d.Pos(), d.End(), d.MessageArgs()})
	}
	return r
}
func TestPhase7CircularAccessor(t *testing.T) {
	rows := []map[string]any{}
	for _, annotation := range []string{"typeof this.x", `A["x"]`, "typeof A.prototype.x"} {
		source := "export class A { accessor x: " + annotation + "; }\n"
		fs := vfstest.FromMap(map[string]string{"/p/main.ts": source}, true)
		opts := &core.CompilerOptions{NoLib: core.TSTrue, Strict: core.TSTrue, Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext}
		p := compiler.NewProgram(compiler.ProgramOptions{Host: compiler.NewCompilerHost("/p", fs, "/lib", nil, nil, nil), Config: &tsoptions.ParsedCommandLine{ParsedConfig: &tsoptions.ParsedOptions{FileNames: []string{"/p/main.ts"}, CompilerOptions: opts}}, SingleThreaded: core.TSTrue})
		c, _ := checker.NewChecker(p, nil)
		file := p.GetSourceFiles()[0]
		before := phase7AccessorFreeze(c.GetGlobalDiagnostics())
		semantic := phase7AccessorFreeze(c.GetDiagnostics(context.Background(), file))
		member := file.Statements.Nodes[0].AsClassDeclaration().Members.Nodes[0]
		display := c.TypeToStringEx(c.GetTypeAtLocation(member.Name()), file.AsNode(), 0, nil)
		after := phase7AccessorFreeze(c.GetGlobalDiagnostics())
		rows = append(rows, map[string]any{"source": source, "before": before, "semantic": semantic, "after": after, "display": display})
	}
	raw, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_ACCESSOR_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
