package core

// S08 checkerbench clocks (data/s08/checker-workload.json). Installed only in
// the read-only source overlay; the compiler, harness and walker hooks report
// into this package because every one of them already imports core.
//
// The interval clock accumulates wall time between Start/Stop. The phase
// clocks are nested and exclusive: pushing a phase stops charging the
// enclosing one until Pop. Mode "alloc" also samples runtime.MemStats at each
// boundary and accumulates TotalAlloc/Mallocs deltas inside the interval.

import (
	"runtime"
	"time"
)

const (
	S08PhaseInit    = 0
	S08PhaseCheck   = 1
	S08PhaseDisplay = 2
)

var S08PhaseNames = [3]string{"init", "check", "display"}

type S08Clock struct {
	Mode         string // "normal", "phase" or "alloc"
	running      bool
	since        time.Time
	IntervalNs   int64
	stack        []int
	PhaseNs      [3]int64
	displayDepth int
	memBefore    runtime.MemStats
	Requested    uint64
	Mallocs      uint64
}

var S08Bench = &S08Clock{Mode: "normal"}

func (c *S08Clock) Reset() {
	c.running = false
	c.IntervalNs = 0
	c.stack = c.stack[:0]
	c.PhaseNs = [3]int64{}
	c.displayDepth = 0
	c.Requested = 0
	c.Mallocs = 0
}

func (c *S08Clock) settle(now time.Time) {
	if !c.running {
		return
	}
	elapsed := now.Sub(c.since).Nanoseconds()
	c.IntervalNs += elapsed
	if n := len(c.stack); n > 0 {
		c.PhaseNs[c.stack[n-1]] += elapsed
	}
	c.since = now
}

// Start begins (or resumes) the interval; the top phase resumes charging.
func (c *S08Clock) Start() {
	if c.running {
		return
	}
	if c.Mode == "alloc" {
		runtime.ReadMemStats(&c.memBefore)
	}
	c.since = time.Now()
	c.running = true
}

// Stop pauses the interval (transport, decoration, GC, census).
func (c *S08Clock) Stop() {
	if !c.running {
		return
	}
	c.settle(time.Now())
	c.running = false
	if c.Mode == "alloc" {
		var after runtime.MemStats
		runtime.ReadMemStats(&after)
		c.Requested += after.TotalAlloc - c.memBefore.TotalAlloc
		c.Mallocs += after.Mallocs - c.memBefore.Mallocs
	}
}

// Push charges `phase` exclusively until Pop (phase mode only).
func (c *S08Clock) Push(phase int) {
	if c.Mode != "phase" {
		return
	}
	c.settle(time.Now())
	c.stack = append(c.stack, phase)
}

func (c *S08Clock) Pop() {
	if c.Mode != "phase" {
		return
	}
	c.settle(time.Now())
	if n := len(c.stack); n > 0 {
		c.stack = c.stack[:n-1]
	}
}

// Display calls nest (symbol display prints types); charge the outermost only.
func (c *S08Clock) DisplayBegin() {
	if c.displayDepth == 0 {
		c.Push(S08PhaseDisplay)
	}
	c.displayDepth++
}

func (c *S08Clock) DisplayEnd() {
	c.displayDepth--
	if c.displayDepth == 0 {
		c.Pop()
	}
}

func (c *S08Clock) Depth() int { return len(c.stack) }

// Pause excludes nested observation/decoration work and restores its caller's state.
func (c *S08Clock) Pause() func() {
	running := c.running
	c.Stop()
	return func() {
		if running {
			c.Start()
		}
	}
}
