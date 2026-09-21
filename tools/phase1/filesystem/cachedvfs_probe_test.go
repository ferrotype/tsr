package cachedvfs

// Access only: replays an ordered action trace against the pinned caching
// wrapper and the shared VFS internals it sits on, and records what each
// action observed.
//
// It compiles into `package cachedvfs` because the capture overlays it at
// tsc/internal/vfs/cachedvfs/. That placement is what makes the second half of
// this group reachable at all: tsc/internal/vfs/internal is an internal
// package whose importers must sit under tsc/internal/vfs, and cachedvfs does.
// Nothing here touches the wrapper's unexported state; every call goes through
// an exported entry point, and the enabled flag and the five caches are
// observed only through what the underlying filesystem is asked.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation. A
// payload that carries order is always an array; a row's own named fields are
// not ordered data and may stay an object.
//
// Three fixtures serve the whole group, and all three are in memory, so every
// case here is valid on any host:
//
//   - `from` builds vfsmock.Wrap(vfstest.FromMapWithClock(...)), the pinned
//     package's own test rig. The mock is the measuring instrument: the
//     wrapper's caches are invisible, but what it asks the underlying is not,
//     and every row carries the twelve per-method call counts as an array so a
//     port cannot pass by forgetting one of them.
//   - `from_common` builds an injected io/fs fixture for internal.Common. It
//     is hand-written rather than borrowed from vfstest because Common's
//     classification rules turn on a directory entry's Type() disagreeing with
//     the stat of the same name -- a Windows junction is an irregular entry
//     that stats as a directory -- and no map-backed fixture can express that.
//     The fixture records every Stat/ReadDir/ReadFile it is asked for, which
//     is how getEntries, an unexported method, is observed at all: it is on
//     the path from GetAccessibleEntries, and the recorded ReadDir carries the
//     raw entry count it returned unfiltered.
//   - RootLength and SplitPath need no fixture; they are string arithmetic.
//
// Two rules keep the observations portable. Anything derived from a Go map's
// iteration order is sorted before it is recorded (Entries.Symlinks is a map,
// and its order is not the contract). Any error is reduced to a class, because
// the wording of "open sub: file does not exist" is the toolchain's, not the
// pin's. A panic is reduced the same way, except that a panic the pinned
// source raises itself keeps its literal text: internal.RootLength's
// `vfs: path %q is not absolute` IS the contract, and a port has to reproduce
// the sentence, not merely fail. One further value is classed for the same
// reason an error is: a `stat` action may carry `"mod_time": "class"`, and the
// one row that does records the token `aliased` because the instant Go answers
// there comes from the fixture's own pointer aliasing rather than from the
// pinned wrapper. See action.ModTime.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"maps"
	"os"
	"path"
	"runtime"
	"slices"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/internal"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfsmock"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// casePrefix claims a request. The schedule is shared with every other
// filesystem group, and a subject string could collide with a neighbour's, so
// ownership is keyed on the case id this group was assigned.
const casePrefix = "filesystem/cachedvfs/"

type fsRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// fileSpec is one entry of a `from` fixture: the vfstest map the pinned
// cachedvfs_test builds its own rig from.
type fileSpec struct {
	Path    *string `json:"path"`
	Kind    *string `json:"kind"`
	Content *string `json:"content"`
	Target  *string `json:"target"`
}

// nodeSpec is one entry of a `from_common` fixture. `entry` is what ReadDir
// reports as the entry's Type(); `stat` is what a stat of the same name
// yields, which is a different thing for a symlink, for a junction and for a
// dangling link.
type nodeSpec struct {
	Name       *string `json:"name"`
	Entry      *string `json:"entry"`
	Stat       *string `json:"stat"`
	ContentHex *string `json:"content_hex"`
}

