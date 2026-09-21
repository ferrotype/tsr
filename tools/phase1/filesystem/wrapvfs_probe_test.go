package wrapvfs

// Access only: replays an ordered action trace against the pinned wrapping and
// tracking adapters and records what each action observed.
//
// Two subjects share one action vocabulary, because they are the same shape of
// thing and the interesting cases are the ones where they differ:
//
//   - `trackingvfs.FS`, which forwards every method to Inner and records the
//     path argument of the read-like ones in SeenFiles
//     (upstream/tsc/internal/vfs/trackingvfs/trackingvfs.go:24-76);
//   - `wrapvfs.Wrap`, which answers from a per-method replacement when one is
//     installed and forwards to the inner otherwise
//     (upstream/tsc/internal/vfs/wrapvfs/wrapvfs.go:24-29 and :37-130).
//
// It compiles into `package wrapvfs` rather than `wrapvfs_test` only because
// the overlay drops one file into the pinned package directory; nothing here
// touches an unexported name of that package, and every identifier this file
// declares is prefixed `phase1` so a later pin cannot collide with it.
//
// WHAT THE INNER IS, AND WHY IT IS ALWAYS SAID OUT LOUD. Forwarding is the
// subject, so every constructor names its inner and its silent methods in the
// request, empty lists included, and the choice is deliberate:
//
//   - `mock_over_mapfs` is a vfsmock.FSMock over vfstest.FromMapWithClock. It
//     answers, so it can prove a call WAS forwarded and what the inner did
//     with it. Each of the mock's twelve Funcs is bound the way vfsmock.Wrap
//     binds them, through a closure that appends to one ordered log first, so
//     the trace carries true call order across methods -- which the generated
//     mock's twelve separate per-method logs cannot show -- while the mock
//     still records its own calls (mock_generated.go appends callInfo before
//     it invokes the Func), so the two views agree.
//   - `silent_methods` leaves the named Funcs nil. The generated mock then
//     panics -- "FSMock.ReadFileFunc: method is nil but FS.ReadFile was just
//     called" -- which is how a case proves a call was NOT forwarded, rather
//     than merely proving the answer came from somewhere else.
//   - `mock_over_readonly_iofs` is iovfs.From over a bare fstest.MapFS, which
//     is not an iovfs.WritableFS. Its four write-class methods are the panicking
//     stubs of iofs.go:91-103 ("writeFile not supported" and friends), so the
//     forwarded capability failure of a read-only inner is observable.
//
// TWO RULES KEEP AN OBSERVATION PORTABLE.
//
// Anything derived from a Go map's iteration order is sorted before it is
// recorded: trackingvfs.SeenFiles is a collections.SyncSet over a sync.Map and
// vfs.Entries.Symlinks is a map, and neither order is the contract. The Files
// and Directories slices of vfs.Entries are NOT sorted: they are slices, their
// order is the inner's answer, and one case exists precisely to show the
// wrapper does not reorder them.
//
// A panic the pinned source raises itself is recorded with its literal text,
// because that text is the contract (iofs.go's "writeFile not supported", the
// generated mock's nil-Func sentence, internal.go's "vfs: path %q is not
// absolute"). A panic the Go runtime raises is reduced to a class instead: its
// wording names dynamic types and can move with the toolchain, and no Rust port
// could reproduce the sentence. Errors are reduced the same way, so that
// fs.ErrNotExist travels as "not_exist" rather than as a stdlib sentence.
//
// An unknown or malformed action panics outside any guard. It is a harness
// failure on both sides, never an observation both sides could agree on
// without executing anything, and every load-bearing key is required rather
// than defaulted for the same reason -- a key of an action and a key of a
// replacement declaration alike. Two mechanisms carry that: a scalar key
// travels as a pointer, so an absent `data`, `text`, `name`, `size` or `perm`
// is distinguishable from the zero value and panics; a list key stays a plain
// slice and is guarded by its nil, so an absent `paths`, `reads`, `files`,
// `directories`, `symlinks` or `arrivals` panics while an explicitly empty one
// is legal and is the way a case says "this declaration answers nothing".

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/trackingvfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfsmock"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// The two sentinels a replacement stub and a walk callback return. They are
// probe-defined, so they are recorded as classes rather than as text: a port
// supplies its own sentinel and only the identity matters.
var (
	errPhase1Stub     = errors.New("phase1: replacement sentinel")
	errPhase1Callback = errors.New("phase1: walk callback sentinel")
)

