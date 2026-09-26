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

func TestC2VarianceLimits(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID      string     `json:"id"`
		Source  string     `json:"source"`
		Queries []string   `json:"queries"`
		Mode    string     `json:"mode"`
		Target  string     `json:"target"`
		Stacks  [][]string `json:"stacks"`
		Max     int        `json:"max"`
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
			if n.Name() != nil && (n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindFunctionDeclaration || n.Kind == ast.KindTypeAliasDeclaration || n.Kind == ast.KindInterfaceDeclaration || n.Kind == ast.KindClassDeclaration || n.Kind == ast.KindParameter || n.Kind == ast.KindTypeParameter) {
				nodes[n.Name().Text()] = n
			}
			n.ForEachChild(visit)
			return false
		}
		visit(file.AsNode())
		c, _ := checker.NewChecker(program, nil)
		row := map[string]any{"id": r.ID}

		if r.Mode == "variance" {
			values := []any{}
			for _, name := range r.Queries {
				values = append(values, checker.C2Variance(c, nodes[name].Name()))
			}
			row["measurements"] = values
		} else {
			checker.C2BeginLimits(c)
			node := nodes[r.Target].Name()
			switch r.Mode {
			case "count":
				row["result"] = checker.C2InstantiationCount(c, node)
			case "subtypes":
				row["result"] = checker.C2SubtypeLimit(c, node)
			case "constraint":
				row["result"] = checker.C2BaseConstraint(c, node)
			case "nesting":
				results := []bool{}
				for _, names := range r.Stacks {
					stack := []*ast.Node{}
					for _, name := range names {
						stack = append(stack, nodes[name].Name())
					}
					results = append(results, checker.C2Nested(c, node, stack, r.Max))
				}
				row["result"] = results
			}
			row["limits"] = checker.C2TakeLimits(c)
			recovery := c.GetTypeAtLocation(nodes["recovery"].Name())
			row["recovery"] = c.TypeToString(recovery)
			row["recovery_count"] = checker.C2InstantiationCountValue(c)
		}
		row["diagnostics"] = diagnostics(c.GetDiagnostics(context.Background(), file))

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
