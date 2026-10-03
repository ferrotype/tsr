package tsctests

// Phase 4 X0: the scenario recorder. scripts/phase4_scenarios.py lays this file
// over the pinned package as phase4_recorder.go, together with patched copies
// of runner.go, sys.go and fs.go (the *.go.diff files beside this one), and
// runs the pinned tests. The patches call the hooks below; nothing here changes
// what the pinned harness does.
//
// Per scenario (one tscInput.run) the recorder writes one JSON row to
// $PHASE4_RECORD_DIR/rows.ndjson and the rendered baseline text to
// $PHASE4_RECORD_DIR/baselines/<family>/<scenario>/<file>. A row holds the
// scenario's inputs, its initial file map, and per edit the primitive
// operations the edit's closure performed at the fake system, once from the
// incremental system and once from the clean-build shadow (the closure for
// edit j runs on the shadow of every edit i >= j; the ops are taken from the
// shadow of edit j and every later shadow must repeat them exactly). File
// texts are hex; the script decodes them. Anything the recorder cannot
// represent is reported in the row's errors, which fail the recording.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"sync"
	"testing/fstest"
	"time"
	"unicode/utf8"
)

type phase4Op struct {
	Op            string  `json:"op"`
	Path          string  `json:"path"`
	TextHex       *string `json:"text_hex,omitempty"`
	ClockReadings int64   `json:"clock_readings"`
	Via           string  `json:"via,omitempty"`
}

type phase4Edit struct {
	Caption          string     `json:"caption"`
	CommandLineArgs  []string   `json:"command_line_args"`
	ExpectedDiff     string     `json:"expected_diff"`
	Edit             bool       `json:"edit"`
	Operations       []phase4Op `json:"operations"`
	ShadowOperations []phase4Op `json:"shadow_operations"`
	shadowRecorded   bool
}

type phase4File struct {
	TextHex *string `json:"text_hex,omitempty"`
	Symlink *string `json:"symlink,omitempty"`
}

type phase4Build struct {
	mu              sync.Mutex
	CommandLineArgs []string `json:"command_line_args"`
	Paths           []string `json:"paths"`
}

type phase4Row struct {
	Family               string                `json:"family"`
	Scenario             string                `json:"scenario"`
	File                 string                `json:"file"`
	SubScenario          string                `json:"sub_scenario"`
	CommandLineArgs      []string              `json:"command_line_args"`
	Cwd                  string                `json:"cwd"`
	Env                  map[string]string     `json:"env"`
	OutputIsTTY          bool                  `json:"output_is_tty"`
	IgnoreCase           bool                  `json:"ignore_case"`
	WindowsStyleRoot     string                `json:"windows_style_root"`
	Files                map[string]phase4File `json:"files"`
	FilesFromBuild       *phase4Build          `json:"files_from_build"`
	LibraryPath          string                `json:"library_path"`
	LibraryFiles         []string              `json:"library_files"`
	LibraryTextHex       string                `json:"library_text_hex"`
	InitialClockReadings int64                 `json:"initial_clock_readings"`
	Edits                []*phase4Edit         `json:"edits"`
	// Diagnostics for the script; not part of the inventory.
	HelperCalls    map[string]int `json:"helper_calls"`
	Shadows        int            `json:"shadows"`
	BaselineSha256 string         `json:"baseline_sha256"`
	Errors         []string       `json:"errors"`
}

type phase4Recording struct {
	mu          sync.Mutex
	row         *phase4Row
	filesDigest string
}

// phase4Active is the closure being recorded on one system.
type phase4Active struct {
	recording     *phase4Recording
	shadow        bool
	shadowFor     int
	index         int
	ops           []phase4Op
	depth         int
	via           string
	startReadings int64
	pendingNow    *time.Time
	helperCalls   map[string]int
	clock         *TestClock
}

var (
	// keyed by *testFs: the patched testFs.Chtimes sees only its file system
	phase4ActiveFs sync.Map
	// keyed by the FileMap's pointer: GetFileMapWithBuild's outputs
	phase4Builds   sync.Map
	phase4OutputMu sync.Mutex
)

func phase4Hex(text string) *string {
	value := hex.EncodeToString([]byte(text))
	return &value
}