// action is the union of every key this group's actions carry. Every payload
// field is a pointer: an absent key must fail the probe, never default to a
// zero value that both sides could agree on without executing anything.
type action struct {
	Op            string      `json:"op"`
	Path          *string     `json:"path"`
	DataHex       *string     `json:"data_hex"`
	Root          *string     `json:"root"`
	CaseSensitive *bool       `json:"case_sensitive"`
	Now           *string     `json:"now"`
	ATime         *string     `json:"a_time"`
	MTime         *string     `json:"m_time"`
	Files         *[]fileSpec `json:"files"`
	Nodes         *[]nodeSpec `json:"nodes"`
	ReadDirErrors *[]string   `json:"read_dir_errors"`
	Reparse       *string     `json:"is_reparse_point"`
	SkipDirAt     *string     `json:"skip_dir_at"`
	SkipAllAt     *string     `json:"skip_all_at"`
	FailAt        *string     `json:"fail_at"`
	// ModTime asks a `stat` action to record the modification time as a class
	// instead of as an instant. It exists for one row: the stat that follows a
	// Chtimes and is served from the wrapper's cache. Go answers that row with
	// the POST-Chtimes instant although nothing was re-queried, because
	// vfstest.MapFS.Chtimes assigns `fileInfo.ModTime = mTime` on the stored
	// *fstest.MapFile in place and the cached vfs.FileInfo interface value
	// still points at that struct. The instant is a property of the fixture's
	// aliasing, not of the pinned wrapper, so freezing it would fail a port
	// whose stat cache holds an owned FileInfo -- which is the only shape
	// crates/tsr_vfs/src/lib.rs:63-67 can express.
	ModTime *string `json:"mod_time"`
}

func (a *action) missing(key string) {
	panic("phase1: action " + a.Op + " is missing required key " + key)
}

func (a *action) str(key string, p *string) string {
	if p == nil {
		a.missing(key)
	}
	return *p
}

func (a *action) flag(key string, p *bool) bool {
	if p == nil {
		a.missing(key)
	}
	return *p
}

func (a *action) instant(key string, p *string) time.Time {
	parsed, err := time.Parse(time.RFC3339Nano, a.str(key, p))
	if err != nil {
		panic("phase1: action " + a.Op + " key " + key + " is not an RFC3339 instant")
	}
	return parsed.UTC()
}

// modTimeClass is "" when the action wants the instant itself and "class" when
// it wants the token in its place. Any other spelling is a typo in the
// schedule, and a typo must fail the probe rather than quietly freeze a value.
func (a *action) modTimeClass() string {
	if a.ModTime == nil {
		return ""
	}
	if *a.ModTime != "class" {
		panic("phase1: action " + a.Op + " key mod_time must be \"class\" when present")
	}
	return "class"
}

func (a *action) bytes(key string, p *string) []byte {
	raw, err := hex.DecodeString(a.str(key, p))
	if err != nil {
		panic("phase1: action " + a.Op + " key " + key + " is not hex")
	}
	return raw
}

// decodeActions unmarshals a request's actions once its case has matched.
// Unknown fields are refused: this probe decodes only its own cases, so an
// unrecognised key is a typo in the schedule, not a neighbour's payload.
func decodeActions(raw json.RawMessage) []action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.DisallowUnknownFields()
	var out []action
	if err := decoder.Decode(&out); err != nil {
		// Decoding happens before any guarded production call. A malformed
		// request must fail the probe, never turn into an empty observation.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	for i := range out {
		if out[i].Op == "" {
			panic("phase1: action without an op")
		}
	}
	return out
}

// ---------------------------------------------------------------------------
// portable classes

func classify(r any) string {
	var text string
	switch v := r.(type) {
	case error:
		text = v.Error()
	case string:
		text = v
	default:
		text = "non-error panic"
	}
	switch {
	case strings.Contains(text, "index out of range"), strings.Contains(text, "slice bounds out of range"):
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"), strings.Contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	case strings.Contains(text, "interface conversion"):
		return "interface_conversion"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract: internal.RootLength's
		// `vfs: path %q is not absolute`.
		return "other:" + text
	}
}

