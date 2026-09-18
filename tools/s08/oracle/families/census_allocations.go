package checker

import (
	"fmt"
	"os"
	"reflect"
	"sort"
	"unsafe"
)

type s08Allocation struct{ address, size, typ, base, slot uintptr }
type s08Allocations struct {
	byBase   map[uintptr][]s08Allocation
	base     func(uintptr) uintptr
	overflow bool
	// Log use reported by the runtime observer: requested records (dropped ones
	// included), the pre-interval snapshot's share, and the log capacity.
	recorded, snapshot, capacity uint64
}

var s08AllocationStart = func() {}
var s08AllocationFinish = func() *s08Allocations { return nil }

// Called after binding, before checker construction, only in allocation runs.
func S08CensusBegin() { s08AllocationStart() }

func (a *s08Allocations) find(p uintptr) (s08Allocation, bool) {
	if a == nil {
		return s08Allocation{}, false
	}
	base := a.base(p)
	if base == 0 {
		return s08Allocation{}, true
	} // immutable image/static data
	for _, allocation := range a.byBase[base] {
		if p >= allocation.address && p < allocation.address+allocation.size {
			return allocation, true
		}
	}
	if records := a.byBase[base]; len(records) > 0 && records[0].size == 0 {
		return records[0], true
	}
	return s08Allocation{}, false
}

// Type descriptors are immutable process-global runtime objects, not observer
// roots. reflect.Type is the pinned runtime's *rtype wrapper at offset zero.
func s08ReflectType(address uintptr) reflect.Type {
	type iface struct{ tab, data unsafe.Pointer }
	sample := reflect.TypeOf(0)
	value := iface{(*iface)(unsafe.Pointer(&sample)).tab, unsafe.Pointer(address)}
	return *(*reflect.Type)(unsafe.Pointer(&value))
}

// reflect.Value's pointer is an address of the value for indirect values.
// This is needed for unexported interface/function fields; Pointer() on a
// function returns the code address rather than its captured environment.
func s08ValueAddress(v reflect.Value) uintptr {
	type value struct {
		typ  unsafe.Pointer
		ptr  unsafe.Pointer
		flag uintptr
	}
	raw := (*value)(unsafe.Pointer(&v))
	if raw.flag&(1<<7) != 0 {
		return uintptr(raw.ptr)
	}
	return uintptr(unsafe.Pointer(&raw.ptr))
}

func s08InterfaceBox(v reflect.Value) uintptr {
	return (*[2]uintptr)(unsafe.Pointer(s08ValueAddress(v)))[1]
}
func s08FunctionEnvironment(v reflect.Value) uintptr {
	return *(*uintptr)(unsafe.Pointer(s08ValueAddress(v)))
}

// Sort individual tiny allocations within a block. A fresh allocation at the
// block's beginning replaces the former generation; duplicate outer mallocgc
// notifications replace the same record rather than charging twice.
func s08IndexAllocations(records []s08Allocation, base func(uintptr) uintptr, overflow bool) *s08Allocations {
	result := &s08Allocations{byBase: map[uintptr][]s08Allocation{}, base: base, overflow: overflow}
	for _, record := range records {
		old := result.byBase[record.base]
		// Only noscan requests smaller than 16 bytes can share a tiny block.
		// Ordinary allocations may start after a malloc header, so address ==
		// base is not a sufficient witness of slot reuse.
		tiny := record.size > 0 && record.size < 16 && record.slot == 16 &&
			(record.typ == 0 || (*s08RtType)(unsafe.Pointer(record.typ)).PtrBytes == 0)
		if !tiny || (len(old) > 0 && record.address <= old[0].address) {
			old = nil
		}
		result.byBase[record.base] = append(old, record)
	}
	for _, records := range result.byBase {
		sort.Slice(records, func(i, j int) bool { return records[i].address < records[j].address })
	}
	return result
}

type s08AllocationCharge struct {
	family string
	bytes  int64
}

func (v2 *s08V2) allocation(family string, address uintptr) bool {
	if address == 0 {
		return false
	}
	allocation, ok := v2.allocations.find(address)
	if !ok {
		if os.Getenv("S08_CENSUS_DEBUG") != "" {
			fmt.Fprintf(os.Stderr, "unknown %s %v address=%x base=%x\n", family, v2.path, address, v2.allocations.base(address))
		}
		v2.markUnavailable("allocation_extent:" + family)
		return false
	}
	if allocation.size == 0 {
		return false
	}
	if v2.charges == nil {
		v2.charges = map[uintptr]s08AllocationCharge{}
	}
	if old, found := v2.charges[allocation.address]; found {
		if s08V2TypeFamilies[family] && !s08V2TypeFamilies[old.family] {
			v2.add(old.family, 0, -old.bytes)
			v2.add(family, 0, old.bytes)
			v2.charges[allocation.address] = s08AllocationCharge{family, old.bytes}
		}
		return false
	}
	bytes := int64(allocation.size)
	v2.charges[allocation.address] = s08AllocationCharge{family, bytes}
	v2.add(family, 0, bytes)
	return true
}

func (v2 *s08V2) record(family string, address uintptr, count, fallbackSize int64) {
	if v2.allocations == nil {
		v2.add(family, count, fallbackSize)
		return
	}
	v2.add(family, count, 0)
	v2.allocation(family, address)
}
func (v2 *s08V2) function(v reflect.Value, family string, depth int) {
	if v.IsNil() {
		return
	}
	if v2.allocations == nil {
		v2.markUnavailable("closure:" + family)
		return
	}
	address := s08FunctionEnvironment(v)
	allocation, ok := v2.allocations.find(address)
	if !ok {
		v2.markUnavailable("closure_extent:" + family)
		return
	}
	if allocation.size == 0 {
		return
	} // stateless function in the binary image
	if v2.visited[address] {
		return
	}
	v2.visited[address] = true
	v2.allocation(family, address)
	if allocation.typ == 0 {
		// A nil malloc type is a noscan allocation. It has no pointer-bearing
		// captures (its complete bytes were charged above), hence no edges.
		return
	}
	typ := s08ReflectType(allocation.typ)
	if typ.Kind() != reflect.Struct || typ.Size() != allocation.size {
		v2.markUnavailable("closure_layout:" + typ.String())
		return
	}
	payload := reflect.NewAt(typ, unsafe.Pointer(address)).Elem()
	v2.walkFields(payload, family, depth+1)
}