func phase4ClockReadings(clock *TestClock) int64 {
	clock.nowMu.Lock()
	defer clock.nowMu.Unlock()
	if clock.now.IsZero() {
		return 0
	}
	elapsed := clock.now.Sub(clock.start)
	if elapsed%time.Second != 0 {
		panic(fmt.Sprintf("phase4: the test clock moved by %v, not whole seconds", elapsed))
	}
	return int64(elapsed / time.Second)
}

func (r *phase4Recording) errorf(format string, args ...any) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.row.Errors = append(r.row.Errors, fmt.Sprintf(format, args...))
}

// text checks a string the row carries as JSON text (file texts are hex).
// Callers must not hold r.mu.
func (r *phase4Recording) text(value string) string {
	if !utf8.ValidString(value) {
		r.errorf("a non-file string is not UTF-8: %q", value)
	}
	return value
}

func (r *phase4Recording) texts(values []string) []string {
	if values == nil {
		return nil
	}
	result := make([]string, len(values))
	for i, value := range values {
		result[i] = r.text(value)
	}
	return result
}

// phase4Files encodes a FileMap as the fake system reads it (vfstest.FromMapWithClock):
// a string or []byte is a file's bytes, vfstest.Symlink is a symlink.
func (r *phase4Recording) files(files FileMap) (map[string]phase4File, string) {
	result := make(map[string]phase4File, len(files))
	digest := sha256.New()
	for _, path := range slices.Sorted(maps.Keys(files)) {
		r.text(path)
		var entry phase4File
		var kind string
		var data []byte
		switch value := files[path].(type) {
		case string:
			kind, data = "text", []byte(value)
			entry.TextHex = phase4Hex(value)
		case []byte:
			kind, data = "text", value
			entry.TextHex = phase4Hex(string(value))
		case *fstest.MapFile:
			if value.Mode != fs.ModeSymlink || !value.ModTime.IsZero() || value.Sys != nil {
				r.errorf("unsupported MapFile at %s: mode %v", path, value.Mode)
			}
			target := r.text(string(value.Data))
			kind, data = "symlink", value.Data
			entry.Symlink = &target
		default:
			r.errorf("unsupported file value at %s: %T", path, value)
		}
		result[path] = entry
		fmt.Fprintf(digest, "%d:%s|%s|%d:", len(path), path, kind, len(data))
		digest.Write(data)
	}
	return result, hex.EncodeToString(digest.Sum(nil))
}

func phase4LibraryFiles(sys *TestSys) []string {
	var names []string
	if sys.fs.defaultLibs != nil {
		sys.fs.defaultLibs.Range(func(path string) bool {
			names = append(names, path)
			return true
		})
	}
	slices.Sort(names)
	return names
}

func phase4BeginScenario(test *tscInput, scenario string, sys *TestSys) *phase4Recording {
	r := &phase4Recording{row: &phase4Row{HelperCalls: map[string]int{}, Errors: []string{}}}
	row := r.row
	row.Family = test.getBaselineSubFolder()
	row.Scenario = r.text(scenario)
	row.File = r.text(strings.ReplaceAll(test.subScenario, " ", "-") + ".js")
	row.SubScenario = r.text(test.subScenario)
	row.CommandLineArgs = r.texts(test.commandLineArgs)
	row.Cwd = r.text(sys.GetCurrentDirectory())
	row.Env = map[string]string{}
	for name, value := range test.env {
		row.Env[r.text(name)] = r.text(value)
	}
	row.OutputIsTTY = sys.outputIsTTY
	row.IgnoreCase = test.ignoreCase
	row.WindowsStyleRoot = r.text(test.windowsStyleRoot)
	row.Files, r.filesDigest = r.files(test.files)
	row.LibraryPath = r.text(sys.defaultLibraryPath)
	row.LibraryFiles = []string{}
	for _, path := range phase4LibraryFiles(sys) {
		name, ok := strings.CutPrefix(path, sys.defaultLibraryPath+"/")
		if !ok {
			r.errorf("library file outside the library path: %s", path)
		}
		row.LibraryFiles = append(row.LibraryFiles, r.text(name))
	}
	row.LibraryTextHex = *phase4Hex(tscDefaultLibContent)
	row.InitialClockReadings = phase4ClockReadings(sys.clock)
	if value, ok := phase4Builds.Load(reflect.ValueOf(test.files).Pointer()); ok {
		build := value.(*phase4Build)
		build.mu.Lock()
		row.FilesFromBuild = &phase4Build{CommandLineArgs: r.texts(build.CommandLineArgs), Paths: slices.Sorted(slices.Values(build.Paths))}
		build.mu.Unlock()
	}
	row.Edits = []*phase4Edit{}
	for _, edit := range test.edits {
		row.Edits = append(row.Edits, &phase4Edit{
			Caption:         r.text(edit.caption),
			CommandLineArgs: r.texts(edit.commandLineArgs),
			ExpectedDiff:    r.text(edit.expectedDiff),
			Edit:            edit.edit != nil,
			Operations:      []phase4Op{},
		})
	}
	return r
}

