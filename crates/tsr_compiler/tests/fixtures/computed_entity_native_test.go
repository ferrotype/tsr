package checker_test

// Real parsed/bound source through public checker type display. Copy into an
// exported pin's internal/checker package; set PHASE7_COMPUTED_OUTPUT.
import (
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"testing"
)

func TestPhase7ComputedEntityDisplay(t *testing.T) {
	source := "declare const key: unique symbol;\nclass Symbols { static readonly [key]: unique symbol; }\nconst value = Symbols[key];\n"
	opts := &core.CompilerOptions{NoLib: core.TSTrue, Strict: core.TSTrue, Module: core.ModuleKindESNext, Target: core.ScriptTargetESNext}
	fs := vfstest.FromMap(map[string]string{"/p/main.ts": source}, true)
	p := compiler.NewProgram(compiler.ProgramOptions{Host: compiler.NewCompilerHost("/p", fs, "/lib", nil, nil, nil), Config: &tsoptions.ParsedCommandLine{ParsedConfig: &tsoptions.ParsedOptions{FileNames: []string{"/p/main.ts"}, CompilerOptions: opts}}, SingleThreaded: core.TSTrue})
	c, _ := checker.NewChecker(p, nil)
	file := p.GetSourceFiles()[0]
	declaration := file.Statements.Nodes[2].AsVariableStatement().DeclarationList.AsVariableDeclarationList().Declarations.Nodes[0]
	if declaration.Name().Kind != ast.KindIdentifier || declaration.Initializer().Kind != ast.KindElementAccessExpression {
		t.Fatal("unexpected source shape")
	}
	rows := []string{}
	for _, flags := range []checker.TypeFormatFlags{0, checker.TypeFormatFlagsUseFullyQualifiedType} {
		rows = append(rows, c.TypeToStringEx(c.GetTypeAtLocation(declaration.Initializer()), file.AsNode(), flags, nil))
	}
	raw, err := json.MarshalIndent(map[string]any{"source": source, "display": rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_COMPUTED_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
