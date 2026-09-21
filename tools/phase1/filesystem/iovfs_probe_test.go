package iovfs_test

// Access only: replays an ordered action trace against the pinned io/fs-backed
// filesystem adapter and records what each action observed.
//
// It is an external test file because `vfstest` imports `iovfs`: a file in
// `package iovfs` that imported `vfstest` would be an import cycle in the test
// binary. Nothing in this group needs unexported access -- `From`, `FsWithSys`,
// `RealpathFS` and `WritableFS` are all exported, and `writeFileEnsuringDir` is
// reachable only through `WriteFile`/`AppendFile`, which is exactly how the
// production callers reach it.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
//
// The subject is the ADAPTER, not the backing. Five backing kinds exist here
// only to drive `From`'s two interface probes (iofs.go:45 RealpathFS, iofs.go:68
// WritableFS) into all four corners:
//
//	fstest_map    testing/fstest.MapFS       -- neither
//	realpath_only probe-local                -- RealpathFS only
//	writable_only probe-local spy            -- WritableFS only
//	spy_realpath  probe-local spy + Realpath -- both
//	vfstest_map   *vfstest.MapFS             -- both (vfstest.go:54-57), pinned semantics
//
// The two probe-local backings are test doubles on the far side of the adapter
// boundary, never a reimplementation of a pinned algorithm. `realpathOnlyFS`
// answers `Realpath(p)` with the marker "RP<p>", which records nothing about
// realpath resolution and everything about what the adapter HANDED the backing
// and what it did to the answer (iofs.go:46-56 strips a leading "/" and puts it
// back). `spyFS` records the arguments of every WritableFS call and succeeds or
// fails on a counter the REQUEST declares, which is the only way to observe
// `writeFileEnsuringDir`'s write / mkdirAll / write control flow (iofs.go:201-210)
// and the perms and path forms `From`'s closures pass (iofs.go:69-88). A refused
// write carries the ORDINAL of the attempt it refused, so the error the pin
// returns from its second call (iofs.go:209) is a different observation from
// the one its first call produced. Every behavioral claim still rests on the
// pinned `vfstest.MapFS`, whose semantics are themselves pinned source.
//
// Four rules keep an observation portable.
//
// A panic the pinned source raises itself is recorded as ["literal", text],
// because that text is the contract: `From`'s five capability panics
// (iofs.go:91-103) and `internal.RootLength`'s `vfs: path %q is not absolute`
// (internal.go:23). A panic whose text carries a Go stdlib error sentence is
// reduced to ["class", ...]: `RootFor`'s `vfs: failed to create sub file system
// for %q: %v` (iofs.go:120) interpolates `fs.Sub`'s own *PathError, so only the
// pinned prefix and the pinned %q survive. A Go runtime panic is reduced to its
// shape alone.
//
// An error is reduced to a class the same way. Pinned `fmt.Errorf` shapes
// (vfstest.go:524 "parent path exists but is not a directory", :536 "path
// exists but is not a regular file", :333 "path exists but is not a directory",
// :253 "broken symlink") become their own class; everything else falls back to
// the wrapped `io/fs` sentinel; anything left over is recorded verbatim so a
// new pinned message cannot hide inside a bucket.
//
// File contents travel as hex, never as a JSON string. `ReadFile`'s passthrough
// branch (internal.go:185) returns the raw bytes, which need not be valid
// UTF-8, and Go's encoding/json would silently replace them with U+FFFD.
//
// `GetAccessibleEntries.Symlinks` is a Go map, so its names are sorted before
// being recorded; whether it was nil is recorded separately, because nil and
// empty are different answers (vfs.go:56-60) and only the nil-ness is contract.
// Times come from an injected clock (vfstest.go:80) so no observation is a
// wall-clock reading, and every result is an array: canonicalisation sorts
// object keys, and order is the subject of most of these cases.

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
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// ---------------------------------------------------------------------------
// Request decoding
// ---------------------------------------------------------------------------

type iovfsRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// Every field is a pointer so an absent key is distinguishable from a zero
// value. An action that needs a key it was not given is a harness failure, not
// an observation of a default: a defaulted action would let both sides agree
// without either having executed the thing the case is about.
type iovfsAction struct {
	Op string `json:"op"`

	Backing       *string     `json:"backing"`
	CaseSensitive *bool       `json:"case_sensitive"`
	Files         *[][]string `json:"files"`
	FailWrites    *int        `json:"fail_writes"`
	FailMkdir     *bool       `json:"fail_mkdir"`

	Path    *string `json:"path"`
	Content *string `json:"content"`
	Root    *string `json:"root"`
	// Control is an ordered list of [path, decision] pairs a walk callback
	// consults. The decision travels in the request so the trace says what the
	// callback returned; a rule that lived only in this file would leave a Rust
	// port reading the request with no way to know it.
	Control *[][]string `json:"control"`
	ATime   *string     `json:"atime"`
	MTime   *string     `json:"mtime"`
}

