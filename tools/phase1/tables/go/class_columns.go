// Phase 1 operation tables: group class (classes, heritage, decorators,
// modifiers). Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/class.rs and the spec is
// data/phase1/tables/class.json.
package main

import (
	"encoding/json"
	"fmt"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// legacyBits observes a decorator predicate under both decorator modes: bit
// 0 without, bit 1 with legacy decorators.
func legacyBits(f func(legacy bool) bool) any {
	bits := 0
	if f(false) {
		bits |= 1
	}
	if f(true) {
		bits |= 2
	}
	return Int(bits)
}

// parentOf and grandparentOf are the parent arguments the decorator callers
// pass for a node in place.
func parentOf(node *ast.Node) *ast.Node { return node.Parent }

func grandparentOf(node *ast.Node) *ast.Node {
	if node.Parent == nil {
		return nil
	}
	return node.Parent.Parent
}

// memberListKinds are the kinds whose Node.Members does not panic.
var memberListKinds = Kinds(ast.KindClassDeclaration, ast.KindClassExpression, ast.KindInterfaceDeclaration,
	ast.KindEnumDeclaration, ast.KindTypeLiteral, ast.KindMappedType)

// classElementInClass admits a class element whose parent is the class, as
// the decorator transforms pass it.
func classElementInClass(node *ast.Node) bool {
	return ast.IsClassElement(node) && node.Parent != nil && ast.IsClassLike(node.Parent)
}

// accessorInMemberList admits accessors whose parent's members GetAllAccessorDeclarations searches.
func accessorInMemberList(node *ast.Node) bool {
	return ast.IsAccessor(node) && node.Parent != nil && memberListKinds(node.Parent)
}

// functionLikeData admits the nodes whose Node.Parameters does not panic.
func functionLikeData(node *ast.Node) bool { return node.FunctionLikeData() != nil }

// accessorRefs projects AllAccessorDeclarations.
func accessorRefs(p *Parsed, all ast.AllAccessorDeclarations) any {
	var set, get *ast.Node
	if all.SetAccessor != nil {
		set = all.SetAccessor.AsNode()
	}
	if all.GetAccessor != nil {
		get = all.GetAccessor.AsNode()
	}
	return []any{RefOf(p, all.FirstAccessor), RefOf(p, all.SecondAccessor), RefOf(p, set), RefOf(p, get)}
}

// replaced projects a ReplaceModifiers result: true for the node itself, else
// the new node's kind and the references of its children in ForEachChild
// order (they are the original's children, all in the walk).
func replaced(p *Parsed, node *ast.Node, result *ast.Node) any {
	if result == node {
		return true
	}
	children := []any{}
	result.ForEachChild(func(child *ast.Node) bool {
		children = append(children, p.Ref(child))
		return false
	})
	return []any{int(result.Kind), children}
}

// replaceModifiersKinds are the kinds ReplaceModifiers handles; its default arm panics.
var replaceModifiersKinds = Kinds(ast.KindTypeParameter, ast.KindParameter, ast.KindConstructorType,
	ast.KindPropertySignature, ast.KindPropertyDeclaration, ast.KindMethodSignature, ast.KindMethodDeclaration,
	ast.KindConstructor, ast.KindGetAccessor, ast.KindSetAccessor, ast.KindIndexSignature,
	ast.KindFunctionExpression, ast.KindArrowFunction, ast.KindClassExpression, ast.KindVariableStatement,
	ast.KindFunctionDeclaration, ast.KindClassDeclaration, ast.KindInterfaceDeclaration,
	ast.KindTypeAliasDeclaration, ast.KindEnumDeclaration, ast.KindModuleDeclaration,
	ast.KindImportEqualsDeclaration, ast.KindImportDeclaration, ast.KindExportAssignment,
	ast.KindExportDeclaration)

func init() {
	Register("class",
		// ast/ast.go:Node.Decorators over every node: [index, [decorator
		// references]] for each node with a nonempty result. Callers read
		// only length and elements (checker.go:10240,10271,29040;
		// legacydecorators.go:666,723), so nil and empty are one value.
		Column{
			ID:    "ast.Node.Decorators",
			Input: "source",
			Build: func(raw json.RawMessage) (func() any, error) {
				p, err := ParseSource(raw)
				if err != nil {
					return nil, err
				}
				return func() any {
					out := []any{}
					for i, node := range p.Nodes {
						if decorators := node.Decorators(); len(decorators) > 0 {
							out = append(out, []any{i, p.Refs(decorators)})
						}
					}
					return out
				}, nil
			},
			// Per node with modifiers: kind, decorator count (0, 1, 2+) and
			// whether other modifiers stand beside them.
			Survey: func(p *Parsed) []string {
				classes := []string{}
				for _, node := range p.Nodes {
					modifiers := node.Modifiers()
					if modifiers == nil {
						continue
					}
					decorators := len(node.Decorators())
					classes = append(classes, fmt.Sprintf("%d/%d/%d", node.Kind, min(decorators, 2),
						min(len(modifiers.Nodes)-decorators, 1)))
				}
				return classes
			},
		},
		NodeMap("ast.ClassOrConstructorParameterIsDecorated", "source",
			Kinds(ast.KindClassDeclaration, ast.KindClassExpression),
			func(_ *Parsed, node *ast.Node) any {
				return legacyBits(func(legacy bool) bool { return ast.ClassOrConstructorParameterIsDecorated(legacy, node) })
			}),
		NodeMap("ast.ClassElementOrClassElementParameterIsDecorated", "source", classElementInClass,
			func(_ *Parsed, node *ast.Node) any {
				return legacyBits(func(legacy bool) bool {
					return ast.ClassElementOrClassElementParameterIsDecorated(legacy, node, node.Parent)
				})
			}),
		NodeMap("ast.NodeOrChildIsDecorated", "source", notSourceFile,
			func(_ *Parsed, node *ast.Node) any {
				return legacyBits(func(legacy bool) bool {
					return ast.NodeOrChildIsDecorated(legacy, node, parentOf(node), grandparentOf(node))
				})
			}),
		NodeMap("ast.NodeCanBeDecorated", "source", notSourceFile,
			func(_ *Parsed, node *ast.Node) any {
				return legacyBits(func(legacy bool) bool {
					return ast.NodeCanBeDecorated(legacy, node, parentOf(node), grandparentOf(node))
				})
			}),
		NodeMap("ast.GetAllAccessorDeclarations", "source", accessorInMemberList,
			func(p *Parsed, node *ast.Node) any {
				return accessorRefs(p, ast.GetAllAccessorDeclarations(node.Parent.Members(), node))
			}),
		// With the accessor symbol's declarations, as the checker passes them.
		NodeMap("ast.GetAllAccessorDeclarationsForDeclaration", "bound", Kinds(ast.KindGetAccessor, ast.KindSetAccessor),
			func(p *Parsed, node *ast.Node) any {
				var declarations []*ast.Node
				if symbol := node.Symbol(); symbol != nil {
					declarations = symbol.Declarations
				}
				return accessorRefs(p, ast.GetAllAccessorDeclarationsForDeclaration(node, declarations))
			}),
		NodeMap("ast.GetClassLikeDeclarationOfSymbol", "bound", All,
			func(p *Parsed, node *ast.Node) any {
				symbol := node.Symbol()
				if symbol == nil {
					return nil
				}
				return RefOf(p, ast.GetClassLikeDeclarationOfSymbol(symbol))
			}),
		NodeMap("ast.GetHeritageClause", "source", All,
			func(p *Parsed, node *ast.Node) any {
				extends, implements := ast.GetHeritageClause(node, ast.KindExtendsKeyword), ast.GetHeritageClause(node, ast.KindImplementsKeyword)
				if extends == nil && implements == nil {
					return nil
				}
				return []any{RefOf(p, extends), RefOf(p, implements)}
			}),
		NodeMap("ast.GetHeritageElements", "source", All,
			func(p *Parsed, node *ast.Node) any {
				extends, implements := ast.GetHeritageElements(node, ast.KindExtendsKeyword), ast.GetHeritageElements(node, ast.KindImplementsKeyword)
				if len(extends) == 0 && len(implements) == 0 {
					return nil
				}
				return []any{RefsOf(p, extends), RefsOf(p, implements)}
			}),
		NodeMap("ast.GetClassExtendsHeritageElement", "source", All,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetClassExtendsHeritageElement(node)) }),
		NodeMap("ast.GetExtendsHeritageClauseElements", "source", All,
			func(p *Parsed, node *ast.Node) any { return RefsOf(p, ast.GetExtendsHeritageClauseElements(node)) }),
		NodeMap("ast.GetImplementsHeritageClauseElements", "source", All,
			func(p *Parsed, node *ast.Node) any { return RefsOf(p, ast.GetImplementsHeritageClauseElements(node)) }),
		NodeMap("ast.GetHeritageClauseElementName", "source", Kinds(ast.KindExpressionWithTypeArguments, ast.KindTypeReference),
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetHeritageClauseElementName(node)) }),
		NodeMap("ast.GetFirstConstructorWithBody", "source", memberListKinds,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetFirstConstructorWithBody(node)) }),
		NodeMap("ast.GetThisParameter", "source", functionLikeData,
			func(p *Parsed, node *ast.Node) any { return RefOf(p, ast.GetThisParameter(node)) }),
		NodePredicate("ast.HasAbstractModifier", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasAbstractModifier(node) }),
		NodePredicate("ast.HasAmbientModifier", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.HasAmbientModifier(node) }),
		NodePredicate("ast.IsClassOrTypeElement", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsClassOrTypeElement(node) }),
		NodePredicate("ast.IsExpressionWithTypeArgumentsInClassExtendsClause", "source", All,
			func(_ *Parsed, node *ast.Node) bool {
				return ast.IsExpressionWithTypeArgumentsInClassExtendsClause(node)
			}),
		NodePredicate("ast.IsInitializedProperty", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsInitializedProperty(node) }),
		NodePredicate("ast.IsNameOfHeritageClauseTypeReference", "source", notSourceFile,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsNameOfHeritageClauseTypeReference(node) }),
		NodePredicate("ast.IsThisParameter", "source", All,
			func(_ *Parsed, node *ast.Node) bool { return ast.IsThisParameter(node) }),
		NodeMap("ast.TryGetClassExtendingExpressionWithTypeArguments", "source", All,
			func(p *Parsed, node *ast.Node) any {
				return RefOf(p, ast.TryGetClassExtendingExpressionWithTypeArguments(node))
			}),
		NodeMap("ast.TryGetClassImplementingOrExtendingHeritageClauseElement", "source", All,
			func(p *Parsed, node *ast.Node) any {
				class, isImplements := ast.TryGetClassImplementingOrExtendingHeritageClauseElement(node)
				if class == nil {
					return nil
				}
				return []any{p.Ref(class), isImplements}
			}),
		// [result with nil modifiers, result with the node's own modifiers],
		// each projected by replaced, from a fresh factory.
		NodeMap("ast.ReplaceModifiers", "source", replaceModifiersKinds,
			func(p *Parsed, node *ast.Node) any {
				factory := ast.NewNodeFactory(ast.NodeFactoryHooks{})
				return []any{replaced(p, node, ast.ReplaceModifiers(factory, node, nil)),
					replaced(p, node, ast.ReplaceModifiers(factory, node, node.Modifiers()))}
			}),
	)
}
