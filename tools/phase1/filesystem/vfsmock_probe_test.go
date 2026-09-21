package vfsmock

// Access only: replays an ordered action trace against the pinned FSMock and
// records what each action observed.
//
// The subject of this group is not a filesystem. FSMock is the generated
// recording wrapper (moq, see the go:generate line in vfs.go), and what it
// contributes is a *call log*: for every method of vfs.FS it appends the
// caller's arguments to a private slice and then forwards to a Func field.
// Wrap fills those twelve fields from a real vfs.FS, so a wrapped filesystem
// behaves exactly like the one it wraps while recording every request.
//
// So the observations here are about forwarding, not about files. What each
// case pins is which calls reached the wrapper, in what order, with which
// arguments, and whether the log entry survives a call that failed. The inner
// filesystem is vfstest.FromMap -- an in-memory fstest.MapFS behind iovfs --
// and it is a substrate, not a subject: no case here witnesses a vfstest or
// iovfs operation, and every case is host independent because nothing touches
// a real filesystem.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
//
// Four rules keep an observation portable.
//
// An error is reduced to a class. The pinned inner filesystem spells its
// failures with fmt.Errorf and the text names a path and a Go verb ("mkdir
// %q: path exists but is not a directory"); that wording belongs to vfstest,
// not to the wrapper this group is about, so only ok/not_exist/error crosses
// the boundary.
//
// Every panic is reduced to a class, including the one the pinned generated
// source raises itself. mock_generated.go:189 spells "FSMock.AppendFileFunc:
// method is nil but FS.AppendFile was just called", and that sentence names
// three Go identifiers over a per-method function field -- a shape the Rust
// surface this family asks for (a wrapper taking the delegate whole) does not
// have and so has no "method is nil" state to report. A port could only
// answer the sentence by copying it out of the frozen row, so it is recorded
// as `unwired_method` and the literal stays in the case's prose, where it is
// documentation rather than an expected value. A panic raised by a delegate
// this probe supplies is a class for a different reason -- it is this file's
// wording, not the pin's -- and a Go runtime panic is a class for the usual
// one, that its text names dynamic types and moves with the toolchain.
//
// Anything derived from a Go map is sorted. vfs.Entries.Symlinks is a map, so
// its names are sorted before they are recorded; Files and Directories are
// left in the order the pin produced them, which is fs.ReadDir's documented
// filename order and is part of what these cases pin.
//
// A wall clock never reaches an observation. The instants a chtimes action
// sends come from the request, so the log can be recorded verbatim; a
// filesystem's resulting modification time is recorded as a comparison
// against those two instants, never as a timestamp.
//
// One aliasing hazard is worth naming. Every XxxCalls method returns the
// mock's own backing slice rather than a copy (mock_generated.go:216-219 and
// its siblings), so a slice held across a later call may or may not observe
// the append depending on capacity growth. Every read here renders the log
// into fresh values inside the action that read it and keeps nothing.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io/fs"
	"os"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// Names declared here are prefixed, because this is an in-package test file
// and `package vfsmock` is generated: an unprefixed `action` or `state` could
// collide with a later regeneration.

// phase1Sentinel is the path the callback probe sends through a recorded
// WalkDirFunc. It is not a path any walk can produce, so a recorder that sees
// it knows the call came from the probe and not from the walk.
//
// It establishes reachability, not identity. A wrapper that wraps the
// caller's callback and forwards (path, entry, err) unchanged reaches the
// same recorder with the same arguments, and nothing here -- nothing that
// does not serialise a function value -- can tell that apart from the
// caller's own function. What the probe does catch is a recorded callback
// that never reaches the caller's, one handed rewritten arguments (a
// rewritten path misses this branch entirely and lands in the walk trace
// instead, leaving probed false), and one whose answer is swallowed on the
// way back.
const phase1Sentinel = "\x00phase1-walk-sentinel"

// phase1Delegate is what a deliberately failing delegate panics with. It is
// this file's wording, so it is reduced to a class rather than recorded.
const phase1Delegate = "phase1: delegate panicked on purpose"

