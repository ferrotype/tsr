package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// Only one checker is driven in each witness process. Births retain the actual
// creating stack; snapshots read initialized fields, never a lazy query or id.
func c2TraceTypeBirth(t *Type) {
	if ast.C2TraceActive() {
		ast.C2TraceRecord(map[string]any{"event": "birth", "kind": "type", "owner": 1, "token": t.id,
			"semantic_id": t.id, "flags": t.flags, "object_flags": t.objectFlags, "origin": ast.C2TraceOrigin()})
	}
}

func c2TraceType(t *Type) any {
	if t == nil {
		return nil
	}
	return map[string]any{"owner": 1, "token": t.id, "semantic_id": t.id, "flags": t.flags,
		"object_flags": t.objectFlags, "symbol": ast.C2TraceSymbol(t.symbol)}
}

func c2TraceTypeFallback(a, b *Type, result int) {
	if !ast.C2TraceActive() {
		return
	}
	ast.C2TraceRecord(map[string]any{"event": "fallback", "kind": "type", "branch": "final_type_id",
		"left": c2TraceType(a), "right": c2TraceType(b), "sign": c2TraceSign(result)})
}

func c2TraceSymbolFallback(a, b *ast.Symbol, result int) {
	if !ast.C2TraceActive() {
		return
	}
	branch := "declarationless"
	if len(a.Declarations) != 0 {
		branch = "equal_first_declaration"
	}
	ast.C2TraceRecord(map[string]any{"event": "fallback", "kind": "symbol", "branch": branch,
		"left": ast.C2TraceSymbol(a), "right": ast.C2TraceSymbol(b), "sign": c2TraceSign(result)})
}

func c2TraceSign(value int) int {
	if value < 0 {
		return -1
	}
	if value > 0 {
		return 1
	}
	return 0
}

func c2TraceTypeSort(event string, types []*Type) {
	if !ast.C2TraceActive() {
		return
	}
	values := []any{}
	for _, t := range types {
		values = append(values, c2TraceType(t))
	}
	ast.C2TraceRecord(map[string]any{"event": event, "kind": "type", "operation": "sort_types", "values": values})
}

func c2TraceSymbolSort(event string, symbols []*ast.Symbol) {
	if !ast.C2TraceActive() {
		return
	}
	values := []any{}
	for _, s := range symbols {
		values = append(values, ast.C2TraceSymbol(s))
	}
	ast.C2TraceRecord(map[string]any{"event": event, "kind": "symbol", "operation": "sort_symbols", "values": values})
}

func C2TracePropertyOrder(c *Checker, types []*Type, name string) any {
	symbols := []*ast.Symbol{}
	before := []any{}
	for _, t := range types {
		symbol := c.getPropertyOfType(t, name)
		if symbol == nil {
			panic("missing source-program trace property")
		}
		symbols = append(symbols, symbol)
		before = append(before, ast.C2TraceSymbol(symbol))
	}
	original := append([]*ast.Symbol{}, symbols...)
	c.sortSymbols(symbols)
	order := []int{}
	after := []any{}
	for _, s := range symbols {
		for index, prior := range original {
			if prior == s {
				order = append(order, index)
				break
			}
		}
		after = append(after, ast.C2TraceSymbol(s))
	}
	return map[string]any{"before": before, "order": order, "after": after}
}

func C2TraceUnion(c *Checker, types []*Type) any {
	typ := c.getUnionTypeEx(types, UnionReductionNone, nil, nil)
	order := []int{}
	for _, member := range typ.Types() {
		found := false
		for index, input := range types {
			if member == input { order = append(order,index); found = true; break }
		}
		if !found { panic("trace union introduced an unexpected member") }
	}
	return map[string]any{"display":c.TypeToStringEx(typ,nil,0,nil), "order":order}
}

func C2TraceTypes(types []*Type) []any {
    result := []any{}
    for _, t := range types { result = append(result,c2TraceType(t)) }
    return result
}