// errSentinel is the callback error a walk case returns to observe what
// WalkDir does with it. It is this file's own value, so its class is stable.
var errSentinel = errors.New("phase1: sentinel")

func errorClass(err error) string {
	switch {
	case err == nil:
		return "none"
	case errors.Is(err, errSentinel):
		return "sentinel"
	case errors.Is(err, fs.SkipAll):
		return "skip_all"
	case errors.Is(err, fs.SkipDir):
		return "skip_dir"
	case errors.Is(err, fs.ErrNotExist):
		return "not_exist"
	case errors.Is(err, fs.ErrPermission):
		return "permission"
	case errors.Is(err, fs.ErrInvalid):
		return "invalid"
	case errors.Is(err, fs.ErrExist):
		return "exist"
	default:
		// Deliberately not the message: it names dynamic types, syscall names
		// and host paths, none of which a Rust port could reproduce.
		return "other"
	}
}

// ---------------------------------------------------------------------------
// the `from` fixture: vfsmock over vfstest

type fixedClock struct{ at time.Time }

func (c fixedClock) Now() time.Time            { return c.at }
func (c fixedClock) SinceStart() time.Duration { return 0 }

type cachedState struct {
	mock   *vfsmock.FSMock
	cached *FS
}

// counts renders the twelve per-method call counts as an array in a fixed
// order. An array, because the counts are the subject of nearly every case
// here and an object's member order does not survive canonicalisation; all
// twelve, because a port must not pass by leaving one of them unobserved.
//
// Order: DirectoryExists, FileExists, GetAccessibleEntries, Realpath, Stat,
// ReadFile, UseCaseSensitiveFileNames, WalkDir, Remove, Chtimes, WriteFile,
// AppendFile.
func (s *cachedState) counts() []any {
	if s.mock == nil {
		return []any{0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}
	}
	return []any{
		len(s.mock.DirectoryExistsCalls()),
		len(s.mock.FileExistsCalls()),
		len(s.mock.GetAccessibleEntriesCalls()),
		len(s.mock.RealpathCalls()),
		len(s.mock.StatCalls()),
		len(s.mock.ReadFileCalls()),
		len(s.mock.UseCaseSensitiveFileNamesCalls()),
		len(s.mock.WalkDirCalls()),
		len(s.mock.RemoveCalls()),
		len(s.mock.ChtimesCalls()),
		len(s.mock.WriteFileCalls()),
		len(s.mock.AppendFileCalls()),
	}
}

func (s *cachedState) require() *FS {
	if s.cached == nil {
		panic("phase1: the trace used the wrapper before `from` built it")
	}
	return s.cached
}

func buildMap(specs []fileSpec) map[string]*fstest.MapFile {
	out := make(map[string]*fstest.MapFile, len(specs))
	for i := range specs {
		spec := specs[i]
		if spec.Path == nil {
			panic("phase1: a `from` file entry is missing required key path")
		}
		if spec.Kind == nil {
			panic("phase1: a `from` file entry is missing required key kind")
		}
		switch *spec.Kind {
		case "file":
			if spec.Content == nil {
				panic("phase1: a `from` file entry is missing required key content")
			}
			out[*spec.Path] = &fstest.MapFile{Data: []byte(*spec.Content)}
		case "dir":
			out[*spec.Path] = &fstest.MapFile{Mode: fs.ModeDir | 0o777}
		case "symlink":
			if spec.Target == nil {
				panic("phase1: a `from` symlink entry is missing required key target")
			}
			out[*spec.Path] = vfstest.Symlink(*spec.Target)
		default:
			panic("phase1: unsupported `from` file kind: " + *spec.Kind)
		}
	}
	return out
}

func renderStat(info vfs.FileInfo, modTime string) any {
	if info == nil {
		return nil
	}
	// An array, never an object: a stat is ordered data inside an ordered
	// payload. Name, size, is-dir, mod time.
	when := info.ModTime().UTC().Format(time.RFC3339Nano)
	if modTime == "class" {
		// The action asked for the class rather than the instant; see
		// action.ModTime for the one row that does.
		when = "aliased"
	}
	return []any{info.Name(), info.Size(), info.IsDir(), when}
}