type phase1Entry struct {
	Path string `json:"path"`
	Kind string `json:"kind"`
	// Pointers, because "" is a legal file body and a request that omits the
	// key must fail rather than be read as an empty file.
	Content *string `json:"content"`
	Target  *string `json:"target"`
}

type phase1Action struct {
	Op string `json:"op"`
	// Every payload field is a pointer. An absent key is a malformed request
	// and must fail the probe: a field that defaulted would let both sides
	// agree on a row neither of them actually executed.
	Path          *string        `json:"path"`
	Data          *string        `json:"data"`
	Root          *string        `json:"root"`
	ATime         *string        `json:"atime"`
	MTime         *string        `json:"mtime"`
	CaseSensitive *bool          `json:"case_sensitive"`
	Files         *[]phase1Entry `json:"files"`
	FilesOther    *[]phase1Entry `json:"files_other"`
	StopKind      *string        `json:"stop_kind"`
	StopAt        *string        `json:"stop_at"`
}

func (a phase1Action) needPath() string {
	if a.Path == nil {
		panic("phase1: action " + a.Op + " requires `path`")
	}
	return *a.Path
}

func (a phase1Action) needData() string {
	if a.Data == nil {
		panic("phase1: action " + a.Op + " requires `data`")
	}
	return *a.Data
}

func (a phase1Action) needRoot() string {
	if a.Root == nil {
		panic("phase1: action " + a.Op + " requires `root`")
	}
	return *a.Root
}

func (a phase1Action) needTime(field string, raw *string) time.Time {
	if raw == nil {
		panic("phase1: action " + a.Op + " requires `" + field + "`")
	}
	parsed, err := time.Parse(time.RFC3339Nano, *raw)
	if err != nil {
		panic("phase1: action " + a.Op + " has an unparseable `" + field + "`: " + err.Error())
	}
	return parsed
}

func (a phase1Action) needCaseSensitive() bool {
	if a.CaseSensitive == nil {
		panic("phase1: action " + a.Op + " requires `case_sensitive`")
	}
	return *a.CaseSensitive
}

func (a phase1Action) needFiles(field string, raw *[]phase1Entry) []phase1Entry {
	if raw == nil {
		panic("phase1: action " + a.Op + " requires `" + field + "`")
	}
	return *raw
}

