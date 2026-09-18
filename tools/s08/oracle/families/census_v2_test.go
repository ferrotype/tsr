package checker

import (
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"
	"unsafe"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

func s08TestWalker() *s08V2 {
	return &s08V2{
		families: map[string]*s08Family{}, seen: map[uintptr]bool{},
		unavailable: map[string]bool{}, visited: map[uintptr]bool{}, types: map[*Type]bool{},
		fileTextLengths: map[uintptr]int{}, boundReferenced: map[uintptr]bool{},
	}
}

func TestS08CensusConcreteMapperIncludesItsEdges(t *testing.T) {
	source, target := &Type{}, &Type{}
	v2 := s08TestWalker()
	v2.walk(reflect.ValueOf(newSimpleTypeMapper(source, target)), "mappers", 0)
	if len(v2.types) != 2 || v2.families["mappers"].Bytes != int64(unsafe.Sizeof(SimpleTypeMapper{})) {
		t.Fatalf("mapper: reached %d types, charged %d bytes; want 2 types and %d bytes",
			len(v2.types), v2.families["mappers"].Bytes, unsafe.Sizeof(SimpleTypeMapper{}))
	}
}

func TestS08CensusLongerSharedSliceStillVisitsEveryEdge(t *testing.T) {
	shared := []*Type{{}, {}}
	v2 := s08TestWalker()
	v2.walk(reflect.ValueOf(shared[:1]), "type_lists", 0)
	v2.walk(reflect.ValueOf(shared), "type_lists", 0)
	if len(v2.types) != 2 || v2.families["type_lists"].Bytes != int64(2*unsafe.Sizeof((*Type)(nil))) {
		t.Fatalf("shared slice: reached %d types, charged %d bytes", len(v2.types), v2.families["type_lists"].Bytes)
	}
}

func TestS08CensusDepthLimitIsUnavailable(t *testing.T) {
	type link struct{ next *link }
	root := &link{}
	for range 80 {
		root = &link{root}
	}
	v2 := s08TestWalker()
	v2.walk(reflect.ValueOf(root), "mappers", 0)
	if len(v2.unavailable) == 0 {
		t.Fatal("depth-limited walk silently claimed a complete census")
	}
}

func allocationTraffic(work func()) uint64 {
	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	work()
	runtime.ReadMemStats(&after)
	return after.TotalAlloc - before.TotalAlloc
}

// A fresh small map is one header and one group, exactly what the runtime
// allocates for it; the layout mirror must agree with reflect's descriptor.
func TestS08MapBytesMatchesRuntimeForSmallMaps(t *testing.T) {
	var sink map[int]int
	traffic := allocationTraffic(func() {
		sink = make(map[int]int)
		for i := range 8 {
			sink[i] = i
		}
	})
	bytes, ok := s08MapBytes(reflect.ValueOf(sink))
	if !ok {
		t.Fatal("map layout self-check failed")
	}
	// Structural bytes: the header and one group of eight slots. The runtime
	// rounds the group to its size class; that allocator slack is excluded, as
	// the Rust census excludes mimalloc's.
	mt := (*s08RtMapType)(unsafe.Pointer(s08TypePointer(reflect.TypeOf(sink))))
	if want := int64(unsafe.Sizeof(s08RtMap{})) + int64(mt.GroupSize); bytes != want {
		t.Fatalf("small map: layout bytes %d, want header %d + group %d", bytes, unsafe.Sizeof(s08RtMap{}), mt.GroupSize)
	}
	if uint64(bytes) > traffic || traffic-uint64(bytes) > 64 {
		t.Fatalf("small map: layout bytes %d, runtime allocated %d", bytes, traffic)
	}
	var empty map[int]int
	if bytes, ok := s08MapBytes(reflect.ValueOf(empty)); !ok || bytes != 0 {
		t.Fatalf("nil map must be free: %d %v", bytes, ok)
	}
	header := make(map[string]*Type)
	if bytes, ok := s08MapBytes(reflect.ValueOf(header)); !ok || bytes < int64(unsafe.Sizeof(s08RtMap{})) {
		t.Fatalf("unwritten map is at least one header: %d %v", bytes, ok)
	}
}

// Deleting entries keeps the tables and groups: the actual layout does not
// shrink, while a same-length replica would.
func TestS08MapBytesKeepCapacityAfterDeletion(t *testing.T) {
	grown := make(map[int]int)
	for i := range 1000 {
		grown[i] = i
	}
	before, ok := s08MapBytes(reflect.ValueOf(grown))
	if !ok {
		t.Fatal("map layout self-check failed")
	}
	if before < int64(1000/8)*int64(unsafe.Sizeof(s08RtMap{})) {
		t.Fatalf("large map layout bytes implausibly small: %d", before)
	}
	for i := range 900 {
		delete(grown, i)
	}
	after, ok := s08MapBytes(reflect.ValueOf(grown))
	if !ok || after != before {
		t.Fatalf("deletion changed the layout bytes: %d -> %d (%v)", before, after, ok)
	}
	if replica := mapBytes(grown); replica >= before {
		t.Fatalf("a same-length replica (%d) should be smaller than the grown map (%d)", replica, before)
	}
}

// The chunk replay reproduces the real arena's chunk sequence (including the
// size-class rounding of pointer-carrying chunks), so a capacity replayed from a
// record count is the arena's actual reserved slots; the current-chunk
// inference is exact below the plateau and refuses above it.
func TestS08ArenaReplayAndInference(t *testing.T) {
	elem := reflect.TypeOf(ast.Symbol{})
	var arena core.Arena[ast.Symbol]
	data := unexportedField(&arena, "data")
	actual := []int{}
	for range 4000 {
		arena.New()
		if data.Len() == 1 {
			actual = append(actual, data.Cap())
		}
	}
	if replayed := s08ArenaChunkCaps(elem, len(actual)); !reflect.DeepEqual(replayed, actual) {
		t.Fatalf("replayed chunks %v, actual %v", replayed, actual)
	}
	for _, count := range []int{0, 1, 2, 3, 7, 100, 300, 511, 512, 1000, 4000} {
		want, filled := 0, 0
		for _, capacity := range actual {
			if filled >= count && count != 0 {
				break
			}
			want += capacity
			filled += capacity
		}
		if count == 0 {
			want = 0
		}
		if got := s08ArenaCapacityByCount(elem, count); got != want {
			t.Fatalf("count %d: replay %d, actual %d", count, got, want)
		}
		var probe core.Arena[ast.Symbol]
		for range count {
			probe.New()
		}
		current := unexportedField(&probe, "data")
		records, capacity, ok := s08ArenaFromCurrentChunk(elem, current)
		plateau := s08ArenaChunkCaps(elem, 64)[63]
		if current.Cap() == plateau {
			if ok {
				t.Fatalf("count %d: plateau chunk must be ambiguous", count)
			}
			continue
		}
		if !ok || records != count || capacity != want {
			t.Fatalf("count %d: inferred %d records / %d slots (%v), want %d / %d", count, records, capacity, ok, count, want)
		}
	}
}

// Every Checker field is classified, so a new field cannot vanish silently.
func TestS08CensusClassifiesEveryCheckerField(t *testing.T) {
	checkerType := reflect.TypeOf(Checker{})
	for i := range checkerType.NumField() {
		field := checkerType.Field(i)
		if field.Type == s08TypeType || field.Type.Kind() == reflect.Func {
			continue
		}
		if _, ok := s08V2FieldFamilies[field.Name]; !ok {
			t.Errorf("unclassified Checker field %s", field.Name)
		}
	}
	for name := range s08V2FieldFamilies {
		if _, ok := checkerType.FieldByName(name); !ok {
			t.Errorf("classified field %s does not exist", name)
		}
	}
}

// One allocation is charged once however many references reach it: a shared
// backing slice and a shared string are each counted a single time, and a
// string inside a file text is a bound input charged to no family.
func TestS08CensusChargesSharedAllocationsOnce(t *testing.T) {
	v2 := &s08V2{families: map[string]*s08Family{}, seen: map[uintptr]bool{}, unavailable: map[string]bool{},
		fileTextLengths: map[uintptr]int{}, boundReferenced: map[uintptr]bool{}}
	shared := make([]*Type, 3, 8)
	v2.sliceBytes("type_lists", reflect.ValueOf(shared))
	v2.sliceBytes("type_lists", reflect.ValueOf(shared[:1]))
	if got := v2.families["type_lists"].Bytes; got != int64(8*unsafe.Sizeof((*Type)(nil))) {
		t.Fatalf("shared slice charged %d bytes (pointers %#x %#x)", got, reflect.ValueOf(shared).UnsafePointer(), reflect.ValueOf(shared[:1]).UnsafePointer())
	}
	text := "shared text"
	v2.text("literal", text)
	v2.text("union", text)
	if v2.families["literal"].Bytes != int64(len(text)) || v2.families["union"] != nil {
		t.Fatalf("shared string charged twice: %v", v2.families)
	}
	file := "export const x = 'inside the file';"
	start := uintptr(unsafe.Pointer(unsafe.StringData(file)))
	v2.fileTexts = append(v2.fileTexts, [2]uintptr{start, start + uintptr(len(file))})
	v2.fileTextLengths[start] = len(file)
	v2.text("symbols", file[17:33])
	if v2.families["symbols"] != nil || v2.boundReferences != 1 || !v2.boundReferenced[start] {
		t.Fatalf("source-backed string must be a bound input: %v %d", v2.families["symbols"], v2.boundReferences)
	}
}

var s08AllocationTestRoots []any

func TestS08CensusAllocationProvenance(t *testing.T) {
	S08CensusBegin()
	shared := make([]int64, 5)
	text := strings.Clone("shared owned string")
	number := float64(time.Now().UnixNano())
	var box any = number
	endpoint := &Type{}
	closure := func() *Type { return endpoint }
	// Escape all operands before stopping the allocator observer.
	roots := []any{shared, text, &box, closure}
	s08AllocationTestRoots = roots
	defer func() { s08AllocationTestRoots = nil }()
	v2 := s08TestWalker()
	v2.allocations = s08AllocationFinish()
	for _, view := range [][]int64{shared[1:], shared[:1:1], shared} {
		v2.sliceBytes("type_lists", reflect.ValueOf(view))
	}
	if got := v2.families["type_lists"].Bytes; got != 40 {
		t.Fatalf("shared allocation: %d, want 40", got)
	}
	v2.text("symbols", text[:3])
	v2.text("literal", text)
	if v2.families["literal"].Bytes != int64(len(text)) || v2.families["symbols"].Bytes != 0 {
		t.Fatalf("shared text attribution: %v", v2.families)
	}
	before := v2.families["literal"].Bytes
	v2.walk(reflect.ValueOf(&box).Elem(), "literal", 0)
	if v2.families["literal"].Bytes-before != 8 {
		t.Fatal("numeric interface box was not charged exactly once")
	}
	v2.walk(reflect.ValueOf(closure), "mappers", 0)
	if !v2.types[endpoint] || len(v2.unavailable) != 0 {
		t.Fatalf("closure edges/unavailable: %v / %v", v2.types, v2.unavailable)
	}
	runtime.KeepAlive(roots)
}

// The allocation log is uintptr metadata, not an all-types registry/root.
func TestS08CensusDoesNotRootCreatedTypes(t *testing.T) {
	c := &Checker{}
	root := c.newIntrinsicType(TypeFlagsUnknown, "root")
	_ = c.newIntrinsicType(TypeFlagsUnknown, "discarded")
	v2 := s08TestWalker()
	v2.walk(reflect.ValueOf(root), "intrinsic", 0)
	if c.TypeCount != 2 || len(v2.types) != 1 || !v2.types[root] {
		t.Fatalf("created/reachable = %d/%d", c.TypeCount, len(v2.types))
	}
}

func TestS08CensusAllocationSlotReuseDoesNotInheritBoundStatus(t *testing.T) {
	base := func(uintptr) uintptr { return 1024 }
	a := s08IndexAllocations([]s08Allocation{
		{address: 1024, base: 1024, slot: 1024},
		{address: 1032, base: 1024, size: 600, slot: 1024},
	}, base, false)
	if got, ok := a.find(1100); !ok || got.size != 600 {
		t.Fatal("current allocation extent missing")
	}
	if _, ok := a.find(1800); ok {
		t.Fatal("reused slot inherited the old bound-input marker")
	}
	// New tiny requests can legitimately coexist with a live bound allocation.
	a = s08IndexAllocations([]s08Allocation{
		{address: 1024, base: 1024, slot: 16},
		{address: 1032, base: 1024, size: 4, slot: 16},
	}, base, false)
	if got, ok := a.find(1024); !ok || got.size != 0 {
		t.Fatal("bound tiny allocation lost")
	}
	if got, ok := a.find(1033); !ok || got.size != 4 {
		t.Fatal("new tiny allocation lost")
	}
}

func TestS08CensusPagedStoreKeepsEveryReferencedArenaChunk(t *testing.T) {
	type link struct {
		value   *Type
		padding [5]uintptr
	}
	S08CensusBegin()
	first, last := make([]link, 2), make([]link, 4)
	endpoint := &Type{}
	first[0].value = endpoint
	store := struct {
		pageList []*[2]*link
		pageMap  map[uint32]*[2]*link
	}{
		pageList: []*[2]*link{{&first[0], &last[0]}},
	}
	s08AllocationTestRoots = []any{first, last, store}
	defer func() { s08AllocationTestRoots = nil }()
	v2 := s08TestWalker()
	v2.allocations = s08AllocationFinish()
	v2.pagedLinkStore("query_links", reflect.ValueOf(store))
	for _, records := range [][]link{first, last} {
		allocation, ok := v2.allocations.find(uintptr(unsafe.Pointer(&records[0])))
		if !ok {
			t.Fatal("missing allocation provenance")
		}
		if _, found := v2.charges[allocation.address]; !found {
			t.Fatal("referenced arena chunk omitted")
		}
	}
	if !v2.types[endpoint] || len(v2.unavailable) != 0 {
		t.Fatal("record edges lost", v2.unavailable)
	}
	runtime.KeepAlive(store)
}