func renderEntries(row map[string]any, entries vfs.Entries) {
	files := entries.Files
	if files == nil {
		files = []string{}
	}
	directories := entries.Directories
	if directories == nil {
		directories = []string{}
	}
	row["files"] = files
	row["directories"] = directories
	// Symlinks is a Go map. Its iteration order is randomised and is not the
	// contract, so it is sorted; whether the map exists at all is recorded
	// separately, because nil means "symlink information unavailable".
	row["symlinks_present"] = entries.Symlinks != nil
	names := slices.Sorted(maps.Keys(entries.Symlinks))
	if names == nil {
		names = []string{}
	}
	row["symlinks"] = names
}

func (s *cachedState) apply(a *action, row map[string]any) {
	switch a.Op {
	case "from":
		files := a.Files
		if files == nil {
			a.missing("files")
		}
		clock := fixedClock{at: a.instant("now", a.Now)}
		s.mock = vfsmock.Wrap(vfstest.FromMapWithClock(
			buildMap(*files), a.flag("case_sensitive", a.CaseSensitive), clock))
		s.cached = From(s.mock)
	case "enable":
		s.require().Enable()
	case "clear_cache":
		s.require().ClearCache()
	case "disable_and_clear":
		s.require().DisableAndClearCache()
	case "directory_exists":
		p := a.str("path", a.Path)
		row["value"] = s.require().DirectoryExists(p)
	case "file_exists":
		p := a.str("path", a.Path)
		row["value"] = s.require().FileExists(p)
	case "read_file":
		p := a.str("path", a.Path)
		contents, ok := s.require().ReadFile(p)
		// Hex, because a file's bytes need not be valid UTF-8 and
		// encoding/json cannot carry the ones that are not.
		row["contents_hex"] = hex.EncodeToString([]byte(contents))
		row["ok"] = ok
	case "realpath":
		p := a.str("path", a.Path)
		row["value"] = s.require().Realpath(p)
	case "stat":
		p := a.str("path", a.Path)
		row["stat"] = renderStat(s.require().Stat(p), a.modTimeClass())
	case "entries":
		p := a.str("path", a.Path)
		renderEntries(row, s.require().GetAccessibleEntries(p))
	case "use_case_sensitive_file_names":
		row["value"] = s.require().UseCaseSensitiveFileNames()
	case "write_file":
		p := a.str("path", a.Path)
		row["error"] = errorClass(s.require().WriteFile(p, string(a.bytes("data_hex", a.DataHex))))
		calls := s.mock.WriteFileCalls()
		last := calls[len(calls)-1]
		row["forwarded"] = []any{last.Path, hex.EncodeToString([]byte(last.Data))}
	case "append_file":
		p := a.str("path", a.Path)
		row["error"] = errorClass(s.require().AppendFile(p, string(a.bytes("data_hex", a.DataHex))))
		calls := s.mock.AppendFileCalls()
		last := calls[len(calls)-1]
		row["forwarded"] = []any{last.Path, hex.EncodeToString([]byte(last.Data))}
	case "remove":
		p := a.str("path", a.Path)
		row["error"] = errorClass(s.require().Remove(p))
		calls := s.mock.RemoveCalls()
		row["forwarded"] = []any{calls[len(calls)-1].Path}
	case "chtimes":
		p := a.str("path", a.Path)
		aTime := a.instant("a_time", a.ATime)
		mTime := a.instant("m_time", a.MTime)
		row["error"] = errorClass(s.require().Chtimes(p, aTime, mTime))
		// Both instants are read back off the mock. The wrapper's forwarding
		// is invisible in the return value, so a dropped or swapped argument
		// is only observable here.
		calls := s.mock.ChtimesCalls()
		last := calls[len(calls)-1]
		row["forwarded"] = []any{
			last.Path,
			last.ATime.UTC().Format(time.RFC3339Nano),
			last.MTime.UTC().Format(time.RFC3339Nano),
		}
	case "walk":
		root := a.str("root", a.Root)
		visited, err := walk(s.require().WalkDir, root, a)
		row["visited"] = visited
		row["error"] = errorClass(err)
	default:
		panic("phase1: unsupported action: " + a.Op)
	}
}

