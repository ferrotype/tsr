package printer

// Copy this test into the pinned internal/printer package. It exercises
// public factory helpers directly; it does not claim source-code reachability.
import (
	"encoding/json"
	"fmt"
	"os"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

func TestPhase7NilDeclarationNames(t *testing.T) {
	type row struct {
		Helper    string `json:"helper"`
		Kind      int    `json:"kind"`
		NodeNil   bool   `json:"node_nil"`
		EmitFlags int    `json:"emit_flags"`
		Output    string `json:"output"`
	}
	rows := []row{}
	for _, helper := range []string{"local", "export", "declaration"} {
		ec := NewEmitContext()
		var name *ast.Node
		switch helper {
		case "local":
			name = ec.Factory.GetLocalName(nil)
		case "export":
			name = ec.Factory.GetExportName(nil)
		case "declaration":
			name = ec.Factory.GetDeclarationName(nil)
		}
		info := ec.GetAutoGenerateInfo(name)
		p := NewPrinter(PrinterOptions{NewLine: core.NewLineKindLF}, PrintHandlers{}, ec)
		rows = append(rows, row{helper, int(info.Flags), info.Node == nil, int(ec.EmitFlags(name)), phase7EmitNil(p, name)})
	}
	raw, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE7_NIL_NAME_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}

// The factory accepts nil; printing the resulting node is a separate invalid-input boundary.
func phase7EmitNil(p *Printer, name *ast.Node) (result string) {
	defer func() {
		if v := recover(); v != nil {
			result = fmt.Sprint(v)
		}
	}()
	return p.Emit(name, nil)
}
