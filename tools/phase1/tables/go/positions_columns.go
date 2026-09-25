// Phase 1 operation tables: group positions (names, type and expression
// positions, access kinds). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/positions.rs and the spec is
// data/phase1/tables/positions.json.
package main

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// notSourceFile admits every node that has a parent: the operations that read
// name.Parent unconditionally are never called on the file itself.
func notSourceFile(node *ast.Node) bool { return node.Kind != ast.KindSourceFile }

// expressionContextDomain admits the nodes IsInExpressionContext returns for:
// it reads node.Parent unconditionally, and Node.Expression panics on a
// default clause, whose children are statements no caller asks about.
func expressionContextDomain(node *ast.Node) bool {
	return notSourceFile(node) && node.Parent.Kind != ast.KindDefaultClause
}

// writeAccessDecided is whether declarationIsWriteAccess returns for the
// declaration GetDeclarationFromName finds: nil, ambient, or one of the kinds
// its switch handles. Its default arm panics, so a name whose declaration has
// another kind (a named tuple member, a JSDoc template tag) is outside the
// domain the callers establish.
func writeAccessDecided(node *ast.Node) bool {
	decl := ast.GetDeclarationFromName(node)
	if decl == nil || decl.Flags&ast.NodeFlagsAmbient != 0 {
		return true
	}
	switch decl.Kind {
	case ast.KindBinaryExpression, ast.KindBindingElement, ast.KindClassDeclaration, ast.KindClassExpression,
		ast.KindDefaultKeyword, ast.KindEnumDeclaration, ast.KindEnumMember, ast.KindExportSpecifier,
		ast.KindImportClause, ast.KindImportEqualsDeclaration, ast.KindImportSpecifier, ast.KindInterfaceDeclaration,
		ast.KindJSDocCallbackTag, ast.KindJSDocTypedefTag, ast.KindJsxAttribute, ast.KindModuleDeclaration,
		ast.KindNamespaceExportDeclaration, ast.KindNamespaceImport, ast.KindNamespaceExport, ast.KindParameter,
		ast.KindShorthandPropertyAssignment, ast.KindTypeAliasDeclaration, ast.KindJSTypeAliasDeclaration,
		ast.KindTypeParameter, ast.KindPropertyAssignment, ast.KindFunctionDeclaration, ast.KindFunctionExpression,
		ast.KindConstructor, ast.KindMethodDeclaration, ast.KindGetAccessor, ast.KindSetAccessor,
		ast.KindVariableDeclaration, ast.KindPropertyDeclaration, ast.KindMethodSignature,
		ast.KindPropertySignature, ast.KindJSDocPropertyTag, ast.KindJSDocParameterTag:
		return true
	}
	return false
}

func init() {
	Register("positions",
		NodePredicate("ast.IsDeclarationName", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsDeclarationName(node) }),
		// The declaration a name declares, over the JSDoc walk so that a
		// @param tag's qualified name reaches its tag. Bound, because an
		// expando assignment is a declaration only when the binder gave its
		// left side or itself a symbol.
		NodeMap("ast.GetDeclarationFromName", "bound_jsdoc", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetDeclarationFromName(node)) }),
		NodePredicate("ast.IsArrayLiteralOrObjectLiteralDestructuringPattern", "source", All,
			func(_ *Parsed, node *ast.Node) bool {
				return ast.IsArrayLiteralOrObjectLiteralDestructuringPattern(node)
			}),
		NodePredicate("ast.IsTypeOrJSTypeAliasDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsTypeOrJSTypeAliasDeclaration(node) }),
		NodePredicate("ast.IsWriteAccess", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsWriteAccess(node) }),
		NodePredicate("ast.IsWriteOnlyAccess", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsWriteOnlyAccess(node) }),
		// [index, bool] for every node whose declaration the pinned switch
		// decides; the others are omitted (writeAccessDecided).
		NodeMap("ast.IsWriteAccessForReference", "bound_jsdoc", All,
			func(_ *Parsed, node *ast.Node) any {
				if !writeAccessDecided(node) {
					return nil
				}
				return ast.IsWriteAccessForReference(node)
			}),
		// The meaning bits, omitted where they are SemanticMeaningAll (the
		// default arm and the explicit All arm).
		NodeMap("ast.GetMeaningFromDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) any {
				meaning := ast.GetMeaningFromDeclaration(node)
				if meaning == ast.SemanticMeaningAll {
					return nil
				}
				return Int(int(meaning))
			}),
		NodePredicate("ast.IsArrayBindingOrAssignmentElement", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsArrayBindingOrAssignmentElement(node) }),
		NodePredicate("ast.IsDeclarationNameOrImportPropertyName", "source", notSourceFile,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsDeclarationNameOrImportPropertyName(node) }),
		NodePredicate("ast.IsExpression", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExpression(node) }),
		// Over the JSDoc walk: names under @link and JSDoc name references
		// are expressions.
		NodePredicate("ast.IsExpressionNode", "source_jsdoc", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsExpressionNode(node) }),
		NodePredicate("ast.IsInExpressionContext", "source_jsdoc", expressionContextDomain,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsInExpressionContext(node) }),
		NodePredicate("ast.IsLiteralComputedPropertyDeclarationName", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsLiteralComputedPropertyDeclarationName(node) }),
		// [index, result] for every node: parsed nodes are never synthesized,
		// so the value is the same everywhere and pairs survey it.
		NodeMap("ast.IsParseTreeNode", "source", All,
			func(_ *Parsed, node *ast.Node) any { return ast.IsParseTreeNode(node) }),
		// Over the JSDoc walk: @implements and @augments hold type
		// expressions.
		NodePredicate("ast.IsPartOfTypeNode", "source_jsdoc", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsPartOfTypeNode(node) }),
		NodePredicate("ast.IsThisInTypeQuery", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsThisInTypeQuery(node) }),
		NodePredicate("ast.IsTypeDeclaration", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsTypeDeclaration(node) }),
		NodePredicate("ast.IsTypeDeclarationName", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsTypeDeclarationName(node) }),
		// The node the parentheses wrap, omitted where the result is the node
		// itself.
		NodeMap("ast.SkipTypeParentheses", "source", All,
			func(p *Parsed, node *ast.Node) any {
				skipped := ast.SkipTypeParentheses(node)
				if skipped == node {
					return nil
				}
				return RefOf(p, skipped)
			}),
		NodeMap("ast.TryGetPropertyNameOfBindingOrAssignmentElement", "source", All,
			func(p *Parsed, node *ast.Node) any {
				return RefOf(p, ast.TryGetPropertyNameOfBindingOrAssignmentElement(node))
			}),
	)
}
