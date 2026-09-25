// Phase 1 operation tables: group runtime, the harness's own columns. They
// claim no operation: they show that the Go and Rust setups agree on what
// every other column builds on (the canonical encoding, both document-order
// walks of a parsed file and the symbol keys of a bound one), so a column that
// differs is the column's difference, not the harness's. The Rust side is
// tools/phase1/mutation/driver/src/table/runtime.rs and the spec is
// data/phase1/tables/runtime.json.
package main

import (
	"encoding/json"
	"fmt"
)

func init() {
	Register("runtime",
		// The canonical encoding itself: the input's "value", decoded and
		// returned. Go, Python and Rust must write the same bytes for every
		// JSON value a column may observe (escapes, key order, integers).
		Column{
			ID:    "runtime.values",
			Input: "values",
			Build: func(raw json.RawMessage) (func() any, error) {
				var in struct {
					Value json.RawMessage `json:"value"`
				}
				if err := DecodeInput(raw, &in); err != nil {
					return nil, err
				}
				value, err := DecodeValue(in.Value)
				if err != nil {
					return nil, err
				}
				return func() any { return value }, nil
			},
		},
		// The walks: [kind, pos, end, parent reference] of every node, read
		// in setup, under Walk (source) and WalkJSDoc (source_jsdoc).
		Column{ID: "runtime.walk", Input: "source", Build: walkRows(ParseSource), Survey: kindClasses},
		Column{ID: "runtime.walk_jsdoc", Input: "source_jsdoc", Build: walkRows(ParseSourceJSDoc),
			Survey: kindClasses},
		// Symbol keys after binding: [index, symbol key, symbol flags] of
		// every node whose Node.Symbol is not nil, read in setup, under Walk
		// (bound) and WalkJSDoc (bound_jsdoc, synthetic rows only).
		Column{ID: "runtime.symbols", Input: "bound", Build: symbolRows(BindSource), Survey: symbolClasses},
		Column{ID: "runtime.symbols_jsdoc", Input: "bound_jsdoc", Build: symbolRows(BindSourceJSDoc)},
	)
}

// walkRows builds the walk's rows in setup; the column only returns them.
func walkRows(setup func(json.RawMessage) (*Parsed, error)) func(json.RawMessage) (func() any, error) {
	return func(raw json.RawMessage) (func() any, error) {
		p, err := setup(raw)
		if err != nil {
			return nil, err
		}
		rows := make([]any, len(p.Nodes))
		for i, node := range p.Nodes {
			rows[i] = []any{int(node.Kind), node.Pos(), node.End(), p.Ref(node.Parent)}
		}
		return func() any { return rows }, nil
	}
}

// kindClasses: the node kinds of the walk.
func kindClasses(p *Parsed) []string {
	classes := []string{}
	for _, node := range p.Nodes {
		classes = append(classes, fmt.Sprintf("k%d", node.Kind))
	}
	return classes
}

// symbolRows builds the symbol rows in setup; the column only returns them.
func symbolRows(setup func(json.RawMessage) (*Parsed, error)) func(json.RawMessage) (func() any, error) {
	return func(raw json.RawMessage) (func() any, error) {
		p, err := setup(raw)
		if err != nil {
			return nil, err
		}
		rows := []any{}
		for i, node := range p.Nodes {
			if symbol := node.Symbol(); symbol != nil {
				rows = append(rows, []any{i, p.SymbolKey(symbol), uint32(symbol.Flags)})
			}
		}
		return func() any { return rows }, nil
	}
}

// symbolClasses: the kind and low symbol flags of every node with a symbol.
func symbolClasses(p *Parsed) []string {
	classes := []string{}
	for _, node := range p.Nodes {
		if symbol := node.Symbol(); symbol != nil {
			classes = append(classes, fmt.Sprintf("k%d/f%d", node.Kind, uint32(symbol.Flags)&0xff))
		}
	}
	return classes
}