// beginShadow checks that a clean-build shadow starts from the scenario's initial state.
func (r *phase4Recording) beginShadow(test *tscInput, sys *TestSys, index int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.row.Shadows++
	scratch := &phase4Recording{row: &phase4Row{}}
	if _, digest := scratch.files(test.files); digest != r.filesDigest {
		r.row.Errors = append(r.row.Errors, fmt.Sprintf("the shadow of edit %d starts from a changed file map", index))
	}
	libraries := phase4LibraryFiles(sys)
	for i, path := range libraries {
		libraries[i] = strings.TrimPrefix(path, sys.defaultLibraryPath+"/")
	}
	if !slices.Equal(libraries, r.row.LibraryFiles) {
		r.row.Errors = append(r.row.Errors, fmt.Sprintf("the shadow of edit %d has other library files", index))
	}
	if readings := phase4ClockReadings(sys.clock); readings != r.row.InitialClockReadings {
		r.row.Errors = append(r.row.Errors, fmt.Sprintf("the shadow of edit %d starts after %d clock readings", index, readings))
	}
}

func (r *phase4Recording) beginEdit(sys *TestSys, shadowFor int, index int) {
	active := &phase4Active{
		recording:     r,
		shadow:        sys.forIncrementalCorrectness,
		shadowFor:     shadowFor,
		index:         index,
		startReadings: phase4ClockReadings(sys.clock),
		helperCalls:   map[string]int{},
		clock:         sys.clock,
	}
	if _, loaded := phase4ActiveFs.LoadOrStore(sys.fs, active); loaded {
		r.errorf("edit %d began inside another edit on the same system", index)
	}
}

