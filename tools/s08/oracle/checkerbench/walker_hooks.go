package tsbaseline

import (
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

// Count-only query recording keeps JSON/struct transport out of the interval;
// the counts prove the executed action schedule. Type results are retained as
// roots of the retained checkpoint (data/s08/type-footprint.json).
var S08CountOnly bool
var S08QueryCounts = map[string]int{}
var S08CollectRoots bool
var S08Roots []*checker.Type

func s08Record(q S08Query) {
	if S08CountOnly {
		S08QueryCounts[q.Operation]++
		return
	}
	S08Queries = append(S08Queries, q)
}

func s08DisplayBegin() { core.S08Bench.DisplayBegin() }
func s08DisplayEnd()   { core.S08Bench.DisplayEnd() }
