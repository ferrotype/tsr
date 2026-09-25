// Phase 1 operation tables: group containers (containers, function flags,
// precedence, reparse identity, source-file tables, outer expressions).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/containers.rs and the spec is
// data/phase1/tables/containers.json.
package main

import (
	"encoding/json"
	"fmt"
	"slices"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
)

// statementContainers are the nodes whose Node.CanHaveStatements holds
// (ast.go:604), with their statement lists, both taken in setup.
func statementContainers(p *Parsed) ([]int, [][]*ast.Node) {
	at, lists := []int{}, [][]*ast.Node{}
	for i, node := range p.Nodes {
		if node.CanHaveStatements() {
			at = append(at, i)
			lists = append(lists, node.Statements())
		}
	}
	return at, lists
}

// outerKinds are the OuterExpressionKinds the outer-expression columns are
// observed under, one bit or one result each in this order.
var outerKinds = []ast.OuterExpressionKinds{ast.OEKParentheses, ast.OEKTypeAssertions, ast.OEKNonNullAssertions,
	ast.OEKPartiallyEmittedExpressions, ast.OEKExpressionsWithTypeArguments, ast.OEKSatisfies,
	ast.OEKParentheses | ast.OEKExcludeJSDocTypeAssertion, ast.OEKAssignments, ast.OEKComma, ast.OEKAll}

// typeNodePrecedenceKinds are the kinds GetTypeNodePrecedence handles; its
// default arm panics.
var typeNodePrecedenceKinds = Kinds(ast.KindConditionalType, ast.KindJSDocOptionalType, ast.KindJSDocVariadicType,
	ast.KindFunctionType, ast.KindConstructorType, ast.KindUnionType, ast.KindIntersectionType, ast.KindTypeOperator,
	ast.KindInferType, ast.KindIndexedAccessType, ast.KindArrayType, ast.KindOptionalType, ast.KindTypeQuery,
	ast.KindAnyKeyword, ast.KindUnknownKeyword, ast.KindStringKeyword, ast.KindNumberKeyword, ast.KindBigIntKeyword,
	ast.KindSymbolKeyword, ast.KindBooleanKeyword, ast.KindUndefinedKeyword, ast.KindNeverKeyword, ast.KindObjectKeyword,
	ast.KindIntrinsicKeyword, ast.KindVoidKeyword, ast.KindJSDocAllType, ast.KindJSDocNullableType,
	ast.KindJSDocNonNullableType, ast.KindLiteralType, ast.KindTypePredicate, ast.KindTypeReference, ast.KindTypeLiteral,
	ast.KindTupleType, ast.KindRestType, ast.KindParenthesizedType, ast.KindThisType, ast.KindMappedType,
	ast.KindNamedTupleMember, ast.KindTemplateLiteralType, ast.KindImportType, ast.KindPropertyAccessExpression,
	ast.KindExpressionWithTypeArguments)

// sortedTable projects a string-keyed table as [[key hex, value], ...] by key.
func sortedTable[V any](table map[string]V, value func(V) any) any {
	keys := make([]string, 0, len(table))
	for key := range table {
		keys = append(keys, key)
	}
	slices.Sort(keys)
	out := []any{}
	for _, key := range keys {
		out = append(out, []any{Hex(key), value(table[key])})
	}
	return out
}

// identifierProbes are the names HasIdentifier is asked about: the text of
// every node of the kinds collectIdentifiersForSourceFile collects, sorted
// and unique, then two names no file declares.
func identifierProbes(p *Parsed) []string {
	set := map[string]bool{}
	for _, node := range p.Nodes {
		switch node.Kind {
		case ast.KindIdentifier, ast.KindPrivateIdentifier, ast.KindStringLiteral, ast.KindNumericLiteral,
			ast.KindBigIntLiteral, ast.KindNoSubstitutionTemplateLiteral:
			set[node.Text()] = true
		}
	}
	probes := make([]string, 0, len(set)+2)
	for name := range set {
		probes = append(probes, name)
	}
	slices.Sort(probes)
	return append(probes, "", "\x00absent")
}