func decodeIovfsActions(raw json.RawMessage) []iovfsAction {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []iovfsAction
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

func needString(value *string, op string, key string) string {
	if value == nil {
		panic("phase1: action " + op + " requires " + key)
	}
	return *value
}

func needBool(value *bool, op string, key string) bool {
	if value == nil {
		panic("phase1: action " + op + " requires " + key)
	}
	return *value
}

func needFiles(value *[][]string, op string) [][]string {
	if value == nil {
		panic("phase1: action " + op + " requires files")
	}
	return *value
}

func needTime(value *string, op string, key string) time.Time {
	parsed, err := time.Parse(time.RFC3339Nano, needString(value, op, key))
	if err != nil {
		panic("phase1: action " + op + " has an unparsable " + key + ": " + err.Error())
	}
	return parsed
}

// ---------------------------------------------------------------------------
// Portable reductions
// ---------------------------------------------------------------------------

const subFailedPrefix = "vfs: failed to create sub file system for "

// classifyPanic keeps a pinned panic's literal text and reduces everything
// else. See the file header for why the split falls where it does.
func classifyPanic(r any) []any {
	var text string
	switch value := r.(type) {
	case error:
		text = value.Error()
	case string:
		text = value
	default:
		return []any{"class", "non_error_panic"}
	}
	switch {
	case strings.HasPrefix(text, subFailedPrefix):
		// iofs.go:120 interpolates fs.Sub's *PathError. The pinned prefix and
		// the pinned %q are the contract; the stdlib sentence is not.
		rest := text[len(subFailedPrefix):]
		if end := strings.Index(rest, `": `); end >= 0 {
			return []any{"class", "sub_failed:" + rest[:end+1]}
		}
		return []any{"class", "sub_failed"}
	case strings.Contains(text, "index out of range"), strings.Contains(text, "slice bounds out of range"):
		return []any{"class", "index_out_of_range"}
	case strings.Contains(text, "nil pointer dereference"), strings.Contains(text, "invalid memory address"):
		return []any{"class", "nil_pointer_dereference"}
	case strings.Contains(text, "interface conversion"):
		return []any{"class", "interface_conversion"}
	default:
		// Reached by a panic the pinned source raises itself: From's five
		// capability panics and internal.RootLength's rootedness assertion.
		return []any{"literal", text}
	}
}

func guarded(f func() any) (result any, panicked []any) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = classifyPanic(r)
		}
	}()
	return f(), nil
}

// errorClass reduces an error to what a Rust port could reproduce. The pinned
// shapes are matched before the sentinels because the pinned messages wrap
// them: vfstest.go:521 wraps fs.ErrNotExist inside `write %q: %w`.
func errorClass(err error) string {
	if err == nil {
		return ""
	}
	text := err.Error()
	var refusedWrite *spyRefusedWrite
	switch {
	case strings.Contains(text, "parent path exists but is not a directory"):
		return "parent_not_a_directory"
	case strings.Contains(text, "path exists but is not a directory"):
		return "not_a_directory"
	case strings.Contains(text, "path exists but is not a regular file"):
		return "not_a_regular_file"
	case strings.Contains(text, "broken symlink"):
		return "broken_symlink"
	case errors.Is(err, errWalkStop):
		return "walk_sentinel"
	case errors.As(err, &refusedWrite):
		// The ordinal is part of the class: which attempt's error came back is
		// the whole difference between the pinned retry and a port that
		// returns the first failure.
		return fmt.Sprintf("spy_refused_write:%d", refusedWrite.attempt)
	case errors.Is(err, errSpyMkdirRefused):
		return "spy_refused_mkdir"
	case errors.Is(err, fs.SkipAll):
		return "skip_all"
	case errors.Is(err, fs.SkipDir):
		return "skip_dir"
	case errors.Is(err, fs.ErrNotExist):
		return "not_exist"
	case errors.Is(err, fs.ErrInvalid):
		return "invalid"
	case errors.Is(err, fs.ErrExist):
		return "exist"
	case errors.Is(err, fs.ErrPermission):
		return "permission"
	default:
		// Not a bucket: a message the pin grows later must be visible, not
		// absorbed. The probe-local doubles never reach here.
		return "other:" + text
	}
}

