package tsctests

// Phase 4 X0: the replay check. scripts/phase4_scenarios.py lays this file over
// the UNPATCHED pinned package as phase4_replay_test.go. It reads a recorded
// inventory (data/phase4/scenarios.json.gz), rebuilds every scenario as a
// tscInput whose edits apply the recorded operations instead of the pin's
// closures, and runs each through the pin's own tscInput.run, so the pin's
// runner, fake system and baseline comparison judge the recording.

import (
	"compress/gzip"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase4ReplayBytes struct {
	Text    *string `json:"text"`
	TextHex *string `json:"text_hex"`
	Symlink *string `json:"symlink"`
}

func (b phase4ReplayBytes) bytes() string {
	switch {
	case b.Text != nil && b.TextHex == nil:
		return *b.Text
	case b.TextHex != nil && b.Text == nil:
		data, err := hex.DecodeString(*b.TextHex)
		if err != nil {
			panic(err)
		}
		return string(data)
	}
	panic("phase4 replay: an entry needs exactly one of text and text_hex")
}

type phase4ReplayOp struct {
	phase4ReplayBytes
	Op            string `json:"op"`
	Path          string `json:"path"`
	ClockReadings int64  `json:"clock_readings"`
}

type phase4ReplayEdit struct {
	Caption          string            `json:"caption"`
	CommandLineArgs  []string          `json:"command_line_args"`
	ExpectedDiff     string            `json:"expected_diff"`
	Edit             bool              `json:"edit"`
	Operations       []phase4ReplayOp  `json:"operations"`
	ShadowOperations *[]phase4ReplayOp `json:"shadow_operations"`
}

type phase4ReplayScenario struct {
	ID               string                       `json:"id"`
	Family           string                       `json:"family"`
	Scenario         string                       `json:"scenario"`
	File             string                       `json:"file"`
	SubScenario      string                       `json:"sub_scenario"`
	CommandLineArgs  []string                     `json:"command_line_args"`
	Cwd              string                       `json:"cwd"`
	Env              map[string]string            `json:"env"`
	OutputIsTTY      bool                         `json:"output_is_tty"`
	IgnoreCase       bool                         `json:"ignore_case"`
	WindowsStyleRoot string                       `json:"windows_style_root"`
	Files            map[string]phase4ReplayBytes `json:"files"`
	Library          struct {
		Path  string   `json:"path"`
		Files []string `json:"files"`
	} `json:"library"`
	InitialClockReadings int64              `json:"initial_clock_readings"`
	Edits                []phase4ReplayEdit `json:"edits"`
}

type phase4ReplayDocument struct {
	LibraryText string                 `json:"library_text"`
	Scenarios   []phase4ReplayScenario `json:"scenarios"`
}

func phase4ReplayReadings(clock *TestClock) int64 {
	clock.nowMu.Lock()
	defer clock.nowMu.Unlock()
	if clock.now.IsZero() {
		return 0
	}
	return int64(clock.now.Sub(clock.start) / time.Second)
}

// phase4ReplayApply performs one recorded operation as the pin's closures do,
// and requires the clock to advance by the recorded number of readings.
func phase4ReplayApply(sys *TestSys, op phase4ReplayOp) {
	before := phase4ReplayReadings(sys.clock)
	switch op.Op {
	case "write":
		sys.writeFileNoError(op.Path, op.bytes())
	case "remove":
		sys.removeNoError(op.Path)
	case "chtimes":
		if err := sys.FS().Chtimes(op.Path, time.Time{}, sys.Now()); err != nil {
			panic(err)
		}
	default:
		panic("phase4 replay: unknown operation " + op.Op)
	}
	if readings := phase4ReplayReadings(sys.clock) - before; readings != op.ClockReadings {
		panic(fmt.Sprintf("phase4 replay: %s %s read the clock %d times, recorded %d", op.Op, op.Path, readings, op.ClockReadings))
	}
}

func (s *phase4ReplayScenario) input() *tscInput {
	files := FileMap{}
	for path, entry := range s.Files {
		if entry.Symlink != nil {
			files[path] = vfstest.Symlink(*entry.Symlink)
		} else {
			files[path] = entry.bytes()
		}
	}
	outputIsTTY := s.OutputIsTTY
	input := &tscInput{
		subScenario:      s.SubScenario,
		commandLineArgs:  s.CommandLineArgs,
		files:            files,
		cwd:              s.Cwd,
		env:              s.Env,
		outputIsTTY:      &outputIsTTY,
		ignoreCase:       s.IgnoreCase,
		windowsStyleRoot: s.WindowsStyleRoot,
	}
	for _, recorded := range s.Edits {
		edit := &tscEdit{caption: recorded.Caption, commandLineArgs: recorded.CommandLineArgs, expectedDiff: recorded.ExpectedDiff}
		if recorded.Edit {
			edit.edit = func(sys *TestSys) {
				operations := recorded.Operations
				if sys.forIncrementalCorrectness && recorded.ShadowOperations != nil {
					operations = *recorded.ShadowOperations
				}
				for _, op := range operations {
					phase4ReplayApply(sys, op)
				}
			}
		}
		input.edits = append(input.edits, edit)
	}
	return input
}

// checkInitialState compares the recorded initial facts with a fresh system
// built by the pin's newTestSys from the same input.
func (s *phase4ReplayScenario) checkInitialState(t *testing.T, input *tscInput, libraryText string) {
	sys := newTestSys(input, false)
	if sys.GetCurrentDirectory() != s.Cwd || sys.defaultLibraryPath != s.Library.Path || sys.outputIsTTY != s.OutputIsTTY {
		t.Fatalf("%s: the recorded working directory, library path or TTY flag differs", s.ID)
	}
	var names []string
	if sys.fs.defaultLibs != nil {
		sys.fs.defaultLibs.Range(func(path string) bool {
			names = append(names, strings.TrimPrefix(path, s.Library.Path+"/"))
			return true
		})
	}
	slices.Sort(names)
	if !slices.Equal(names, s.Library.Files) {
		t.Fatalf("%s: the recorded library files differ from newTestSys's", s.ID)
	}
	if readings := phase4ReplayReadings(sys.clock); readings != s.InitialClockReadings {
		t.Fatalf("%s: newTestSys read the clock %d times, recorded %d", s.ID, readings, s.InitialClockReadings)
	}
	if libraryText != tscDefaultLibContent {
		t.Fatalf("the recorded library text differs from the pin's")
	}
	if input.getBaselineSubFolder() != s.Family || strings.ReplaceAll(s.SubScenario, " ", "-")+".js" != s.File ||
		s.ID != s.Family+"/"+s.Scenario+"/"+s.File {
		t.Fatalf("%s: the recorded baseline path differs from the runner's", s.ID)
	}
}

func TestPhase4Replay(t *testing.T) {
	inventory, summary := os.Getenv("PHASE4_REPLAY_INVENTORY"), os.Getenv("PHASE4_REPLAY_SUMMARY")
	if inventory == "" || summary == "" {
		t.Fatal("PHASE4_REPLAY_INVENTORY and PHASE4_REPLAY_SUMMARY are required")
	}
	file, err := os.Open(inventory)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	reader, err := gzip.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	var document phase4ReplayDocument
	if err := json.NewDecoder(reader).Decode(&document); err != nil {
		t.Fatal(err)
	}
	// Runs after every parallel subtest has finished.
	t.Cleanup(func() {
		data, err := json.Marshal(map[string]any{
			"scenarios": len(document.Scenarios), "failed": t.Failed(),
			"go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH,
		})
		if err == nil {
			err = os.WriteFile(summary, data, 0o644)
		}
		if err != nil {
			t.Error(err)
		}
	})
	for i := range document.Scenarios {
		scenario := &document.Scenarios[i]
		input := scenario.input()
		scenario.checkInitialState(t, input, document.LibraryText)
		input.run(t, scenario.Scenario)
	}
}
