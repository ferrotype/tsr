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
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// Focused native witnesses: program syntactic and semantic diagnostics for small
// no-lib programs. Strict checking defaults on; optional flags cover JS and
// noImplicitAny without changing the earlier strict fixture requests.
func s08P6Diagnostics(values []*ast.Diagnostic) []map[string]any {
	result := make([]map[string]any, 0, len(values))
	for _, d := range values {
		var file any
		if d.File() != nil {
			file = d.File().FileName()
		}
		result = append(result, map[string]any{"file": file, "pos": d.Pos(), "end": d.End(), "code": d.Code(), "category": d.Category(),
			"args": d.MessageArgs(), "chain": s08P6Diagnostics(d.MessageChain()), "related": s08P6Diagnostics(d.RelatedInformation()),
			"unnecessary": d.ReportsUnnecessary(), "skipped_on_no_emit": d.SkippedOnNoEmit()})
	}
	return result
}

func TestS08P6SemanticDiagnostics(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Version  int `json:"version"`
		Programs []struct {
			ID             string            `json:"id"`
			NoUnusedLocals bool              `json:"no_unused_locals"`
			Strict         *bool             `json:"strict"`
			AllowJs        *bool             `json:"allow_js"`
			CheckJs        *bool             `json:"check_js"`
			NoImplicitAny  *bool             `json:"no_implicit_any"`
			Files          map[string]string `json:"files"`
			Roots          []string          `json:"roots"`
		} `json:"programs"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	if request.Version != 1 {
		t.Fatal("unknown request version")
	}
	programs := []any{}
	for _, r := range request.Programs {
		host := compiler.NewCompilerHost("/", vfstest.FromMap(r.Files, true), "/no-default-lib", nil, nil, nil)
		opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, NoLib: core.TSTrue,
			NoUnusedLocals: core.IfElse(r.NoUnusedLocals, core.TSTrue, core.TSUnknown)}
		for _, option := range []struct {
			value  *bool
			target *core.Tristate
		}{
			{r.Strict, &opts.Strict}, {r.AllowJs, &opts.AllowJs}, {r.CheckJs, &opts.CheckJs}, {r.NoImplicitAny, &opts.NoImplicitAny},
		} {
			if option.value != nil {
				*option.target = core.IfElse(*option.value, core.TSTrue, core.TSFalse)
			}
		}
		config := tsoptions.NewParsedCommandLine(opts, r.Roots, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host, SingleThreaded: core.TSTrue})
		ctx := context.Background()
		programs = append(programs, map[string]any{"id": r.ID,
			"syntactic": s08P6Diagnostics(program.GetSyntacticDiagnostics(ctx, nil)),
			"semantic":  s08P6Diagnostics(program.GetSemanticDiagnostics(ctx, nil))})
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "programs": programs}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
