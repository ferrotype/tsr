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

func TestC2AliasRawArguments(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID     string   `json:"id"`
		Source string   `json:"source"`
		Alias  string   `json:"alias"`
		Order  []string `json:"order"`
	}
	if err := json.Unmarshal(raw, &requests); err != nil {
		t.Fatal(err)
	}
	rows := []any{}
	for _, request := range requests {
		fs := bundled.WrapFS(vfstest.FromMap(map[string]string{"/main.ts": request.Source}, false))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		options := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, SkipLibCheck: core.TSTrue}
		config := tsoptions.NewParsedCommandLine(options, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		file := program.GetSourceFile("/main.ts")
		declarations := map[string]*ast.Node{}
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			if (n.Kind == ast.KindTypeAliasDeclaration || n.Kind == ast.KindVariableDeclaration) && n.Name() != nil {
				declarations[n.Name().Text()] = n
			}
			n.ForEachChild(visit)
			return false
		}
		visit(file.AsNode())
		c, _ := checker.NewChecker(program, nil)
		alias := c.GetSymbolAtLocation(declarations[request.Alias].Name())
		c.GetDeclaredTypeOfSymbol(alias)
		baseline := checker.C2AliasCacheSnapshot(c, alias)
		queries := []any{}
		types := []*checker.Type{}
		nodes := []*ast.Node{}
		for _, name := range request.Order {
			node := declarations[name]
			if node == nil {
				t.Fatal("missing declaration", name)
			}
			before := checker.C2AliasCacheSnapshot(c, alias)
			typ := c.GetTypeAtLocation(node.Name())
			after := checker.C2AliasCacheSnapshot(c, alias)
			types = append(types, typ)
			nodes = append(nodes, node)
			queries = append(queries, map[string]any{"name": name, "before": before, "after": after})
		}
		// Only pointer equality is observed; no ID accessors are called.
		same := [][]bool{}
		for _, a := range types {
			row := []bool{}
			for _, b := range types {
				row = append(row, a == b)
			}
			same = append(same, row)
		}
		// Display and alias rendering occur after the entire lookup sequence.
		displays := []string{}
		aliasArguments := [][]string{}
		for index, typ := range types {
			displays = append(displays, c.TypeToStringEx(typ, nodes[index], 0, nil))
			aliasArguments = append(aliasArguments, checker.C2AliasArguments(c, typ))
		}
		// Diagnostics use a fresh checker so display cannot affect this observation.
		dc, _ := checker.NewChecker(program, nil)
		diagnostics := []any{}
		for _, d := range dc.GetDiagnostics(context.Background(), file) {
			diagnostics = append(diagnostics, map[string]any{"code": d.Code(), "pos": d.Pos(), "end": d.End(), "text": d.MessageText()})
		}
		rows = append(rows, map[string]any{"id": request.ID, "baseline": baseline, "queries": queries, "same": same, "display": displays, "alias_arguments": aliasArguments, "diagnostics": diagnostics})
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "request_sha256": hex.EncodeToString(hash[:]), "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("C2_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
