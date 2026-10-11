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

func TestPhase7TupleLabelDisplay(t *testing.T) {
	rows := []map[string]any{}
	for _, parameters := range []string{"{x}: {x: number}, [y]: [string], z?: boolean", "...[first, ...tail]: [number, ...string[]]", "first: number, second?: string"} {
		source := "type Args<T extends (...args: any) => any> = T extends (...args: infer P) => any ? P : never;\ndeclare function f(" + parameters + "): void;\ntype Result = Args<typeof f>;\n"
		fs := vfstest.FromMap(map[string]string{"/main.ts": source}, true)
		opts := &core.CompilerOptions{NoLib: core.TSTrue, Strict: core.TSTrue, Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext}
		p := compiler.NewProgram(compiler.ProgramOptions{Host: compiler.NewCompilerHost("/", fs, "/lib", nil, nil, nil), Config: &tsoptions.ParsedCommandLine{ParsedConfig: &tsoptions.ParsedOptions{FileNames: []string{"/main.ts"}, CompilerOptions: opts}}, SingleThreaded: core.TSTrue})
		c, _ := checker.NewChecker(p, nil)
		file := p.GetSourceFiles()[0]
		alias := file.Statements.Nodes[2]
		ty := c.GetTypeAtLocation(alias.Name())
		display := c.TypeToStringEx(ty, file.AsNode(), checker.TypeFormatFlagsInTypeAlias, nil)
		rows = append(rows, map[string]any{"source": source, "display": display})
	}
	raw, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_TUPLE_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