func modeClass(mode fs.FileMode) string {
	switch {
	case mode.IsDir():
		return "dir"
	case mode.IsRegular():
		return "regular"
	case mode&fs.ModeSymlink != 0:
		return "symlink"
	case mode&fs.ModeIrregular != 0:
		return "irregular"
	default:
		return "other"
	}
}

func permOctal(mode fs.FileMode) string {
	return fmt.Sprintf("0o%03o", uint32(mode.Perm()))
}

func stamp(when time.Time) string {
	return when.UTC().Format(time.RFC3339Nano)
}

func hexOf(contents string) string {
	return hex.EncodeToString([]byte(contents))
}

// ---------------------------------------------------------------------------
// Probe-local backings
// ---------------------------------------------------------------------------

var errWalkStop = errors.New("phase1: walk callback stop")

// errSpyRefused is what the spy's refusals wrap while its declared failure
// budget lasts. It is a probe sentinel, not a pinned message: the case is
// about what the ADAPTER does after a write fails, not about why it failed.
var errSpyRefused = errors.New("phase1: spy refused the write")

// spyRefusedWrite names WHICH attempt was refused. A single shared sentinel
// would reduce every refusal to one class, and then an implementation that
// returned the FIRST write's error would produce a row byte-identical to the
// pin's, which returns what the SECOND call returned (iofs.go:209). The
// ordinal is what tells those two apart. It is reproducible: the request
// declares the failure budget through fail_writes, so the attempt number a
// refusal carries is a function of the trace, not of this file.
type spyRefusedWrite struct {
	attempt int
}

func (e *spyRefusedWrite) Error() string {
	return fmt.Sprintf("phase1: spy refused write %d: %v", e.attempt, errSpyRefused)
}

func (e *spyRefusedWrite) Unwrap() error { return errSpyRefused }

var errSpyMkdirRefused = errors.New("phase1: spy refused the mkdir")

// realpathOnlyFS implements iovfs.RealpathFS and nothing else, so From takes
// the realpath branch (iofs.go:45) and the read-only mutation branch
// (iofs.go:89). Realpath answers a marker rather than a resolution: the case
// records what the adapter handed the backing and what it did to the answer.
type realpathOnlyFS struct {
	fstest.MapFS
}

func (f *realpathOnlyFS) Realpath(path string) (string, error) {
	if strings.Contains(path, "boom") {
		return "", fs.ErrNotExist
	}
	return "RP<" + path + ">", nil
}

// spyFS implements iovfs.WritableFS and records every call it is handed. Its
// success or failure is declared by the request, so the trace observes the
// adapter's control flow rather than a backing's semantics.
type spyFS struct {
	fstest.MapFS
	calls []any
	// writeAttempts counts every write or append the adapter hands the spy,
	// refused or not, so a refusal can say which attempt it came from.
	writeAttempts int
	failWrites    int
	failMkdir     bool
}

func (f *spyFS) record(row []any) {
	f.calls = append(f.calls, row)
}

func (f *spyFS) refuseWrite() error {
	f.writeAttempts++
	if f.failWrites > 0 {
		f.failWrites--
		return &spyRefusedWrite{attempt: f.writeAttempts}
	}
	return nil
}

func (f *spyFS) WriteFile(path string, data string, perm fs.FileMode) error {
	err := f.refuseWrite()
	f.record([]any{"WriteFile", path, hexOf(data), permOctal(perm), errorClass(err)})
	return err
}

func (f *spyFS) AppendFile(path string, data string, perm fs.FileMode) error {
	err := f.refuseWrite()
	f.record([]any{"AppendFile", path, hexOf(data), permOctal(perm), errorClass(err)})
	return err
}

func (f *spyFS) MkdirAll(path string, perm fs.FileMode) error {
	var err error
	if f.failMkdir {
		err = errSpyMkdirRefused
	}
	f.record([]any{"MkdirAll", path, "", permOctal(perm), errorClass(err)})
	return err
}

func (f *spyFS) Remove(path string) error {
	f.record([]any{"Remove", path, "", "", ""})
	return nil
}

func (f *spyFS) Chtimes(path string, aTime time.Time, mTime time.Time) error {
	f.record([]any{"Chtimes", path, stamp(aTime), stamp(mTime), ""})
	return nil
}

