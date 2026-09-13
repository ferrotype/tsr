package declarations

import (
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
	"os"
	"testing"
)

type selectorNode struct {
	Kind uint16 `json:"kind"`
	Pos  int    `json:"pos"`
	End  int    `json:"end"`
}

func selectorRef(node *ast.Node) *selectorNode {
	if node == nil {
		return nil
	}
	return &selectorNode{uint16(node.Kind), node.Pos(), node.End()}
}

type selectorObservation struct {
	Node        *selectorNode `json:"node"`
	NameContext bool          `json:"name_context"`
	Variant     int           `json:"variant"`
	Code        *int32        `json:"code"`
	ErrorNode   *selectorNode `json:"error_node"`
	TypeName    *selectorNode `json:"type_name"`
}

func TestS08DeclarationSelectors(t *testing.T) {
	source, err := os.ReadFile(os.Getenv("S08_SELECTOR_SOURCE"))
	if err != nil {
		t.Fatal(err)
	}
	file := parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: "/selectors.ts", Path: "/selectors.ts"}, string(source), core.ScriptKindTS)
	if len(file.Diagnostics()) != 0 {
		t.Fatal("selector source has parse diagnostics")
	}
	var result []selectorObservation
	var visit func(*ast.Node) bool
	visit = func(node *ast.Node) bool {
		if canProduceDiagnostics(node) {
			for _, nameContext := range []bool{false, true} {
				var selectDiagnostic GetSymbolAccessibilityDiagnostic
				if nameContext {
					selectDiagnostic = createGetSymbolAccessibilityDiagnosticForNodeName(node)
				} else {
					selectDiagnostic = createGetSymbolAccessibilityDiagnosticForNode(node)
				}
				for variant := 0; variant < 4; variant++ {
					input := printer.SymbolAccessibilityResult{Accessibility: printer.SymbolAccessibilityNotAccessible, ErrorSymbolName: "Hidden"}
					if variant&1 != 0 {
						input.ErrorModuleName = "module"
					}
					if variant&2 != 0 {
						input.Accessibility = printer.SymbolAccessibilityCannotBeNamed
					}
					record := selectorObservation{Node: selectorRef(node), NameContext: nameContext, Variant: variant}
					if diagnostic := selectDiagnostic(input); diagnostic != nil {
						code := diagnostic.diagnosticMessage.Code()
						record.Code = &code
						record.ErrorNode = selectorRef(diagnostic.errorNode)
						record.TypeName = selectorRef(diagnostic.typeName)
					}
					result = append(result, record)
				}
			}
		}
		node.ForEachChild(visit)
		return false
	}
	visit(file.AsNode())
	data, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("S08_SELECTOR_OUTPUT"), append(data, '\n'), 0644); err != nil {
		t.Fatal(err)
	}
}
