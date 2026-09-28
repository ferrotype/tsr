package checker

import (
	"crypto/sha256"
	"encoding/hex"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// C4JsxLinkState reads the JSX entities the emit resolver reads at location,
// in the order of the Rust contracts' probe: getJsxFactoryEntity,
// getJsxFragmentFactoryEntity, getJsxNamespaceAt and
// getJsxNamespaceContainerForImplicitImport.
func (c *Checker) C4JsxLinkState(location *ast.Node) map[string]any {
	factory := c.getJsxFactoryEntity(location)
	fragment := c.getJsxFragmentFactoryEntity(location)
	namespace := c.getJsxNamespaceAt(location)
	container := c.getJsxNamespaceContainerForImplicitImport(location)
	return map[string]any{
		"factory":          c4EntityText(factory),
		"fragment_factory": c4EntityText(fragment),
		"namespace":        c4DeclaredIn(namespace),
		"implicit_import":  c4DeclaredIn(container),
	}
}

// C4AliasReferenced reads aliasSymbolLinks.referenced of the declaration's
// symbol without creating the links.
func (c *Checker) C4AliasReferenced(declaration *ast.Node) bool {
	links := c.aliasSymbolLinks.TryGet(c.getSymbolOfDeclaration(declaration))
	return links != nil && links.referenced
}

func c4EntityText(entity *ast.Node) any {
	if entity == nil {
		return nil
	}
	text := ast.EntityNameToString(entity, nil)
	// A long name (the deep pragma case) is recorded by its length and digest.
	if len(text) > 200 {
		digest := sha256.Sum256([]byte(text))
		return map[string]any{"bytes": len(text), "sha256": hex.EncodeToString(digest[:])}
	}
	return text
}

func c4DeclaredIn(symbol *ast.Symbol) any {
	if symbol == nil || len(symbol.Declarations) == 0 {
		return nil
	}
	return ast.GetSourceFileOfNode(symbol.Declarations[0]).FileName()
}