// spyRealpathFS is the fourth corner: both capabilities at once.
type spyRealpathFS struct {
	spyFS
}

func (f *spyRealpathFS) Realpath(path string) (string, error) {
	if strings.Contains(path, "boom") {
		return "", fs.ErrNotExist
	}
	return "RP<" + path + ">", nil
}

// fixedClock is the injected clock (vfstest.go:37-40) that keeps every recorded
// time reproducible. The pin calls Now once per seeded entry in sorted order
// and once per synthesized intermediate directory, so the sequence is fixed.
type fixedClock struct {
	ticks int64
}

var clockBase = time.Unix(1700000000, 0).UTC()

func (c *fixedClock) Now() time.Time {
	c.ticks++
	return clockBase.Add(time.Duration(c.ticks) * time.Second)
}

func (c *fixedClock) SinceStart() time.Duration {
	return time.Duration(c.ticks) * time.Second
}

// ---------------------------------------------------------------------------
// Fixture construction
// ---------------------------------------------------------------------------

func seedFile(entry []string, op string) (string, *fstest.MapFile) {
	if len(entry) != 3 {
		panic("phase1: action " + op + " needs [path, kind, content] file entries")
	}
	path, kind, content := entry[0], entry[1], entry[2]
	switch kind {
	case "file":
		return path, &fstest.MapFile{Data: []byte(content)}
	case "hex":
		data, err := hex.DecodeString(content)
		if err != nil {
			panic("phase1: file entry " + path + " has unparsable hex: " + err.Error())
		}
		return path, &fstest.MapFile{Data: data}
	case "symlink":
		return path, vfstest.Symlink(content)
	case "dir":
		return path, &fstest.MapFile{Mode: fs.ModeDir | 0o755}
	default:
		panic("phase1: unsupported file kind: " + kind)
	}
}

func seedMap(files [][]string, op string) map[string]*fstest.MapFile {
	out := make(map[string]*fstest.MapFile, len(files))
	for _, entry := range files {
		path, file := seedFile(entry, op)
		out[path] = file
	}
	return out
}

// plainMap builds a testing/fstest.MapFS. The leading separator is stripped
// here because fs.ValidPath rejects a rooted name and the adapter's RootFor
// hands the backing the path below the root (internal.go:38-44); vfstest does
// the same strip at vfstest.go:132.
func plainMap(files [][]string, op string) fstest.MapFS {
	out := make(fstest.MapFS, len(files))
	for _, entry := range files {
		path, file := seedFile(entry, op)
		path, _ = strings.CutPrefix(path, "/")
		out[path] = file
	}
	return out
}

type probeState struct {
	kind string
	// FsWithSys, not vfs.FS: FSys is one of the operations under test and the
	// pinned From returns it (iofs.go:43).
	adapter iovfs.FsWithSys
	handed  fs.FS
	// pointerBacking says whether `handed` may be compared with ==. A map type
	// is not comparable and an interface comparison of two would panic.
	pointerBacking bool
	mapfs          *vfstest.MapFS
	spy            *spyFS
}

func buildState(a iovfsAction) *probeState {
	kind := needString(a.Backing, a.Op, "backing")
	caseSensitive := needBool(a.CaseSensitive, a.Op, "case_sensitive")
	files := needFiles(a.Files, a.Op)
	state := &probeState{kind: kind}

	switch kind {
	case "fstest_map":
		backing := plainMap(files, a.Op)
		state.handed = backing
		state.adapter = iovfs.From(backing, caseSensitive)
	case "realpath_only":
		backing := &realpathOnlyFS{MapFS: plainMap(files, a.Op)}
		state.handed, state.pointerBacking = backing, true
		state.adapter = iovfs.From(backing, caseSensitive)
	case "writable_only":
		backing := &spyFS{MapFS: plainMap(files, a.Op)}
		if a.FailWrites != nil {
			backing.failWrites = *a.FailWrites
		}
		if a.FailMkdir != nil {
			backing.failMkdir = *a.FailMkdir
		}
		state.handed, state.pointerBacking, state.spy = backing, true, backing
		state.adapter = iovfs.From(backing, caseSensitive)
	case "spy_realpath":
		backing := &spyRealpathFS{spyFS: spyFS{MapFS: plainMap(files, a.Op)}}
		if a.FailWrites != nil {
			backing.failWrites = *a.FailWrites
		}
		if a.FailMkdir != nil {
			backing.failMkdir = *a.FailMkdir
		}
		state.handed, state.pointerBacking, state.spy = backing, true, &backing.spyFS
		state.adapter = iovfs.From(backing, caseSensitive)
	case "vfstest_map":
		// Built the way vfstest.go:140 builds it, but with the backing kept so
		// the adapter under test is the one this probe constructed through the
		// operation under test and FSys identity means something.
		seeded := vfstest.FromMapWithClock(seedMap(files, a.Op), caseSensitive, &fixedClock{})
		backing := seeded.(iovfs.FsWithSys).FSys().(*vfstest.MapFS)
		state.handed, state.pointerBacking, state.mapfs = backing, true, backing
		state.adapter = iovfs.From(backing, caseSensitive)
	default:
		panic("phase1: unsupported backing: " + kind)
	}
	return state
}

