package pseudochecker

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"os"
	"runtime"
	"testing"
)

func pseudoNode(n *ast.Node) any {
	if n == nil {
		return nil
	}
	return []int{int(n.Kind), n.Pos(), n.End()}
}
func pseudoNodes(ns []*ast.Node) []any {
	r := make([]any, 0, len(ns))
	for _, n := range ns {
		r = append(r, pseudoNode(n))
	}
	return r
}
func pseudoTypeParameters(ns []*ast.TypeParameterDeclaration) []any {
	r := make([]any, 0, len(ns))
	for _, n := range ns {
		r = append(r, pseudoNode(n.AsNode()))
	}
	return r
}
func pseudoParameter(p *PseudoParameter) any {
	return map[string]any{"rest": p.Rest, "name": pseudoNode(p.Name), "optional": p.Optional, "type": pseudoTree(p.Type)}
}
func pseudoParameters(ps []*PseudoParameter) []any {
	r := make([]any, 0, len(ps))
	for _, p := range ps {
		r = append(r, pseudoParameter(p))
	}
	return r
}
func pseudoTrees(ts []*PseudoType) []any {
	r := make([]any, 0, len(ts))
	for _, t := range ts {
		r = append(r, pseudoTree(t))
	}
	return r
}
func pseudoTree(t *PseudoType) any {
	r := map[string]any{"kind": t.Kind}
	switch t.Kind {
	case PseudoTypeKindDirect:
		r["node"] = pseudoNode(t.AsPseudoTypeDirect().TypeNode)
	case PseudoTypeKindInferred:
		d := t.AsPseudoTypeInferred()
		r["node"] = pseudoNode(d.Expression)
		r["errors"] = pseudoNodes(d.ErrorNodes)
		r["return"] = d.IsSignatureReturn
	case PseudoTypeKindNoResult:
		r["node"] = pseudoNode(t.AsPseudoTypeNoResult().Declaration)
	case PseudoTypeKindMaybeConstLocation:
		d := t.AsPseudoTypeMaybeConstLocation()
		r["node"] = pseudoNode(d.Node)
		r["const"] = pseudoTree(d.ConstType)
		r["regular"] = pseudoTree(d.RegularType)
	case PseudoTypeKindUnion:
		r["types"] = pseudoTrees(t.AsPseudoTypeUnion().Types)
	case PseudoTypeKindTuple:
		r["types"] = pseudoTrees(t.AsPseudoTypeTuple().Elements)
	case PseudoTypeKindStringLiteral:
		r["node"] = pseudoNode(t.AsPseudoTypeLiteral().Node)
	case PseudoTypeKindNumericLiteral:
		r["node"] = pseudoNode(t.AsPseudoTypeLiteral().Node)
	case PseudoTypeKindBigIntLiteral:
		r["node"] = pseudoNode(t.AsPseudoTypeLiteral().Node)
	case PseudoTypeKindSingleCallSignature:
		d := t.AsPseudoTypeSingleCallSignature()
		r["node"] = pseudoNode(d.Signature)
		r["parameters"] = pseudoParameters(d.Parameters)
		r["type_parameters"] = pseudoTypeParameters(d.TypeParameters)
		r["return"] = pseudoTree(d.ReturnType)
	case PseudoTypeKindObjectLiteral:
		es := make([]any, 0)
		for _, e := range t.AsPseudoTypeObjectLiteral().Elements {
			m := map[string]any{"kind": e.Kind, "name": pseudoNode(e.Name), "optional": e.Optional}
			switch e.Kind {
			case PseudoObjectElementKindMethod:
				d := e.AsPseudoObjectMethod()
				m["node"] = pseudoNode(d.Signature)
				m["parameters"] = pseudoParameters(d.Parameters)
				m["type_parameters"] = pseudoTypeParameters(d.TypeParameters)
				m["return"] = pseudoTree(d.ReturnType)
			case PseudoObjectElementKindPropertyAssignment:
				d := e.AsPseudoPropertyAssignment()
				m["readonly"] = d.Readonly
				m["type"] = pseudoTree(d.Type)
			case PseudoObjectElementKindSetAccessor:
				d := e.AsPseudoSetAccessor()
				m["node"] = pseudoNode(d.Signature)
				m["parameter"] = pseudoParameter(d.Parameter)
			case PseudoObjectElementKindGetAccessor:
				d := e.AsPseudoGetAccessor()
				m["node"] = pseudoNode(d.Signature)
				m["type"] = pseudoTree(d.Type)
			}
			es = append(es, m)
		}
		r["elements"] = es
	}
	return r
}
func pseudoOperations(n *ast.Node) []string {
	ops := make([]string, 0)
	switch n.Kind {
	case ast.KindParameter, ast.KindVariableDeclaration, ast.KindPropertySignature, ast.KindPropertyDeclaration, ast.KindJSDocPropertyTag, ast.KindBindingElement, ast.KindExportAssignment, ast.KindPropertyAccessExpression, ast.KindElementAccessExpression, ast.KindBinaryExpression, ast.KindPropertyAssignment, ast.KindShorthandPropertyAssignment, ast.KindCallExpression:
		ops = append(ops, "declaration")
	}
	switch n.Kind {
	case ast.KindOmittedExpression, ast.KindParenthesizedExpression, ast.KindIdentifier, ast.KindNullKeyword, ast.KindArrowFunction, ast.KindFunctionExpression, ast.KindTypeAssertionExpression, ast.KindAsExpression, ast.KindPrefixUnaryExpression, ast.KindArrayLiteralExpression, ast.KindObjectLiteralExpression, ast.KindClassExpression, ast.KindTemplateExpression, ast.KindNumericLiteral, ast.KindNoSubstitutionTemplateLiteral, ast.KindStringLiteral, ast.KindBigIntLiteral, ast.KindTrueKeyword, ast.KindFalseKeyword, ast.KindCallExpression:
		ops = append(ops, "expression")
	}
	switch n.Kind {
	case ast.KindGetAccessor, ast.KindMethodDeclaration, ast.KindFunctionDeclaration, ast.KindConstructor, ast.KindMethodSignature, ast.KindCallSignature, ast.KindConstructSignature, ast.KindSetAccessor, ast.KindIndexSignature, ast.KindFunctionType, ast.KindConstructorType, ast.KindFunctionExpression, ast.KindArrowFunction, ast.KindJSDocSignature:
		ops = append(ops, "return")
	}
	if ast.IsAccessor(n) {
		ops = append(ops, "accessor")
	}
	return ops
}
func TestS08PseudoChecker(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		ID     string `json:"id"`
		Source string `json:"source"`
		File   string `json:"file"`
	}
	if err = json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	rows := make([]any, 0)
	for _, c := range cases {
		file := c.File
		if file == "" {
			file = "/pseudo.ts"
		}
		sf := parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: file, Path: tspath.Path(file)}, c.Source, core.GetScriptKindFromFileName(file))
		binder.BindSourceFile(sf)
		for _, strict := range []bool{false, true} {
			ch := NewPseudoChecker(strict, false)
			var visit func(*ast.Node) bool
			visit = func(n *ast.Node) bool {
				for _, op := range pseudoOperations(n) {
					var ty *PseudoType
					switch op {
					case "declaration":
						ty = ch.GetTypeOfDeclaration(n)
					case "expression":
						ty = ch.GetTypeOfExpression(n)
					case "return":
						ty = ch.GetReturnTypeOfSignature(n)
					case "accessor":
						ty = ch.GetTypeOfAccessor(n)
					}
					rows = append(rows, map[string]any{"case": c.ID, "strict": strict, "node": pseudoNode(n), "operation": op, "const_context": IsInConstContext(n), "could_be_undefined": CouldAlreadyReferToUndefinedType(ty), "type": pseudoTree(ty)})
				}
				n.ForEachChild(visit)
				return false
			}
			visit(sf.AsNode())
		}
	}
	sum := sha256.Sum256(raw)
	data, err := json.Marshal(map[string]any{"request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("S08_OUTPUT"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
