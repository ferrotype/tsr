// Phase 1 operation tables: group class (classes, heritage, decorators,
// modifiers). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/class.rs and the spec is
// data/phase1/tables/class.json.
package main

import (
	"encoding/json"
	"fmt"
)

func init() {
	Register("class",
		// ast/ast.go:Node.Decorators over every node: [index, [decorator
		// references]] for each node with a nonempty result. Callers read
		// only length and elements (checker.go:10240,10271,29040;
		// legacydecorators.go:666,723), so nil and empty are one value.
		Column{
			ID:    "ast.Node.Decorators",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for i, node := range p.Nodes {
						if decorators := node.Decorators(); len(decorators) > 0 {
							out = append(out, []any{i, p.Refs(decorators)})
						}
					}
					return out
				}, nil
			},
			// Per node with modifiers: kind, decorator count (0, 1, 2+) and
			// whether other modifiers stand beside them.
			Survey: func(p *Parsed) []string {
				classes := []string{}
				for _, node := range p.Nodes {
					modifiers := node.Modifiers()
					if modifiers == nil {
						continue
					}
					decorators := len(node.Decorators())
					classes = append(classes, fmt.Sprintf("%d/%d/%d", node.Kind, min(decorators, 2),
						min(len(modifiers.Nodes)-decorators, 1)))
				}
				return classes
			},
		},
	)
}
