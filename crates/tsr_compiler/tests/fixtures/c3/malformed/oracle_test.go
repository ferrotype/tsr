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

// The command line stops at a file's syntax errors; this records what the
// checker itself reports for such a file: the parser's, the binder's, the
// checker's file diagnostics and the global diagnostics, in production order.
func TestC3Malformed(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C3_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID    string            `json:"id"`
		Root  string            `json:"root"`
		Files map[string]string `json:"files"`
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
		opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue}
		config := tsoptions.NewParsedCommandLine(opts, []string{r.Root}, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		file := program.GetSourceFile(r.Root)
		c, _ := checker.NewChecker(program, nil)
		ctx := context.Background()
		rows = append(rows, map[string]any{"id": r.ID,
			"syntactic": diagnostics(program.GetSyntacticDiagnostics(ctx, file)),
			"bind":      diagnostics(file.BindDiagnostics()),
			"checker":   diagnostics(c.GetDiagnostics(ctx, file)),
			"global":    diagnostics(c.GetGlobalDiagnostics())})
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C3_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
