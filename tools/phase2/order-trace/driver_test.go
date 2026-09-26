package checker_test

import (
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

func TestC2OrderTrace(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_ORDER_REQUEST"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Source   string   `json:"source"`
		Queries  []string `json:"queries"`
		Property string   `json:"property"`
		Union    bool     `json:"union"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	tracing := os.Getenv("C2_ORDER_TRACE") == "1"
	if tracing {
		ast.C2TraceBegin()
	}
	fs := vfstest.FromMap(map[string]string{"/main.ts": request.Source}, false)
	host := compiler.NewCompilerHost("/", fs, "/", nil, nil, nil)
	options := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, NoLib: core.TSTrue}
	config := tsoptions.NewParsedCommandLine(options, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
	program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
	program.BindSourceFiles()
	file := program.GetSourceFile("/main.ts")
	c, _ := checker.NewChecker(program, nil)
	diagnostics := []any{}
	for _, d := range c.GetDiagnostics(context.Background(), file) {
		var text strings.Builder
		diagnosticwriter.WriteFlattenedDiagnosticMessage(&text, &diagnosticwriter.ASTDiagnostic{Diagnostic: d}, "\n", locale.Default)
		diagnostics = append(diagnostics, map[string]any{"code": d.Code(), "pos": d.Pos(), "end": d.End(), "text": text.String()})
	}
	names := map[string]*ast.Node{}
	var visit func(*ast.Node) bool
	visit = func(n *ast.Node) bool {
		if (n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindTypeAliasDeclaration) && n.Name() != nil {
			names[n.Name().Text()] = n.Name()
		}
		n.ForEachChild(visit)
		return false
	}
	visit(file.AsNode())
	types := []*checker.Type{}
	displays := []string{}
	for _, name := range request.Queries {
		node := names[name]
		if node == nil {
			t.Fatal("query missing", name)
		}
		typ := c.GetTypeAtLocation(node)
		types = append(types, typ)
		displays = append(displays, c.TypeToStringEx(typ, node, 0, nil))
	}
	queried := checker.C2TraceTypes(types)
	var order any
	var propertyOrder any
	if request.Property != "" {
		order = checker.C2TracePropertyOrder(c, types, request.Property)
		propertyOrder = order.(map[string]any)["order"]
	}
	var union any
	if request.Union {
		union = checker.C2TraceUnion(c, types)
	}
	trace := []map[string]any{}
	if tracing {
		trace = ast.C2TraceFinish()
	}
	output := map[string]any{"ordinary": map[string]any{"diagnostics": diagnostics, "display": displays, "union": union,"property_order":propertyOrder}, "order": order, "types":queried, "trace": trace}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("C2_ORDER_OUTPUT"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
