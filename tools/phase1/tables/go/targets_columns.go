// Phase 1 operation tables: group targets (callee targets, JSX tags, JSDoc deprecation, member helpers).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/targets.rs and the spec is
// data/phase1/tables/targets.json.
package main

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// entityNameDomain admits the names EntityNameToString returns for: this,
// identifiers, and qualified names, property accesses and namespaced names of
// them. Its default arm panics, and its callers pass entity names.
func entityNameDomain(node *ast.Node) bool {
	switch node.Kind {
	case ast.KindThisKeyword, ast.KindIdentifier, ast.KindPrivateIdentifier:
		return true
	case ast.KindQualifiedName:
		return entityNameDomain(node.AsQualifiedName().Left) && entityNameDomain(node.AsQualifiedName().Right)
	case ast.KindPropertyAccessExpression:
		return entityNameDomain(node.Expression()) && entityNameDomain(node.Name())
	case ast.KindJsxNamespacedName:
		return entityNameDomain(node.AsJsxNamespacedName().Namespace) && entityNameDomain(node.Name())
	}
	return false
}

// bracketedText is the harness's getTextOfNode: a caller's text source that
// differs from Node.Text, so the column tells which one a name used.
func bracketedText(node *ast.Node) string { return "<" + node.Text() + ">" }

// callee observes one callee predicate under its four argument combinations:
// bit (2*skipPastOuterExpressions + includeElementAccess) is set where it is
// true. Its columns skip the source file, since the climb reads node.Parent
// unconditionally.
func callee(target func(*ast.Node, bool, bool) bool) func(*Parsed, *ast.Node) any {
	return func(_ *Parsed, node *ast.Node) any {
		bits := 0
		for bit, args := range [][2]bool{{false, false}, {true, false}, {false, true}, {true, true}} {
			if target(node, args[0], args[1]) {
				bits |= 1 << bit
			}
		}
		return Int(bits)
	}
}

func init() {
	Register("targets",
		// [index, [hex without a text source, hex with bracketedText]].
		NodeMap("ast.EntityNameToString", "source", entityNameDomain,
			func(_ *Parsed, node *ast.Node) any {
				return []any{Hex(ast.EntityNameToString(node, nil)), Hex(ast.EntityNameToString(node, bracketedText))}
			}),
		NodeMap("ast.GetAssignedName", "source", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetAssignedName(node)) }),
		NodeMap("ast.GetHostSignatureFromJSDoc", "source_jsdoc", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetHostSignatureFromJSDoc(node)) }),
		NodeMap("ast.GetJSDocDeprecatedTag", "source_jsdoc", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetJSDocDeprecatedTag(node)) }),
		NodeMap("ast.GetNamespaceDeclarationNode", "source",
			Kinds(ast.KindImportDeclaration, ast.KindJSImportDeclaration, ast.KindImportEqualsDeclaration, ast.KindExportDeclaration),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetNamespaceDeclarationNode(node)) }),
		NodeMap("ast.GetPropertyNameForPropertyNameNode", "source",
			Kinds(ast.KindIdentifier, ast.KindPrivateIdentifier, ast.KindStringLiteral, ast.KindNoSubstitutionTemplateLiteral,
				ast.KindNumericLiteral, ast.KindBigIntLiteral, ast.KindJsxNamespacedName, ast.KindComputedPropertyName),
			func(_ *Parsed, node *ast.Node) any { return Hex(ast.GetPropertyNameForPropertyNameNode(node)) }),
		NodeMap("ast.GetTypeAnnotationNode", "source_jsdoc", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetTypeAnnotationNode(node)) }),
		NodePredicate("ast.HasInitializer", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasInitializer(node) }),
		NodePredicate("ast.HasQuestionToken", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasQuestionToken(node) }),
		NodeMap("ast.IsCallExpressionTarget", "source", notSourceFile, callee(ast.IsCallExpressionTarget)),
		NodeMap("ast.IsCallOrNewExpressionTarget", "source", notSourceFile, callee(ast.IsCallOrNewExpressionTarget)),
		NodeMap("ast.IsDecoratorTarget", "source", notSourceFile, callee(ast.IsDecoratorTarget)),
		NodeMap("ast.IsNewExpressionTarget", "source", notSourceFile, callee(ast.IsNewExpressionTarget)),
		NodeMap("ast.IsTaggedTemplateTag", "source", notSourceFile, callee(ast.IsTaggedTemplateTag)),
		NodeMap("ast.IsJsxOpeningLikeElementTagName", "source", notSourceFile, callee(ast.IsJsxOpeningLikeElementTagName)),
		NodePredicate("ast.IsDeprecatedDeclaration", "source_jsdoc", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsDeprecatedDeclaration(node) }),
		// With the node's own flags as the cached flags, which the checker
		// passes for a node it has not combined.
		NodePredicate("ast.IsDeprecatedDeclarationWithCachedFlags", "source_jsdoc", All,
			func(_ *Parsed, node *ast.Node) bool {
				return ast.IsDeprecatedDeclarationWithCachedFlags(node, node.Flags)
			}),
		// Bit 0 without, bit 1 with lookInLabeledStatements.
		NodeMap("ast.IsIterationStatement", "source", All,
			func(_ *Parsed, node *ast.Node) any {
				bits := 0
				if ast.IsIterationStatement(node, false) {
					bits |= 1
				}
				if ast.IsIterationStatement(node, true) {
					bits |= 2
				}
				return Int(bits)
			}),
		NodePredicate("ast.IsJsxTagName", "source", notSourceFile,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsJsxTagName(node) }),
		NodePredicate("ast.IsLet", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsLet(node) }),
		NodePredicate("ast.IsPrototypeAccess", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsPrototypeAccess(node) }),
	)
}
