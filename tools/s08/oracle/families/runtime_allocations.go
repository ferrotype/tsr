package runtime

// Diagnostic-only runtime overlay. No application pointer is retained here.
// Normal/phase benchmark binaries use the stock runtime, without this file.
import (
	"internal/runtime/atomic"
	"unsafe"
)

type S08Allocation struct{ Address, Size, Type, Base, Slot uintptr }

// Fixed log: the recorder runs inside mallocgc and cannot allocate. The first
// full checkerbench capture (2026-09-16) needed up to 1.93M interval records
// plus a pre-interval snapshot below 0.67M, and overflowed the former 1<<20
// entries on 9 of 9,369 variants. 1<<23 entries of 40 bytes is 335 MB of BSS,
// resident only as far as it is written (about 100 MB on the largest variant).
var s08AllocationLog [1 << 23]S08Allocation
var s08AllocationCount atomic.Uint64
var s08AllocationSnapshot atomic.Uint64
var s08AllocationActive atomic.Uint32

//go:nosplit
func s08RecordAllocation(result *unsafe.Pointer, size uintptr, typ *_type) {
	if size == 0 || s08AllocationActive.Load() == 0 {
		return
	}
	i := s08AllocationCount.Add(1) - 1
	if i >= uint64(len(s08AllocationLog)) {
		return
	}
	p := uintptr(*result)
	base, span, _ := findObject(p, 0, 0)
	slot := uintptr(0)
	if span != nil && base != 0 {
		slot = span.elemsize
	}
	s08AllocationLog[i] = S08Allocation{p, size, uintptr(unsafe.Pointer(typ)), base, slot}
}

func S08AllocationBegin() {
	stw := stopTheWorld(stwWriteHeapDump)
	s08AllocationCount.Store(0)
	// Snapshot pre-check heap identities. A later missed lookup is accepted as
	// bound input only with this explicit before-interval witness.
	for _, span := range mheap_.allspans {
		if span.state.get() != mSpanInUse {
			continue
		}
		bits := span.allocBitsForIndex(0)
		for i := uintptr(0); i < uintptr(span.nelems); i++ {
			if bits.index < uintptr(span.freeindex) || bits.isMarked() {
				at := s08AllocationCount.Add(1) - 1
				if at < uint64(len(s08AllocationLog)) {
					base := span.base() + i*span.elemsize
					s08AllocationLog[at] = S08Allocation{Address: base, Base: base, Slot: span.elemsize}
				}
			}
			bits.advance()
		}
	}
	s08AllocationSnapshot.Store(s08AllocationCount.Load())
	s08AllocationActive.Store(1)
	startTheWorld(stw)
}

// S08AllocationStatus reports the log's use after S08AllocationEnd: records
// requested (including those dropped past the capacity), the pre-interval
// snapshot's share of them, and the capacity.
func S08AllocationStatus() (recorded, snapshot, capacity uint64) {
	return s08AllocationCount.Load(), s08AllocationSnapshot.Load(), uint64(len(s08AllocationLog))
}
func S08AllocationEnd() ([]S08Allocation, bool) {
	stw := stopTheWorld(stwWriteHeapDump)
	s08AllocationActive.Store(0)
	n := s08AllocationCount.Load()
	startTheWorld(stw)
	return s08AllocationLog[:min(n, uint64(len(s08AllocationLog)))], n > uint64(len(s08AllocationLog))
}
func S08AllocationBase(p uintptr) uintptr {
	base, _, _ := findObject(p, 0, 0)
	return base
}