// walkFn drives one walk, recording every callback in order and answering with
// the decision the request asked for at each path.
func walk(run func(string, vfs.WalkDirFunc) error, root string, a *action) ([]any, error) {
	skipDirAt := a.str("skip_dir_at", a.SkipDirAt)
	skipAllAt := a.str("skip_all_at", a.SkipAllAt)
	failAt := a.str("fail_at", a.FailAt)
	visited := []any{}
	err := run(root, func(p string, d fs.DirEntry, walkErr error) error {
		present := d != nil
		name := ""
		isDir := false
		if present {
			name = d.Name()
			isDir = d.IsDir()
		}
		decision := "continue"
		var answer error
		switch p {
		case skipDirAt:
			decision, answer = "skip_dir", fs.SkipDir
		case skipAllAt:
			decision, answer = "skip_all", fs.SkipAll
		case failAt:
			decision, answer = "sentinel", errSentinel
		}
		// An array, not an object: this is ordered data nested inside the
		// ordered payload, and canonicalisation sorts an object's keys.
		visited = append(visited, []any{p, present, name, isDir, errorClass(walkErr), decision})
		return answer
	})
	return visited, err
}

// ---------------------------------------------------------------------------
// the `from_common` fixture: an injected io/fs for internal.Common

type fixtureNode struct {
	name      string
	entryMode fs.FileMode
	statMode  fs.FileMode
	statable  bool
	content   []byte
}

func parseEntryMode(kind string) fs.FileMode {
	switch kind {
	case "file":
		return 0o644
	case "dir":
		return fs.ModeDir | 0o755
	case "symlink":
		return fs.ModeSymlink | 0o777
	case "irregular":
		return fs.ModeIrregular
	case "fifo":
		return fs.ModeNamedPipe
	default:
		panic("phase1: unsupported fixture entry kind: " + kind)
	}
}

type fixtureFS struct {
	nodes map[string]*fixtureNode
	names []string
	// readDirErrors maps a directory name to the error class ReadDir answers
	// with, for the unreadable-directory rows.
	readDirErrors map[string]struct{}
	now           time.Time
	calls         []any
}

func buildFixture(now time.Time, specs []nodeSpec, unreadable []string) *fixtureFS {
	f := &fixtureFS{
		nodes:         map[string]*fixtureNode{},
		readDirErrors: map[string]struct{}{},
		now:           now,
		calls:         []any{},
	}
	f.nodes["."] = &fixtureNode{name: ".", entryMode: fs.ModeDir | 0o755, statMode: fs.ModeDir | 0o755, statable: true}
	for i := range specs {
		spec := specs[i]
		if spec.Name == nil {
			panic("phase1: a `from_common` node is missing required key name")
		}
		if spec.Entry == nil {
			panic("phase1: a `from_common` node is missing required key entry")
		}
		if spec.Stat == nil {
			panic("phase1: a `from_common` node is missing required key stat")
		}
		node := &fixtureNode{name: *spec.Name, entryMode: parseEntryMode(*spec.Entry)}
		if *spec.Stat == "missing" {
			// A dangling link: the entry is listed, the stat is not there.
			node.statable = false
		} else {
			node.statable = true
			node.statMode = parseEntryMode(*spec.Stat)
		}
		if spec.ContentHex != nil {
			raw, err := hex.DecodeString(*spec.ContentHex)
			if err != nil {
				panic("phase1: a `from_common` node has a non-hex content_hex")
			}
			node.content = raw
		}
		f.nodes[node.name] = node
	}
	for _, name := range unreadable {
		f.readDirErrors[name] = struct{}{}
	}
	f.names = slices.Sorted(maps.Keys(f.nodes))
	return f
}

