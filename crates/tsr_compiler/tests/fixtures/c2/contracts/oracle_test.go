package checker_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"path"
	"runtime"
	"testing"
)

func TestC2Contracts(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID      string   `json:"id"`
		Source  string   `json:"source"`
		Queries []string `json:"queries"`
		Mode    string   `json:"mode"`
	}
	if err = json.Unmarshal(raw, &requests); err != nil {
		t.Fatal(err)
	}
	var diagnostics func([]*ast.Diagnostic) []any
	diagnostics = func(ds []*ast.Diagnostic) []any {
		out := []any{}
		for _, d := range ds {
			var f any
			if d.File() != nil {
				f = path.Base(d.File().FileName())
			}
			out = append(out, map[string]any{"file": f, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(), "message": d.Localize(locale.Default), "chain": diagnostics(d.MessageChain()), "related": diagnostics(d.RelatedInformation())})
		}
		return out
	}
	rows := []any{}
	for _, r := range requests {
		fs := bundled.WrapFS(vfstest.FromMap(map[string]string{"/main.ts": r.Source}, false))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, SkipLibCheck: core.TSTrue}
		config := tsoptions.NewParsedCommandLine(opts, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		file := program.GetSourceFile("/main.ts")
		nodes := map[string]*ast.Node{}
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			if n.Name() != nil && (n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindFunctionDeclaration || n.Kind == ast.KindTypeAliasDeclaration) {
				nodes[n.Name().Text()] = n
			}
			n.ForEachChild(visit)
			return false
		}
		visit(file.AsNode())
		c, _ := checker.NewChecker(program, nil)
		row := map[string]any{"id": r.ID}
		if r.Mode == "inference" {
			row["inference"] = checker.C2ContractInference(c, nodes["seed"].Name())
		}
		row["before_context_depth"] = checker.C2ContractContextDepth(c)
		checker.C2ContractBeginInstantiation(c)
		row["diagnostics"] = diagnostics(c.GetDiagnostics(context.Background(), file))
		row["after_context_depth"] = checker.C2ContractContextDepth(c)
		row["instantiation"] = checker.C2ContractTakeInstantiation(c)
		if r.ID == "overload" {
			row["overload_symbol"] = checker.C2ContractOverloadSymbol(c, nodes["combined"])
		}
		types := []*checker.Type{}
		displays := []string{}
		same := [][]bool{}
		for _, name := range r.Queries {
			n := nodes[name]
			if n == nil {
				t.Fatal("missing", name)
			}
			types = append(types, c.GetTypeAtLocation(n.Name()))
		}
		for _, a := range types {
			eq := []bool{}
			for _, b := range types {
				eq = append(eq, a == b)
			}
			same = append(same, eq)
		}
		for i, typ := range types {
			displays = append(displays, c.TypeToStringEx(typ, nodes[r.Queries[i]], 0, nil))
		}
		row["display"] = displays
		row["same"] = same
		if r.Mode == "higher_order" {
			row["higher_order"] = checker.C2ContractHigherOrder(c, types[0])
		}
		rows = append(rows, row)
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C2_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
