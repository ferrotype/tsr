package checker

import "runtime"

func init() {
	s08AllocationStart = runtime.S08AllocationBegin
	s08AllocationFinish = func() *s08Allocations {
		raw, overflow := runtime.S08AllocationEnd()
		records := make([]s08Allocation, len(raw))
		for i, r := range raw {
			records[i] = s08Allocation{r.Address, r.Size, r.Type, r.Base, r.Slot}
		}
		return s08IndexAllocations(records, runtime.S08AllocationBase, overflow)
	}
}
