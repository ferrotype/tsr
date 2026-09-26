package checker_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

func TestC2AwaitedOperators(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Source string `json:"source"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	fs := bundled.WrapFS(vfstest.FromMap(map[string]string{
		"/awaited_operators.ts": request.Source,
	}, true))
	host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
	opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Strict: core.TSTrue}
	config := tsoptions.NewParsedCommandLine(opts, []string{"/awaited_operators.ts"}, nil, tspath.ComparePathsOptions{})
	program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
	program.BindSourceFiles()
	c, _ := checker.NewChecker(program, nil)
	file := program.GetSourceFile("/awaited_operators.ts")
	var diagnostics func([]*ast.Diagnostic) []map[string]any
	diagnostics = func(values []*ast.Diagnostic) []map[string]any {
		result := make([]map[string]any, 0, len(values))
		for _, d := range values {
			var file any
			if d.File() != nil {
				file = d.File().FileName()
			}
			result = append(result, map[string]any{
				"file": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(),
				"category": d.Category(), "message": d.Localize(locale.Default),
				"chain": diagnostics(d.MessageChain()), "related": diagnostics(d.RelatedInformation()),
			})
		}
		return result
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{
		"version": 1, "request_sha256": hex.EncodeToString(hash[:]),
		"go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH,
		"diagnostics": diagnostics(c.GetDiagnostics(context.Background(), file)),
	}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
