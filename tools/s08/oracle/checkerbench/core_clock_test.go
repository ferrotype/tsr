package core

import "testing"

// Run directly with core_clock.go; no compiler workload is needed.
func TestObservationRestoresNestedClockState(t *testing.T) {
	c := &S08Clock{Mode: "phase"}
	c.Push(S08PhaseCheck)
	c.Start()
	outer := c.Pause()
	before := c.IntervalNs
	inner := c.Pause()
	inner()
	if c.running || c.IntervalNs != before || c.Depth() != 1 {
		t.Fatal("nested observer resumed its suspended caller")
	}
	outer()
	if !c.running || c.Depth() != 1 {
		t.Fatal("observer did not restore its caller")
	}
	c.Stop()
	c.Pop()
	if c.PhaseNs[S08PhaseCheck] != c.IntervalNs || c.Depth() != 0 {
		t.Fatal("exclusive phase and interval clocks differ")
	}
}
