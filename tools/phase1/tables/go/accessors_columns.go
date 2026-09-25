// Phase 1 operation tables: group accessors (per-kind node accessors, list
// and flow helpers). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/accessors.rs and the spec is
// data/phase1/tables/accessors.json.
package main

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// jsdocCommentKinds are the kinds Node.CommentList handles; any other kind
// panics.
var jsdocCommentKinds = []ast.Kind{ast.KindJSDoc, ast.KindJSDocUnknownTag, ast.KindJSDocAugmentsTag,
	ast.KindJSDocImplementsTag, ast.KindJSDocDeprecatedTag, ast.KindJSDocPublicTag, ast.KindJSDocPrivateTag,
	ast.KindJSDocProtectedTag, ast.KindJSDocReadonlyTag, ast.KindJSDocOverrideTag, ast.KindJSDocCallbackTag,
	ast.KindJSDocOverloadTag, ast.KindJSDocParameterTag, ast.KindJSDocPropertyTag, ast.KindJSDocReturnTag,
	ast.KindJSDocThisTag, ast.KindJSDocTypeTag, ast.KindJSDocTemplateTag, ast.KindJSDocTypedefTag,
	ast.KindJSDocSeeTag, ast.KindJSDocSatisfiesTag, ast.KindJSDocThrowsTag, ast.KindJSDocImportTag}

// listRefs projects a node list: nil, or its nodes' references.
func listRefs(p *Parsed, list *ast.NodeList) any {
	if list == nil {
		return nil
	}
	return RefsOf(p, list.Nodes)
}

func init() {
	Register("accessors",
		// Each accessor over the kinds it handles (any other kind panics).
		NodeMap("ast.Node.Attributes", "source", Kinds(ast.KindJsxOpeningElement, ast.KindJsxSelfClosingElement,
			ast.KindModuleDeclaration), func(p *Parsed, node *ast.Node) any { return RefOf(p, node.Attributes()) }),
		NodeMap("ast.Node.Children", "source", Kinds(ast.KindJsxElement, ast.KindJsxFragment),
			func(p *Parsed, node *ast.Node) any { return listRefs(p, node.Children()) }),
		NodeMap("ast.Node.ClassName", "source_jsdoc", Kinds(ast.KindJSDocAugmentsTag, ast.KindJSDocImplementsTag),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, node.ClassName()) }),
		NodeMap("ast.Node.CommentList", "source_jsdoc", Kinds(jsdocCommentKinds...),
			func(p *Parsed, node *ast.Node) any { return listRefs(p, node.CommentList()) }),
		NodeMap("ast.Node.Comments", "source_jsdoc", Kinds(jsdocCommentKinds...),
			func(p *Parsed, node *ast.Node) any { return RefsOf(p, node.Comments()) }),
		NodeMap("ast.Node.TypeExpression", "source_jsdoc", Kinds(ast.KindJSDocParameterTag, ast.KindJSDocPropertyTag,
			ast.KindJSDocReturnTag, ast.KindJSDocTypeTag, ast.KindJSDocTypedefTag, ast.KindJSDocCallbackTag,
			ast.KindJSDocSatisfiesTag, ast.KindJSDocThrowsTag),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, node.TypeExpression()) }),
		NodeMap("ast.Node.Statement", "source", Kinds(ast.KindDoStatement, ast.KindWhileStatement, ast.KindForStatement,
			ast.KindForInStatement, ast.KindForOfStatement, ast.KindWithStatement, ast.KindLabeledStatement),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, node.Statement()) }),
		// Claims isBlockStatement, which IsStatement asks of every Block.
		NodePredicate("ast.IsStatement", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsStatement(node) }),
		// The argument, element and property lists of calls, news, array and
		// object literals: whether each ends in a comma.
		NodeMap("ast.NodeList.HasTrailingComma", "source", Kinds(ast.KindCallExpression, ast.KindNewExpression,
			ast.KindArrayLiteralExpression, ast.KindObjectLiteralExpression),
			func(_ *Parsed, node *ast.Node) any {
				var list *ast.NodeList
				switch node.Kind {
				case ast.KindCallExpression:
					list = node.AsCallExpression().Arguments
				case ast.KindNewExpression:
					list = node.AsNewExpression().Arguments
				case ast.KindArrayLiteralExpression:
					list = node.AsArrayLiteralExpression().Elements
				case ast.KindObjectLiteralExpression:
					list = node.AsObjectLiteralExpression().Properties
				}
				if list == nil {
					return nil
				}
				return list.HasTrailingComma()
			}),
		// IsEmpty of a switch-clause flow datum per [start, end] pair.
		typedValuesColumn("ast.FlowSwitchClauseData.IsEmpty", func(in struct {
			Clauses [][2]int `json:"clauses"`
		}) any {
			out := []any{}
			for _, clause := range in.Clauses {
				out = append(out, ast.NewFlowSwitchClauseData(nil, clause[0], clause[1]).AsFlowSwitchClauseData().IsEmpty())
			}
			return out
		}),
	)
}