// phase1MethodNames is the vfs.FS method set in the order a passthrough case
// calls it. It is fixed here rather than derived, because a case that asserts
// "every method reached the inner" has to name every method it expected.
var phase1MethodNames = []string{
	"UseCaseSensitiveFileNames", "FileExists", "ReadFile", "WriteFile",
	"AppendFile", "Remove", "Chtimes", "DirectoryExists",
	"GetAccessibleEntries", "Stat", "WalkDir", "Realpath",
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// phase1Read is one row of a ReadFile replacement's answer table. Go's
// ReadFile contract is an independent (contents, ok) pair, so the table
// carries both and a case can install ("", true) and ("X", false).
type phase1Read struct {
	Path     string `json:"path"`
	Contents string `json:"contents"`
	Ok       *bool  `json:"ok"`
}

// phase1Arrival is one synthetic callback arrival a WalkDir replacement
// replays. It travels in the request rather than living in this file so a port
// reading the schedule knows what the stub does.
type phase1Arrival struct {
	Path  string `json:"path"`
	Name  string `json:"name"`
	IsDir *bool  `json:"is_dir"`
}

// phase1Stub declares one entry of a wrapvfs.Replacements table. `behavior`
// names the rule, the way the leaves probe's `key_of` and `equality` do: a rule
// that lived only in this file would leave a port reading the request with no
// way to know what the replacement answers.
type phase1Stub struct {
	Method   string `json:"method"`
	Behavior string `json:"behavior"`

	Text  *string      `json:"text"`
	Flag  *bool        `json:"flag"`
	Paths []string     `json:"paths"`
	Reads []phase1Read `json:"reads"`

	Files       []string `json:"files"`
	Directories []string `json:"directories"`
	Symlinks    []string `json:"symlinks"`
	NilSymlinks *bool    `json:"nil_symlinks"`

	Name    *string `json:"name"`
	Size    *int64  `json:"size"`
	Perm    *uint32 `json:"perm"`
	Dir     *bool   `json:"dir"`
	ModTime string  `json:"mod_time"`

	Arrivals []phase1Arrival `json:"arrivals"`
}

type phase1Action struct {
	Op string `json:"op"`

	// Constructor fields.
	Inner         string            `json:"inner"`
	Files         map[string]string `json:"files"`
	Symlinks      map[string]string `json:"symlinks"`
	CaseSensitive *bool             `json:"case_sensitive"`
	ClockStart    string            `json:"clock_start"`
	SilentMethods []string          `json:"silent_methods"`
	Replace       *[]phase1Stub     `json:"replace"`

	// Method fields.
	Path     string  `json:"path"`
	Data     *string `json:"data"`
	ATime    string  `json:"a_time"`
	MTime    string  `json:"m_time"`
	Callback string  `json:"callback"`
	At       string  `json:"at"`
}

// phase1Clock is the vfstest.Clock every in-memory inner is built with. It is
// fixed rather than stepping: every entry then carries exactly the instant the
// request named, so a recorded ModTime is stated by the case instead of
// depending on how many times vfstest happens to call Now() while it creates
// intermediate directories.
type phase1Clock struct{ at time.Time }

func (c *phase1Clock) Now() time.Time            { return c.at }
func (c *phase1Clock) SinceStart() time.Duration { return 0 }

type phase1FileInfo struct {
	name    string
	size    int64
	mode    fs.FileMode
	modTime time.Time
}

func (f phase1FileInfo) Name() string       { return f.name }
func (f phase1FileInfo) Size() int64        { return f.size }
func (f phase1FileInfo) Mode() fs.FileMode  { return f.mode }
func (f phase1FileInfo) ModTime() time.Time { return f.modTime }
func (f phase1FileInfo) IsDir() bool        { return f.mode.IsDir() }
func (f phase1FileInfo) Sys() any           { return "phase1-sys" }

type phase1DirEntry struct{ info phase1FileInfo }

func (d phase1DirEntry) Name() string               { return d.info.name }
func (d phase1DirEntry) IsDir() bool                { return d.info.IsDir() }
func (d phase1DirEntry) Type() fs.FileMode          { return d.info.mode.Type() }
func (d phase1DirEntry) Info() (fs.FileInfo, error) { return d.info, nil }

// phase1State is one trace's world: the inner, the mock the subject was handed,
// the two ordered call logs and the subject itself.
type phase1State struct {
	subject string

	mock *vfsmock.FSMock

	innerLog     []any
	innerDrained int
	stubLog      []any
	stubDrained  int

	tracker *trackingvfs.FS
	wrapper vfs.FS

	// table is the caller's own Replacements variable. Wrap takes it by value,
	// so mutating this after Wrap must not reach the wrapper; one case exists
	// to observe exactly that.
	table Replacements
}

func (s *phase1State) noteInner(parts ...any) {
	s.innerLog = append(s.innerLog, parts)
}

func (s *phase1State) noteStub(parts ...any) {
	s.stubLog = append(s.stubLog, parts)
}

// drainInner returns the inner calls made since the previous action, in true
// call order. A delta rather than the whole log, because the question every row
// answers is what THIS action forwarded.
func (s *phase1State) drainInner() []any {
	out := []any{}
	out = append(out, s.innerLog[s.innerDrained:]...)
	s.innerDrained = len(s.innerLog)
	return out
}

func (s *phase1State) drainStubs() []any {
	out := []any{}
	out = append(out, s.stubLog[s.stubDrained:]...)
	s.stubDrained = len(s.stubLog)
	return out
}

// subjectFS is the adapter under test. phase1Replay refuses a trace whose
// first action is not a constructor before any guarded call runs, so a nil
// here would be a probe bug rather than a recordable observation.
func (s *phase1State) subjectFS() vfs.FS { return s.wrapper }

// phase1Require returns a scalar key's value and refuses an absent one. Every
// scalar a rule consumes travels as a pointer so that "the key was omitted"
// cannot be mistaken for the zero value: an omitted `data` has to fail the
// harness rather than become an empty write that both sides would agree on
// without either of them exercising the operation.
func phase1Require[T any](value *T, field string, op string) T {
	if value == nil {
		panic("phase1: action " + op + " is missing required key " + field)
	}
	return *value
}

// phase1RequireSlice returns a list key's value and refuses an absent one. An
// explicitly empty list stays legal and is how a declaration says it answers
// nothing: encoding/json leaves the field nil only when the key was absent
// altogether, which is the rule phase1BuildBacking already applies to files,
// symlinks and silent_methods.
func phase1RequireSlice[T any](value []T, field string, op string) []T {
	if value == nil {
		panic("phase1: action " + op + " is missing required key " + field)
	}
	return value
}

func phase1RequirePath(value string, op string) string {
	if value == "" {
		panic("phase1: action " + op + " is missing required key path")
	}
	return value
}

func phase1RequireTime(value string, field string, op string) time.Time {
	if value == "" {
		panic("phase1: action " + op + " is missing required key " + field)
	}
	parsed, err := time.Parse(time.RFC3339Nano, value)
	if err != nil {
		panic("phase1: action " + op + " has an unparseable " + field + ": " + err.Error())
	}
	return parsed
}

// phase1BuildBacking builds the filesystem the mock forwards to. Every key is
// required, including the empty ones: a case whose subject is forwarding has to
// say what the inner holds, and `silent_methods: []` states in the request
// itself that nothing about this inner refuses to answer.
func phase1BuildBacking(a phase1Action) vfs.FS {
	caseSensitive := phase1Require(a.CaseSensitive, "case_sensitive", a.Op)
	if a.Files == nil {
		panic("phase1: action " + a.Op + " is missing required key files")
	}
	if a.Symlinks == nil {
		panic("phase1: action " + a.Op + " is missing required key symlinks")
	}
	if a.SilentMethods == nil {
		panic("phase1: action " + a.Op + " is missing required key silent_methods")
	}
	switch a.Inner {
	case "mock_over_mapfs":
		clock := &phase1Clock{at: phase1RequireTime(a.ClockStart, "clock_start", a.Op)}
		entries := make(map[string]*fstest.MapFile, len(a.Files)+len(a.Symlinks))
		for path, contents := range a.Files {
			entries[path] = &fstest.MapFile{Data: []byte(contents)}
		}
		for path, target := range a.Symlinks {
			entries[path] = vfstest.Symlink(target)
		}
		return vfstest.FromMapWithClock(entries, caseSensitive, clock)
	case "mock_over_readonly_iofs":
		if len(a.Symlinks) != 0 {
			panic("phase1: a mock_over_readonly_iofs inner cannot carry symlinks")
		}
		if a.ClockStart != "" {
			// A bare fstest.MapFS has no clock: every ModTime is the zero
			// instant, which is deterministic but is not the request's to set.
			panic("phase1: a mock_over_readonly_iofs inner takes no clock_start")
		}
		// io/fs paths are unrooted, so the request's rooted paths lose their
		// leading slash the way vfstest strips it. iovfs.From puts it back when
		// it splits the root off an incoming path.
		entries := make(fstest.MapFS, len(a.Files))
		for path, contents := range a.Files {
			entries[strings.TrimPrefix(path, "/")] = &fstest.MapFile{Data: []byte(contents)}
		}
		return iovfs.From(entries, caseSensitive)
	default:
		panic("phase1: unsupported inner: " + a.Inner)
	}
}

// phase1BuildMock binds the mock to the backing filesystem, logging each call
// in order, and leaves the Funcs named by silent_methods nil so that a call
// reaching them panics with the generated mock's own sentence.
func (s *phase1State) phase1BuildMock(a phase1Action) {
	silent := make(map[string]bool, len(a.SilentMethods))
	for _, name := range a.SilentMethods {
		if !slices.Contains(phase1MethodNames, name) {
			panic("phase1: unsupported silent method: " + name)
		}
		silent[name] = true
	}
	inner := phase1BuildBacking(a)
	mock := &vfsmock.FSMock{}
	s.mock = mock
	bind := func(name string, install func()) {
		if !silent[name] {
			install()
		}
	}
	bind("UseCaseSensitiveFileNames", func() {
		mock.UseCaseSensitiveFileNamesFunc = func() bool {
			s.noteInner("UseCaseSensitiveFileNames")
			return inner.UseCaseSensitiveFileNames()
		}
	})
	bind("FileExists", func() {
		mock.FileExistsFunc = func(path string) bool {
			s.noteInner("FileExists", path)
			return inner.FileExists(path)
		}
	})
	bind("ReadFile", func() {
		mock.ReadFileFunc = func(path string) (string, bool) {
			s.noteInner("ReadFile", path)
			return inner.ReadFile(path)
		}
	})
	bind("WriteFile", func() {
		mock.WriteFileFunc = func(path string, data string) error {
			s.noteInner("WriteFile", path, data)
			return inner.WriteFile(path, data)
		}
	})
	bind("AppendFile", func() {
		mock.AppendFileFunc = func(path string, data string) error {
			s.noteInner("AppendFile", path, data)
			return inner.AppendFile(path, data)
		}
	})
	bind("Remove", func() {
		mock.RemoveFunc = func(path string) error {
			s.noteInner("Remove", path)
			return inner.Remove(path)
		}
	})
	bind("Chtimes", func() {
		mock.ChtimesFunc = func(path string, aTime time.Time, mTime time.Time) error {
			s.noteInner("Chtimes", path, phase1Instant(aTime), phase1Instant(mTime))
			return inner.Chtimes(path, aTime, mTime)
		}
	})
	bind("DirectoryExists", func() {
		mock.DirectoryExistsFunc = func(path string) bool {
			s.noteInner("DirectoryExists", path)
			return inner.DirectoryExists(path)
		}
	})
	bind("GetAccessibleEntries", func() {
		mock.GetAccessibleEntriesFunc = func(path string) vfs.Entries {
			s.noteInner("GetAccessibleEntries", path)
			return inner.GetAccessibleEntries(path)
		}
	})
	bind("Stat", func() {
		mock.StatFunc = func(path string) vfs.FileInfo {
			s.noteInner("Stat", path)
			return inner.Stat(path)
		}
	})
	bind("WalkDir", func() {
		mock.WalkDirFunc = func(root string, walkFn vfs.WalkDirFunc) error {
			s.noteInner("WalkDir", root)
			return inner.WalkDir(root, walkFn)
		}
	})
	bind("Realpath", func() {
		mock.RealpathFunc = func(path string) string {
			s.noteInner("Realpath", path)
			return inner.Realpath(path)
		}
	})
}

// phase1Fill installs the request's stub declarations into a Replacements
// value, leaving the fields no stub names alone. A method named twice, an
// unknown method and an unknown behavior are all harness failures.
//
// It fills in place rather than returning a fresh value so that
// mutate_replacements really mutates the caller's own variable: that is the
// only way to observe that Wrap took the table by value.
func (s *phase1State) phase1Fill(table *Replacements, stubs []phase1Stub) {
	installed := map[string]bool{}
	for _, stub := range stubs {
		if !slices.Contains(phase1MethodNames, stub.Method) {
			panic("phase1: unsupported replacement method: " + stub.Method)
		}
		if installed[stub.Method] {
			panic("phase1: replacement method declared twice: " + stub.Method)
		}
		installed[stub.Method] = true
		s.phase1Install(table, stub)
	}
}

func (s *phase1State) phase1Install(table *Replacements, stub phase1Stub) {
	// Each arm pairs one method with the behaviors that method accepts. A
	// behavior the method does not accept falls through to the panic, because
	// silently installing some other rule would let the case pass without
	// exercising what it claims to exercise.
	switch stub.Method {
	case "UseCaseSensitiveFileNames":
		if stub.Behavior == "constant_bool" {
			answer := phase1Require(stub.Flag, "flag", "replace:UseCaseSensitiveFileNames")
			table.UseCaseSensitiveFileNames = func() bool {
				s.noteStub("UseCaseSensitiveFileNames")
				return answer
			}
			return
		}
	case "FileExists":
		if stub.Behavior == "true_for" {
			paths := phase1RequireSlice(stub.Paths, "paths", "replace:FileExists")
			table.FileExists = func(path string) bool {
				s.noteStub("FileExists", path)
				return slices.Contains(paths, path)
			}
			return
		}
	case "DirectoryExists":
		if stub.Behavior == "true_for" {
			paths := phase1RequireSlice(stub.Paths, "paths", "replace:DirectoryExists")
			table.DirectoryExists = func(path string) bool {
				s.noteStub("DirectoryExists", path)
				return slices.Contains(paths, path)
			}
			return
		}
	case "ReadFile":
		if stub.Behavior == "read_table" {
			rows := phase1RequireSlice(stub.Reads, "reads", "replace:ReadFile")
			table.ReadFile = func(path string) (string, bool) {
				s.noteStub("ReadFile", path)
				for _, row := range rows {
					if row.Path == path {
						return row.Contents, phase1Require(row.Ok, "ok", "replace:ReadFile")
					}
				}
				return "", false
			}
			return
		}
	case "WriteFile":
		if err, ok := phase1StubError(stub); ok {
			table.WriteFile = func(path string, data string) error {
				s.noteStub("WriteFile", path, data)
				return err
			}
			return
		}
	case "AppendFile":
		if err, ok := phase1StubError(stub); ok {
			table.AppendFile = func(path string, data string) error {
				s.noteStub("AppendFile", path, data)
				return err
			}
			return
		}
	case "Remove":
		if err, ok := phase1StubError(stub); ok {
			table.Remove = func(path string) error {
				s.noteStub("Remove", path)
				return err
			}
			return
		}
	case "Chtimes":
		if err, ok := phase1StubError(stub); ok {
			table.Chtimes = func(path string, aTime time.Time, mTime time.Time) error {
				s.noteStub("Chtimes", path, phase1Instant(aTime), phase1Instant(mTime))
				return err
			}
			return
		}
	case "GetAccessibleEntries":
		if stub.Behavior == "constant_entries" {
			const where = "replace:GetAccessibleEntries"
			answer := vfs.Entries{
				Files:       phase1RequireSlice(stub.Files, "files", where),
				Directories: phase1RequireSlice(stub.Directories, "directories", where),
			}
			// `symlinks` is required even when `nil_symlinks` is true, so that a
			// declaration always says what it answers: the nil map and the empty
			// map mean different things to the pinned interface, and the case
			// that wants the nil one still has to spell the empty list out.
			names := phase1RequireSlice(stub.Symlinks, "symlinks", where)
			if !phase1Require(stub.NilSymlinks, "nil_symlinks", where) {
				answer.Symlinks = map[string]struct{}{}
				for _, name := range names {
					answer.Symlinks[name] = struct{}{}
				}
			}
			table.GetAccessibleEntries = func(path string) vfs.Entries {
				s.noteStub("GetAccessibleEntries", path)
				return answer
			}
			return
		}
	case "Stat":
		switch stub.Behavior {
		case "constant_info":
			answer := phase1FileInfo{
				name:    phase1Require(stub.Name, "name", "replace:Stat"),
				size:    phase1Require(stub.Size, "size", "replace:Stat"),
				mode:    phase1Mode(stub),
				modTime: phase1RequireTime(stub.ModTime, "mod_time", "replace:Stat"),
			}
			table.Stat = func(path string) vfs.FileInfo {
				s.noteStub("Stat", path)
				return answer
			}
			return
		case "nil_info":
			table.Stat = func(path string) vfs.FileInfo {
				s.noteStub("Stat", path)
				return nil
			}
			return
		}
	case "WalkDir":
		switch stub.Behavior {
		case "replay_arrivals", "replay_arrivals_then_fail":
			arrivals := phase1RequireSlice(stub.Arrivals, "arrivals", "replace:WalkDir")
			failing := stub.Behavior == "replay_arrivals_then_fail"
			table.WalkDir = func(root string, walkFn vfs.WalkDirFunc) error {
				s.noteStub("WalkDir", root)
				// Deliberately unconditional: the stub replays every arrival
				// whatever the callback returns, so the trace records the
				// sentinel the caller's own walkFn produced for each one and a
				// port that substituted its own closure is visible here.
				for _, arrival := range arrivals {
					entry := phase1DirEntry{info: phase1FileInfo{
						name: arrival.Name,
						mode: phase1ArrivalMode(arrival),
					}}
					returned := walkFn(arrival.Path, entry, nil)
					s.noteStub("WalkDirCallbackReturned", arrival.Path, phase1ErrClass(returned))
				}
				if failing {
					return errPhase1Stub
				}
				return nil
			}
			return
		}
	case "Realpath":
		if stub.Behavior == "constant_text" {
			answer := phase1Require(stub.Text, "text", "replace:Realpath")
			table.Realpath = func(path string) string {
				s.noteStub("Realpath", path)
				return answer
			}
			return
		}
	}
	panic("phase1: unsupported replacement behavior " + stub.Behavior + " for method " + stub.Method)
}

// phase1StubError is the shared shape of the four error-returning replacements.
func phase1StubError(stub phase1Stub) (error, bool) {
	switch stub.Behavior {
	case "record_and_succeed":
		return nil, true
	case "record_and_fail":
		return errPhase1Stub, true
	default:
		return nil, false
	}
}

func phase1Mode(stub phase1Stub) fs.FileMode {
	mode := fs.FileMode(phase1Require(stub.Perm, "perm", "replace:Stat"))
	if phase1Require(stub.Dir, "dir", "replace:Stat") {
		mode |= fs.ModeDir
	}
	return mode
}

func phase1ArrivalMode(arrival phase1Arrival) fs.FileMode {
	if phase1Require(arrival.IsDir, "is_dir", "replace:WalkDir arrival") {
		return fs.ModeDir | 0o755
	}
	return 0o644
}

func phase1Instant(t time.Time) string {
	return t.UTC().Format(time.RFC3339Nano)
}

// phase1ErrClass reduces an error to something a port could reproduce. The
// stdlib sentinels travel as names, the two probe sentinels as their identity,
// and a pinned-source message travels literally because that text is the
// contract.
func phase1ErrClass(err error) string {
	switch {
	case err == nil:
		return ""
	case errors.Is(err, errPhase1Stub):
		return "probe_stub_sentinel"
	case errors.Is(err, errPhase1Callback):
		return "probe_callback_sentinel"
	case errors.Is(err, fs.SkipAll):
		return "skip_all"
	case errors.Is(err, fs.SkipDir):
		return "skip_dir"
	case errors.Is(err, fs.ErrNotExist):
		return "not_exist"
	case errors.Is(err, fs.ErrExist):
		return "exist"
	case errors.Is(err, fs.ErrPermission):
		return "permission"
	case errors.Is(err, fs.ErrInvalid):
		return "invalid"
	default:
		return "other:" + err.Error()
	}
}

func phase1Classify(r any) string {
	var text string
	switch value := r.(type) {
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		text = "non-error panic"
	}
	switch {
	case strings.Contains(text, "index out of range"),
		strings.Contains(text, "slice bounds out of range"):
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	case strings.Contains(text, "interface conversion"):
		return "interface_conversion"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract: iofs.go's "writeFile not supported", the
		// generated mock's nil-Func sentence, internal.go's
		// "vfs: path %q is not absolute".
		return "other:" + text
	}
}

func phase1Guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

// phase1RenderEntries renders vfs.Entries as [files, directories, symlinks].
// Files and Directories keep the inner's own order, because one case exists to
// show the wrapper does not reorder them. Symlinks is a map, so it is sorted;
// a nil map is rendered as the string "nil", since the pinned interface gives
// nil a meaning distinct from empty.
func phase1RenderEntries(entries vfs.Entries) []any {
	files := []any{}
	for _, name := range entries.Files {
		files = append(files, name)
	}
	directories := []any{}
	for _, name := range entries.Directories {
		directories = append(directories, name)
	}
	var symlinks any = "nil"
	if entries.Symlinks != nil {
		names := make([]string, 0, len(entries.Symlinks))
		for name := range entries.Symlinks {
			names = append(names, name)
		}
		slices.Sort(names)
		rendered := []any{}
		for _, name := range names {
			rendered = append(rendered, name)
		}
		symlinks = rendered
	}
	return []any{files, directories, symlinks}
}

// phase1RenderInfo renders a vfs.FileInfo as an array, never a nested object.
// Sys is reduced to a presence flag: its value is a pointer into the inner's
// own bookkeeping and no port could reproduce it.
func phase1RenderInfo(info vfs.FileInfo) any {
	if info == nil {
		return "nil"
	}
	return []any{
		info.Name(),
		info.Size(),
		info.IsDir(),
		info.Mode().String(),
		phase1Instant(info.ModTime()),
		info.Sys() != nil,
	}
}

func (s *phase1State) phase1Seen() ([]any, int) {
	names := s.tracker.SeenFiles.ToSlice()
	slices.Sort(names)
	out := []any{}
	for _, name := range names {
		out = append(out, name)
	}
	return out, s.tracker.SeenFiles.Size()
}

// phase1Callback builds the walk callback a walk_dir action hands the subject.
func phase1Callback(a phase1Action) func(string) error {
	switch a.Callback {
	case "collect":
		if a.At != "" {
			panic("phase1: the collect callback takes no at")
		}
		return func(string) error { return nil }
	case "skip_dir_at", "skip_all_at", "error_at":
		if a.At == "" {
			panic("phase1: action walk_dir is missing required key at")
		}
		at, kind := a.At, a.Callback
		return func(path string) error {
			if path != at {
				return nil
			}
			switch kind {
			case "skip_dir_at":
				return fs.SkipDir
			case "skip_all_at":
				return fs.SkipAll
			default:
				return errPhase1Callback
			}
		}
	default:
		panic("phase1: unsupported walk callback: " + a.Callback)
	}
}

func phase1DecodeActions(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		// Decoding happens before any guarded production call. A malformed
		// request must fail the probe, never turn into an empty observation.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

//nolint:gocyclo // One switch, one arm per action; splitting it would hide the vocabulary.
func phase1Replay(request phase1Request) []any {
	state := &phase1State{subject: request.Subject}
	ordered := []any{}
	for _, a := range phase1DecodeActions(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_tracking", "wrap":
		default:
			// Outside every guard: a trace that acts before it constructs is a
			// malformed request, not an observation.
			if state.wrapper == nil {
				panic("phase1: action " + a.Op + " before a constructor")
			}
		}
		switch a.Op {
		case "new_tracking":
			if state.subject != "trackingvfs.FS" {
				panic("phase1: new_tracking is only legal for the trackingvfs.FS subject")
			}
			if a.Replace != nil {
				panic("phase1: trackingvfs has no replacement table")
			}
			state.phase1BuildMock(a)
			state.tracker = &trackingvfs.FS{Inner: state.mock}
			state.wrapper = state.tracker
			row["inner"] = a.Inner
		case "wrap":
			if state.subject != "wrapvfs.Wrap" {
				panic("phase1: wrap is only legal for the wrapvfs.Wrap subject")
			}
			if a.Replace == nil {
				panic("phase1: action wrap is missing required key replace")
			}
			state.phase1BuildMock(a)
			state.table = Replacements{}
			state.phase1Fill(&state.table, *a.Replace)
			state.wrapper = Wrap(state.mock, state.table)
			names := []any{}
			for _, stub := range *a.Replace {
				names = append(names, stub.Method)
			}
			row["inner"] = a.Inner
			row["replaced"] = names
		case "mutate_replacements":
			// Wrap takes Replacements by value. Rewriting the caller's own
			// variable afterwards must not reach the wrapper.
			if state.wrapper == nil {
				panic("phase1: mutate_replacements before a constructor")
			}
			if a.Replace == nil {
				panic("phase1: action mutate_replacements is missing required key replace")
			}
			state.phase1Fill(&state.table, *a.Replace)
			names := []any{}
			for _, stub := range *a.Replace {
				names = append(names, stub.Method)
			}
			row["replaced"] = names
		case "use_case_sensitive_file_names":
			value, panicked := phase1Guarded(func() any {
				return state.subjectFS().UseCaseSensitiveFileNames()
			})
			row["result"], row["panic"] = value, panicked
		case "file_exists":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any { return state.subjectFS().FileExists(path) })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "directory_exists":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any {
				return state.subjectFS().DirectoryExists(path)
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "read_file":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any {
				contents, ok := state.subjectFS().ReadFile(path)
				return []any{contents, ok}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "write_file":
			path := phase1RequirePath(a.Path, a.Op)
			data := phase1Require(a.Data, "data", a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1ErrClass(state.subjectFS().WriteFile(path, data))
			})
			row["path"], row["data"], row["error"], row["panic"] = path, data, value, panicked
		case "append_file":
			path := phase1RequirePath(a.Path, a.Op)
			data := phase1Require(a.Data, "data", a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1ErrClass(state.subjectFS().AppendFile(path, data))
			})
			row["path"], row["data"], row["error"], row["panic"] = path, data, value, panicked
		case "remove":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1ErrClass(state.subjectFS().Remove(path))
			})
			row["path"], row["error"], row["panic"] = path, value, panicked
		case "chtimes":
			path := phase1RequirePath(a.Path, a.Op)
			aTime := phase1RequireTime(a.ATime, "a_time", a.Op)
			mTime := phase1RequireTime(a.MTime, "m_time", a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1ErrClass(state.subjectFS().Chtimes(path, aTime, mTime))
			})
			row["path"] = path
			row["times"] = []any{phase1Instant(aTime), phase1Instant(mTime)}
			row["error"], row["panic"] = value, panicked
		case "entries":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1RenderEntries(state.subjectFS().GetAccessibleEntries(path))
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "stat":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any {
				return phase1RenderInfo(state.subjectFS().Stat(path))
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "realpath":
			path := phase1RequirePath(a.Path, a.Op)
			value, panicked := phase1Guarded(func() any { return state.subjectFS().Realpath(path) })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "walk_dir":
			path := phase1RequirePath(a.Path, a.Op)
			decide := phase1Callback(a)
			arrivals := []any{}
			value, panicked := phase1Guarded(func() any {
				return phase1ErrClass(state.subjectFS().WalkDir(path, func(p string, d vfs.DirEntry, err error) error {
					name, isDir := "", false
					if d != nil {
						name, isDir = d.Name(), d.IsDir()
					}
					returned := decide(p)
					arrivals = append(arrivals, []any{
						p, name, isDir, phase1ErrClass(err), phase1ErrClass(returned),
					})
					return returned
				}))
			})
			row["path"], row["callback"] = path, a.Callback
			row["arrivals"] = arrivals
			row["error"], row["panic"] = value, panicked
		default:
			// Never an observation: an unknown action is a harness failure on
			// both sides, and a row both sides could agree on without running
			// anything would be worse than no row at all.
			panic("phase1: unsupported action: " + a.Op)
		}
		row["inner_calls"] = state.drainInner()
		if state.subject == "wrapvfs.Wrap" {
			row["stub_calls"] = state.drainStubs()
		}
		if state.tracker != nil {
			seen, size := state.phase1Seen()
			row["seen"], row["seen_size"] = seen, size
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1FilesystemWrapvfs(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch request.Subject {
		case "trackingvfs.FS", "wrapvfs.Wrap":
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = fmt.Sprintf(
				"subject %q is not served by the wrapvfs probe", request.Subject)
		}
		observations = append(observations, row)
	}

	hash := sha256.Sum256(input)
	output := map[string]any{
		"request_sha256": hex.EncodeToString(hash[:]),
		"go":             runtime.Version(),
		"goos":           runtime.GOOS,
		"goarch":         runtime.GOARCH,
		"version":        1,
		"observations":   observations,
	}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
