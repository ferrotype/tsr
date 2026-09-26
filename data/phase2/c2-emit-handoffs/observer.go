package checker

import (
	"fmt"
	"os"
	rdebug "runtime/debug"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// Called only at the pinned circular-diagnostic production sites. Reading node
// kind/range and the Go stack does not ask the checker any additional question.
func c2EmitStack(label string, current *ast.Node, declaration *ast.Node) {
	fmt.Fprintf(os.Stderr, "C2STACK %s current=%v decl=%v\n", label, c2Location(current), c2Location(declaration))
	os.Stderr.Write(rdebug.Stack())
}

func c2Location(node *ast.Node) any {
	if node == nil {
		return nil
	}
	return []any{node.Kind, node.Pos(), node.End()}
}