func (a phase1Action) needStop() (string, string) {
	if a.StopKind == nil {
		panic("phase1: action " + a.Op + " requires `stop_kind`")
	}
	if a.StopAt == nil {
		panic("phase1: action " + a.Op + " requires `stop_at`")
	}
	switch *a.StopKind {
	case "none", "skip_dir", "skip_all":
	default:
		panic("phase1: unsupported stop kind: " + *a.StopKind)
	}
	return *a.StopKind, *a.StopAt
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

func phase1Decode(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1ErrClass reduces an error to something a port could reproduce. The
// pinned inner filesystem's messages name paths and Go verbs, so only the
// distinction the vfs.FS contract makes -- succeeded, nothing there, refused
// -- crosses the boundary.
func phase1ErrClass(err error) string {
	switch {
	case err == nil:
		return "ok"
	case phase1IsNotExist(err):
		return "not_exist"
	default:
		return "error"
	}
}

func phase1IsNotExist(err error) bool {
	// vfs re-exports fs.ErrNotExist; the pinned MapFS returns it directly for
	// a chtimes on a missing path (vfstest.go:613) and wraps it with %w for a
	// write whose parent is gone (vfstest.go:520).
	for e := err; e != nil; {
		if e == fs.ErrNotExist {
			return true
		}
		unwrapped, ok := e.(interface{ Unwrap() error })
		if !ok {
			return false
		}
		e = unwrapped.Unwrap()
	}
	return false
}

// phase1Classify renders a recovered panic as a class. Nothing here is
// recorded verbatim, the pin's own sentence included: see the note on panics
// in the file header.
func phase1Classify(recovered any) string {
	text := ""
	switch value := recovered.(type) {
	case nil:
		return ""
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		return "non_error_panic"
	}
	switch {
	case text == phase1Delegate:
		return "delegate_panicked"
	case strings.HasPrefix(text, "FSMock.") && strings.Contains(text, ": method is nil but FS."):
		// mock_generated.go:189 raises this itself, but the sentence names Go
		// identifiers of a per-method function field no port has. The class
		// crosses the boundary; the literal stays in the case's prose.
		return "unwired_method"
	case strings.Contains(text, "index out of range"),
		strings.Contains(text, "slice bounds out of range"):
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	default:
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

// phase1Build makes the inner filesystem an action wraps. It is in-memory, so
// every case in this group is host independent.
func phase1Build(entries []phase1Entry, caseSensitive bool) vfs.FS {
	files := make(map[string]*fstest.MapFile, len(entries))
	for _, entry := range entries {
		if entry.Path == "" {
			panic("phase1: a file entry requires `path`")
		}
		switch entry.Kind {
		case "file":
			if entry.Content == nil {
				panic("phase1: file entry " + entry.Path + " requires `content`")
			}
			files[entry.Path] = &fstest.MapFile{Data: []byte(*entry.Content)}
		case "symlink":
			if entry.Target == nil {
				panic("phase1: symlink entry " + entry.Path + " requires `target`")
			}
			files[entry.Path] = vfstest.Symlink(*entry.Target)
		default:
			panic("phase1: unsupported file kind: " + entry.Kind)
		}
	}
	return vfstest.FromMap(files, caseSensitive)
}

// phase1Fields is the completeness observation the pinned TestWrap asserts,
// turned into portable data: how many of the mock's recorded delegates are
// wired and how many are not.
//
// It counts rather than names. The names are the generated `AppendFileFunc`,
// `ChtimesFunc`, ... fields of a Go struct; a Rust recorder has no such
// fields, so a list of them would be a row no port could answer except by
// writing the Go identifiers into Rust. The count is language neutral: a port
// answers it about its own trait's members.
func phase1Fields(mock *FSMock) (wired, unwired int) {
	value := reflect.ValueOf(mock).Elem()
	kind := value.Type()
	for i := range kind.NumField() {
		field := kind.Field(i)
		if !field.IsExported() {
			continue
		}
		if value.Field(i).IsZero() {
			unwired++
		} else {
			wired++
		}
	}
	return wired, unwired
}

func phase1Strings(in []string) []string {
	if in == nil {
		return []string{}
	}
	return in
}

// phase1Entries renders vfs.Entries. Files and Directories keep the pin's own
// order, which fs.ReadDir documents as sorted by filename; Symlinks is a Go
// map, so its names are sorted. The nil-valued members are named separately:
// the pinned type distinguishes a nil member from an empty one, and this
// group's whole business is what the wrapper's caller can see.
func phase1Entries(entries vfs.Entries) []any {
	absent := []string{}
	if entries.Files == nil {
		absent = append(absent, "files")
	}
	if entries.Directories == nil {
		absent = append(absent, "directories")
	}
	if entries.Symlinks == nil {
		absent = append(absent, "symlinks")
	}
	symlinks := []string{}
	for name := range entries.Symlinks {
		symlinks = append(symlinks, name)
	}
	sort.Strings(symlinks)
	return []any{phase1Strings(entries.Files), phase1Strings(entries.Directories), symlinks, absent}
}

// phase1Stat records which FileInfo members survive the wrapper. ModTime and
// Sys are recorded as presence rather than value: one is a wall clock and the
// other is an untyped Go escape hatch, and the capability-matrix question is
// whether the member is carried at all.
func phase1Stat(info vfs.FileInfo) []any {
	if info == nil {
		return []any{false}
	}
	return []any{
		true,
		info.Name(),
		info.Size(),
		info.IsDir(),
		info.ModTime().IsZero(),
		info.Sys() != nil,
		info.Mode().String(),
	}
}

type phase1Walk struct {
	stopKind string
	stopAt   string
	trace    []any
	probed   bool
	// What the sentinel invocation handed this recorder, so a wrapper that
	// rewrote or dropped the walk's arguments on the way through is visible.
	handed []any
}

// phase1WalkReturn classifies what a recorded WalkDirFunc handed back. The
// three values are the pinned callback protocol's own control signals, not a
// message, so they cross the language boundary.
func phase1WalkReturn(err error) string {
	switch {
	case err == nil:
		return "ok"
	case err == vfs.SkipDir:
		return "skip_dir"
	case err == vfs.SkipAll:
		return "skip_all"
	default:
		return "error"
	}
}

func (w *phase1Walk) callback(path string, entry fs.DirEntry, err error) error {
	if path == phase1Sentinel {
		// Not a walk step: the callback probe invoking the recorded function.
		// Record what arrived, and answer SkipAll rather than nil so a
		// wrapper that swallowed the caller's answer reads back as "ok".
		w.probed = true
		w.handed = []any{entry == nil, phase1ErrClass(err)}
		return vfs.SkipAll
	}
	// entry is nil when the walk reports an error for the root itself, so it
	// is never dereferenced without the guard.
	w.trace = append(w.trace, []any{path, entry != nil, entry != nil && entry.IsDir(), phase1ErrClass(err)})
	if path == w.stopAt {
		switch w.stopKind {
		case "skip_dir":
			return vfs.SkipDir
		case "skip_all":
			return vfs.SkipAll
		}
	}
	return nil
}

type phase1State struct {
	inner vfs.FS
	mock  *FSMock
	// One record per walk_dir action, aligned with WalkDirCalls by position.
	walks []*phase1Walk
}

func (s *phase1State) need() *FSMock {
	if s.mock == nil {
		panic("phase1: the trace used the wrapper before a `wrap` action built it")
	}
	return s.mock
}

func (s *phase1State) needInner() vfs.FS {
	if s.inner == nil {
		panic("phase1: the trace used the inner filesystem before a `wrap` action built it")
	}
	return s.inner
}

// phase1Names lists the inner filesystem's paths. It never goes through the
// wrapper: it is the effect view, used where a case has to show that one
// logged call moved several entries.
func phase1Names(inner vfs.FS, root string) []string {
	found := []string{}
	_ = inner.WalkDir(root, func(path string, _ fs.DirEntry, _ error) error {
		found = append(found, path)
		return nil
	})
	sort.Strings(found)
	return found
}

//nolint:gocyclo // One switch, one arm per action; splitting it would hide the vocabulary.
func phase1Replay(request phase1Request) []any {
	state := &phase1State{}
	ordered := []any{}
	for _, a := range phase1Decode(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "wrap":
			caseSensitive := a.needCaseSensitive()
			state.inner = phase1Build(a.needFiles("files", a.Files), caseSensitive)
			state.mock = Wrap(state.inner)
			state.walks = nil
			wired, unwired := phase1Fields(state.mock)
			row["case_sensitive"] = caseSensitive
			row["wired_count"] = wired
			row["unwired_count"] = unwired
			row["every_member_wired"] = unwired == 0

		case "zero_mock_fields":
			// The contrast that makes the completeness assertion mean
			// something: a mock nobody wired has no recorded delegate at all,
			// so `every_member_wired` is produced by something that can also
			// answer false.
			wired, unwired := phase1Fields(&FSMock{})
			row["wired_count"] = wired
			row["unwired_count"] = unwired
			row["every_member_wired"] = unwired == 0

		case "interface_methods":
			// Wrap has to cover vfs.FS exactly. The member count is the
			// portable form of that assertion -- a port answers it about its
			// own trait -- and recording it beside the live wrapper's wired
			// count makes a member that grew without a matching recorded
			// delegate visible as data.
			kind := reflect.TypeOf((*vfs.FS)(nil)).Elem()
			wired, unwired := phase1Fields(state.need())
			row["count"] = kind.NumMethod()
			row["wired_count"] = wired
			row["unwired_count"] = unwired
			row["every_member_wired"] = unwired == 0 && wired == kind.NumMethod()

		case "rebind_source":
			// Wrap captures bound method values, not the variable it was
			// handed. Reassigning the source cannot redirect the wrapper.
			// This action replaces the trace's current filesystem with A.
			first := phase1Build(a.needFiles("files", a.Files), a.needCaseSensitive())
			second := phase1Build(a.needFiles("files_other", a.FilesOther), a.needCaseSensitive())
			source := first
			state.inner = first
			state.mock = Wrap(source)
			state.walks = nil
			source = second
			_ = source
			contents, ok := state.mock.ReadFile(a.needPath())
			row["result"] = []any{contents, ok}

		case "mutate_source":
			// The other half: the delegate is live, so a change made to the
			// wrapped filesystem behind the wrapper's back is visible through
			// it. The write goes to the inner filesystem directly, so it adds
			// nothing to the wrapper's log.
			path := a.needPath()
			class := phase1ErrClass(state.needInner().WriteFile(path, a.needData()))
			contents, ok := state.need().ReadFile(path)
			row["result"] = []any{class, contents, ok}

		case "use_case_sensitive_file_names":
			row["result"] = state.need().UseCaseSensitiveFileNames()

		case "file_exists":
			path := a.needPath()
			row["path"] = path
			row["result"] = state.need().FileExists(path)

		case "directory_exists":
			path := a.needPath()
			row["path"] = path
			row["result"] = state.need().DirectoryExists(path)

		case "stat":
			path := a.needPath()
			row["path"] = path
			row["result"] = phase1Stat(state.need().Stat(path))

		case "read_file":
			path := a.needPath()
			contents, ok := state.need().ReadFile(path)
			row["path"] = path
			row["result"] = []any{contents, ok}

		case "realpath":
			path := a.needPath()
			row["path"] = path
			row["result"] = state.need().Realpath(path)

		case "get_accessible_entries":
			path := a.needPath()
			row["path"] = path
			row["result"] = phase1Entries(state.need().GetAccessibleEntries(path))

		case "write_file":
			path, data := a.needPath(), a.needData()
			row["path"] = path
			row["result"] = phase1ErrClass(state.need().WriteFile(path, data))

		case "append_file":
			path, data := a.needPath(), a.needData()
			row["path"] = path
			row["result"] = phase1ErrClass(state.need().AppendFile(path, data))

		case "remove":
			path := a.needPath()
			row["path"] = path
			row["result"] = phase1ErrClass(state.need().Remove(path))

		case "chtimes":
			path := a.needPath()
			row["path"] = path
			row["result"] = phase1ErrClass(state.need().Chtimes(
				path, a.needTime("atime", a.ATime), a.needTime("mtime", a.MTime)))

		case "walk_dir":
			stopKind, stopAt := a.needStop()
			record := &phase1Walk{stopKind: stopKind, stopAt: stopAt, trace: []any{}, handed: []any{}}
			state.walks = append(state.walks, record)
			err := state.need().WalkDir(a.needRoot(), record.callback)
			row["root"] = a.needRoot()
			row["returned"] = phase1ErrClass(err)
			row["trace"] = record.trace

		case "unconfigured_append_file":
			// The nil check precedes the append, so a method nobody wired
			// panics with nothing recorded.
			//
			// The payload is read before the guard, never inside it: a
			// request missing `path` or `data` must fail the probe, and a
			// needPath panic raised under phase1Guarded would be recovered
			// and reported as this row's observation instead.
			path, data := a.needPath(), a.needData()
			mock := &FSMock{}
			_, panicked := phase1Guarded(func() any {
				return mock.AppendFile(path, data)
			})
			row["panic"] = panicked
			row["log_length"] = len(mock.AppendFileCalls())

		case "panicking_append_file":
			// The append precedes the call, so a delegate that dies still
			// leaves its arguments in the log. The payload is read before the
			// guard for the reason above.
			path, data := a.needPath(), a.needData()
			mock := &FSMock{AppendFileFunc: func(string, string) error { panic(phase1Delegate) }}
			_, panicked := phase1Guarded(func() any {
				return mock.AppendFile(path, data)
			})
			row["panic"] = panicked
			row["log_length"] = len(mock.AppendFileCalls())
			logged := []any{}
			for _, call := range mock.AppendFileCalls() {
				logged = append(logged, []any{call.Path, call.Data})
			}
			row["log"] = logged

		case "read_append_file_calls":
			calls := state.need().AppendFileCalls()
			logged := []any{}
			for _, call := range calls {
				logged = append(logged, []any{call.Path, call.Data})
			}
			row["length"] = len(calls)
			row["log"] = logged

		case "read_chtimes_calls":
			calls := state.need().ChtimesCalls()
			logged := []any{}
			for _, call := range calls {
				// Both instants came from the request, so recording them is
				// not recording a clock.
				logged = append(logged, []any{
					call.Path,
					call.ATime.UTC().Format(time.RFC3339Nano),
					call.MTime.UTC().Format(time.RFC3339Nano),
				})
			}
			row["length"] = len(calls)
			row["log"] = logged

		case "read_directory_exists_calls":
			calls := state.need().DirectoryExistsCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_file_exists_calls":
			calls := state.need().FileExistsCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_get_accessible_entries_calls":
			calls := state.need().GetAccessibleEntriesCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_read_file_calls":
			calls := state.need().ReadFileCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_realpath_calls":
			calls := state.need().RealpathCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_remove_calls":
			calls := state.need().RemoveCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_stat_calls":
			calls := state.need().StatCalls()
			paths := []string{}
			for _, call := range calls {
				paths = append(paths, call.Path)
			}
			row["length"] = len(calls)
			row["log"] = paths

		case "read_use_case_sensitive_file_names_calls":
			// []struct{}: the count is the whole payload.
			row["length"] = len(state.need().UseCaseSensitiveFileNamesCalls())

		case "read_walk_dir_calls":
			// The log carries the callback the caller supplied. Serialising a
			// function value is meaningless across languages, so the recorded
			// function is invoked instead: with a sentinel path, a nil entry
			// and a not-exist error, and its answer is classified. That pins
			// reachability and pass-through -- did the caller's callback get
			// called, with the arguments the probe sent, and did its SkipAll
			// come back -- not identity; see the note on phase1Sentinel.
			calls := state.need().WalkDirCalls()
			if len(calls) != len(state.walks) {
				panic("phase1: the walk log and the trace's walk records disagree")
			}
			logged := []any{}
			for index, call := range calls {
				returned := phase1WalkReturn(call.WalkFn(phase1Sentinel, nil, fs.ErrNotExist))
				record := state.walks[index]
				logged = append(logged, []any{call.Root, record.probed, record.handed, returned})
			}
			row["length"] = len(calls)
			row["log"] = logged

		case "read_write_file_calls":
			calls := state.need().WriteFileCalls()
			logged := []any{}
			for _, call := range calls {
				logged = append(logged, []any{call.Path, call.Data})
			}
			row["length"] = len(calls)
			row["log"] = logged

		case "inner_read_file":
			// Effect view. Goes to the inner filesystem, so it adds nothing to
			// the wrapper's log, which is the point wherever it is used.
			path := a.needPath()
			contents, ok := state.needInner().ReadFile(path)
			row["path"] = path
			row["result"] = []any{contents, ok}

		case "inner_entry_names":
			root := a.needRoot()
			row["root"] = root
			row["result"] = phase1Names(state.needInner(), root)

		case "inner_modtime_class":
			// Never a timestamp: which of the two instants the request sent
			// actually landed on the file.
			path := a.needPath()
			aTime, mTime := a.needTime("atime", a.ATime), a.needTime("mtime", a.MTime)
			info := state.needInner().Stat(path)
			class := "missing"
			if info != nil {
				switch {
				case info.ModTime().Equal(mTime):
					class = "equals_mtime"
				case info.ModTime().Equal(aTime):
					class = "equals_atime"
				default:
					class = "equals_neither"
				}
			}
			row["path"] = path
			row["result"] = class

		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1FilesystemVfsmock(t *testing.T) {
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
		if request.Subject == "vfsmock.FSMock" {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the vfsmock probe"
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