func (r *phase4Recording) endEdit(sys *TestSys, shadowFor int, index int) {
	value, ok := phase4ActiveFs.LoadAndDelete(sys.fs)
	if !ok {
		r.errorf("edit %d ended without beginning", index)
		return
	}
	active := value.(*phase4Active)
	readings := phase4ClockReadings(sys.clock) - active.startReadings
	var counted int64
	for _, op := range active.ops {
		counted += op.ClockReadings
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	row := r.row
	if active.pendingNow != nil {
		row.Errors = append(row.Errors, fmt.Sprintf("edit %d read the clock without a Chtimes", index))
	}
	if active.depth != 0 {
		row.Errors = append(row.Errors, fmt.Sprintf("edit %d ended inside a helper", index))
	}
	if readings != counted {
		row.Errors = append(row.Errors, fmt.Sprintf("edit %d read the clock %d times, its operations %d", index, readings, counted))
	}
	ops := active.ops
	if ops == nil {
		ops = []phase4Op{}
	}
	edit := row.Edits[index]
	switch {
	case !active.shadow:
		if shadowFor != index || len(edit.Operations) != 0 {
			row.Errors = append(row.Errors, fmt.Sprintf("edit %d ran twice on the incremental system", index))
		}
		edit.Operations = ops
		for name, count := range active.helperCalls {
			row.HelperCalls[name] += count
		}
	case shadowFor == index:
		edit.ShadowOperations = ops
		edit.shadowRecorded = true
	case !edit.shadowRecorded:
		row.Errors = append(row.Errors, fmt.Sprintf("edit %d ran on a later shadow before its own", index))
	case !reflect.DeepEqual(edit.ShadowOperations, ops):
		row.Errors = append(row.Errors, fmt.Sprintf("edit %d did different operations on the shadow of edit %d", index, shadowFor))
	}
}

func (r *phase4Recording) finish(baseline string) {
	r.mu.Lock()
	for index, edit := range r.row.Edits {
		if edit.Edit && !edit.shadowRecorded {
			r.row.Errors = append(r.row.Errors, fmt.Sprintf("edit %d never ran on a shadow", index))
		}
	}
	sum := sha256.Sum256([]byte(baseline))
	r.row.BaselineSha256 = hex.EncodeToString(sum[:])
	data, err := json.Marshal(r.row)
	family, scenario, file := r.row.Family, r.row.Scenario, r.row.File
	r.mu.Unlock()
	if err != nil {
		panic("phase4: cannot encode a scenario row: " + err.Error())
	}
	phase4WriteRow(family, scenario, file, baseline, data)
}

func phase4WriteRow(family string, scenario string, file string, baseline string, row []byte) {
	directory := os.Getenv("PHASE4_RECORD_DIR")
	if directory == "" {
		panic("phase4: the recorder overlay needs PHASE4_RECORD_DIR")
	}
	phase4OutputMu.Lock()
	defer phase4OutputMu.Unlock()
	path := filepath.Join(directory, "baselines", family, scenario, file)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(path, []byte(baseline), 0o644); err != nil {
		panic(err)
	}
	rows, err := os.OpenFile(filepath.Join(directory, "rows.ndjson"), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		panic(err)
	}
	defer rows.Close()
	if _, err := rows.Write(append(row, '\n')); err != nil {
		panic(err)
	}
}

func phase4ActiveFor(sys *TestSys) *phase4Active {
	if value, ok := phase4ActiveFs.Load(sys.fs); ok {
		return value.(*phase4Active)
	}
	return nil
}

func (a *phase4Active) count(name string) {
	if !a.shadow {
		a.helperCalls[name]++
	}
}

// phase4Operation records writeFileNoError and removeNoError, with the clock
// readings the write made (one per directory it created, then one for the file).
func phase4Operation(sys *TestSys, op string, path string, content *string) func() {
	active := phase4ActiveFor(sys)
	if active == nil {
		return func() {}
	}
	if active.depth == 0 {
		active.count(map[string]string{"write": "writeFileNoError", "remove": "removeNoError"}[op])
	}
	active.depth++
	before := phase4ClockReadings(sys.clock)
	return func() {
		active.depth--
		entry := phase4Op{Op: op, Path: active.recording.text(path), ClockReadings: phase4ClockReadings(sys.clock) - before, Via: active.via}
		if content != nil {
			entry.TextHex = phase4Hex(*content)
		}
		active.ops = append(active.ops, entry)
	}
}

// phase4Helper names the helper a closure called directly (replaceFileText and
// the others reduce to readFileNoError, writeFileNoError and removeNoError).
func phase4Helper(sys *TestSys, name string) func() {
	active := phase4ActiveFor(sys)
	if active == nil {
		return func() {}
	}
	if active.depth == 0 {
		active.count(name)
		active.via = name
	}
	active.depth++
	return func() {
		active.depth--
		if active.depth == 0 {
			active.via = ""
		}
	}
}

func phase4OnNow(sys *TestSys, now time.Time) {
	active := phase4ActiveFor(sys)
	if active == nil {
		return
	}
	if active.depth == 0 {
		active.count("Now")
	}
	if active.pendingNow != nil {
		active.recording.errorf("edit %d read the clock twice before a Chtimes", active.index)
	}
	active.pendingNow = &now
}

func phase4OnChtimes(f *testFs, path string, aTime time.Time, mTime time.Time) {
	value, ok := phase4ActiveFs.Load(f)
	if !ok {
		return
	}
	active := value.(*phase4Active)
	if active.depth == 0 {
		active.count("FS().Chtimes")
	}
	if !aTime.IsZero() || active.pendingNow == nil || !mTime.Equal(*active.pendingNow) {
		active.recording.errorf("edit %d set a modification time that is not the clock's last reading", active.index)
	}
	active.pendingNow = nil
	active.ops = append(active.ops, phase4Op{Op: "chtimes", Path: active.recording.text(path), ClockReadings: 1, Via: active.via})
}

// phase4NoteBuiltFile remembers which entries GetFileMapWithBuild added to a FileMap.
func phase4NoteBuiltFile(files FileMap, commandLineArgs []string, path string) {
	value, _ := phase4Builds.LoadOrStore(reflect.ValueOf(files).Pointer(), &phase4Build{CommandLineArgs: slices.Clone(commandLineArgs)})
	build := value.(*phase4Build)
	build.mu.Lock()
	defer build.mu.Unlock()
	build.Paths = append(build.Paths, path)
}
