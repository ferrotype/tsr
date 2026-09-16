package checker

// S08 P7 census (data/s08/type-footprint.json) over a complete checker.
//
// Unlike the P1 constructor-trace census in bridge.go, this walks every
// semantic root the checker retains (named types, the retained query results,
// every link store, every interning and query cache, the relation caches and
// the active inference/flow state) and every type payload kind, charges each
// allocation once by physical identity with its actual capacity, separates
// bound inputs (source file text, binder symbols and tables) from checker
// allocations, and names everything it cannot measure as unavailable.
//
// Map bytes are the runtime's actual allocation for the map's current layout
// (header, directory, tables and groups), read from the pinned Go 1.27.1
// swiss-map structures; a self-check against reflect's own type descriptor
// marks the family unavailable if the layout assumptions ever fail.

import (
	"fmt"
	"os"
	"reflect"
	"slices"
	"sort"
	"strings"
	"unsafe"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

// Type families whose bytes sum to the footprint numerator (the Rust list).
var s08V2TypeFamilies = map[string]bool{
	"type_records": true, "intrinsic": true, "literal": true, "unique_es_symbol": true, "anonymous": true,
	"evolving_arrays": true, "reference": true, "interface": true, "tuple": true, "union": true, "intersection": true,
	"type_parameter": true, "template_literal": true, "mapped": true, "reverse_mapped": true,
	"instantiation_expression": true, "index": true, "indexed_access": true, "string_mapping": true,
	"substitution": true, "conditional": true, "alias": true, "type_lists": true, "type_caches": true,
}

// Every family this census reports, so an absent family is a mismatch, not a zero.
var s08V2Families = []string{
	"type_records", "intrinsic", "literal", "unique_es_symbol", "anonymous", "evolving_arrays", "reference",
	"interface", "tuple", "union", "intersection", "type_parameter", "template_literal", "mapped",
	"reverse_mapped", "instantiation_expression", "index", "indexed_access", "string_mapping", "substitution",
	"conditional", "alias", "type_lists", "type_caches",
	"symbols", "symbol_tables", "signatures", "index_infos", "type_predicates", "value_symbol_links",
	"synthetic_expression_links", "checker_ast", "display_cache", "display_ast", "display_emit", "mappers",
	"inference", "relations", "query_links", "conditional_roots", "variance", "late_members",
	"mapped_symbol_links", "signature_caches", "declarations", "program_indices", "resolution",
	"diagnostics", "flow_analysis", "enum_links", "enum_relations", "body_check_state", "call_resolution",
	"deferred_checks", "iteration_cache", "module_aliases",
}

// Family of every Checker field. Fields absent here are reported unavailable
// (`unclassified_field:<name>`), never silently skipped. "" means the field
// holds no checker allocation (options, scalars, functions, bound inputs).
var s08V2FieldFamilies = map[string]string{
	"id": "", "program": "", "compilerOptions": "", "files": "", "fileIndexMap": "program_indices",
	"compareSymbols": "", "compareSymbolChains": "", "TypeCount": "", "SymbolCount": "", "SignatureCount": "",
	"TotalInstantiationCount": "", "instantiationCount": "", "instantiationDepth": "", "conditionalConstraintDepth": "",
	"inlineLevel": "", "serializationLevel": "", "currentNode": "", "varianceTypeParameter": "",
	"languageVersion": "", "moduleKind": "", "moduleResolutionKind": "", "isInferencePartiallyBlocked": "",
	"legacyDecorators": "", "emitStandardClassFields": "", "strictNullChecks": "", "strictFunctionTypes": "",
	"strictBindCallApply": "", "strictPropertyInitialization": "", "strictBuiltinIteratorReturn": "",
	"noImplicitAny": "", "noImplicitThis": "", "useUnknownInCatchVariables": "", "exactOptionalPropertyTypes": "",
	"canCollectSymbolAliasAccessibilityData": "", "wasCanceled": "", "arrayVariances": "variance",
	"globals": "symbol_tables", "evaluate": "",
	"stringLiteralTypes": "type_caches", "numberLiteralTypes": "type_caches", "nanType": "",
	"bigintLiteralTypes": "type_caches", "enumLiteralTypes": "enum_links", "enumNaNLiteralTypes": "enum_links",
	"indexedAccessTypes": "type_caches", "templateLiteralTypes": "type_caches", "stringMappingTypes": "type_caches",
	"uniqueESSymbolTypes": "type_caches", "thisExpandoKinds": "query_links", "thisExpandoLocations": "query_links",
	"subtypeReductionCache": "type_caches", "cachedTypes": "type_caches", "cachedSignatures": "signature_caches",
	"undefinedProperties": "symbols", "narrowedTypes": "flow_analysis", "assignmentReducedTypes": "flow_analysis",
	"discriminatedContextualTypes": "query_links", "instantiationExpressionTypes": "type_caches",
	"substitutionTypes": "type_caches", "reverseMappedCache": "type_caches", "reverseHomomorphicMappedCache": "type_caches",
	"iterationTypesCache": "iteration_cache", "markerTypes": "variance", "resolvingExplicitTypeOfSymbol": "resolution",
	"undefinedSymbol": "symbols", "argumentsSymbol": "symbols", "requireSymbol": "symbols", "unknownSymbol": "symbols",
	"unresolvedSymbols": "symbols", "errorTypes": "type_caches", "moduleSymbols": "module_aliases",
	"globalThisSymbol": "symbols", "symbolTableAliasCache": "query_links", "classExpressionNameTables": "symbol_tables",
	"resolveName": "", "resolveNameForSymbolSuggestion": "", "tupleTypes": "type_caches", "unionTypes": "type_caches",
	"unionOfUnionTypes": "type_caches", "intersectionTypes": "type_caches", "propertiesTypes": "type_caches",
	"diagnostics": "diagnostics", "suggestionDiagnostics": "diagnostics",
	"symbolArena": "symbols", "signatureArena": "signatures", "indexInfoArena": "index_infos",
	"mergedSymbols": "symbols", "factory": "checker_ast",
	"nodeLinks": "query_links", "signatureLinks": "query_links", "symbolNodeLinks": "query_links",
	"typeNodeLinks": "query_links", "enumMemberLinks": "enum_links", "assertionLinks": "query_links",
	"arrayLiteralLinks": "query_links", "switchStatementLinks": "flow_analysis", "jsxElementLinks": "query_links",
	"computedNameLinks": "late_members", "symbolReferenceLinks": "query_links", "valueSymbolLinks": "value_symbol_links",
	"mappedSymbolLinks": "mapped_symbol_links", "deferredSymbolLinks": "query_links", "aliasSymbolLinks": "module_aliases",
	"moduleSymbolLinks": "module_aliases", "lateBoundLinks": "late_members", "exportTypeLinks": "module_aliases",
	"membersAndExportsLinks": "symbol_tables", "typeAliasLinks": "query_links", "declaredTypeLinks": "query_links",
	"spreadLinks": "query_links", "varianceLinks": "variance", "ReverseMappedSymbolLinks": "mapped_symbol_links",
	"markedAssignmentSymbolLinks": "flow_analysis", "symbolContainerLinks": "query_links", "sourceFileLinks": "query_links",
	"regExpScanner": "query_links", "patternForType": "query_links", "contextFreeTypes": "query_links",
	"uniqueLiteralMapper": "mappers", "reliabilityFlags": "", "reportUnreliableMapper": "mappers",
	"reportUnmeasurableMapper": "mappers", "restrictiveMapper": "mappers", "permissiveMapper": "mappers",
	"noTypePredicate": "type_predicates", "anySignature": "", "unknownSignature": "", "resolvingSignature": "",
	"silentNeverSignature": "", "cachedArgumentsReferenced": "query_links", "enumNumberIndexInfo": "",
	"anyBaseTypeIndexInfo": "", "patternAmbientModules": "module_aliases",
	"patternAmbientModuleAugmentations": "symbol_tables", "patternAmbientModuleAugmentationTargets": "symbol_tables",
	"deferredGlobalImportMetaExpressionType": "", "contextualBindingPatterns": "query_links",
	"resolutionStart": "", "varianceStack": "variance", "apparentArgumentCount": "call_resolution",
	"lastGetCombinedNodeFlagsNode": "", "lastGetCombinedNodeFlagsResult": "", "lastGetCombinedModifierFlagsNode": "",
	"lastGetCombinedModifierFlagsResult": "", "freeinferenceState": "inference", "freeFlowState": "flow_analysis",
	"flowLoopCache": "flow_analysis", "flowLoopStack": "flow_analysis", "sharedFlows": "flow_analysis",
	"antecedentTypes": "flow_analysis", "flowAnalysisDisabled": "", "flowInvocationCount": "",
	"flowTypeCache": "flow_analysis", "lastFlowNode": "", "lastFlowNodeReachable": "",
	"flowNodeReachable": "flow_analysis", "flowNodePostSuper": "flow_analysis",
	"renamedBindingElementsInTypes": "query_links", "contextualInfos": "call_resolution",
	"inferenceContextInfos": "inference", "awaitedTypeStack": "type_caches", "reverseMappedSourceStack": "mapped_symbol_links",
	"reverseMappedTargetStack": "mapped_symbol_links", "reverseExpandingFlags": "", "freeRelater": "relations",
	"subtypeRelation": "relations", "strictSubtypeRelation": "relations", "assignableRelation": "relations",
	"comparableRelation": "relations", "identityRelation": "relations", "enumRelation": "enum_relations",
	"syncIterationTypesResolver": "iteration_cache", "asyncIterationTypesResolver": "iteration_cache", "isPrimitiveOrObjectOrEmptyType": "",
	"containsMissingType": "", "couldContainTypeVariables": "", "isStringIndexSignatureOnlyType": "",
	"markNodeAssignments": "", "compareTypesAssignable": "", "emitResolver": "declarations", "emitResolverOnce": "",
	"_jsxNamespace": "query_links", "_jsxFactoryEntity": "", "skipDirectInferenceNodes": "query_links", "ctx": "",
	"packagesMap": "module_aliases", "activeMappers": "mappers", "activeTypeMappersCaches": "type_caches",
	"ambientModulesOnce": "", "ambientModules": "module_aliases", "withinUnreachableCode": "",
	"reportedUnreachableNodes": "body_check_state", "nonExistentProperties": "deferred_checks",
	"deferredDiagnosticCallbacks": "deferred_checks", "mu": "", "tracer": "",
	"typeResolutions": "resolution", "typeToStringNodebuilder": "display_cache",
	"moduleImportAttributesTypes": "module_aliases",
}

// ---------------------------------------------------------------------------
// Runtime layouts of the pinned toolchain (go1.27.1, swiss maps).

type s08RtType struct {
	Size_       uintptr
	PtrBytes    uintptr
	Hash        uint32
	TFlag       uint8
	Align_      uint8
	FieldAlign_ uint8
	Kind_       uint8
	Equal       unsafe.Pointer
	GCData      unsafe.Pointer
	Str         int32
	PtrToThis   int32
}

type s08RtMapType struct {
	s08RtType
	Key        *s08RtType
	Elem       *s08RtType
	Group      *s08RtType
	Hasher     unsafe.Pointer
	GroupSize  uintptr
	KeysOff    uintptr
	KeyStride  uintptr
	ElemsOff   uintptr
	ElemStride uintptr
	ElemOff    uintptr
	Flags      uint32
}

type s08RtMap struct {
	used              uint64
	seed              uintptr
	dirPtr            unsafe.Pointer
	dirLen            int
	globalDepth       uint8
	globalShift       uint8
	writing           uint8
	tombstonePossible bool
	clearSeq          uint64
}

type s08RtTable struct {
	used             uint16
	capacity         uint16
	growthLeft       uint16
	localDepth       uint8
	index            int
	groupsData       unsafe.Pointer
	groupsLengthMask uint64
}

func s08TypePointer(t reflect.Type) *s08RtType {
	// reflect.Type is an interface whose data word is the *abi.Type.
	return (*s08RtType)((*[2]unsafe.Pointer)(unsafe.Pointer(&t))[1])
}

// s08MapBytes is what the runtime holds for this map's current layout: the
// header, the table directory, every distinct table and its group array. A
// small map (up to eight slots) is one group. Deleted slots keep their groups.
// ok is false when the runtime layout does not match the pinned assumptions.
func s08MapBytes(v reflect.Value) (bytes int64, ok bool) {
	if v.Kind() != reflect.Map {
		return 0, false
	}
	if v.IsNil() {
		return 0, true
	}
	mt := (*s08RtMapType)(unsafe.Pointer(s08TypePointer(v.Type())))
	if mt.Key != s08TypePointer(v.Type().Key()) || mt.Elem != s08TypePointer(v.Type().Elem()) ||
		mt.GroupSize < 8+8*(uintptr(v.Type().Key().Size())+uintptr(v.Type().Elem().Size())) ||
		mt.GroupSize > 8+8*(uintptr(v.Type().Key().Size())+uintptr(v.Type().Elem().Size()))+128 {
		return 0, false
	}
	m := (*s08RtMap)(v.UnsafePointer())
	if uint64(v.Len()) != m.used {
		return 0, false
	}
	bytes = int64(unsafe.Sizeof(s08RtMap{}))
	if m.dirPtr == nil {
		return bytes, true
	}
	if m.dirLen == 0 {
		return bytes + int64(mt.GroupSize), true
	}
	bytes += int64(m.dirLen) * int64(unsafe.Sizeof(uintptr(0)))
	directory := unsafe.Slice((**s08RtTable)(m.dirPtr), m.dirLen)
	seen := map[*s08RtTable]bool{}
	for _, table := range directory {
		if table == nil || seen[table] {
			continue
		}
		seen[table] = true
		bytes += int64(unsafe.Sizeof(s08RtTable{})) + int64(table.groupsLengthMask+1)*int64(mt.GroupSize)
	}
	return bytes, true
}

// s08ArenaChunkCaps replays core.Arena's chunk growth for records of the
// element type: nextArenaSize doubles up to 256 and slices.Grow rounds each
// chunk up to the runtime's size class, which depends on the element type
// (pointer-carrying objects above 512 bytes carry a malloc header), so the
// replay appends through reflect with the real element type.
func s08ArenaChunkCaps(elem reflect.Type, chunks int) []int {
	caps := make([]int, 0, chunks)
	previous := 0
	sliceType := reflect.SliceOf(elem)
	for len(caps) < chunks {
		next := max(previous, 1)
		next = min(next*2, 256)
		grown := reflect.AppendSlice(reflect.MakeSlice(sliceType, 0, 0), reflect.MakeSlice(sliceType, next, next))
		capacity := grown.Cap()
		caps = append(caps, capacity)
		previous = capacity
	}
	return caps
}

// s08ArenaCapacityByCount is the total slot capacity across every chunk an
// arena allocated for count records.
func s08ArenaCapacityByCount(elem reflect.Type, count int) int {
	if count == 0 {
		return 0
	}
	total, filled := 0, 0
	caps := s08ArenaChunkCaps(elem, 64)
	for _, capacity := range caps {
		total += capacity
		filled += capacity
		if filled >= count {
			return total
		}
	}
	// Beyond the replayed prefix every chunk has the plateau capacity.
	plateau := caps[63]
	for filled < count {
		total += plateau
		filled += plateau
	}
	return total
}

// s08ArenaFromCurrentChunk infers records and capacity from the arena's
// current chunk alone: exact while the chunk capacities still grow, ambiguous
// once they plateau (ok=false), because earlier plateau chunks leave no trace.
func s08ArenaFromCurrentChunk(elem reflect.Type, current reflect.Value) (records int, capacity int, ok bool) {
	if current.Cap() == 0 {
		return 0, 0, true
	}
	caps := s08ArenaChunkCaps(elem, 64)
	plateau := caps[63]
	if current.Cap() == plateau {
		// The first plateau chunk is still unambiguous only if no earlier
		// plateau chunk could exist, which the current chunk cannot tell.
		return 0, 0, false
	}
	total := 0
	for _, c := range caps {
		if c == current.Cap() {
			return total + current.Len(), total + c, true
		}
		total += c
	}
	return 0, 0, false
}

// ---------------------------------------------------------------------------
// The census.

type s08V2 struct {
	path        []string
	allocations *s08Allocations
	charges     map[uintptr]s08AllocationCharge
	c           *Checker
	families    map[string]*s08Family
	seen        map[uintptr]bool
	unavailable map[string]bool

	fileTexts        [][2]uintptr
	fileTextLengths  map[uintptr]int
	boundReferences  int
	boundReferenced  map[uintptr]bool
	binderTables     map[uintptr]bool
	boundTableRefs   int
	boundSymbolRefs  int
	transientSymbols map[*ast.Symbol]bool

	types       map[*Type]bool
	pending     []*Type
	signatures  map[*Signature]bool
	visited     map[uintptr]bool
	sliceVisits map[s08SliceView]int
	symbolSeen  map[*ast.Symbol]bool
	mapFamilies map[uintptr]string
	tables      map[uintptr]s08TableCharge
}

// Allocation charging and edge traversal are independent: a short view may
// be encountered before a longer view of the very same backing allocation.
type s08SliceView struct {
	address uintptr
	typeOf  reflect.Type
}

// s08TableCharge is where one checker-created SymbolTable's bytes were charged.
type s08TableCharge struct {
	family string
	bytes  int64
}

var (
	s08TypeType             = reflect.TypeOf((*Type)(nil))
	s08SignatureType        = reflect.TypeOf((*Signature)(nil))
	s08IndexInfoType        = reflect.TypeOf((*IndexInfo)(nil))
	s08TypeMapperType       = reflect.TypeOf((*TypeMapper)(nil))
	s08TypeAliasType        = reflect.TypeOf((*TypeAlias)(nil))
	s08TypePredicateType    = reflect.TypeOf((*TypePredicate)(nil))
	s08InferenceContextType = reflect.TypeOf((*InferenceContext)(nil))
	s08InferenceInfoType    = reflect.TypeOf((*InferenceInfo)(nil))
	s08InferenceStateType   = reflect.TypeOf((*InferenceState)(nil))
	s08FlowStateType        = reflect.TypeOf((*FlowState)(nil))
	s08ConditionalRootType  = reflect.TypeOf((*ConditionalRoot)(nil))
	s08RelationType         = reflect.TypeOf((*Relation)(nil))
	s08RelaterType          = reflect.TypeOf((*Relater)(nil))
	s08DiagnosticType       = reflect.TypeOf((*ast.Diagnostic)(nil))
	s08SymbolType           = reflect.TypeOf((*ast.Symbol)(nil))
	s08SymbolTableType      = reflect.TypeOf(ast.SymbolTable(nil))
	s08NodeType             = reflect.TypeOf((*ast.Node)(nil))
	s08SourceFileType       = reflect.TypeOf((*ast.SourceFile)(nil))
	s08CheckerType          = reflect.TypeOf((*Checker)(nil))
	s08CacheHashKeyType     = reflect.TypeOf(CacheHashKey{})
)

func (v2 *s08V2) add(family string, count, bytes int64) {
	entry := v2.families[family]
	if entry == nil {
		entry = &s08Family{}
		v2.families[family] = entry
	}
	entry.Count += count
	entry.Bytes += bytes
}

func (v2 *s08V2) markUnavailable(name string) {
	v2.unavailable[name] = true
}

// once reports whether the allocation at address is charged for the first time.
func (v2 *s08V2) once(address uintptr) bool {
	if address == 0 || v2.seen[address] {
		return false
	}
	v2.seen[address] = true
	return true
}

func (v2 *s08V2) isBoundText(s string) bool {
	if len(s) == 0 {
		return false
	}
	address := uintptr(unsafe.Pointer(unsafe.StringData(s)))
	for _, span := range v2.fileTexts {
		if address >= span[0] && address < span[1] {
			v2.boundReferences++
			v2.boundReferenced[span[0]] = true
			return true
		}
	}
	return false
}

func (v2 *s08V2) text(family string, s string) {
	if len(s) == 0 || v2.isBoundText(s) {
		return
	}
	if v2.allocations != nil {
		v2.allocation(family, uintptr(unsafe.Pointer(unsafe.StringData(s))))
		return
	}
	if v2.once(uintptr(unsafe.Pointer(unsafe.StringData(s)))) {
		v2.add(family, 0, int64(len(s)))
	}
}

func (v2 *s08V2) sliceBytes(family string, v reflect.Value) {
	if v2.allocations != nil {
		v2.allocation(family, uintptr(v.UnsafePointer()))
		return
	}
	if v.Cap() == 0 {
		return
	}
	if v2.once(uintptr(v.UnsafePointer())) {
		v2.add(family, 0, int64(v.Cap())*int64(v.Type().Elem().Size()))
	}
}

func (v2 *s08V2) mapBytes(family string, v reflect.Value, count bool) {
	if v.IsNil() {
		return
	}
	if !v2.once(uintptr(v.UnsafePointer())) {
		if v2.allocations != nil {
			v2.mapAllocations(family, v)
		}
		return
	}
	bytes, ok := s08MapBytes(v)
	if !ok {
		v2.markUnavailable("map_layout:" + v.Type().String())
		return
	}
	n := int64(0)
	if count {
		n = int64(v.Len())
	}
	if v2.allocations != nil {
		v2.mapAllocations(family, v)
		bytes = 0
	}
	v2.add(family, n, bytes)
}

// Runtime layout selects the live pieces; provenance supplies their requested
// extents and shared-allocation identity (including table/group coallocation).
func (v2 *s08V2) mapAllocations(family string, v reflect.Value) {
	m := (*s08RtMap)(v.UnsafePointer())
	v2.allocation(family, uintptr(v.UnsafePointer()))
	if m.dirPtr == nil {
		return
	}
	v2.allocation(family, uintptr(m.dirPtr))
	if m.dirLen == 0 {
		return
	}
	for _, table := range unsafe.Slice((**s08RtTable)(m.dirPtr), m.dirLen) {
		if table != nil {
			v2.allocation(family, uintptr(unsafe.Pointer(table)))
			v2.allocation(family, uintptr(table.groupsData))
		}
	}
}

// symbolTable charges a checker-owned table or records a bound-input reference.
func (v2 *s08V2) symbolTable(family string, v reflect.Value) {
	if v.IsNil() {
		return
	}
	address := uintptr(v.UnsafePointer())
	if v2.binderTables[address] {
		v2.boundTableRefs++
		return
	}
	if charge, ok := v2.tables[address]; ok {
		if v2.allocations != nil {
			v2.mapAllocations(family, v)
		}
		// Type-owned member backing has precedence over the other checker
		// buckets: a table first reached through a symbol or a link moves to the
		// type family that owns it. Its entries were walked on the first visit.
		if s08V2TypeFamilies[family] && !s08V2TypeFamilies[charge.family] {
			v2.add(charge.family, -1, -charge.bytes)
			v2.add(family, 0, charge.bytes)
			v2.tables[address] = s08TableCharge{family: family, bytes: charge.bytes}
			iter := v.MapRange()
			for iter.Next() {
				v2.text(family, iter.Key().String())
			}
		}
		return
	}
	bytes, ok := s08MapBytes(v)
	if !ok {
		v2.markUnavailable("map_layout:SymbolTable")
		return
	}
	if v2.allocations != nil {
		v2.mapAllocations(family, v)
		bytes = 0
	}
	v2.tables[address] = s08TableCharge{family: family, bytes: bytes}
	// A member table is its type record's storage, not another record.
	count := int64(1)
	if s08V2TypeFamilies[family] {
		count = 0
	}
	v2.add(family, count, bytes)
	iter := v.MapRange()
	for iter.Next() {
		v2.text(family, iter.Key().String())
		v2.symbol(iter.Value())
	}
}

// symbol records a transient (checker-created) symbol's owned storage once;
// binder symbols are bound inputs and never charged.
func (v2 *s08V2) symbol(v reflect.Value) {
	if v.IsNil() {
		return
	}
	symbol := (*ast.Symbol)(v.UnsafePointer())
	if v2.symbolSeen[symbol] {
		return
	}
	v2.symbolSeen[symbol] = true
	if symbol.Flags&ast.SymbolFlagsTransient == 0 {
		v2.boundSymbolRefs++
		return
	}
	v2.transientSymbols[symbol] = true
	if v2.allocations != nil {
		v2.allocation("symbols", uintptr(v.UnsafePointer()))
	}
	v2.text("symbols", symbol.Name)
	v2.sliceBytes("symbols", reflect.ValueOf(symbol.Declarations))
	if symbol.Members != nil {
		v2.symbolTable("symbol_tables", reflect.ValueOf(symbol.Members))
	}
	if symbol.Exports != nil {
		v2.symbolTable("symbol_tables", reflect.ValueOf(symbol.Exports))
	}
}

// walk charges the owned storage reachable from v under `family` and collects
// semantic roots (types, signatures, mappers, inference state). Pointers to
// bound inputs (nodes, source files, the program) stop the walk.
func (v2 *s08V2) walk(v reflect.Value, family string, depth int) {
	if !v.IsValid() {
		return
	}
	if depth > 64 {
		v2.markUnavailable("walk_depth:" + v.Type().String())
		return
	}
	t := v.Type()
	switch v.Kind() {
	case reflect.Pointer:
		if v.IsNil() {
			return
		}
		switch t {
		case s08TypeType:
			ty := (*Type)(v.UnsafePointer())
			if !v2.types[ty] {
				v2.types[ty] = true
				v2.pending = append(v2.pending, ty)
			}
			return
		case s08NodeType, s08SourceFileType:
			allocation, ok := v2.allocations.find(uintptr(v.UnsafePointer()))
			if v2.allocations != nil && !ok {
				v2.markUnavailable("allocation_extent:checker_ast")
			}
			if ok && allocation.size != 0 {
				address := uintptr(v.UnsafePointer())
				if v2.visited[address] {
					return
				}
				v2.visited[address] = true
				v2.allocation("checker_ast", address)
				payload := v.Elem()
				if t == s08NodeType {
					payload = payload.FieldByName("data").Elem().Elem()
				}
				v2.walkFields(payload, "checker_ast", depth+1)
			}
			return
		case s08CheckerType:
			return
		case s08SymbolType:
			v2.symbol(v)
			return
		}
		address := uintptr(v.UnsafePointer())
		if allocation, ok := v2.allocations.find(address); ok && allocation.size == 0 {
			return
		}
		if v2.visited[address] {
			return
		}
		v2.visited[address] = true
		element := v.Elem()
		switch t {
		case s08SignatureType:
			// Provenance identifies the complete retained arena chunk.
			if v2.allocations != nil {
				v2.allocation("signatures", address)
			}
			v2.walkFields(element, "signatures", depth+1)
		case s08IndexInfoType:
			if v2.allocations != nil {
				v2.allocation("index_infos", address)
			}
			v2.walkFields(element, "index_infos", depth+1)
		case s08TypeMapperType:
			// TypeMapper is embedded at offset zero of its concrete payload.
			// Recursing through data normally would skip it as already visited.
			mapper := (*TypeMapper)(v.UnsafePointer())
			switch mapper.data.(type) {
			case *SimpleTypeMapper, *ArrayTypeMapper, *ArrayToSingleTypeMapper,
				*MergedTypeMapper, *CompositeTypeMapper, *InferenceTypeMapper:
			case *DeferredTypeMapper, *FunctionTypeMapper:
				if v2.allocations == nil {
					v2.markUnavailable("mapper_closure:" + reflect.TypeOf(mapper.data).String())
				}
			default:
				v2.markUnavailable(fmt.Sprintf("mapper_payload:%T", mapper.data))
				return
			}
			payload := reflect.ValueOf(mapper.data).Elem()
			v2.record("mappers", address, 1, int64(payload.Type().Size()))
			v2.walkFields(payload, "mappers", depth+1)
		case s08TypeAliasType:
			v2.record("alias", address, 1, int64(unsafe.Sizeof(TypeAlias{})))
			v2.walkFields(element, "alias", depth+1)
		case s08TypePredicateType:
			v2.record("type_predicates", address, 1, int64(unsafe.Sizeof(TypePredicate{})))
			v2.walkFields(element, "type_predicates", depth+1)
		case s08InferenceContextType, s08InferenceInfoType, s08InferenceStateType:
			v2.record("inference", address, 1, int64(element.Type().Size()))
			v2.walkFields(element, "inference", depth+1)
		case s08FlowStateType:
			v2.record("flow_analysis", address, 1, int64(element.Type().Size()))
			v2.walkFields(element, "flow_analysis", depth+1)
		case s08ConditionalRootType:
			v2.record("conditional_roots", address, 1, int64(unsafe.Sizeof(ConditionalRoot{})))
			v2.walkFields(element, "conditional_roots", depth+1)
		case s08RelationType, s08RelaterType:
			v2.record("relations", address, 1, int64(element.Type().Size()))
			v2.walkFields(element, "relations", depth+1)
		case s08DiagnosticType:
			v2.record("diagnostics", address, 1, int64(unsafe.Sizeof(ast.Diagnostic{})))
			v2.walkFields(element, "diagnostics", depth+1)
		default:
			switch element.Kind() {
			case reflect.Struct:
				v2.record(family, address, 0, int64(element.Type().Size()))
				v2.walkFields(element, family, depth+1)
			case reflect.Slice, reflect.Map, reflect.Array, reflect.String, reflect.Interface, reflect.Pointer:
				v2.record(family, address, 0, int64(element.Type().Size()))
				v2.walk(element, family, depth+1)
			default:
				v2.record(family, address, 0, int64(element.Type().Size()))
			}
		}
	case reflect.Interface:
		if v.IsNil() {
			return
		}
		inner := v.Elem()
		if v2.allocations != nil {
			// Only indirect interface payloads have a separate box.
			abi := (*s08RtType)(unsafe.Pointer(s08TypePointer(inner.Type())))
			if abi.Kind_&(1<<5) == 0 {
				v2.allocation(family, s08InterfaceBox(v))
			}
		}
		switch inner.Kind() {
		case reflect.Pointer:
			v2.walk(inner, family, depth+1)
		case reflect.String:
			// Interface-boxed strings: header plus text.
			v2.text(family, inner.String())
		case reflect.Struct:
			// Boxed struct value (an interface holding a non-pointer): its
			// allocation is the struct itself.
			if v2.allocations == nil {
				v2.add(family, 0, int64(inner.Type().Size()))
			}
			v2.walkFields(inner, family, depth+1)
		default:
			v2.walk(inner, family, depth+1)
		}
	case reflect.Func:
		v2.function(v, family, depth)
	case reflect.Map:
		if t == s08SymbolTableType {
			v2.symbolTable(family, v)
			return
		}
		if v.IsNil() {
			return
		}
		valueFamily := family
		if t.Key() == s08CacheHashKeyType && t.Elem() == s08TypeType {
			valueFamily = "type_caches"
		}
		address := uintptr(v.UnsafePointer())
		if v2.mapFamilies == nil {
			v2.mapFamilies = map[uintptr]string{}
		}
		oldFamily, visited := v2.mapFamilies[address]
		upgrade := s08V2TypeFamilies[valueFamily] && !s08V2TypeFamilies[oldFamily]
		if !visited || upgrade {
			v2.mapFamilies[address] = valueFamily
		}
		v2.mapBytes(valueFamily, v, depth == 0 || valueFamily != family)
		if !visited || upgrade {
			// Entries count as records only for the checker's own cache maps (its
			// fields and the type caches); a map nested in a record is that record's
			// storage, so it adds bytes without inflating the family's record count.
			iter := v.MapRange()
			for iter.Next() {
				v2.walk(iter.Key(), valueFamily, depth+1)
				v2.walk(iter.Value(), valueFamily, depth+1)
			}
		}
	case reflect.Slice:
		if v.IsNil() {
			return
		}
		elementFamily := family
		if t.Elem() == s08TypeType {
			elementFamily = "type_lists"
		}
		v2.sliceBytes(elementFamily, v)
		if v2.sliceVisits == nil {
			v2.sliceVisits = map[s08SliceView]int{}
		}
		view := s08SliceView{uintptr(v.UnsafePointer()), t}
		start := v2.sliceVisits[view]
		if v.Len() > start {
			v2.sliceVisits[view] = v.Len()
			for i := start; i < v.Len(); i++ {
				v2.walk(v.Index(i), family, depth+1)
			}
		}
	case reflect.Array:
		for i := range v.Len() {
			v2.walk(v.Index(i), family, depth+1)
		}
	case reflect.Struct:
		v2.walkFields(v, family, depth+1)
	case reflect.String:
		v2.text(family, v.String())
	default:
		// Scalars, funcs, channels: no owned allocation.
	}
}

func (v2 *s08V2) walkFields(v reflect.Value, family string, depth int) {
	t := v.Type()
	for i := range t.NumField() {
		field := t.Field(i)
		if field.Type.Kind() == reflect.Chan || field.Type.Kind() == reflect.UnsafePointer {
			continue
		}
		v2.path = append(v2.path, field.Name)
		v2.walk(v.Field(i), family, depth)
		v2.path = v2.path[:len(v2.path)-1]
	}
}

// linkStore charges a core.LinkStore[K, V]: the entry map, the record arena
// replayed for its entry count, and every record's owned storage.
func (v2 *s08V2) linkStore(family string, v reflect.Value) {
	entries := v.FieldByName("entries")
	arena := v.FieldByName("arena")
	if !entries.IsValid() || !arena.IsValid() {
		v2.markUnavailable("link_store_layout:" + v.Type().String())
		return
	}
	v2.mapBytes(family, entries, true)
	recordType := arena.Type().Field(0).Type.Elem()
	if v2.allocations == nil {
		v2.add(family, 0, int64(s08ArenaCapacityByCount(recordType, entries.Len()))*int64(recordType.Size()))
	}
	iter := entries.MapRange()
	for iter.Next() {
		record := iter.Value()
		if record.IsNil() {
			continue
		}
		if v2.allocations != nil {
			v2.allocation(family, uintptr(record.UnsafePointer()))
		}
		v2.visited[uintptr(record.UnsafePointer())] = true
		v2.walk(record.Elem(), family, 1)
	}
}

// pagedLinkStore charges a core.PagedLinkStore[V]: the page list, the page
// map and every allocated page, plus each slot's owned storage.
func (v2 *s08V2) pagedLinkStore(family string, v reflect.Value) (slots int) {
	pageList := v.FieldByName("pageList")
	pageMap := v.FieldByName("pageMap")
	if !pageList.IsValid() || !pageMap.IsValid() {
		v2.markUnavailable("paged_link_store_layout:" + v.Type().String())
		return 0
	}
	v2.sliceBytes(family, pageList)
	v2.mapBytes(family, pageMap, false)
	visitPage := func(page reflect.Value) {
		if page.IsNil() {
			return
		}
		if v2.once(uintptr(page.UnsafePointer())) {
			v2.record(family, uintptr(page.UnsafePointer()), 0, int64(page.Elem().Type().Size()))
		}
		array := page.Elem()
		for i := range array.Len() {
			slot := array.Index(i)
			if slot.Kind() == reflect.Pointer {
				if slot.IsNil() {
					continue
				}
				slots++
				if v2.allocations != nil {
					v2.allocation(family, uintptr(slot.UnsafePointer()))
				}
				v2.visited[uintptr(slot.UnsafePointer())] = true
				v2.walk(slot.Elem(), family, 1)
			} else {
				v2.walk(slot, family, 1)
			}
		}
	}
	for i := range pageList.Len() {
		visitPage(pageList.Index(i))
	}
	iter := pageMap.MapRange()
	for iter.Next() {
		visitPage(iter.Value())
	}
	return slots
}

// arena charges a core.Arena[T] by replaying its chunk growth for count
// records; a negative count infers the count from the current chunk.
func (v2 *s08V2) arena(family string, v reflect.Value, count int) {
	data := v.FieldByName("data")
	if !data.IsValid() {
		v2.markUnavailable("arena_layout:" + v.Type().String())
		return
	}
	elem := data.Type().Elem()
	elemSize := int(elem.Size())
	if v2.allocations != nil {
		v2.sliceBytes(family, data)
		if count >= 0 {
			v2.add(family, int64(count), 0)
		}
		if family == "checker_ast" {
			for i := 0; i < data.Len(); i++ {
				v2.walk(data.Index(i), family, 0)
			}
		}
		return
	}
	if count < 0 {
		records, capacity, ok := s08ArenaFromCurrentChunk(elem, data)
		if !ok {
			v2.markUnavailable("arena_saturated:" + family)
			return
		}
		v2.add(family, int64(records), int64(capacity*elemSize))
		return
	}
	v2.add(family, int64(count), int64(s08ArenaCapacityByCount(elem, count)*elemSize))
}

// collectBinderTables marks every symbol table reachable from the program's
// source files through binder symbols as bound input.
func (v2 *s08V2) collectBinderTables() {
	var visit func(table ast.SymbolTable)
	seen := map[*ast.Symbol]bool{}
	visit = func(table ast.SymbolTable) {
		if table == nil {
			return
		}
		address := reflect.ValueOf(table).Pointer()
		if v2.binderTables[address] {
			return
		}
		v2.binderTables[address] = true
		for _, symbol := range table {
			if symbol == nil || seen[symbol] || symbol.Flags&ast.SymbolFlagsTransient != 0 {
				continue
			}
			seen[symbol] = true
			visit(symbol.Members)
			visit(symbol.Exports)
		}
	}
	for _, file := range v2.c.files {
		visit(file.Locals)
		if file.Symbol != nil {
			visit(file.Symbol.Members)
			visit(file.Symbol.Exports)
		}
	}
}

// typePayload charges one reached type's payload and owned storage and
// enqueues the types it references.
func (v2 *s08V2) typePayload(t *Type) {
	var family string
	var size uintptr
	switch data := t.data.(type) {
	case *IntrinsicType:
		family, size = "intrinsic", unsafe.Sizeof(IntrinsicType{})
	case *LiteralType:
		family, size = "literal", unsafe.Sizeof(LiteralType{})
		_ = data
	case *UniqueESSymbolType:
		family, size = "unique_es_symbol", unsafe.Sizeof(UniqueESSymbolType{})
	case *ObjectType:
		family, size = "anonymous", unsafe.Sizeof(ObjectType{})
	case *TypeReference:
		family, size = "reference", unsafe.Sizeof(TypeReference{})
	case *InterfaceType:
		family, size = "interface", unsafe.Sizeof(InterfaceType{})
	case *TupleType:
		family, size = "tuple", unsafe.Sizeof(TupleType{})
	case *UnionType:
		family, size = "union", unsafe.Sizeof(UnionType{})
	case *IntersectionType:
		family, size = "intersection", unsafe.Sizeof(IntersectionType{})
	case *TypeParameter:
		family, size = "type_parameter", unsafe.Sizeof(TypeParameter{})
	case *TemplateLiteralType:
		family, size = "template_literal", unsafe.Sizeof(TemplateLiteralType{})
	case *MappedType:
		family, size = "mapped", unsafe.Sizeof(MappedType{})
	case *ReverseMappedType:
		family, size = "reverse_mapped", unsafe.Sizeof(ReverseMappedType{})
	case *EvolvingArrayType:
		family, size = "evolving_arrays", unsafe.Sizeof(EvolvingArrayType{})
	case *InstantiationExpressionType:
		family, size = "instantiation_expression", unsafe.Sizeof(InstantiationExpressionType{})
	case *IndexType:
		family, size = "index", unsafe.Sizeof(IndexType{})
	case *IndexedAccessType:
		family, size = "indexed_access", unsafe.Sizeof(IndexedAccessType{})
	case *StringMappingType:
		family, size = "string_mapping", unsafe.Sizeof(StringMappingType{})
	case *SubstitutionType:
		family, size = "substitution", unsafe.Sizeof(SubstitutionType{})
	case *ConditionalType:
		family, size = "conditional", unsafe.Sizeof(ConditionalType{})
	default:
		v2.markUnavailable(fmt.Sprintf("type_payload:%T", t.data))
		return
	}
	v2.record(family, uintptr(reflect.ValueOf(t.data).UnsafePointer()), 1, int64(size))
	v2.add("type_records", 1, 0)
	payload := reflect.ValueOf(t.data).Elem()
	v2.visited[uintptr(reflect.ValueOf(t.data).UnsafePointer())] = true
	v2.walkFields(payload, family, 1)
}

// inventory lists the reached types as kind:symbol for cross-runtime diagnosis
// when S08_CENSUS_INVENTORY is set; nil otherwise.
func (v2 *s08V2) inventory() []string {
	if os.Getenv("S08_CENSUS_INVENTORY") == "" {
		return nil
	}
	names := make([]string, 0, len(v2.types))
	for t := range v2.types {
		label := s08TypeLabel(t)
		switch t.data.(type) {
		case *TypeReference, *ObjectType:
			label += fmt.Sprintf("{flags=%#x", uint32(t.objectFlags))
			if v2.c.patternForType[t] != nil {
				label += ",pattern"
			}
			label += "}"
		}
		if ref, ok := t.data.(*TypeReference); ok && ref.target != nil {
			args := make([]string, 0, len(ref.resolvedTypeArguments))
			for _, arg := range ref.resolvedTypeArguments {
				args = append(args, s08TypeLabel(arg))
			}
			label += "->" + s08TypeLabel(ref.target) + "[" + strings.Join(args, ",") + "]"
		}
		names = append(names, label)
	}
	sort.Strings(names)
	return names
}

// s08TypeLabel is kind:name, with a literal type's value in place of a name.
func s08TypeLabel(t *Type) string {
	name := ""
	if t.symbol != nil {
		name = t.symbol.Name
	}
	if literal, ok := t.data.(*LiteralType); ok {
		name = fmt.Sprintf("%v", literal.value)
	}
	if intrinsic, ok := t.data.(*IntrinsicType); ok {
		name = intrinsic.intrinsicName
	}
	return fmt.Sprintf("%T:%s", t.data, name)
}

// S08Census is the P7 structural census of one complete checker with `roots`
// as the retained query results.
func S08Census(c *Checker, roots []*Type) map[string]any {
	v2 := &s08V2{
		allocations:      s08AllocationFinish(),
		c:                c,
		families:         map[string]*s08Family{},
		seen:             map[uintptr]bool{},
		unavailable:      map[string]bool{},
		fileTextLengths:  map[uintptr]int{},
		boundReferenced:  map[uintptr]bool{},
		binderTables:     map[uintptr]bool{},
		transientSymbols: map[*ast.Symbol]bool{},
		types:            map[*Type]bool{},
		signatures:       map[*Signature]bool{},
		visited:          map[uintptr]bool{},
		symbolSeen:       map[*ast.Symbol]bool{},
		tables:           map[uintptr]s08TableCharge{},
	}
	// The certified census needs the runtime observer for closure environments
	// and allocation extents; without it, or once its log overflowed, the
	// variant is unavailable rather than partially charged. The observer's log
	// use is reported so the capture shows the headroom.
	observer := map[string]any{"present": v2.allocations != nil, "overflow": v2.allocations != nil && v2.allocations.overflow}
	if v2.allocations != nil {
		observer["recorded"] = v2.allocations.recorded
		observer["snapshot"] = v2.allocations.snapshot
		observer["capacity"] = v2.allocations.capacity
	}
	if v2.allocations == nil || v2.allocations.overflow {
		v2.markUnavailable("allocation_observer")
	}
	for _, name := range s08V2Families {
		v2.add(name, 0, 0)
	}
	v2.record("query_links", uintptr(unsafe.Pointer(c)), 0, int64(unsafe.Sizeof(Checker{})))
	for _, file := range c.files {
		text := file.Text()
		if len(text) == 0 {
			continue
		}
		start := uintptr(unsafe.Pointer(unsafe.StringData(text)))
		v2.fileTexts = append(v2.fileTexts, [2]uintptr{start, start + uintptr(len(text))})
		v2.fileTextLengths[start] = len(text)
	}
	v2.collectBinderTables()

	// Roots: retained results, then every Checker field by classification.
	for _, root := range roots {
		v2.walk(reflect.ValueOf(root), "", 0)
	}
	checker := reflect.ValueOf(c).Elem()
	checkerType := checker.Type()
	fieldBytes := map[string]int64{}
	totalSoFar := func() int64 {
		var total int64
		for _, family := range v2.families {
			total += family.Bytes
		}
		return total
	}
	for i := range checkerType.NumField() {
		field := checkerType.Field(i)
		before := totalSoFar()
		family, known := s08V2FieldFamilies[field.Name]
		if !known {
			// Named types and memoized accessors are roots. The diagnostic
			// runtime supplies typed allocation metadata for closure captures.
			switch {
			case field.Type == s08TypeType:
				family, known = "", true
			case field.Type.Kind() == reflect.Func:
				family, known = "query_links", true
			default:
				v2.markUnavailable("unclassified_field:" + field.Name)
				continue
			}
		}
		v2.path = []string{field.Name}
		value := checker.Field(i)
		typeName := field.Type.String()
		switch {
		case field.Name == "factory":
			v2.nodeFactory(value)
		case field.Name == "symbolArena":
			v2.arena("symbols", value, int(c.SymbolCount))
		case field.Name == "signatureArena":
			v2.arena("signatures", value, int(c.SignatureCount))
		case field.Name == "indexInfoArena":
			v2.arena("index_infos", value, -1)
		case strings.HasPrefix(typeName, "core.LinkStore["):
			v2.linkStore(family, value)
		case strings.HasPrefix(typeName, "checker.nodeLinkStore["):
			v2.pagedLinkStore(family, value.FieldByName("store"))
		case strings.HasPrefix(typeName, "checker.symbolArenaLinkStore["):
			slots := v2.pagedLinkStore(family, value.FieldByName("store"))
			v2.arena(family, value.FieldByName("arena"), slots)
		case family == "":
			// Options, scalars, functions, bound inputs: nothing owned here.
			// Named types are still roots.
			if field.Type.Kind() == reflect.Func {
				v2.walk(value, "query_links", 0)
			}
			if field.Type == s08TypeType || field.Type == s08SignatureType || field.Type == s08IndexInfoType {
				v2.walk(value, "", 0)
			}
		default:
			v2.walk(value, family, 0)
		}
		if delta := totalSoFar() - before; delta != 0 {
			fieldBytes[field.Name] = delta
		}
	}
	// Types: payloads and the edges they add, until the closure is complete.
	for len(v2.pending) != 0 {
		t := v2.pending[len(v2.pending)-1]
		v2.pending = v2.pending[:len(v2.pending)-1]
		v2.typePayload(t)
	}

	var typeStorage, total int64
	for name, family := range v2.families {
		total += family.Bytes
		if s08V2TypeFamilies[name] {
			typeStorage += family.Bytes
		}
	}
	unavailable := make([]string, 0, len(v2.unavailable))
	for name := range v2.unavailable {
		unavailable = append(unavailable, name)
	}
	slices.Sort(unavailable)
	var boundBytes, referencedBytes int64
	for start, length := range v2.fileTextLengths {
		boundBytes += int64(length)
		if v2.boundReferenced[start] {
			referencedBytes += int64(length)
		}
	}
	reachable := len(v2.types)
	return map[string]any{
		"families": v2.families, "type_storage_bytes": typeStorage, "checker_bytes": total,
		"types": map[string]any{"created": c.TypeCount, "reachable": reachable,
			"unreachable_occupied": max(int64(c.TypeCount)-int64(reachable), 0)},
		"unavailable": unavailable,
		"bound_inputs": map[string]any{"backings": len(v2.fileTextLengths), "bytes": boundBytes,
			"referenced_backings": len(v2.boundReferenced), "referenced_bytes": referencedBytes,
			"references": v2.boundReferences, "binder_table_references": v2.boundTableRefs,
			"binder_symbol_references": v2.boundSymbolRefs},
		"symbols_transient": len(v2.transientSymbols),
		"observer":          observer,
		"inventory":         v2.inventory(),
		// Bytes each Checker field's walk attributed (before type payloads), for diagnosis.
		"fields": fieldBytes,
		"record_sizes": map[string]any{
			"Type": unsafe.Sizeof(Type{}), "IntrinsicType": unsafe.Sizeof(IntrinsicType{}), "LiteralType": unsafe.Sizeof(LiteralType{}),
			"ObjectType": unsafe.Sizeof(ObjectType{}), "TypeReference": unsafe.Sizeof(TypeReference{}), "InterfaceType": unsafe.Sizeof(InterfaceType{}),
			"TupleType": unsafe.Sizeof(TupleType{}), "UnionType": unsafe.Sizeof(UnionType{}), "IntersectionType": unsafe.Sizeof(IntersectionType{}),
			"TypeParameter": unsafe.Sizeof(TypeParameter{}), "TemplateLiteralType": unsafe.Sizeof(TemplateLiteralType{}),
			"MappedType": unsafe.Sizeof(MappedType{}), "ConditionalType": unsafe.Sizeof(ConditionalType{}),
			"TypeAlias": unsafe.Sizeof(TypeAlias{}), "Signature": unsafe.Sizeof(Signature{}), "IndexInfo": unsafe.Sizeof(IndexInfo{}),
			"Symbol": unsafe.Sizeof(ast.Symbol{}), "ValueSymbolLinks": unsafe.Sizeof(ValueSymbolLinks{}), "Map": unsafe.Sizeof(s08RtMap{}),
		},
	}
}

// nodeFactory charges the checker's synthetic AST arenas from their current
// chunks; an arena at the plateau chunk size is unavailable.
func (v2 *s08V2) nodeFactory(v reflect.Value) {
	t := v.Type()
	for i := range t.NumField() {
		field := t.Field(i)
		if !strings.HasPrefix(field.Type.String(), "core.Arena[") {
			continue
		}
		v2.arena("checker_ast", v.Field(i), -1)
	}
}
