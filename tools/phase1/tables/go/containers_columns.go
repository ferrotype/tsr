// Phase 1 operation tables: group containers (containers, function flags,
// precedence, reparse identity, source-file tables, outer expressions).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/containers.rs and the spec is
// data/phase1/tables/containers.json.
package main

import (
	"encoding/json"
	"fmt"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
)

// statementContainers are the nodes whose Node.CanHaveStatements holds
// (ast.go:604), with their statement lists, both taken in setup.
func statementContainers(p *Parsed) ([]int, [][]*ast.Node) {
	at, lists := []int{}, [][]*ast.Node{}
	for i, node := range p.Nodes {
		if node.CanHaveStatements() {
			at = append(at, i)
			lists = append(lists, node.Statements())
		}
	}
	return at, lists
}

func init() {
	Register("containers",
		// binder/binder.go:FindUseStrictPrologue (and its helper
		// isUseStrictPrologueDirective) on the statements of every statement
		// container: [container index, result reference] where non-nil.
		Column{
			ID:    "binder.FindUseStrictPrologue",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				at, lists := statementContainers(p)
				return func() any {
					out := []any{}
					for i, statements := range lists {
						if found := binder.FindUseStrictPrologue(p.File, statements); found != nil {
							out = append(out, []any{at[i], p.Ref(found)})
						}
					}
					return out
				}, nil
			},
			// Per container: kind, prologue count (0-3), the position of the
			// found prologue (-1, 0, 1, 2+) and whether statements follow.
			Survey: func(p *Parsed) []string {
				classes := []string{}
				at, lists := statementContainers(p)
				for i, statements := range lists {
					prologues := 0
					for _, statement := range statements {
						if !ast.IsPrologueDirective(statement) {
							break
						}
						prologues++
					}
					found := binder.FindUseStrictPrologue(p.File, statements)
					position := -1
					for j, statement := range statements {
						if statement == found {
							position = j
						}
					}
					classes = append(classes, fmt.Sprintf("%d/%d/%d/%d", p.Nodes[at[i]].Kind, min(prologues, 3),
						min(position, 2), min(len(statements)-prologues, 1)))
				}
				return classes
			},
		},
	)
}