func (f *fixtureFS) record(entry ...any) {
	f.calls = append(f.calls, entry)
}

func (f *fixtureFS) take() []any {
	taken := f.calls
	f.calls = []any{}
	if taken == nil {
		return []any{}
	}
	return taken
}

func (f *fixtureFS) info(node *fixtureNode) fs.FileInfo {
	return &fixtureInfo{node: node, now: f.now}
}

func (f *fixtureFS) find(op, name string) (*fixtureNode, error) {
	if !fs.ValidPath(name) {
		return nil, &fs.PathError{Op: op, Path: name, Err: fs.ErrInvalid}
	}
	node, ok := f.nodes[name]
	if !ok || !node.statable {
		return nil, &fs.PathError{Op: op, Path: name, Err: fs.ErrNotExist}
	}
	return node, nil
}

func (f *fixtureFS) Stat(name string) (fs.FileInfo, error) {
	node, err := f.find("stat", name)
	f.record("stat", name, errorClass(err))
	if err != nil {
		return nil, err
	}
	return f.info(node), nil
}

func (f *fixtureFS) ReadDir(name string) ([]fs.DirEntry, error) {
	node, err := f.find("readdir", name)
	if err == nil {
		if _, blocked := f.readDirErrors[name]; blocked {
			err = &fs.PathError{Op: "readdir", Path: name, Err: fs.ErrPermission}
		} else if !node.statMode.IsDir() {
			err = &fs.PathError{Op: "readdir", Path: name, Err: fs.ErrInvalid}
		}
	}
	if err != nil {
		f.record("read_dir", name, 0, errorClass(err))
		return nil, err
	}
	out := []fs.DirEntry{}
	for _, child := range f.names {
		if child == "." || path.Dir(child) != name {
			continue
		}
		out = append(out, &fixtureEntry{fsys: f, node: f.nodes[child]})
	}
	// ReadDirFS is specified to answer in filename order; f.names is sorted by
	// full path, which for one directory's children is the same order.
	f.record("read_dir", name, len(out), "none")
	return out, nil
}

func (f *fixtureFS) ReadFile(name string) ([]byte, error) {
	node, err := f.find("open", name)
	if err == nil && node.statMode.IsDir() {
		err = &fs.PathError{Op: "read", Path: name, Err: fs.ErrInvalid}
	}
	if err != nil {
		f.record("read_file", name, 0, errorClass(err))
		return nil, err
	}
	f.record("read_file", name, len(node.content), "none")
	return slices.Clone(node.content), nil
}

func (f *fixtureFS) Open(name string) (fs.File, error) {
	node, err := f.find("open", name)
	f.record("open", name, errorClass(err))
	if err != nil {
		return nil, err
	}
	return &fixtureFile{fsys: f, node: node}, nil
}

type fixtureInfo struct {
	node *fixtureNode
	now  time.Time
}

func (i *fixtureInfo) Name() string       { return path.Base(i.node.name) }
func (i *fixtureInfo) Size() int64        { return int64(len(i.node.content)) }
func (i *fixtureInfo) Mode() fs.FileMode  { return i.node.statMode }
func (i *fixtureInfo) ModTime() time.Time { return i.now }
func (i *fixtureInfo) IsDir() bool        { return i.node.statMode.IsDir() }
func (i *fixtureInfo) Sys() any           { return nil }

// fixtureEntry is the point of the hand-written fixture: Type() answers with
// the directory entry's own mode while Info() stats the name, and the two
// disagree exactly where Common's classification rules turn.
type fixtureEntry struct {
	fsys *fixtureFS
	node *fixtureNode
}

