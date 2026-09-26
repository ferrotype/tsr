package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

var c2VarianceObservers = map[*Checker]*[2]int{}

func c2VarianceCycle(c *Checker, restart bool) {
	if p := c2VarianceObservers[c]; p != nil {
		if restart {
			p[1]++
		} else {
			p[0]++
		}
	}
}
func C2Variance(c *Checker, node *ast.Node) any {
	s := c.GetSymbolAtLocation(node)
	typ := c.getDeclaredTypeOfSymbol(s)
	c2VarianceObservers[c] = &[2]int{}
	query := func() []VarianceFlags {
		if s.Flags&ast.SymbolFlagsTypeAlias != 0 {
			return c.getAliasVariances(s)
		}
		return c.getVariances(typ)
	}
	values := query()
	counts := *c2VarianceObservers[c]
	delete(c2VarianceObservers, c)
	repeated := query()
	return map[string]any{"flags": values, "cached_flags": repeated, "cycles": counts[0], "restarts": counts[1], "restored_stack": len(c.varianceStack), "restored_reliability": c.reliabilityFlags}
}

// Explicit repeated production-call schedule; neither count nor threshold is seeded.
func C2InstantiationCount(c *Checker, node *ast.Node) any {
	t := c.GetTypeAtLocation(node)
	p := c.getSignaturesOfType(t, SignatureKindCall)[0].typeParameters[0]
	mapper := newTypeMapper([]*Type{p}, []*Type{c.stringType})
	start := c.instantiationCount
	total := c.TotalInstantiationCount
	saved := c.currentNode
	c.currentNode = node
	completed := 0
	errorType := false
	for i := 0; i <= 5_000_000; i++ {
		result := c.instantiateType(p, mapper)
		if result == c.errorType {
			errorType = true
			break
		}
		if result != c.stringType {
			panic("unexpected substitution")
		}
		completed++
	}
	c.currentNode = saved
	return map[string]any{"starting_count": start, "completed": completed, "count": c.instantiationCount, "total_delta": c.TotalInstantiationCount - total, "error_type": errorType, "restored_depth": c.instantiationDepth}
}
func C2SubtypeLimit(c *Checker, node *ast.Node) any {
	t := c.GetTypeAtLocation(node)
	parts := t.Types()
	saved := c.currentNode
	c.currentNode = node
	result := c.getUnionTypeEx(parts, UnionReductionSubtype, nil, nil)
	c.currentNode = saved
	return map[string]any{"constituents": len(parts), "error_type": result == c.errorType, "result_constituents": func() int {
		if result.flags&TypeFlagsUnion != 0 {
			return len(result.Types())
		}
		return 0
	}()}
}
func C2Nested(c *Checker, node *ast.Node, stack []*ast.Node, max int) bool {
	t := c.GetTypeAtLocation(node)
	types := []*Type{}
	for _, n := range stack {
		types = append(types, c.GetTypeAtLocation(n))
	}
	return c.isDeeplyNestedType(t, types, max)
}

type c2LimitsStats struct {
	SubtypeEstimates  [][2]int `json:"subtype_estimates"`
	BaseDepths        []int    `json:"base_depths"`
	ConditionalDepths []int    `json:"conditional_depths"`
}

var c2LimitsObservers = map[*Checker]*c2LimitsStats{}

func C2BeginLimits(c *Checker) {
	c2LimitsObservers[c] = &c2LimitsStats{SubtypeEstimates: [][2]int{}, BaseDepths: []int{}, ConditionalDepths: []int{}}
}
func C2TakeLimits(c *Checker) any {
	p := c2LimitsObservers[c]
	delete(c2LimitsObservers, c)
	return map[string]any{"subtype_estimates": p.SubtypeEstimates, "base_depths": p.BaseDepths, "conditional_depths": p.ConditionalDepths, "restored_conditional_depth": c.conditionalConstraintDepth}
}
func c2SubtypeLimit(c *Checker, count, estimate int) {
	if p := c2LimitsObservers[c]; p != nil {
		p.SubtypeEstimates = append(p.SubtypeEstimates, [2]int{count, estimate})
	}
}
func c2BaseLimit(c *Checker, depth int) {
	if p := c2LimitsObservers[c]; p != nil {
		p.BaseDepths = append(p.BaseDepths, depth)
	}
}
func c2ConditionalLimit(c *Checker) {
	if p := c2LimitsObservers[c]; p != nil {
		p.ConditionalDepths = append(p.ConditionalDepths, int(c.conditionalConstraintDepth))
	}
}
func C2BaseConstraint(c *Checker, node *ast.Node) any {
	t := c.GetTypeAtLocation(node)
	constraint := c.getBaseConstraintOfType(t)
	var display any
	if constraint != nil {
		display = c.TypeToString(constraint)
	}
	return map[string]any{"constraint": display}
}
func C2InstantiationCountValue(c *Checker) uint32 { return c.instantiationCount }
