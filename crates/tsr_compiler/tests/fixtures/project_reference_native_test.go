package checker_test

import (
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"testing"
)

func TestPhase7ReferencedImporterDisplay(t *testing.T) {
	files := map[string]string{
		"/p/tsconfig.json":     `{"compilerOptions":{"module":"esnext","declaration":true,"noLib":true},"files":["main.ts"],"references":[{"path":"./ref"}]}`,
		"/p/ref/tsconfig.json": `{"compilerOptions":{"composite":true,"outDir":"out","rootDir":"src","module":"esnext","noLib":true}}`,
		"/p/ref/src/a.ts":      "export interface Foo { x: number }\nexport declare function make(): Foo;\n",
		"/p/ref/out/a.d.ts":    "export interface Foo {\n    x: number;\n}\nexport declare function make(): Foo;\n",
		"/p/ref/src/b.ts":      "import { make } from \"./a\";\nimport {} from \"../out/a\";\nexport const w = make;\n",
		"/p/ref/out/b.d.ts":    "import { make } from \"./a\";\nimport {} from \"../out/a\";\nexport declare const w: typeof make;\n",
		"/p/main.ts":           "import { w } from \"./ref/src/b\";\nexport const v = w;\n",
	}
	fs := vfstest.FromMap(files, true)
	host := compiler.NewCompilerHost("/p", fs, "/lib", nil, nil, nil)
	parsed, errors := tsoptions.GetParsedCommandLineOfConfigFile("/p/tsconfig.json", nil, nil, host, nil)
	if len(errors) != 0 {
		t.Fatalf("config diagnostics: %v", errors)
	}
	p := compiler.NewProgram(compiler.ProgramOptions{Host: host, Config: parsed, SingleThreaded: core.TSTrue})
	c, _ := checker.NewChecker(p, nil)
	rows := []map[string]any{}
	for _, query := range []struct {
		File      string
		Statement int
	}{{"/p/main.ts", 1}, {"/p/ref/out/b.d.ts", 2}} {
		file := p.GetSourceFile(query.File)
		if file == nil {
			t.Fatalf("missing file %s", query.File)
		}
		d := file.Statements.Nodes[query.Statement].AsVariableStatement().DeclarationList.AsVariableDeclarationList().Declarations.Nodes[0]
		display := c.TypeToStringEx(c.GetTypeAtLocation(d.Name()), d, 0, nil)
		rows = append(rows, map[string]any{"file": query.File, "statement": query.Statement, "display": display})
	}
	raw, err := json.MarshalIndent(map[string]any{"files": files, "queries": rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_PROJECTREF_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
