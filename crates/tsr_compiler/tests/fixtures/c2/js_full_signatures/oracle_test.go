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
	"runtime"
	"testing"
)

func TestC2JSFullSignatures(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID    string            `json:"id"`
		Files map[string]string `json:"files"`
		Roots []string          `json:"roots"`
		Cwd   string            `json:"cwd"`
	}
	if err = json.Unmarshal(raw, &requests); err != nil {
		t.Fatal(err)
	}
	var diagnostics func([]*ast.Diagnostic) []any
	diagnostics = func(ds []*ast.Diagnostic) []any {
		out := []any{}
		for _, d := range ds {
			var file any
			if d.File() != nil {
				file = d.File().FileName()
			}
			out = append(out, map[string]any{"file": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(), "message": d.Localize(locale.Default), "chain": diagnostics(d.MessageChain()), "related": diagnostics(d.RelatedInformation())})
		}
		return out
	}
	rows := []any{}
	for _, r := range requests {
		fs := bundled.WrapFS(vfstest.FromMap(r.Files, false))
		host := compiler.NewCompilerHost(r.Cwd, fs, bundled.LibPath(), nil, nil, nil)
		opts := &core.CompilerOptions{Target: core.ScriptTargetES2015, Module: core.ModuleKindCommonJS, AllowJs: core.TSTrue, CheckJs: core.TSTrue, NoEmit: core.TSTrue, SkipDefaultLibCheck: core.TSTrue}
		config := tsoptions.NewParsedCommandLine(opts, r.Roots, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		c, _ := checker.NewChecker(program, nil)
		ds := []any{}
		queries := []any{}
		for _, name := range r.Roots {
			file := program.GetSourceFile(name)
			ds = append(ds, diagnostics(c.GetDiagnostics(context.Background(), file))...)
			var visit func(*ast.Node) bool
			visit = func(n *ast.Node) bool {
				if n.Name() != nil && (n.Kind == ast.KindVariableDeclaration || n.Kind == ast.KindFunctionDeclaration) {
					queries = append(queries, map[string]any{"file": name, "pos": n.Name().Pos(), "end": n.Name().End(), "text": c.TypeToString(c.GetTypeAtLocation(n.Name()))})
				}
				n.ForEachChild(visit)
				return false
			}
			visit(file.AsNode())
		}
		rows = append(rows, map[string]any{"id": r.ID, "diagnostics": ds, "queries": queries})
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C2_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
