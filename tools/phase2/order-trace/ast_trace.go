package ast

// Diagnostic overlay only. These tokens are independent of GetSymbolId.
import (
	"runtime"
	"sync"
)

var c2Trace struct {
	sync.Mutex
	active  bool
	events  []map[string]any
	symbols map[*Symbol]int
	next    int
}

func C2TraceBegin() {
	c2Trace.Lock()
	defer c2Trace.Unlock()
	if c2Trace.active {
		panic("creation trace already active")
	}
	c2Trace.active = true
	c2Trace.events = []map[string]any{}
	c2Trace.symbols = map[*Symbol]int{}
	c2Trace.next = 0
}

func C2TraceFinish() []map[string]any {
	c2Trace.Lock()
	defer c2Trace.Unlock()
	c2Trace.active = false
	return c2Trace.events
}

func C2TraceActive() bool {
	c2Trace.Lock()
	defer c2Trace.Unlock()
	return c2Trace.active
}

func C2TraceRecord(event map[string]any) {
	c2Trace.Lock()
	defer c2Trace.Unlock()
	if c2Trace.active {
		c2Trace.events = append(c2Trace.events, event)
	}
}

func C2TraceOrigin() map[string]any {
	pcs := make([]uintptr, 12)
	n := runtime.Callers(2, pcs)
	frames := runtime.CallersFrames(pcs[:n])
	origins := []map[string]any{}
	for {
		frame, more := frames.Next()
		origins = append(origins, map[string]any{"function": frame.Function, "file": frame.File, "line": frame.Line})
		if !more {
			break
		}
	}
	return map[string]any{"stack": origins}
}

func C2TraceSymbolBirth(symbol *Symbol) {
	if !C2TraceActive() {
		return
	}
	origin := C2TraceOrigin()
	c2Trace.Lock()
	defer c2Trace.Unlock()
	c2Trace.next++
	c2Trace.symbols[symbol] = c2Trace.next
	c2Trace.events = append(c2Trace.events, map[string]any{"event": "birth", "kind": "symbol", "token": c2Trace.next,
		"origin": origin, "flags": symbol.Flags, "name": C2TraceBytes(symbol.Name), "semantic_id": symbol.id.Load()})
}

func C2TraceSymbolAssigned(symbol *Symbol, id uint64) {
	if !C2TraceActive() {
		return
	}
	c2Trace.Lock()
	defer c2Trace.Unlock()
	var token any
	if value, ok := c2Trace.symbols[symbol]; ok {
		token = value
	}
	c2Trace.events = append(c2Trace.events, map[string]any{"event": "id_assignment", "kind": "symbol", "token": token, "semantic_id": id})
}

func C2TraceBytes(s string) []int {
	result := make([]int, len(s))
	for i := range len(s) {
		result[i] = int(s[i])
	}
	return result
}

func C2TraceSymbol(symbol *Symbol) any {
	if symbol == nil {
		return nil
	}
	c2Trace.Lock()
	var token any
	if value, ok := c2Trace.symbols[symbol]; ok {
		token = value
	}
	c2Trace.Unlock()
	declarations := []any{}
	for _, node := range symbol.Declarations {
		declarations = append(declarations, map[string]any{"kind": node.Kind, "pos": node.Pos(), "end": node.End()})
	}
	return map[string]any{"token": token, "semantic_id": symbol.id.Load(), "flags": symbol.Flags,
		"check_flags": symbol.CheckFlags, "name": C2TraceBytes(symbol.Name), "declarations": declarations}
}
