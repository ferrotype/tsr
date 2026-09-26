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

func TestC2AuditGaps(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID      string            `json:"id"`
		Files   map[string]string `json:"files"`
		Queries []string          `json:"queries"`
		Mode    string            `json:"mode"`
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
			out = append(out, map[string]any{"file": f, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(), "message": d.Localize(locale.Default), "reports_deprecated": d.ReportsDeprecated(), "chain": diagnostics(d.MessageChain()), "related": diagnostics(d.RelatedInformation())})
		}
		return out
	}
	rows := []any{}
	for _, r := range requests {
		fs := bundled.WrapFS(vfstest.FromMap(r.Files, false))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, SkipLibCheck: core.TSTrue}
		config := tsoptions.NewParsedCommandLine(opts, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		file := program.GetSourceFile("/main.ts")
		nodes := map[string]*ast.Node{}
		var visit func(*ast.Node) bool
		visit = func(n *ast.Node) bool {
			if n.Name() != nil && (n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindFunctionDeclaration || n.Kind == ast.KindTypeAliasDeclaration || n.Kind == ast.KindTypeParameter || n.Kind == ast.KindClassDeclaration || n.Kind == ast.KindInterfaceDeclaration || n.Kind == ast.KindImportEqualsDeclaration) {
				nodes[n.Name().Text()] = n
			}
			n.ForEachChild(visit)
			return false
		}
		visit(file.AsNode())
		c, _ := checker.NewChecker(program, nil)
		row := map[string]any{"id": r.ID}
		if r.ID == "mapped_property_cycle" {
			row["cycle"] = checker.C2AuditMappedCycle(c, nodes["mapped"].Name())
		}
		if r.ID == "type_parameter_helpers" {
			row["parameters"] = checker.C2AuditParameters(c, nodes)
		}
		row["diagnostics"] = diagnostics(c.GetDiagnostics(context.Background(), file))
		row["suggestions"] = diagnostics(c.GetSuggestionDiagnostics(context.Background(), file))
		types := []*checker.Type{}
		for _, name := range r.Queries {
			types = append(types, c.GetTypeAtLocation(nodes[name].Name()))
		}
		same := [][]bool{}
		for _, a := range types {
			eq := []bool{}
			for _, b := range types {
				eq = append(eq, checker.C2AuditIdentical(c, a, b))
			}
			same = append(same, eq)
		}
		row["identity"] = same
		row["later"] = c.TypeToString(c.GetTypeAtLocation(nodes["later"].Name()))
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