func (e *fixtureEntry) Name() string      { return path.Base(e.node.name) }
func (e *fixtureEntry) IsDir() bool       { return e.node.entryMode.IsDir() }
func (e *fixtureEntry) Type() fs.FileMode { return e.node.entryMode.Type() }
func (e *fixtureEntry) Info() (fs.FileInfo, error) {
	if !e.node.statable {
		return nil, &fs.PathError{Op: "stat", Path: e.node.name, Err: fs.ErrNotExist}
	}
	return e.fsys.info(e.node), nil
}

type fixtureFile struct {
	fsys   *fixtureFS
	node   *fixtureNode
	offset int
}

func (f *fixtureFile) Stat() (fs.FileInfo, error) { return f.fsys.info(f.node), nil }
func (f *fixtureFile) Close() error               { return nil }

func (f *fixtureFile) Read(p []byte) (int, error) {
	if f.offset >= len(f.node.content) {
		return 0, io.EOF
	}
	n := copy(p, f.node.content[f.offset:])
	f.offset += n
	return n, nil
}

type commonState struct {
	common  internal.Common
	fixture *fixtureFS
	roots   []any
	reparse []any
	built   bool
}

func (s *commonState) require() *internal.Common {
	if !s.built {
		panic("phase1: the trace used vfs.Common before `from_common` built it")
	}
	return &s.common
}

func (s *commonState) build(a *action) {
	nodes := a.Nodes
	if nodes == nil {
		a.missing("nodes")
	}
	unreadable := a.ReadDirErrors
	if unreadable == nil {
		a.missing("read_dir_errors")
	}
	s.fixture = buildFixture(a.instant("now", a.Now), *nodes, *unreadable)
	s.common = internal.Common{
		RootFor: func(root string) fs.FS {
			s.roots = append(s.roots, root)
			if root == "/" {
				return s.fixture
			}
			// A literal nil, so the interface value is nil and Common's
			// `fsys == nil` guards see it.
			return nil
		},
	}
	switch a.str("is_reparse_point", a.Reparse) {
	case "nil":
		s.common.IsReparsePoint = nil
	case "true":
		s.common.IsReparsePoint = func(p string) bool {
			s.reparse = append(s.reparse, p)
			return true
		}
	case "false":
		s.common.IsReparsePoint = func(p string) bool {
			s.reparse = append(s.reparse, p)
			return false
		}
	default:
		panic("phase1: is_reparse_point must be nil, true or false")
	}
	s.built = true
}

func (s *commonState) apply(a *action, row map[string]any) {
	switch a.Op {
	case "from_common":
		s.build(a)
	case "root_and_path":
		p := a.str("path", a.Path)
		fsys, rootName, rest := s.require().RootAndPath(p)
		row["root_name"] = rootName
		row["rest"] = rest
		row["fs_nil"] = fsys == nil
	case "stat":
		p := a.str("path", a.Path)
		row["stat"] = renderStat(s.require().Stat(p), a.modTimeClass())
	case "file_exists":
		p := a.str("path", a.Path)
		row["value"] = s.require().FileExists(p)
	case "directory_exists":
		p := a.str("path", a.Path)
		row["value"] = s.require().DirectoryExists(p)
	case "entries":
		p := a.str("path", a.Path)
		renderEntries(row, s.require().GetAccessibleEntries(p))
	case "read_file":
		p := a.str("path", a.Path)
		contents, ok := s.require().ReadFile(p)
		row["contents_hex"] = hex.EncodeToString([]byte(contents))
		row["ok"] = ok
	case "walk":
		root := a.str("root", a.Root)
		visited, err := walk(s.require().WalkDir, root, a)
		row["visited"] = visited
		row["error"] = errorClass(err)
	default:
		panic("phase1: unsupported action: " + a.Op)
	}
}

// ---------------------------------------------------------------------------
// replay

// echo writes back the request arguments a row repeats. They are the request's
// own strings, not results, so they are written outside the guard: a row that
// records a panic still says which path raised it.
func echo(row map[string]any, a *action) {
	if a.Path != nil {
		row["path"] = *a.Path
	}
	if a.Root != nil {
		row["root"] = *a.Root
	}
}

