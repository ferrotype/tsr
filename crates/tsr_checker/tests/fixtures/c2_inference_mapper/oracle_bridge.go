package checker

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// Calls existing private algorithms with constructed checker-owned types;
// observes mapping results and comparison signs without changing any algorithm.
func C2InferenceMapperProbe(c *Checker, site *ast.Node) map[string]any {
	p := c.newTypeParameter(nil)
	this := c.newTypeParameter(nil)
	this.AsTypeParameter().isThisType = true
	mappers := []*TypeMapper{
		newArrayToSingleTypeMapper(nil, c.anyType),
		newArrayToSingleTypeMapper([]*Type{p}, c.anyType),
		newArrayToSingleTypeMapper([]*Type{this}, c.anyType),
		newArrayToSingleTypeMapper([]*Type{p, this}, c.anyType),
		newSimpleTypeMapper(p, c.anyType),
		newArrayTypeMapper([]*Type{p, this}, []*Type{c.anyType, c.anyType}),
		newDeferredTypeMapper([]*Type{this}, []func() *Type{func() *Type { return c.stringType }}),
	}
	label := func(t *Type) string {
		switch t {
		case p:
			return "parameter"
		case this:
			return "this"
		case c.anyType:
			return "any"
		case c.stringType:
			return "string"
		default:
			panic("unexpected mapped type")
		}
	}
	rows := []any{}
	for _, m := range mappers {
		mapped := []string{}
		for _, t := range []*Type{p, this, c.stringType} {
			mapped = append(mapped, label(m.Map(t)))
		}
		cmp := []int{}
		for _, other := range mappers {
			v := compareTypeMappers(m, other)
			if v < 0 {
				v = -1
			} else if v > 0 {
				v = 1
			}
			cmp = append(cmp, v)
		}
		rows = append(rows, map[string]any{"maps_this_only": m.MapsThisOnly(), "mapped": mapped, "compare": cmp})
	}
	inferred := []any{}
	for _, scenario := range []string{"ordinary-index", "pattern-index", "enum-index"} {
		key := c.newTypeParameter(nil)
		value := c.newTypeParameter(nil)
		target := c.newObjectType(ObjectFlagsMapped, nil)
		target.AsMappedType().templateType = value
		members := make(ast.SymbolTable)
		var indexes []*IndexInfo
		if scenario == "enum-index" {
			indexes = []*IndexInfo{c.enumNumberIndexInfo}
		} else {
			prop := c.newSymbol(ast.SymbolFlagsProperty, "x")
			c.valueSymbolLinks.Get(prop).resolvedType = c.stringType
			members["x"] = prop
			indexes = []*IndexInfo{c.newIndexInfo(c.stringType, c.stringType, false, nil, nil)}
		}
		source := c.newAnonymousType(nil, members, nil, nil, indexes)
		if scenario == "pattern-index" {
			c.patternForType[source] = site
		}
		infos := []*InferenceInfo{newInferenceInfo(key), newInferenceInfo(value)}
		n := c.getInferenceState()
		n.inferences = infos
		n.originalSource, n.originalTarget = source, target
		n.inferencePriority = InferencePriorityMaxValue
		c.inferToMappedType(n, source, target, key)
		values := []string{}
		for _, info := range infos {
			values = append(values, c.TypeToString(c.getTypeFromInference(info)))
		}
		c.putInferenceState(n)
		inferred = append(inferred, map[string]any{"id": scenario, "inferred": values})
	}
	// A generic tuple is a non-generic object for the intersection reduction
	// predicate: retaining it permits inference through its length property.
	u := c.newTypeParameter(nil)
	source := c.createTupleTypeEx([]*Type{p}, []TupleElementInfo{{flags: ElementFlagsVariadic}}, false)
	length := c.newSymbol(ast.SymbolFlagsProperty, "length")
	c.valueSymbolLinks.Get(length).resolvedType = u
	object := c.newAnonymousType(nil, ast.SymbolTable{"length": length}, nil, nil, nil)
	object.objectFlags |= ObjectFlagsCouldContainTypeVariables | ObjectFlagsCouldContainTypeVariablesComputed
	target := c.getIntersectionType([]*Type{source, object})
	context := c.newInferenceContext([]*Type{u}, nil, 0, nil)
	c.inferTypes(context.inferences, source, target, InferencePriorityNone, false)
	inference := c.getTypeFromInference(context.inferences[0])
	tupleIntersection := map[string]any{"generic_tuple": c.isGenericTupleType(source), "candidate_count": len(context.inferences[0].candidates)}
	if inference != nil {
		tupleIntersection["inferred"] = c.TypeToString(inference)
	}
	signature := c.newSignature(0, nil, []*Type{p}, nil, nil, p, nil, 0)
	permissiveSignature := c.instantiateSignature(signature, c.permissiveMapper)
	restrictiveSignature := c.instantiateSignature(signature, c.restrictiveMapper)
	signatures := []any{}
	for _, sig := range []*Signature{permissiveSignature, restrictiveSignature} {
		cached := sig.resolvedReturnType != nil
		result := c.getReturnTypeOfSignature(sig)
		signatures = append(signatures, map[string]any{"type_parameters": len(sig.typeParameters), "return_cached_before": cached, "return_is_wildcard": result == c.wildcardType})
	}
	defaults := []any{}
	var visit func(*ast.Node) bool
	visit = func(n *ast.Node) bool {
		if n.Kind == ast.KindTypeParameter {
			t := c.GetTypeAtLocation(n.Name())
			before := t.AsTypeParameter().resolvedDefaultType != nil
			present := hasTypeParameterDefault(t)
			after := t.AsTypeParameter().resolvedDefaultType != nil
			defaults = append(defaults, map[string]any{"name": n.Parent.Name().Text(), "syntactic": present, "cached_before": before, "cached_after": after})
		}
		n.ForEachChild(visit)
		return false
	}
	visit(site)
	return map[string]any{"mappers": rows, "mapped_inference": inferred, "defaults": defaults, "tuple_intersection": tupleIntersection, "generic_signatures": signatures}
}
