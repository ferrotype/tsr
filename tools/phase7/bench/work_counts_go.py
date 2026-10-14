"""Untimed Go checker counters, produced as an overlay over the pinned module.

``edits(root)`` reads a pristine ``upstream/tsc`` module and returns relative
paths and replacement contents. It never writes upstream files. Counters count
executed boundaries, not unique expressions/types, and do not join numeric IDs
across runtimes. Atomic increments deliberately make this unsuitable for timing.
"""
from pathlib import Path


# Shared with the Rust overlay. Preserve this order: generated increments use
# literal array indexes, with no string lookup on a counter's ordinary path.
COUNTERS = (
    'choose_overload',
    'overload_candidate',
    'overload_retry',
    'signature_applicable',
    'applicability_argument',
    'infer_type_arguments',
    'infer_types',
    'infer_from_types',
    'expression_worker',
    'expression_object',
    'expression_function',
    'expression_identifier',
    'expression_property',
    'expression_call',
    'expression_array',
    'expression_cached_request',
    'expression_cached_miss',
    'expression_contextual',
    'expression_context_free_hit',
    'instantiate_type_request',
    'instantiate_type_early_return',
    'instantiate_type_cache_hit',
    'instantiate_type_worker',
    'signature_instantiation_request',
    'signature_instantiation_cache_hit',
    'instantiate_signature_worker',
    'signature_created',
    'type_created',
    'type_created_object',
    'type_created_union',
    'type_created_intersection',
    'type_created_conditional',
    'relation_request',
    'relation_worker',
    'relation_recursive',
    'relation_cache_hit',
    'relation_recursive_cache_hit',
    'relation_properties',
    'flow_reference',
    'flow_node_visit',
    'flow_shared_hit',
    'reachable_request',
    'reachable_node_visit',
    'reachable_cache_hit',
)

CHECKER = 'internal/checker/checker.go'
INFERENCE = 'internal/checker/inference.go'
RELATER = 'internal/checker/relater.go'
FLOW = 'internal/checker/flow.go'
HELPER = 'internal/checker/tsr_work_counts.go'
MAIN = 'cmd/tsc/main.go'


def _hit(counter: str) -> str:
    return f'tsrWorkCounters[{COUNTERS.index(counter)}].Add(1)'


def _once(source: str, old: str, new: str, context: str) -> str:
    count = source.count(old)
    if count != 1:
        raise ValueError(f'{context}: expected one pristine match, found {count}: {old!r}')
    return source.replace(old, new, 1)


def _helper(relation_origins: bool = False) -> str:
    names = ',\n'.join(f'\t"{name}"' for name in COUNTERS) + ',\n'
    expression_cases = (
        ('ast.KindObjectLiteralExpression', 'expression_object'),
        ('ast.KindFunctionExpression, ast.KindArrowFunction', 'expression_function'),
        ('ast.KindIdentifier', 'expression_identifier'),
        ('ast.KindPropertyAccessExpression, ast.KindQualifiedName', 'expression_property'),
        ('ast.KindCallExpression, ast.KindNewExpression', 'expression_call'),
        ('ast.KindArrayLiteralExpression', 'expression_array'),
    )
    expression = '\n'.join(f'\tcase {kinds}:\n\t\t{_hit(key)}' for kinds, key in expression_cases)
    type_cases = (
        ('TypeFlagsObject', 'type_created_object'),
        ('TypeFlagsUnion', 'type_created_union'),
        ('TypeFlagsIntersection', 'type_created_intersection'),
        ('TypeFlagsConditional', 'type_created_conditional'),
    )
    types = '\n'.join(f'\tif flags&{flag} != 0 {{\n\t\t{_hit(key)}\n\t}}' for flag, key in type_cases)
    source = '''// Diagnostic overlay only; never use this executable for timing.
package checker

import (
    "encoding/json"
    "os"
    "sync/atomic"

    "github.com/microsoft/TypeScript/tsc/internal/ast"
)

var tsrWorkCounters [_COUNT_]atomic.Uint64
var tsrWorkNames = [...]string{
_NAMES_}

func tsrWorkExpression(node *ast.Node) {
    _EXPRESSION_ENTRY_
    switch node.Kind {
_EXPRESSION_CASES_
    }
}

func tsrWorkTypeCreated(flags TypeFlags) {
    _TYPE_ENTRY_
_TYPE_CASES_
}

// DumpWorkCounts runs after normal CLI work and before os.Exit. Exclusive
// creation prevents a stale result from being mistaken for this execution.
func DumpWorkCounts() {
    path := os.Getenv("TSR_WORK_COUNTS")
    if path == "" {
        return
    }
    counts := make(map[string]uint64, len(tsrWorkNames))
    for index, name := range tsrWorkNames {
        counts[name] = tsrWorkCounters[index].Load()
    }
    file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
    if err != nil {
        panic(err)
    }
    if err := json.NewEncoder(file).Encode(map[string]any{"format": 1, "counters": counts}); err != nil {
        file.Close()
        panic(err)
    }
    if err := file.Close(); err != nil {
        panic(err)
    }
}
'''.replace('_COUNT_', str(len(COUNTERS))).replace('_NAMES_', names).replace(
        '_EXPRESSION_ENTRY_', _hit('expression_worker')).replace(
        '_EXPRESSION_CASES_', expression).replace('_TYPE_ENTRY_', _hit('type_created')).replace(
        '_TYPE_CASES_', types)
    if relation_origins:
        source = _once(source, '    "os"\n',
                       '    "os"\n    "fmt"\n    "path/filepath"\n    "runtime"\n    "sync"\n', HELPER)
        source = _once(source,
                       '    if err := json.NewEncoder(file).Encode(map[string]any{"format": 1, "counters": counts}); err != nil {\n',
                       '''    origins := make(map[string]uint64)
    var originTotal uint64
    tsrWorkOriginMu.Lock()
    for origin, count := range tsrWorkOrigins {
        origins[origin] = count
        originTotal += count
    }
    tsrWorkOriginMu.Unlock()
    if originTotal != counts["relation_request"] {
        panic("relation origin total does not match relation_request")
    }
    if err := json.NewEncoder(file).Encode(map[string]any{"format": 1, "counters": counts, "relation_origins": origins}); err != nil {
''', HELPER)
        source += _origin_helper()
    return source


