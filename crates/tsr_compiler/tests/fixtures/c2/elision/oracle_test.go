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
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

func TestC2Elision(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Source  string   `json:"source"`
		Queries []string `json:"queries"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	fs := bundled.WrapFS(vfstest.FromMap(map[string]string{"/recursive_conditional.ts": request.Source}, true))
	host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
	opts := &core.CompilerOptions{Target: core.ScriptTargetES2015, NoErrorTruncation: core.TSTrue, SkipDefaultLibCheck: core.TSTrue}
	config := tsoptions.NewParsedCommandLine(opts, []string{"/recursive_conditional.ts"}, nil, tspath.ComparePathsOptions{})
	program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
	program.BindSourceFiles()
	c, _ := checker.NewChecker(program, nil)
	file := program.GetSourceFile("/recursive_conditional.ts")
	c.GetDiagnostics(context.Background(), file)
	queries := []any{}
	for _, name := range request.Queries {
		var found *ast.Node
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			if n.Kind == ast.KindTypeAliasDeclaration && n.Name().Text() == name {
				found = n.Name()
			}
			n.ForEachChild(visit)
			return false
		}
		visit(file.AsNode())
		if found == nil {
			t.Fatal("missing declaration", name)
		}
		typ := c.GetTypeAtLocation(found)
		text := c.TypeToStringEx(typ, nil, checker.TypeFormatFlagsAllowUniqueESSymbolType|checker.TypeFormatFlagsUseAliasDefinedOutsideCurrentScope, nil)
		hash := sha256.Sum256([]byte(text))
		queries = append(queries, map[string]any{"declaration": name, "bytes": len(text), "sha256": hex.EncodeToString(hash[:])})
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "queries": queries}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
