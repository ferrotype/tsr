package core

// Phase 2 C5.5 diagnostic overlay (never part of the pinned sources): the
// fourslash test that is running and the files of its virtual file system,
// which the checker's services recorder attaches to every program it sees.
// Tests run one at a time under the recorder, so one current test suffices.

import "sync"

type Phase2ServicesTest struct {
	Name     string
	Files    map[string]string
	Symlinks map[string]string
}

var (
	phase2ServicesMu      sync.Mutex
	phase2ServicesCurrent *Phase2ServicesTest
	phase2ServicesHooks   []func(ended *Phase2ServicesTest)
)

// Phase2ServicesBegin marks the start of a fourslash test.
func Phase2ServicesBegin(test *Phase2ServicesTest) {
	phase2ServicesMu.Lock()
	defer phase2ServicesMu.Unlock()
	phase2ServicesCurrent = test
}

// Phase2ServicesEnd marks the end of the current test and runs the hooks.
func Phase2ServicesEnd() {
	phase2ServicesMu.Lock()
	ended := phase2ServicesCurrent
	phase2ServicesCurrent = nil
	hooks := phase2ServicesHooks
	phase2ServicesMu.Unlock()
	for _, hook := range hooks {
		hook(ended)
	}
}

// Phase2ServicesCurrentTest is the running test, or nil outside one.
func Phase2ServicesCurrentTest() *Phase2ServicesTest {
	phase2ServicesMu.Lock()
	defer phase2ServicesMu.Unlock()
	return phase2ServicesCurrent
}

// Phase2ServicesOnEnd registers a hook run after each test ends.
func Phase2ServicesOnEnd(hook func(ended *Phase2ServicesTest)) {
	phase2ServicesMu.Lock()
	defer phase2ServicesMu.Unlock()
	phase2ServicesHooks = append(phase2ServicesHooks, hook)
}
