package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// The schedule calls production inference operations; snapshots only read state.
func C2ContractInference(c *Checker, node *ast.Node) any {
	t := c.GetTypeAtLocation(node)
	p := c.getSignaturesOfType(t, SignatureKindCall)[0].typeParameters[0]
	n := c.newInferenceContext([]*Type{p}, nil, InferenceFlagsNone, nil)
	c.inferTypes(n.inferences, c.stringType, p, InferencePriorityNone, false)
	rows := []any{}
	snapshot := func(label string, ctx *InferenceContext, typ *Type) {
		rows = append(rows, map[string]any{"label": label, "fixed": ctx.inferences[0].isFixed, "candidates": len(ctx.inferences[0].candidates), "type": c.TypeToString(typ)})
	}
	snapshot("nonfixing", n, c.instantiateType(p, n.nonFixingMapper))
	clone := c.cloneInferenceContext(n, InferenceFlagsNone)
	snapshot("clone_fixed", clone, c.instantiateType(p, clone.mapper))
	snapshot("original_after_clone", n, c.instantiateType(p, n.nonFixingMapper))
	c.inferTypes(n.inferences, c.numberType, p, InferencePriorityNone, false)
	snapshot("fixed", n, c.instantiateType(p, n.mapper))
	c.inferTypes(n.inferences, c.booleanType, p, InferencePriorityNone, false)
	snapshot("later_candidate", n, c.instantiateType(p, n.mapper))
	return rows
}

func C2ContractHigherOrder(c *Checker, t *Type) any {
	signatures := c.getSignaturesOfType(t, SignatureKindCall)
	rows := []any{}
	for _, s := range signatures {
		params := []any{}
		for _, p := range s.typeParameters {
			permissive := c.getPermissiveInstantiation(p)
			inference := c.newInferenceContext([]*Type{permissive}, s, InferenceFlagsNone, nil)
			params = append(params, map[string]any{"type": c.TypeToString(p), "permissive": c.TypeToString(permissive), "wildcard": permissive == c.wildcardType, "has_default": c.getDefaultFromTypeParameter(permissive) != nil, "inferred": c.TypeToString(c.getInferredType(inference, 0))})
		}
		rows = append(rows, map[string]any{"parameters": params})
	}
	return rows
}

func C2ContractContextDepth(c *Checker) int { return len(c.contextualInfos) }

type c2InstantiationStats struct {
	MaximumDepth   uint32 `json:"maximum_depth"`
	DepthLimitHits int    `json:"depth_limit_hits"`
	CountLimitHits int    `json:"count_limit_hits"`
}

var c2InstantiationObservers = map[*Checker]*c2InstantiationStats{}

func C2ContractBeginInstantiation(c *Checker) { c2InstantiationObservers[c] = &c2InstantiationStats{} }
func c2ContractInstantiationEnter(c *Checker) {
	if p := c2InstantiationObservers[c]; p != nil && c.instantiationDepth > p.MaximumDepth {
		p.MaximumDepth = c.instantiationDepth
	}
}
func c2ContractInstantiationLimit(c *Checker) {
	if p := c2InstantiationObservers[c]; p != nil {
		if c.instantiationDepth == 100 {
			p.DepthLimitHits++
		}
		if c.instantiationCount >= 5_000_000 {
			p.CountLimitHits++
		}
	}
}
func C2ContractTakeInstantiation(c *Checker) any {
	p := c2InstantiationObservers[c]
	delete(c2InstantiationObservers, c)
	return map[string]any{"maximum_depth": p.MaximumDepth, "depth_limit_hits": p.DepthLimitHits, "count_limit_hits": p.CountLimitHits, "restored_depth": c.instantiationDepth}
}

func C2ContractOverloadSymbol(c *Checker, node *ast.Node) any {
	symbol := c.GetSymbolAtLocation(node.Initializer())
	typ := c.GetTypeAtLocation(node.Initializer())
	return map[string]any{"name": symbol.Name, "declarations": len(symbol.Declarations), "signatures": len(c.getSignaturesOfType(typ, SignatureKindCall))}
}