def _origin_helper() -> str:
    # Only forwarding wrappers are skipped. Helpers with semantic conditions
    # (such as areTypesComparable) remain visible as the actual request origin.
    wrappers = (
        'isTypeRelatedTo', 'isTypeIdenticalTo', 'compareTypesIdentical',
        'compareTypesAssignableSimple', 'compareTypesAssignableWorker',
        'compareTypesSubtypeOf', 'isTypeAssignableTo', 'isTypeSubtypeOf',
        'isTypeStrictSubtypeOf', 'isTypeComparableTo',
    )
    names = ',\n'.join(
        f'            "github.com/microsoft/TypeScript/tsc/internal/checker.(*Checker).{name}{suffix}"'
        for name in wrappers
        for suffix in ('', '-fm')
    )
    return '''
// Relation origins are bounded by static call sites, never by type/node IDs.
// CallersFrames expands inlined forwarding wrappers before the filter below.
var tsrWorkOriginMu sync.Mutex
var tsrWorkOrigins = make(map[string]uint64)

func tsrWorkRelationOrigin() {
    var pcs [16]uintptr
    count := runtime.Callers(2, pcs[:])
    frames := runtime.CallersFrames(pcs[:count])
    origin := "unknown:0:stack-exhausted"
    for {
        frame, more := frames.Next()
        switch frame.Function {
        case
_WRAPPERS_:
            // Find the caller requesting a specific relation, not its mode wrapper.
        default:
            origin = fmt.Sprintf("%s:%d:%s", filepath.Base(frame.File), frame.Line, frame.Function)
            tsrWorkOriginMu.Lock()
            tsrWorkOrigins[origin]++
            tsrWorkOriginMu.Unlock()
            return
        }
        if !more {
            break
        }
    }
    tsrWorkOriginMu.Lock()
    tsrWorkOrigins[origin]++
    tsrWorkOriginMu.Unlock()
}
'''.replace('_WRAPPERS_', names)


