package checker

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/nodebuilder"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
)

func C2AuditIdentical(c *Checker, a, b *Type) bool { return c.isTypeIdenticalTo(a, b) }

// This explicit state schedule isolates the interval between an intermediate
// constraint being cached and its successful resolution frame being popped.
// It never changes the source mapped property or the production cycle algorithm.
func C2AuditMappedCycle(c *Checker, node *ast.Node) any {
	typ := c.GetTypeAtLocation(node)
	prop := c.getPropertyOfType(typ, "a")
	if prop == nil || prop.CheckFlags&ast.CheckFlagsMapped == 0 || c.valueSymbolLinks.Get(prop).resolvedType != nil {
		panic("unresolved source mapped property required")
	}
	depth := len(c.typeResolutions)
	p := c.newTypeParameter(nil)
	if !c.pushTypeResolution(prop, TypeSystemPropertyNameType) {
		panic("mapped push")
	}
	without := c.isCircularMappedProperty(prop)
	if !c.pushTypeResolution(p, TypeSystemPropertyNameResolvedBaseConstraint) {
		panic("constraint push")
	}
	unfinished := c.isCircularMappedProperty(prop)
	p.AsConstrainedType().resolvedBaseConstraint = c.unknownType
	completed := c.isCircularMappedProperty(prop)
	first := c.popTypeResolution()
	after := c.isCircularMappedProperty(prop)
	second := c.popTypeResolution()
	return map[string]any{"without_intermediate": without, "unfinished_intermediate": unfinished, "completed_intermediate": completed, "after_pop": after, "pops": []bool{first, second}, "restored": len(c.typeResolutions) == depth}
}

func C2AuditParameters(c *Checker, nodes map[string]*ast.Node) any {
	aliases := []any{}
	for _, name := range []string{"Plain", "Erased", "Defaults", "Bound"} {
		symbol := c.GetSymbolAtLocation(nodes[name].Name())
		parameters := c.GetTypeAliasTypeParameters(symbol)
		again := c.GetTypeAliasTypeParameters(symbol)
		names := []string{}
		same := len(parameters) == len(again)
		for i, p := range parameters {
			names = append(names, p.symbol.Name)
			same = same && p == again[i]
		}
		aliases = append(aliases, map[string]any{"name": name, "parameters": names, "same": same})
	}
	capabilities := []bool{}
	for _, name := range []string{"Outer", "Face", "fun", "arrow", "functionExpr", "plain", "Plain", "Alias"} {
		capabilities = append(capabilities, c.canGetTypeParametersOfClassOrInterface(c.GetSymbolAtLocation(nodes[name].Name())))
	}
	unconstrained := []bool{}
	parameters := []*Type{}
	for _, name := range []string{"P", "D", "B", "K", "R"} {
		p := c.getDeclaredTypeOfSymbol(c.getSymbolOfDeclaration(nodes[name]))
		parameters = append(parameters, p)
		unconstrained = append(unconstrained, isUnconstrainedTypeParameter(p))
	}
	for _, i := range []int{0, 2} {
		clone := c.newTypeParameter(nil)
		clone.AsTypeParameter().target = parameters[i]
		unconstrained = append(unconstrained, isUnconstrainedTypeParameter(clone))
	}
	unconstrained = append(unconstrained, isUnconstrainedTypeParameter(c.newTypeParameter(nil)))
	before := len(c.inferenceContextInfos)
	first := c.newInferenceContext([]*Type{parameters[0], parameters[2]}, nil, 0, nil)
	second := c.newInferenceContext([]*Type{parameters[1], parameters[0]}, nil, 0, nil)
	c.pushInferenceContext(nodes["Plain"], first)
	c.pushInferenceContext(nodes["Plain"], nil)
	c.pushInferenceContext(nodes["Plain"], second)
	outer := []string{}
	for _, p := range c.getOuterInferenceTypeParameters() {
		outer = append(outer, p.symbol.Name)
	}
	c.popInferenceContext()
	c.popInferenceContext()
	c.popInferenceContext()
	outerSymbol := c.GetSymbolAtLocation(nodes["Outer"].Name())
	aliasSymbol := c.GetSymbolAtLocation(nodes["Alias"].Name())
	property := c.getPropertyOfType(c.GetTypeAtLocation(nodes["instance"].Name()), "item")
	original := c.getPropertyOfType(c.getDeclaredTypeOfSymbol(outerSymbol), "item")
	qualified := []any{}
	for _, chain := range [][]*ast.Symbol{{outerSymbol, property}, {aliasSymbol, property}, {outerSymbol, original}} {
		emit := printer.NewEmitContext()
		b := NewNodeBuilder(c, emit)
		b.enterContext(nil, nodebuilder.FlagsWriteTypeParametersInQualifiedName, 0, nil)
		texts := []any{}
		for range 2 {
			list := b.impl.lookupTypeParameterNodes(chain, 0)
			var text any
			if list != nil {
				n := b.impl.f.NewTupleTypeNode(list)
				w := printer.NewTextWriter("", 0)
				printer.NewPrinter(printer.PrinterOptions{}, printer.PrintHandlers{}, emit).Write(n, nil, w, nil)
				text = w.String()
			}
			texts = append(texts, text)
		}
		b.popContext()
		qualified = append(qualified, texts)
	}
	return map[string]any{"qualified": qualified, "aliases": aliases, "capabilities": capabilities, "unconstrained": unconstrained, "outer": outer, "restored": len(c.inferenceContextInfos) == before}
}
