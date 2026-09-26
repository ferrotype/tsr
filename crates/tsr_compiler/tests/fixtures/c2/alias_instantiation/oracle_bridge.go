package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// Access-only snapshots. TryGet does not allocate a link or request a type ID.
func C2AliasCacheSnapshot(c *Checker, symbol *ast.Symbol) map[string]any {
	entries := 0
	if links := c.typeAliasLinks.TryGet(symbol); links != nil {
		entries = len(links.instantiations)
	}
	return map[string]any{"entries": entries, "instantiations": c.TotalInstantiationCount}
}

func C2AliasArguments(c *Checker, t *Type) []string {
	result := []string{}
	if t.alias != nil {
		for _, argument := range t.alias.typeArguments {
			result = append(result, c.TypeToString(argument))
		}
	}
	return result
}
