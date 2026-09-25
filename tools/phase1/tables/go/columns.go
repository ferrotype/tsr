// Generic column shapes shared by the operation-table groups. Each has a Rust
// twin in tools/phase1/mutation/driver/src/table/helpers.rs with the same
// value layout, so a group column is one line on each side.
package main

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// parsedFor is the setup of a parsed input kind.
func parsedFor(input string, raw json.RawMessage) (*Parsed, error) {
	switch input {
	case "source":
		return ParseSource(raw)
	case "bound":
		return BindSource(raw)
	case "source_jsdoc":
		return ParseSourceJSDoc(raw)
	case "bound_jsdoc":
		return BindSourceJSDoc(raw)
	}
	return nil, fmt.Errorf("input kind %s is not parsed", input)
}

// All admits every node of the walk.
func All(*ast.Node) bool { return true }

// Kinds admits the nodes of the given kinds: the precondition an operation's
// callers establish before calling it.
func Kinds(kinds ...ast.Kind) func(*ast.Node) bool {
	return func(node *ast.Node) bool {
		for _, kind := range kinds {
			if node.Kind == kind {
				return true
			}
		}
		return false
	}
}

// NodePredicate is a per-node boolean column: the value is the document-order
// indices, among the nodes filter admits, where pred is true.
func NodePredicate(id, input string, filter func(*ast.Node) bool, pred func(p *Parsed, node *ast.Node) bool) Column {
	return Column{
		ID:    id,
		Input: input,
		Build: func(raw json.RawMessage) (func() any, error) {
			p, err := parsedFor(input, raw)
			if err != nil {
				return nil, err
			}
			return func() any {
				out := []any{}
				for i, node := range p.Nodes {
					if filter(node) && pred(p, node) {
						out = append(out, i)
					}
				}
				return out
			}, nil
		},
		Survey: func(p *Parsed) []string {
			return NodeClasses(p, func(node *ast.Node) string {
				if !filter(node) {
					return "0"
				}
				return BoolClass(pred(p, node))
			})
		},
	}
}

// NodeMap is a per-node valued column: the value is [index, f(node)] for every
// node filter admits where f is not nil. f projects nodes with p.Ref and
// symbols with p.SymbolKey; its survey class is the result's class.
func NodeMap(id, input string, filter func(*ast.Node) bool, f func(p *Parsed, node *ast.Node) any) Column {
	return Column{
		ID:    id,
		Input: input,
		Build: func(raw json.RawMessage) (func() any, error) {
			p, err := parsedFor(input, raw)
			if err != nil {
				return nil, err
			}
			return func() any {
				out := []any{}
				for i, node := range p.Nodes {
					if !filter(node) {
						continue
					}
					if value := f(p, node); value != nil {
						out = append(out, []any{i, value})
					}
				}
				return out
			}, nil
		},
		// The (kind, value) and (parent kind, value) pairs only: a valued
		// column's class is already a referenced kind, a scalar or a list
		// shape, and NodeClasses' third (kind, parent kind, value) class
		// multiplies rows without separating behaviour.
		Survey: func(p *Parsed) []string {
			classes := []string{}
			for _, node := range p.Nodes {
				value := "0"
				if filter(node) {
					value = valueClass(p, f(p, node))
				}
				parent := -1
				if node.Parent != nil {
					parent = int(node.Parent.Kind)
				}
				classes = append(classes, fmt.Sprintf("k%d/%s", node.Kind, value), fmt.Sprintf("p%d/%s", parent, value))
			}
			return classes
		},
	}
}

// valueClass is a short, bounded survey class of a projected value: "0" for
// nil, the referenced node's kind for a reference (an int), the length bucket
// of a list, the prefix class of a hex string, or the value itself for other
// scalars (an int64, as Int and Scalar build).
func valueClass(p *Parsed, value any) string {
	switch v := value.(type) {
	case nil:
		return "0"
	case int:
		// A node reference (RefOf, p.Ref).
		if v >= 0 && v < len(p.Nodes) {
			return fmt.Sprintf("n%d", p.Nodes[v].Kind)
		}
		return fmt.Sprintf("i%d", v)
	case int64:
		// A scalar (Int, Scalar): flags, precedences, bit sets.
		return fmt.Sprintf("i%d", v)
	case bool:
		return BoolClass(v)
	case []any:
		switch {
		case len(v) == 0:
			return "l0"
		case len(v) == 1:
			return "l1"
		}
		return "l2"
	case string:
		// Values carry strings as hex: an internal symbol name (\xFE), a
		// leading minus sign, or anything else.
		switch {
		case strings.HasPrefix(v, "fe"):
			return "sfe"
		case strings.HasPrefix(v, "2d"):
			return "s2d"
		}
		return "s"
	}
	return fmt.Sprintf("%T", value)
}

// RefOf projects a node result as its reference (nil stays nil).
func RefOf(p *Parsed, node *ast.Node) any {
	if node == nil {
		return nil
	}
	return p.Ref(node)
}

// RefsOf projects a node-list result; an empty or nil list is nil, so a
// NodeMap row is written only where the result has nodes.
func RefsOf(p *Parsed, nodes []*ast.Node) any {
	if len(nodes) == 0 {
		return nil
	}
	return p.Refs(nodes)
}

// Int projects an integer-valued result, omitting zero. It is an int64, the
// scalar type valueClass classes by value (an int is a node reference).
func Int(value int) any {
	if value == 0 {
		return nil
	}
	return int64(value)
}

// Scalar projects an integer-valued result that is kept even when zero.
func Scalar(value int) any { return int64(value) }

// ValuesMap is a column over synthetic byte strings (the spec's
// {"names_hex": [...]}): the value is [f(name)] in input order.
func ValuesMap(id string, f func(name string) any) Column {
	return Column{
		ID:    id,
		Input: "values",
		Build: func(raw json.RawMessage) (func() any, error) {
			var in struct {
				NamesHex []string `json:"names_hex"`
			}
			if err := DecodeInput(raw, &in); err != nil {
				return nil, err
			}
			names := make([]string, len(in.NamesHex))
			for i, text := range in.NamesHex {
				name, err := hex.DecodeString(text)
				if err != nil {
					return nil, err
				}
				names[i] = string(name)
			}
			return func() any {
				out := []any{}
				for _, name := range names {
					out = append(out, f(name))
				}
				return out
			}, nil
		},
	}
}

// RefOrFields projects a node result that may lie outside the walk (a
// reparsed clone the walk does not reach): its reference when the walk holds
// it, else ["fields", kind, pos, end].
func RefOrFields(p *Parsed, node *ast.Node) any {
	if node == nil {
		return nil
	}
	if at, ok := p.index[node]; ok {
		return at
	}
	return []any{"fields", int(node.Kind), node.Pos(), node.End()}
}
