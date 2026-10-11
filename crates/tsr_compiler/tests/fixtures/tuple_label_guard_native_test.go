package checker

import (
	"encoding/json"
	"fmt"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"os"
	"testing"
)

func TestPhase7InvalidTupleLabel(t *testing.T) {
	factory := &ast.NodeFactory{}
	c := &Checker{}
	rows := []map[string]any{}
	for _, kind := range []ast.Kind{ast.KindObjectBindingPattern, ast.KindArrayBindingPattern} {
		name := factory.NewBindingPattern(kind, factory.NewNodeList(nil))
		declaration := factory.NewParameterDeclaration(nil, nil, name, nil, nil, nil)
		var recovered any
		func() {
			defer func() { recovered = recover() }()
			c.getTupleElementLabel(TupleElementInfo{flags: ElementFlagsRequired, labeledDeclaration: declaration}, nil, 0)
		}()
		if recovered == nil {
			t.Fatal("invalid label did not panic")
		}
		rows = append(rows, map[string]any{"name_kind": int(kind), "panic": fmt.Sprint(recovered)})
	}
	raw, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE7_TUPLE_GUARD_OUTPUT"), append(raw, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