// selfOmitted is nil where a node result is the node itself, else its reference.
func selfOmitted(p *Parsed, node *ast.Node, result *ast.Node) any {
	if result == node {
		return nil
	}
	return RefOf(p, result)
}

func init() {
	Register("containers",
		// binder/binder.go:FindUseStrictPrologue (and its helper
		// isUseStrictPrologueDirective) on the statements of every statement
		// container: [container index, result reference] where non-nil.
		Column{
			ID:    "binder.FindUseStrictPrologue",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				at, lists := statementContainers(p)
				return func() any {
					out := []any{}
					for i, statements := range lists {
						if found := binder.FindUseStrictPrologue(p.File, statements); found != nil {
							out = append(out, []any{at[i], p.Ref(found)})
						}
					}
					return out
				}, nil
			},
			// Per container: kind, prologue count (0-3), the position of the
			// found prologue (-1, 0, 1, 2+) and whether statements follow.
			Survey: func(p *Parsed) []string {
				classes := []string{}
				at, lists := statementContainers(p)
				for i, statements := range lists {
					prologues := 0
					for _, statement := range statements {
						if !ast.IsPrologueDirective(statement) {
							break
						}
						prologues++
					}
					found := binder.FindUseStrictPrologue(p.File, statements)
					position := -1
					for j, statement := range statements {
						if statement == found {
							position = j
						}
					}
					classes = append(classes, fmt.Sprintf("%d/%d/%d/%d", p.Nodes[at[i]].Kind, min(prologues, 3),
						min(position, 2), min(len(statements)-prologues, 1)))
				}
				return classes
			},
		},
		NodeMap("ast.GetDeclarationName", "source", All,
			func(_ *Parsed, node *ast.Node) any {
				if name := ast.GetDeclarationName(node); name != "" {
					return Hex(name)
				}
				return nil
			}),
		// Bound: an overload group is the declarations that share a symbol.
		NodeMap("ast.SourceFile.GetDeclarationMap", "bound", Kinds(ast.KindSourceFile),
			func(p *Parsed, node *ast.Node) any {
				return sortedTable(node.AsSourceFile().GetDeclarationMap(), func(nodes []*ast.Node) any { return p.Refs(nodes) })
			}),
		// Over the JSDoc walk, since the table reads names in JSDoc.
		NodeMap("ast.SourceFile.GetNameTable", "source_jsdoc", Kinds(ast.KindSourceFile),
			func(_ *Parsed, node *ast.Node) any {
				return sortedTable(node.AsSourceFile().GetNameTable(), func(position int) any { return position })
			}),
		NodeMap("ast.SourceFile.HasIdentifier", "source", Kinds(ast.KindSourceFile),
			func(p *Parsed, node *ast.Node) any {
				out := []any{}
				for _, name := range identifierProbes(p) {
					out = append(out, node.AsSourceFile().HasIdentifier(name))
				}
				return out
			}),
		// Two fresh keys: [first, cached, second key, compute calls].
		NodeMap("ast.GetOrComputeSourceFileData", "source", Kinds(ast.KindSourceFile),
			func(_ *Parsed, node *ast.Node) any {
				file := node.AsSourceFile()
				first, second := ast.NewSourceFileDataKey[int](), ast.NewSourceFileDataKey[int]()
				calls := 0
				compute := func(f *ast.SourceFile) int {
					calls++
					return calls*1000 + len(f.Statements.Nodes)
				}
				a := ast.GetOrComputeSourceFileData(file, first, compute)
				b := ast.GetOrComputeSourceFileData(file, first, compute)
				c := ast.GetOrComputeSourceFileData(file, second, compute)
				return []any{a, b, c, calls}
			}),
		NodeMap("ast.GetFunctionFlags", "source", All,
			func(_ *Parsed, node *ast.Node) any { return Scalar(int(ast.GetFunctionFlags(node))) }),
		NodeMap("ast.GetExpressionPrecedence", "source", All,
			func(_ *Parsed, node *ast.Node) any { return Scalar(int(ast.GetExpressionPrecedence(node))) }),
		NodeMap("ast.GetLeftmostExpression", "source", All,
			func(p *Parsed, node *ast.Node) any {
				through, stopping := ast.GetLeftmostExpression(node, false), ast.GetLeftmostExpression(node, true)
				if through == node && stopping == node {
					return nil
				}
				return []any{p.Ref(through), p.Ref(stopping)}
			}),
		NodeMap("ast.GetTypeNodePrecedence", "source", typeNodePrecedenceKinds,
			func(_ *Parsed, node *ast.Node) any { return Scalar(int(ast.GetTypeNodePrecedence(node))) }),
		// [every return statement, the first one] of each block.
		NodeMap("ast.ForEachReturnStatement", "source", Kinds(ast.KindBlock),
			func(p *Parsed, node *ast.Node) any {
				var all []*ast.Node
				ast.ForEachReturnStatement(node, func(statement *ast.Node) bool {
					all = append(all, statement)
					return false
				})
				var first *ast.Node
				ast.ForEachReturnStatement(node, func(statement *ast.Node) bool {
					first = statement
					return true
				})
				if len(all) == 0 && first == nil {
					return nil
				}
				return []any{RefsOf(p, all), RefOf(p, first)}
			}),
		NodeMap("ast.GetContainingFunction", "source", notSourceFile,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetContainingFunction(node)) }),
		NodeMap("ast.GetEnclosingBlockScopeContainer", "source", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetEnclosingBlockScopeContainer(node)) }),
		NodePredicate("ast.IsBlockScope", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsBlockScope(node, node.Parent) }),
		// GetThisContainer panics on a parentless node.
		NodeMap("ast.GetNewTargetContainer", "source", notSourceFile,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetNewTargetContainer(node)) }),
		// Over the JSDoc walk: a JSDoc node's reparsed clone, omitted where it
		// is the node itself.
		NodeMap("ast.GetReparsedNodeForNode", "source_jsdoc", All,
			func(p *Parsed, node *ast.Node) any {
				if result := ast.GetReparsedNodeForNode(node); result != node {
					return RefOrFields(p, result)
				}
				return nil
			}),
		NodeMap("ast.GetSuperContainer", "source", All,
			func(p *Parsed, node *ast.Node) any {
				through, stopping := ast.GetSuperContainer(node, false), ast.GetSuperContainer(node, true)
				if through == nil && stopping == nil {
					return nil
				}
				return []any{RefOf(p, through), RefOf(p, stopping)}
			}),
		NodePredicate("ast.HasContextSensitiveParameters", "source", functionLikeData,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasContextSensitiveParameters(node) }),
		NodePredicate("ast.IsJSDocTypeAssertion", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsJSDocTypeAssertion(node) }),
		// Bit i under outerKinds[i].
		NodeMap("ast.IsOuterExpression", "source", All,
			func(_ *Parsed, node *ast.Node) any {
				bits := 0
				for bit, kinds := range outerKinds {
					if ast.IsOuterExpression(node, kinds) {
						bits |= 1 << bit
					}
				}
				return Int(bits)
			}),
		// The result under each of outerKinds, omitted where all are the node.
		NodeMap("ast.SkipOuterExpressions", "source", All,
			func(p *Parsed, node *ast.Node) any {
				out, moved := []any{}, false
				for _, kinds := range outerKinds {
					result := ast.SkipOuterExpressions(node, kinds)
					moved = moved || result != node
					out = append(out, p.Ref(result))
				}
				if !moved {
					return nil
				}
				return out
			}),
		// A factory's two partially emitted expressions around each
		// expression: the result is the expression.
		NodeMap("ast.SkipPartiallyEmittedExpressions", "source", ast.IsExpressionNode,
			func(p *Parsed, node *ast.Node) any {
				factory := ast.NewNodeFactory(ast.NodeFactoryHooks{})
				wrapped := factory.NewPartiallyEmittedExpression(factory.NewPartiallyEmittedExpression(node))
				itself := ast.SkipPartiallyEmittedExpressions(node)
				var moved any
				if itself != node {
					moved = RefOrFields(p, itself)
				}
				return []any{RefOrFields(p, ast.SkipPartiallyEmittedExpressions(wrapped)), moved}
			}),
	)
}