func replayCached(actions []action) []any {
	state := &cachedState{}
	ordered := make([]any, 0, len(actions))
	for i := range actions {
		a := &actions[i]
		row := map[string]any{"op": a.Op}
		guard(row, func() { state.apply(a, row) })
		echo(row, a)
		row["counts"] = state.counts()
		ordered = append(ordered, row)
	}
	return ordered
}

func replayCommon(actions []action) []any {
	state := &commonState{}
	ordered := make([]any, 0, len(actions))
	for i := range actions {
		a := &actions[i]
		row := map[string]any{"op": a.Op}
		state.roots = []any{}
		state.reparse = []any{}
		if state.fixture != nil {
			state.fixture.take()
		}
		guard(row, func() { state.apply(a, row) })
		echo(row, a)
		// Every row carries what the injected root factory and the injected
		// filesystem were asked during this action, in order. That is how the
		// root dispatch is observed, and how getEntries -- an unexported
		// method with no other caller reachable from here -- is observed at
		// all: the ReadDir row carries the raw entry count it returned before
		// GetAccessibleEntries classified and dropped anything.
		row["roots"] = state.roots
		row["reparse"] = state.reparse
		if state.fixture != nil {
			row["fs_calls"] = state.fixture.take()
		} else {
			row["fs_calls"] = []any{}
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func replayRootLength(actions []action) []any {
	ordered := make([]any, 0, len(actions))
	for i := range actions {
		a := &actions[i]
		row := map[string]any{"op": a.Op}
		guard(row, func() {
			if a.Op != "root_length" {
				panic("phase1: unsupported action: " + a.Op)
			}
			row["length"] = internal.RootLength(a.str("path", a.Path))
		})
		echo(row, a)
		ordered = append(ordered, row)
	}
	return ordered
}

func replaySplitPath(actions []action) []any {
	ordered := make([]any, 0, len(actions))
	for i := range actions {
		a := &actions[i]
		row := map[string]any{"op": a.Op}
		guard(row, func() {
			if a.Op != "split_path" {
				panic("phase1: unsupported action: " + a.Op)
			}
			rootName, rest := internal.SplitPath(a.str("path", a.Path))
			row["root_name"] = rootName
			row["rest"] = rest
		})
		echo(row, a)
		ordered = append(ordered, row)
	}
	return ordered
}

// guard runs body and turns a panic into a recorded class, so a case whose
// subject is the panic boundary observes it instead of failing the probe. A
// half-written row is discarded first: a row that carried both a partial
// result and a panic would be a shape neither side could agree on.
//
// It does not catch a harness panic: an unsupported action, a missing key and
// a malformed payload all raise the same way, and reducing them to an
// observation is exactly what the contract forbids. Those strings are
// recognisable -- every one begins "phase1:" -- and are re-raised.
func guard(row map[string]any, body func()) {
	defer func() {
		r := recover()
		if r == nil {
			return
		}
		if text, ok := r.(string); ok && strings.HasPrefix(text, "phase1:") {
			panic(r)
		}
		for key := range row {
			if key != "op" {
				delete(row, key)
			}
		}
		row["panic"] = classify(r)
	}()
	body()
}

func replayFor(subject string) func([]action) []any {
	switch subject {
	case "CachedFS":
		return replayCached
	case "vfs.Common":
		return replayCommon
	case "vfs.RootLength":
		return replayRootLength
	case "vfs.SplitPath":
		return replaySplitPath
	}
	return nil
}

func TestPhase1FilesystemCachedvfs(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []fsRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if !strings.HasPrefix(request.Case, casePrefix) {
			row["result"] = "native_unavailable"
			row["reason"] = "case is not served by the cachedvfs probe"
			observations = append(observations, row)
			continue
		}
		replay := replayFor(request.Subject)
		if replay == nil {
			t.Fatalf("case %s declares subject %q, which the cachedvfs probe does not serve",
				request.Case, request.Subject)
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": replay(decodeActions(request.Actions))}
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
