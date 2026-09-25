// Phase 1 operation tables: group positions (names, type and expression
// positions, access kinds). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/positions.rs and the spec is
// data/phase1/tables/positions.json.
package main

import (
	"encoding/json"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

func init() {
	Register("positions",
		// ast/utilities.go:IsDeclarationName over every node of a parsed
		// file: the document-order indices where it is true.
		Column{
			ID:    "ast.IsDeclarationName",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for i, node := range p.Nodes {
						if ast.IsDeclarationName(node) {
							out = append(out, i)
						}
					}
					return out
				}, nil
			},
			Survey: func(p *Parsed) []string {
				return NodeClasses(p, func(node *ast.Node) string { return BoolClass(ast.IsDeclarationName(node)) })
			},
		},
	)
}