func (s *probeState) capabilities() []any {
	_, realpath := s.handed.(iovfs.RealpathFS)
	_, writable := s.handed.(iovfs.WritableFS)
	return []any{s.kind, realpath, writable, s.adapter.UseCaseSensitiveFileNames()}
}

func (s *probeState) requireMapFS(op string) *vfstest.MapFS {
	if s.mapfs == nil {
		panic("phase1: action " + op + " needs a vfstest backing, got " + s.kind)
	}
	return s.mapfs
}

func (s *probeState) requireSpy(op string) *spyFS {
	if s.spy == nil {
		panic("phase1: action " + op + " needs a spy backing, got " + s.kind)
	}
	return s.spy
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

func statRow(info vfs.FileInfo) []any {
	if info == nil {
		return []any{false}
	}
	return []any{
		true,
		info.Name(),
		info.Size(),
		info.IsDir(),
		modeClass(info.Mode()),
		permOctal(info.Mode()),
		stamp(info.ModTime()),
	}
}

func entriesRow(got vfs.Entries) []any {
	files := got.Files
	if files == nil {
		files = []string{}
	}
	directories := got.Directories
	if directories == nil {
		directories = []string{}
	}
	// The pinned Files and Directories slices are ordered (fs.ReadDir sorts,
	// internal.go:123) and that order is the subject; Symlinks is a Go map and
	// its iteration order is not, so it is sorted before being recorded.
	symlinks := make([]string, 0, len(got.Symlinks))
	for name := range got.Symlinks {
		symlinks = append(symlinks, name)
	}
	slices.Sort(symlinks)
	return []any{files, directories, symlinks, got.Symlinks == nil}
}

func walkDecisions(a iovfsAction) map[string]string {
	control := map[string]string{}
	if a.Control == nil {
		return control
	}
	for _, pair := range *a.Control {
		if len(pair) != 2 {
			panic("phase1: action " + a.Op + " needs [path, decision] control entries")
		}
		switch pair[1] {
		case "continue", "skip_dir", "skip_all", "error", "propagate":
		default:
			panic("phase1: unsupported walk decision: " + pair[1])
		}
		control[pair[0]] = pair[1]
	}
	return control
}

func walkOnce(state *probeState, root string, control map[string]string) []any {
	rows := []any{}
	err := state.adapter.WalkDir(root, func(path string, entry vfs.DirEntry, walkErr error) error {
		decision, ok := control[path]
		if !ok {
			decision = "continue"
		}
		name, isDir, class := "", false, "nil"
		if entry != nil {
			// A pinned walk hands the callback a nil DirEntry together with an
			// error (internal.go:136-141 forwards fs.WalkDir's own row), so the
			// nil is contract and must be recorded, not dereferenced.
			name, isDir, class = entry.Name(), entry.IsDir(), modeClass(entry.Type())
		}
		rows = append(rows, []any{path, name, isDir, class, errorClass(walkErr), decision})
		switch decision {
		case "skip_dir":
			return fs.SkipDir
		case "skip_all":
			return fs.SkipAll
		case "error":
			return errWalkStop
		case "propagate":
			return walkErr
		default:
			return nil
		}
	})
	return []any{rows, errorClass(err)}
}

func replayIovfs(request iovfsRequest) []any {
	var state *probeState
	need := func(op string) *probeState {
		if state == nil {
			panic("phase1: action " + op + " ran before new_fs")
		}
		return state
	}
	ordered := []any{}
	for _, a := range decodeIovfsActions(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new_fs":
			// Construction is outside the guard: a fixture that cannot be built
			// is a harness failure, not an observation of From.
			state = buildState(a)
			row["result"] = state.capabilities()
		case "use_case_sensitive_file_names":
			s := need(a.Op)
			value, panicked := guarded(func() any {
				return []any{s.adapter.UseCaseSensitiveFileNames()}
			})
			row["result"], row["panic"] = value, panicked
		case "file_exists":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return []any{s.adapter.FileExists(path)} })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "directory_exists":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return []any{s.adapter.DirectoryExists(path)} })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "stat":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return statRow(s.adapter.Stat(path)) })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "read_file":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any {
				contents, ok := s.adapter.ReadFile(path)
				return []any{ok, hexOf(contents), len(contents)}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "entries":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return entriesRow(s.adapter.GetAccessibleEntries(path)) })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "realpath":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return []any{s.adapter.Realpath(path)} })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "walk":
			s, root := need(a.Op), needString(a.Root, a.Op, "root")
			control := walkDecisions(a)
			value, panicked := guarded(func() any { return walkOnce(s, root, control) })
			row["root"], row["result"], row["panic"] = root, value, panicked
		case "write_file":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			content := needString(a.Content, a.Op, "content")
			value, panicked := guarded(func() any {
				return []any{errorClass(s.adapter.WriteFile(path, content))}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "append_file":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			content := needString(a.Content, a.Op, "content")
			value, panicked := guarded(func() any {
				return []any{errorClass(s.adapter.AppendFile(path, content))}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "remove":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any { return []any{errorClass(s.adapter.Remove(path))} })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "chtimes":
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			aTime, mTime := needTime(a.ATime, a.Op, "atime"), needTime(a.MTime, a.Op, "mtime")
			value, panicked := guarded(func() any {
				return []any{errorClass(s.adapter.Chtimes(path, aTime, mTime))}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "fsys_kind":
			// The recorded kind is a probe label, never %T: a Go type name is
			// not something a Rust port could reproduce.
			s := need(a.Op)
			value, panicked := guarded(func() any {
				_, isAdapter := s.adapter.FSys().(iovfs.FsWithSys)
				return []any{s.kind, isAdapter}
			})
			row["result"], row["panic"] = value, panicked
		case "fsys_identity":
			// FSys is called first and unconditionally. Whether the returned
			// value may be compared with == is a property of the BACKING -- an
			// interface comparison of two Go maps panics -- and deciding that
			// before calling FSys would make the whole row a constant derived
			// from the request, which both sides could agree on with no fsys()
			// at all. So the comparison is skipped where it is unsafe, never
			// the call: every row carries a stat taken THROUGH the value FSys
			// handed back, for a path the request seeded.
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any {
				got := s.adapter.FSys()
				info, err := fs.Stat(got, path)
				reached := []any{errorClass(err)}
				if err == nil {
					reached = append(reached, info.Name(), info.Size(), modeClass(info.Mode()))
				}
				if !s.pointerBacking {
					return []any{"not_comparable", reached}
				}
				return []any{"comparable", got == s.handed, reached}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "fsys_read":
			// Straight at the value FSys handed back, bypassing the adapter, so
			// the case can tell a live backing from a snapshot.
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			value, panicked := guarded(func() any {
				data, err := fs.ReadFile(s.adapter.FSys(), path)
				return []any{errorClass(err), hexOf(string(data)), len(data)}
			})
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "backing_mod_time":
			// vfstest's own accessor, which does NOT follow symlinks
			// (vfstest.go:633-643), and so is the only window onto a Chtimes
			// that landed on a link rather than its target.
			s, path := need(a.Op), needString(a.Path, a.Op, "path")
			mapfs := s.requireMapFS(a.Op)
			value, panicked := guarded(func() any { return []any{stamp(mapfs.GetModTime(path))} })
			row["path"], row["result"], row["panic"] = path, value, panicked
		case "spy_log":
			s := need(a.Op)
			spy := s.requireSpy(a.Op)
			calls := spy.calls
			spy.calls = nil
			if calls == nil {
				calls = []any{}
			}
			row["result"] = calls
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

var iovfsSubjects = map[string]struct{}{
	"iovfs.From":     {},
	"iovfs.Read":     {},
	"iovfs.Entries":  {},
	"iovfs.Realpath": {},
	"iovfs.Walk":     {},
	"iovfs.Mutate":   {},
	"iovfs.Identity": {},
}

func TestPhase1FilesystemIovfs(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []iovfsRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if _, ok := iovfsSubjects[request.Subject]; ok {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replayIovfs(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the iovfs probe"
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
