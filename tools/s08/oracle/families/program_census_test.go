package checker_test

// S08 P7 census over whole programs (tools/s08/p7/census-fixtures.json): build
// each fixture program in memory, check it, retain the named declared types as
// roots and write the structural census of the checker.

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
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

type s08CensusRequest struct {
	ID        string            `json:"id"`
	SourceHex string            `json:"source_hex"`
	Files     map[string]string `json:"files"`
	AllowJS   bool              `json:"allow_js"`
	Roots     []string          `json:"roots"`
	Rule      string            `json:"rule,omitempty"`
}

func TestS08ProgramCensus(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var requests []s08CensusRequest
	if err := decoder.Decode(&requests); err != nil {
		t.Fatal(err)
	}
	if decoder.Decode(new(any)) != io.EOF {
		t.Fatal("trailing request")
	}
	rows := []map[string]any{}
	for _, request := range requests {
		row := map[string]any{"id": request.ID, "state": "not_executed"}
		t.Run(request.ID, func(t *testing.T) {
			defer func() {
				if value := recover(); value != nil {
					row["state"] = "panic"
					row["panic"] = fmt.Sprint(value)
					t.Error(value)
				}
			}()
			content, err := hex.DecodeString(request.SourceHex)
			if err != nil {
				t.Fatal(err)
			}
			files := map[string]string{"/fixture.ts": string(content)}
			for path, text := range request.Files {
				data, err := hex.DecodeString(text)
				if err != nil {
					t.Fatal(err)
				}
				files[path] = string(data)
			}
			fs := bundled.WrapFS(vfstest.FromMap(files, false))
			host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
			options := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, SkipLibCheck: core.TSTrue, SingleThreaded: core.TSTrue}
			if request.AllowJS {
				options.AllowJs = core.TSTrue
			}
			config := tsoptions.NewParsedCommandLine(options, []string{"/fixture.ts"}, nil, tspath.ComparePathsOptions{})
			program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host, SingleThreaded: core.TSTrue})
			program.BindSourceFiles()
			file := program.GetSourceFile("/fixture.ts")
			if file == nil {
				t.Fatal("missing source")
			}
			runtime.GC()
			checker.S08CensusBegin()
			ctx := t.Context()
			diagnostics := len(program.GetSemanticDiagnostics(ctx, nil)) + len(program.GetGlobalDiagnostics(ctx))
			c, done := program.GetTypeChecker(ctx)
			defer done()
			roots := []*checker.Type{}
			for _, wanted := range request.Roots {
				var found *ast.Node
				for _, statement := range file.Statements.Nodes {
					if name := statement.Name(); name != nil && name.Text() == wanted {
						found = name
					}
				}
				if found == nil {
					t.Fatalf("root declaration %s missing", wanted)
				}
				symbol := c.GetSymbolAtLocation(found)
				if symbol == nil {
					t.Fatalf("root symbol %s missing", wanted)
				}
				roots = append(roots, c.GetDeclaredTypeOfSymbol(symbol))
			}
			row["diagnostics"] = diagnostics
			row["roots"] = len(roots)
			row["types_created"] = c.TypeCount
			row["symbols_created"] = c.SymbolCount
			row["signatures_created"] = c.SignatureCount
			row["census"] = checker.S08Census(c, roots)
			row["state"] = "executed"
		})
		rows = append(rows, row)
	}
	output, err := json.Marshal(map[string]any{"version": 1, "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), output, 0600); err != nil {
		t.Fatal(err)
	}
}
