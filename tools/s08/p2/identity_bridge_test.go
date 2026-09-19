package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// S08ExistingNameType observes already-created identity only. TryGet neither
// creates a link nor runs any checker query, so graph snapshots cannot warm it.
func S08ExistingNameType(c *Checker, symbol *ast.Symbol) *Type {
	if c == nil || symbol == nil {
		return nil
	}
	links := c.valueSymbolLinks.TryGet(symbol)
	if links == nil {
		return nil
	}
	return links.nameType
}