def edits(root: Path, relation_origins: bool = False) -> dict[str, str]:
    """Return checked source replacements relative to the pinned Go module."""
    root = Path(root)
    if (root / HELPER).exists():
        raise ValueError(f'{root / HELPER}: diagnostic helper already exists')
    sources = {path: (root / path).read_text() for path in (CHECKER, INFERENCE, RELATER, FLOW, MAIN)}
    if any('tsrWorkCounters' in source or 'checker.DumpWorkCounts()' in source for source in sources.values()):
        raise ValueError('expected pristine Go sources, found work-count instrumentation')

    def entry(path, header, statement):
        sources[path] = _once(sources[path], header + '\n', header + '\n\t' + statement + '\n', path)

    def within(path, header, old, new):
        source = sources[path]
        if source.count(header + '\n') != 1:
            raise ValueError(f'{path}: function header is absent or ambiguous: {header}')
        start = source.index(header + '\n')
        end = source.find('\nfunc ', start + len(header))
        if end == -1:
            end = len(source)
        body = _once(source[start:end], old, new, f'{path}: {header}')
        sources[path] = source[:start] + body + source[end:]

    choose = 'func (c *Checker) chooseOverload(s *CallState, relation *Relation) *Signature {'
    applicable = 'func (c *Checker) isSignatureApplicable(node *ast.Node, args []*ast.Node, signature *Signature, relation *Relation, checkMode CheckMode, reportErrors bool, diagnosticOutput *[]*ast.Diagnostic) bool {'
    cached = 'func (c *Checker) checkExpressionCachedEx(node *ast.Node, checkMode CheckMode) *Type {'
    context_free = 'func (c *Checker) getContextFreeTypeOfExpression(node *ast.Node) *Type {'
    instantiate = 'func (c *Checker) instantiateTypeWithAlias(t *Type, m *TypeMapper, alias *TypeAlias) *Type {'
    signature_cache = 'func (c *Checker) getSignatureInstantiationWithoutFillingInTypeArguments(sig *Signature, typeArguments []*Type) *Signature {'
    related = 'func (c *Checker) isTypeRelatedTo(source *Type, target *Type, relation *Relation) bool {'
    recursive = 'func (r *Relater) recursiveTypeRelatedTo(source *Type, target *Type, reportErrors bool, intersectionState IntersectionState, recursionFlags RecursionFlags) Ternary {'
    flow_type = 'func (c *Checker) getTypeAtFlowNode(f *FlowState, flow *ast.FlowNode) FlowType {'
    reachable = 'func (c *Checker) isReachableFlowNodeWorker(f *FlowState, flow *ast.FlowNode, noCacheCheck bool) bool {'

    entries = (
        (CHECKER, choose, 'choose_overload'),
        (CHECKER, applicable, 'signature_applicable'),
        (CHECKER, 'func (c *Checker) inferTypeArguments(node *ast.Node, signature *Signature, args []*ast.Node, checkMode CheckMode, context *InferenceContext) []*Type {', 'infer_type_arguments'),
        (INFERENCE, 'func (c *Checker) inferTypes(inferences []*InferenceInfo, originalSource *Type, originalTarget *Type, priority InferencePriority, contravariant bool) {', 'infer_types'),
        (INFERENCE, 'func (c *Checker) inferFromTypes(n *InferenceState, source *Type, target *Type) {', 'infer_from_types'),
        (CHECKER, cached, 'expression_cached_request'),
        (CHECKER, 'func (c *Checker) checkExpressionWithContextualType(node *ast.Node, contextualType *Type, inferenceContext *InferenceContext, checkMode CheckMode) *Type {', 'expression_contextual'),
        (CHECKER, instantiate, 'instantiate_type_request'),
        (CHECKER, 'func (c *Checker) instantiateTypeWorker(t *Type, m *TypeMapper, alias *TypeAlias) *Type {', 'instantiate_type_worker'),
        # This helper also has direct callers that bypass getSignatureInstantiation.
        (CHECKER, signature_cache, 'signature_instantiation_request'),
        (CHECKER, 'func (c *Checker) instantiateSignatureEx(sig *Signature, m *TypeMapper, eraseTypeParameters bool) *Signature {', 'instantiate_signature_worker'),
        (CHECKER, 'func (c *Checker) newSignature(flags SignatureFlags, declaration *ast.Node, typeParameters []*Type, thisParameter *ast.Symbol, parameters []*ast.Symbol, resolvedReturnType *Type, resolvedTypePredicate *TypePredicate, minArgumentCount int) *Signature {', 'signature_created'),
        (RELATER, related, 'relation_request'),
        (RELATER, 'func (r *Relater) isRelatedToEx(originalSource *Type, originalTarget *Type, recursionFlags RecursionFlags, reportErrors bool, headMessage *diagnostics.Message, intersectionState IntersectionState) Ternary {', 'relation_worker'),
        (RELATER, recursive, 'relation_recursive'),
        (RELATER, 'func (r *Relater) propertiesRelatedTo(source *Type, target *Type, reportErrors bool, excludedProperties collections.Set[string], optionalsOnly bool, intersectionState IntersectionState) Ternary {', 'relation_properties'),
        (FLOW, 'func (c *Checker) getFlowTypeOfReferenceEx(reference *ast.Node, declaredType *Type, initialType *Type, flowContainer *ast.Node, flowNode *ast.FlowNode) *Type {', 'flow_reference'),
        (FLOW, 'func (c *Checker) isReachableFlowNode(flow *ast.FlowNode) bool {', 'reachable_request'),
    )
    for path, header, counter in entries:
        entry(path, header, _hit(counter))
    if relation_origins:
        entry(RELATER, related, 'tsrWorkRelationOrigin()')
    entry(CHECKER, 'func (c *Checker) checkExpressionWorker(node *ast.Node, checkMode CheckMode) *Type {', 'tsrWorkExpression(node)')
    entry(CHECKER, 'func (c *Checker) newType(flags TypeFlags, objectFlags ObjectFlags, data TypeData) *Type {', 'tsrWorkTypeCreated(flags)')

    # Increments only: preserve all original predicates, short circuits and
    # checker calls. Counter-only conditionals inspect already computed locals.
    branches = (
        (CHECKER, choose, '\tif s.isSingleNonGenericCandidate {\n', '\t\t', 'overload_candidate'),
        (CHECKER, choose, '\tfor candidateIndex, candidate := range s.candidates {\n', '\t\t', 'overload_candidate'),
        (CHECKER, choose, '\t\tif s.argCheckMode != 0 {\n', '\t\t\t', 'overload_retry'),
        (CHECKER, applicable, '\t\tif !ast.IsOmittedExpression(arg) {\n', '\t\t\t', 'applicability_argument'),
        (CHECKER, cached, '\tif links.resolvedType == nil {\n', '\t\t', 'expression_cached_miss'),
        (CHECKER, context_free, '\tif cached := c.contextFreeTypes[node]; cached != nil {\n', '\t\t', 'expression_context_free_hit'),
        (CHECKER, instantiate, '\tif t == nil || m == nil || !(c.couldContainTypeVariables(t) || (t.alias != nil && len(t.alias.typeArguments) > 0 && core.Some(t.alias.typeArguments, c.couldContainTypeVariables))) {\n', '\t\t', 'instantiate_type_early_return'),
        (CHECKER, instantiate, '\tif cachedType, ok := cache[key]; ok {\n', '\t\t', 'instantiate_type_cache_hit'),
        (RELATER, related, '\t\tif related != RelationComparisonResultNone {\n', '\t\t\t', 'relation_cache_hit'),
        (FLOW, flow_type, '\tfor {\n', '\t\t', 'flow_node_visit'),
        (FLOW, flow_type, '\t\t\t\tif c.sharedFlows[i].flow == flow {\n', '\t\t\t\t\t', 'flow_shared_hit'),
        (FLOW, reachable, '\tfor {\n', '\t\t', 'reachable_node_visit'),
        (FLOW, reachable, '\t\tif flow == c.lastFlowNode {\n', '\t\t\t', 'reachable_cache_hit'),
        (FLOW, reachable, '\t\t\t\tif reachable, ok := c.flowNodeReachable[flow]; ok {\n', '\t\t\t\t\t', 'reachable_cache_hit'),
    )
    for path, header, anchor, indent, counter in branches:
        within(path, header, anchor, anchor + indent + _hit(counter) + '\n')

    # Count actual reuse, excluding cached failures repeated to elaborate errors.
    anchor = '\t\t\tr.c.reliabilityFlags |= entry & (RelationComparisonResultReportsUnmeasurable | RelationComparisonResultReportsUnreliable)\n'
    within(RELATER, recursive, anchor, '\t\t\t' + _hit('relation_recursive_cache_hit') + '\n' + anchor)
    anchor = '\tinstantiation := c.cachedSignatures[key]\n'
    within(CHECKER, signature_cache, anchor, anchor + '\tif instantiation != nil {\n\t\t' + _hit('signature_instantiation_cache_hit') + '\n\t}\n')

    sources[MAIN] = _once(sources[MAIN], '\t"github.com/microsoft/TypeScript/tsc/internal/core"\n',
                          '\t"github.com/microsoft/TypeScript/tsc/internal/checker"\n\t"github.com/microsoft/TypeScript/tsc/internal/core"\n', MAIN)
    sources[MAIN] = _once(sources[MAIN], 'func main() {\n\tos.Exit(runMain())\n}',
                          'func main() {\n\tstatus := runMain()\n\tchecker.DumpWorkCounts()\n\tos.Exit(status)\n}', MAIN)
    sources[HELPER] = _helper(relation_origins)
    return sources
