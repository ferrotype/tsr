package vfstest

// Access only: replays an ordered action trace against the pinned in-memory
// test filesystem and records what each action observed.
//
// It is an in-package test file, unlike the leaf probes, because most of this
// group's subject is unexported: getCanonicalPath, getFollowingSymlinks and its
// worker, mkdirAll, open, remove, set, setEntry, splitPath, dirName, baseName,
// comparePathsByParts, convertInfo and convertMapFS are all package scope. The
// pinned vfstest_test.go is in-package for the same reason, so the names this
// file declares are prefixed `p1` to keep them clear of it and of a later pin.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.
//
// Four rules keep an observation reproducible and portable.
//
//  1. No wall-clock value is ever recorded. MapFS stamps every write through
//     m.clock, and the default clockImpl is time.Now (vfstest.go:46). A trace
//     that wants exact stamps builds its filesystem through FromMapWithClock
//     with the step clock below; a trace that only wants the ordering the
//     default clock produces records a relation between two paths, never a
//     value.
//
//  2. Nothing derived from a Go map's iteration order is recorded. This is not
//     hypothetical here: getFollowingSymlinksWorker's prefix search ranges over
//     m.symlinks (vfstest.go:271), so a path lying under two different
//     symlinked directory prefixes resolves to whichever the runtime happens to
//     yield first. Measured over 200 builds of the same map this split 175/25
//     between two different answers, so no case in this file constructs one.
//
//     Entries has a second, subtler form of the same hazard. It sorts with
//     comparePathsByParts (vfstest.go:650), which is not transitive: it falls
//     back to a whole-string compare the moment either side runs out of
//     separators (vfstest.go:192), so "a/b.ts" < "a-x/c.ts" < "a.b" < "a/b.ts"
//     is a genuine cycle. slices.SortFunc is not stable and Entries collects
//     its keys from a Go map, so a key set containing such a cycle enumerates
//     differently from run to run. Every map in this group's request file was
//     checked over 500 random permutations to be free of one.
//
//  3. A panic the pinned source raises itself is recorded with its literal
//     text, because that text is the contract: `non-rooted path %q`,
//     `duplicate path: %q and %q have the same canonical path`,
//     `unexpected synthesized dir: %q`, `empty path`. A panic the Go runtime
//     raises is reduced to a class instead; its wording names dynamic types and
//     no Rust port could reproduce the sentence.
//
//     One pinned panic is reduced for the same reason the runtime's are.
//     FromMapWithClock's default arm formats the offending value's DYNAMIC Go
//     type, `invalid file type %T` (vfstest.go:118-119), and `%T` renders a Go
//     type name: the observed sentence is `invalid file type int`, which a
//     Rust port could only reproduce by hardcoding it. The pin's own words are
//     kept and the operand is dropped, so the recorded class is
//     `pinned:invalid file type`. A port with a typed constructor parameter has
//     no such case to reject at all, which is why the recorded Rust gap for
//     FromMap no longer asks for this sentence.
//
//  4. An error is reduced the same way. The pin's own sentences travel
//     verbatim, because `broken symlink %q -> %q`, `mkdir %q: path exists but
//     is not a directory` and the write/append family are the contract. The one
//     substitution is fs.ErrNotExist's own sentence, which belongs to the Go
//     standard library rather than to the pin: it is rendered `<ErrNotExist>`
//     so a message the pin formats around it, such as `write %q: %w`, keeps
//     every byte the pin wrote.
//
// A path in the request is always rooted, the spelling a vfs.FS caller uses.
// MapFS keys have the leading separator stripped (vfstest.go:132) and iovfs
// strips it again on the way in (iofs.go:47-90), so this probe strips it too,
// and each action records the request's own spelling.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

type p1Action struct {
	Op string `json:"op"`
	// Path is the rooted spelling the action addresses.
	Path   string `json:"path"`
	Other  string `json:"other"`
	Target string `json:"target"`
	Data   string `json:"data"`
	Name   string `json:"name"`
	// Perm travels as an octal string so the request reads the way the pinned
	// source writes it. An action that needs one and does not carry one fails.
	Perm string `json:"perm"`
	// Canonical and Realpath are the two halves set/setEntry keep apart.
	Canonical string `json:"canonical"`
	Realpath  string `json:"realpath"`
	Kind      string `json:"kind"`
	// From and To seed getFollowingSymlinksWorker's error pair directly.
	From string `json:"from"`
	To   string `json:"to"`
	// MTime and ATime are RFC3339 instants. Chtimes takes both and keeps one.
	MTime string `json:"m_time"`
	ATime string `json:"a_time"`
	// Files is a constructor's input: [path, kind, data] rows.
	Files [][]string `json:"files"`
	Paths []string   `json:"paths"`
	// Pointers, so an absent key fails instead of defaulting into a mode or a
	// count the request never asked for.
	CaseSensitive *bool `json:"case_sensitive"`
	N             *int  `json:"n"`
	Offset        *int  `json:"offset"`
	Stop          *int  `json:"stop"`
	HasSys        *bool `json:"has_sys"`
}

type p1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// p1CasePrefix claims a request. The schedule is shared with every other
// filesystem group and a subject string could collide with a neighbour's, so
// ownership is keyed on the case id this group was assigned; the subject then
// only has to pick between this group's own two trace shapes.
const p1CasePrefix = "filesystem/vfstest/"

// p1Decode unmarshals a request's actions once its case has matched. Unknown
// fields are refused: this probe decodes only its own cases, so an unrecognised
// key is a typo in the schedule rather than a neighbour's payload, and silently
// dropping it would run an action the request did not describe.
func p1Decode(raw json.RawMessage) []p1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.DisallowUnknownFields()
	var out []p1Action
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

// p1Need enforces a required string key. A missing key is a request defect, not
// an observation: defaulting it would let two sides agree without executing
// what the case named.
func p1Need(op, field, value string) string {
	if value == "" {
		panic("phase1: action " + op + " needs a non-empty " + field)
	}
	return value
}

func p1NeedBool(op, field string, value *bool) bool {
	if value == nil {
		panic("phase1: action " + op + " needs " + field)
	}
	return *value
}

func p1NeedInt(op, field string, value *int) int {
	if value == nil {
		panic("phase1: action " + op + " needs " + field)
	}
	return *value
}

func p1Perm(op, value string) fs.FileMode {
	parsed, err := strconv.ParseUint(p1Need(op, "perm", value), 8, 32)
	if err != nil {
		panic("phase1: action " + op + " has an unreadable octal perm: " + value)
	}
	return fs.FileMode(parsed)
}

func p1Time(op, field, value string) time.Time {
	parsed, err := time.Parse(time.RFC3339Nano, p1Need(op, field, value))
	if err != nil {
		panic("phase1: action " + op + " has an unreadable " + field + ": " + value)
	}
	return parsed
}

// p1Key turns the request's rooted spelling into the key space MapFS uses.
// FromMap strips the leading separator before storing (vfstest.go:132) and
// iovfs strips it again before every call it forwards (iofs.go:47-90), so a
// MapFS method never sees one except through GetFileInfo, GetModTime and
// GetTargetOfSymlink, which strip it themselves. Those three are exercised with
// both spellings, so p1Key is applied only where the pin would have applied it.
//
// Chtimes is the exception in the other direction: it strips nothing and is
// called directly rather than through iovfs, so no arm applies p1Key to it and
// a request addressing it carries the key spelling. Applying p1Key there would
// have made the probe call a path the request never named, and the paired Rust
// side, which forwards the request verbatim, would have called a different one.
func p1Key(path string) string {
	rest, _ := strings.CutPrefix(path, "/")
	return rest
}

// p1Step is the injectable clock. It advances one second per reading, so a
// trace that asks for exact stamps observes the pinned creation order rather
// than the host's wall clock.
type p1Step struct {
	at time.Time
}

func (c *p1Step) Now() time.Time {
	c.at = c.at.Add(time.Second)
	return c.at
}

func (c *p1Step) SinceStart() time.Duration {
	return c.at.Sub(time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC))
}

func p1NewStep() *p1Step {
	return &p1Step{at: time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC)}
}

func p1Guard(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = p1Classify(r)
		}
	}()
	return f(), ""
}

func p1Classify(r any) string {
	text := ""
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
		// A failed type assertion. The runtime spells out both dynamic types,
		// which is toolchain wording rather than pinned source.
		return "interface_conversion"
	case strings.Contains(text, "stack overflow"):
		return "stack_overflow"
	case strings.HasPrefix(text, "invalid file type "):
		// The pin's own words, without the operand. FromMapWithClock's
		// default arm formats the value's dynamic Go type with %T
		// (vfstest.go:118-119), so the sentence ends in a Go type name that
		// belongs to the toolchain rather than to the pinned contract; see
		// rule 3 above.
		return "pinned:invalid file type"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract.
		return "pinned:" + p1Sentence(text)
	}
}

// p1Sentence removes the one sentence in a recorded message that the pin did
// not write. fs.ErrNotExist's text is the Go standard library's; the pin only
// formats around it, and that formatting is what a port has to reproduce.
func p1Sentence(text string) string {
	return strings.ReplaceAll(text, "file does not exist", "<ErrNotExist>")
}

// p1Err reduces an error to [class, detail]. The pin's own sentences travel
// verbatim; the standard library's sentinel is named rather than quoted.
func p1Err(err error) []any {
	if err == nil {
		return []any{"none", ""}
	}
	var broken *brokenSymlinkError
	if errors.As(err, &broken) {
		// The whole message is pin-authored, including any `write %q: ` that
		// wraps it, so it is recorded as written.
		return []any{"broken_symlink", err.Error()}
	}
	var pathError *fs.PathError
	if errors.As(err, &pathError) {
		// fstest.MapFS reports a miss as *fs.PathError. The operation and the
		// path it names are the contract; the sentinel's sentence is not.
		inner := "other"
		if errors.Is(pathError.Err, fs.ErrNotExist) {
			inner = "not_exist"
		}
		return []any{"path_error", pathError.Op + " " + pathError.Path, inner}
	}
	if errors.Is(err, fs.ErrNotExist) {
		return []any{"not_exist", p1Sentence(err.Error())}
	}
	if errors.Is(err, io.EOF) {
		return []any{"eof", ""}
	}
	return []any{"pinned", p1Sentence(err.Error())}
}

// p1Mode records the mode bits the pin actually sets, rather than Go's own
// rendering of a FileMode, whose layout belongs to the standard library.
func p1Mode(mode fs.FileMode) []any {
	return []any{mode.IsDir(), mode&fs.ModeSymlink != 0, mode.IsRegular(), fmt.Sprintf("%04o", mode.Perm())}
}

// p1Sys renders an entry's Sys payload. The only values these traces store are
// absent and the integer the pinned tests use, so the rendering stays a
// two-element list rather than leaking a Go type name.
func p1Sys(value any) []any {
	switch typed := value.(type) {
	case nil:
		return []any{"nil", 0}
	case int:
		return []any{"int", typed}
	case *sys:
		// Reached only where the wrapper itself is the subject; recorded as a
		// shape, never as the pointer.
		return []any{"wrapper", 0}
	default:
		return []any{"other", 0}
	}
}

// p1Stored unwraps what setEntry put in an entry's Sys field. GetFileInfo hands
// back the stored *fstest.MapFile, so its Sys is vfstest's own wrapper
// (vfstest.go:293) rather than the caller's value; recording the wrapper alone
// would hide both the realpath it carries and the original it nests. An entry
// written through `set` has no wrapper at all, which is the difference that
// makes it indistinguishable from a directory fstest.MapFS synthesized.
func p1Stored(file *fstest.MapFile) []any {
	if wrapper, ok := file.Sys.(*sys); ok {
		return []any{"wrapped", wrapper.realpath, p1Sys(wrapper.original)}
	}
	return []any{"bare", "", p1Sys(file.Sys)}
}

const p1SysValue = 1234

// p1MapFile builds one constructor input row. The kind travels in the request
// so a Rust port reading the request can see what was stored; a kind this probe
// does not know is a request defect.
func p1MapFile(kind, data string) any {
	switch kind {
	case "string":
		return data
	case "bytes":
		return []byte(data)
	case "mapfile":
		return &fstest.MapFile{Data: []byte(data)}
	case "mapfile_sys":
		return &fstest.MapFile{Data: []byte(data), Sys: p1SysValue}
	case "symlink":
		return Symlink(data)
	case "invalid":
		// The `invalid file type %T` branch of FromMapWithClock.
		return p1SysValue
	default:
		panic("phase1: unknown file kind: " + kind)
	}
}

func p1RawFile(kind, data string) *fstest.MapFile {
	switch kind {
	case "file":
		return &fstest.MapFile{Data: []byte(data)}
	case "file_sys":
		return &fstest.MapFile{Data: []byte(data), Sys: p1SysValue}
	case "symlink":
		return Symlink(data)
	case "dir":
		return &fstest.MapFile{Mode: fs.ModeDir | 0o755}
	default:
		panic("phase1: unknown raw file kind: " + kind)
	}
}

func p1Input(op string, files [][]string) map[string]any {
	if len(files) == 0 {
		panic("phase1: action " + op + " needs a files list")
	}
	out := make(map[string]any, len(files))
	for _, row := range files {
		if len(row) != 3 {
			panic("phase1: action " + op + " needs [path, kind, data] rows")
		}
		out[row[0]] = p1MapFile(row[1], row[2])
	}
	return out
}

func p1RawInput(op string, files [][]string) fstest.MapFS {
	if len(files) == 0 {
		panic("phase1: action " + op + " needs a files list")
	}
	out := make(fstest.MapFS, len(files))
	for _, row := range files {
		if len(row) != 3 {
			panic("phase1: action " + op + " needs [path, kind, data] rows")
		}
		out[row[0]] = p1RawFile(row[1], row[2])
	}
	return out
}

func p1Underlying(built any) *MapFS {
	return built.(iovfs.FsWithSys).FSys().(*MapFS)
}

// p1Entry renders one Entries yield. Data carries a symlink's target as well as
// a file's bytes, which is exactly the distinction AppendFile through a broken
// link makes visible, so both are recorded.
func p1Entry(path string, file *fstest.MapFile) []any {
	return []any{path, p1Mode(file.Mode), len(file.Data), string(file.Data)}
}

func p1Entries(m *MapFS, stop int, withTimes bool) []any {
	out := []any{}
	for path, file := range m.Entries() {
		if stop >= 0 && len(out) >= stop {
			break
		}
		row := p1Entry(path, file)
		if withTimes {
			row = append(row, file.ModTime.UTC().Format(time.RFC3339Nano))
		}
		out = append(out, row)
	}
	return out
}

// p1World is one trace's state: the filesystem under test and the one open
// handle a handle-shaped trace carries.
type p1World struct {
	m          *MapFS
	handle     fs.File
	handlePath string
}

func (w *p1World) fs(op string) *MapFS {
	if w.m == nil {
		panic("phase1: action " + op + " ran before a filesystem was built")
	}
	return w.m
}

func (w *p1World) open(op string) fs.File {
	if w.handle == nil {
		panic("phase1: action " + op + " ran with no open handle")
	}
	return w.handle
}

//nolint:gocyclo // One switch per action keeps the vocabulary in one place.
func p1Replay(request p1Request) []any {
	world := &p1World{}
	ordered := []any{}
	for _, a := range p1Decode(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		// ---- construction -------------------------------------------------
		case "from_map":
			built := FromMap(p1Input(a.Op, a.Files), p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive))
			world.m = p1Underlying(built)
			row["result"] = []any{"built", len(a.Files)}
		case "from_empty_map":
			// FromMap over a nil map: the starting point of every pinned
			// writable-filesystem test, and the only way to observe the
			// top-level parent rules on a filesystem that has no drive root.
			built := FromMap[any](nil, p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive))
			world.m = p1Underlying(built)
			row["result"] = []any{"built", 0}
		case "from_map_with_clock":
			built := FromMapWithClock(
				p1Input(a.Op, a.Files),
				p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive),
				p1NewStep(),
			)
			world.m = p1Underlying(built)
			row["result"] = []any{"built", len(a.Files)}
		case "from_map_guarded":
			// Builds a throwaway: the case's subject is the rejection, and a
			// rejected map must not become the filesystem later actions use.
			sensitive := p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive)
			files := p1Input(a.Op, a.Files)
			_, panicked := p1Guard(func() any { return FromMap(files, sensitive) })
			row["result"] = []any{"guarded", panicked}
		case "convert_map_fs":
			world.m = convertMapFS(
				p1RawInput(a.Op, a.Files),
				p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive),
				p1NewStep(),
			)
			row["result"] = []any{"built", len(a.Files)}
		case "convert_map_fs_guarded":
			sensitive := p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive)
			input := p1RawInput(a.Op, a.Files)
			_, panicked := p1Guard(func() any { return convertMapFS(input, sensitive, p1NewStep()) })
			row["result"] = []any{"guarded", panicked}

		// ---- enumeration --------------------------------------------------
		// Enumeration is guarded because Entries asserts every entry's Sys to
		// *sys (vfstest.go:654). An entry written through `set` alone carries
		// none, so a trace whose subject is that difference makes the pinned
		// enumeration fail, and the failure is the observation.
		case "entries":
			m := world.fs(a.Op)
			row["result"], row["panic"] = p1Guard(func() any { return []any{"entries", p1Entries(m, -1, false)} })
		case "entries_with_times":
			m := world.fs(a.Op)
			row["result"], row["panic"] = p1Guard(func() any { return []any{"entries", p1Entries(m, -1, true)} })
		case "entries_break":
			m, stop := world.fs(a.Op), p1NeedInt(a.Op, "stop", a.Stop)
			row["result"], row["panic"] = p1Guard(func() any { return []any{"entries", p1Entries(m, stop, false)} })

		// ---- point reads --------------------------------------------------
		case "get_file_info":
			info := world.fs(a.Op).GetFileInfo(p1Need(a.Op, "path", a.Path))
			if info == nil {
				row["result"] = []any{"absent"}
			} else {
				row["result"] = []any{"info", p1Mode(info.Mode), len(info.Data), string(info.Data), p1Stored(info)}
			}
			row["path"] = a.Path
		case "get_mod_time":
			stamp := world.fs(a.Op).GetModTime(p1Need(a.Op, "path", a.Path))
			if stamp.IsZero() {
				row["result"] = []any{"zero", ""}
			} else {
				row["result"] = []any{"stamp", stamp.UTC().Format(time.RFC3339Nano)}
			}
			row["path"] = a.Path
		case "mod_time_relation":
			// For a filesystem built on the default clock, where no value is
			// reproducible but the order the pin creates entries in is. The
			// relation is deliberately non-strict: clockImpl.Now is time.Now,
			// whose readings are non-decreasing but not distinct, and a probe
			// run on this host saw two files created in sequence take the same
			// truncated instant. Only `not_after` is a contract; `before` is a
			// property of the host's clock resolution.
			left := world.fs(a.Op).GetModTime(p1Need(a.Op, "path", a.Path))
			right := world.fs(a.Op).GetModTime(p1Need(a.Op, "other", a.Other))
			relation := "not_after"
			switch {
			case left.IsZero() || right.IsZero():
				relation = "missing"
			case left.After(right):
				relation = "after"
			}
			row["result"] = []any{"relation", relation}
			row["path"], row["other"] = a.Path, a.Other
		case "get_target_of_symlink":
			target, ok := world.fs(a.Op).GetTargetOfSymlink(p1Need(a.Op, "path", a.Path))
			row["result"] = []any{"target", target, ok}
			row["path"] = a.Path
		case "realpath":
			value, err := world.fs(a.Op).Realpath(p1Key(p1Need(a.Op, "path", a.Path)))
			if err != nil {
				row["result"] = append([]any{"err"}, p1Err(err)...)
			} else {
				row["result"] = []any{"ok", value}
			}
			row["path"] = a.Path

		// ---- resolution ---------------------------------------------------
		case "resolve":
			m := world.fs(a.Op)
			file, canonical, err := m.getFollowingSymlinks(m.getCanonicalPath(p1Key(p1Need(a.Op, "path", a.Path))))
			row["result"] = p1Resolved(file, canonical, err)
			row["path"] = a.Path
		case "resolve_worker":
			// Seeds symlinkFrom/symlinkTo directly, which is the only way to
			// pin the worker's error pair independently of the entry point.
			m := world.fs(a.Op)
			file, canonical, err := m.getFollowingSymlinksWorker(
				m.getCanonicalPath(p1Key(p1Need(a.Op, "path", a.Path))),
				canonicalPath(a.From),
				canonicalPath(a.To),
			)
			row["result"] = p1Resolved(file, canonical, err)
			row["path"], row["from"], row["to"] = a.Path, a.From, a.To
		case "is_broken_symlink":
			m := world.fs(a.Op)
			_, _, err := m.getFollowingSymlinks(m.getCanonicalPath(p1Key(p1Need(a.Op, "path", a.Path))))
			row["result"] = []any{"broken", isBrokenSymlinkError(err), errors.Is(err, fs.ErrNotExist)}
			row["path"] = a.Path

		// ---- pure helpers -------------------------------------------------
		case "canonical":
			row["result"] = []any{"canonical", string(world.fs(a.Op).getCanonicalPath(a.Path))}
			row["path"] = a.Path
		case "compare_paths":
			left, right := p1Need(a.Op, "path", a.Path), p1Need(a.Op, "other", a.Other)
			row["result"] = []any{"compare", comparePathsByParts(left, right), strings.Compare(left, right)}
			row["path"], row["other"] = left, right
		case "split_path":
			before, after := splitPath(a.Path, p1NeedInt(a.Op, "offset", a.Offset))
			row["result"] = []any{"split", before, after}
			row["path"], row["offset"] = a.Path, int64(p1NeedInt(a.Op, "offset", a.Offset))
		case "dir_name":
			row["result"] = []any{"dir_name", dirName(a.Path)}
			row["path"] = a.Path
		case "base_name":
			row["result"] = []any{"base_name", baseName(a.Path)}
			row["path"] = a.Path
		case "symlink_value":
			value := Symlink(p1Need(a.Op, "target", a.Target))
			row["result"] = []any{"symlink", p1Mode(value.Mode), string(value.Data), value.ModTime.IsZero(), p1Sys(value.Sys)}
			row["target"] = a.Target

		// ---- mutation -----------------------------------------------------
		case "write_file":
			err := world.fs(a.Op).WriteFile(p1Key(p1Need(a.Op, "path", a.Path)), a.Data, p1Perm(a.Op, a.Perm))
			row["result"] = p1Err(err)
			row["path"] = a.Path
		case "append_file":
			err := world.fs(a.Op).AppendFile(p1Key(p1Need(a.Op, "path", a.Path)), a.Data, p1Perm(a.Op, a.Perm))
			row["result"] = p1Err(err)
			row["path"] = a.Path
		case "remove":
			row["result"] = p1Err(world.fs(a.Op).Remove(p1Key(p1Need(a.Op, "path", a.Path))))
			row["path"] = a.Path
		case "remove_inner":
			// The lock-free form, where the recursion and the prefix guard live.
			row["result"] = p1Err(world.fs(a.Op).remove(p1Key(p1Need(a.Op, "path", a.Path))))
			row["path"] = a.Path
		case "mkdir_all":
			m, path, perm := world.fs(a.Op), p1Key(p1Need(a.Op, "path", a.Path)), p1Perm(a.Op, a.Perm)
			value, panicked := p1Guard(func() any { return p1Err(m.MkdirAll(path, perm)) })
			row["result"], row["panic"] = value, panicked
			row["path"] = a.Path
		case "mkdir_all_inner":
			// The lock-free form. A request path of "/" reaches it with the
			// empty string, which is the `empty path` panic at vfstest.go:327.
			m, perm := world.fs(a.Op), p1Perm(a.Op, a.Perm)
			path := p1Key(p1Need(a.Op, "path", a.Path))
			value, panicked := p1Guard(func() any { return p1Err(m.mkdirAll(path, perm)) })
			row["result"], row["panic"] = value, panicked
			row["path"] = a.Path
		case "chtimes":
			// Chtimes is the one mutating method that does not strip a leading
			// separator of its own (vfstest.go:605-617), so the request's exact
			// spelling is forwarded and the asymmetry stays observable.
			err := world.fs(a.Op).Chtimes(
				p1Need(a.Op, "path", a.Path),
				p1Time(a.Op, "a_time", a.ATime),
				p1Time(a.Op, "m_time", a.MTime),
			)
			row["result"] = p1Err(err)
			row["path"] = a.Path
		case "add_symlink":
			m := world.fs(a.Op)
			path, target := p1Key(p1Need(a.Op, "path", a.Path)), p1Need(a.Op, "target", a.Target)
			_, panicked := p1Guard(func() any { m.AddSymlink(path, target); return nil })
			row["result"] = []any{"guarded", panicked}
			row["path"], row["target"] = a.Path, a.Target
		case "set_entry":
			m := world.fs(a.Op)
			realpath, canonical := a.Realpath, canonicalPath(a.Canonical)
			file := *p1RawFile(p1Need(a.Op, "kind", a.Kind), a.Data)
			if p1NeedBool(a.Op, "has_sys", a.HasSys) {
				file.Sys = p1SysValue
			}
			_, panicked := p1Guard(func() any { m.setEntry(realpath, canonical, file); return nil })
			row["result"] = []any{"guarded", panicked}
			row["realpath"], row["canonical"] = a.Realpath, a.Canonical
		case "set_raw":
			// set alone: no Sys wrapper and no symlink registration, which is
			// what makes the resulting entry indistinguishable from one
			// fstest.MapFS synthesized.
			m := world.fs(a.Op)
			m.set(canonicalPath(p1Need(a.Op, "canonical", a.Canonical)), p1RawFile(p1Need(a.Op, "kind", a.Kind), a.Data))
			row["result"] = []any{"set", a.Canonical}
			row["canonical"] = a.Canonical

		// ---- handles ------------------------------------------------------
		case "open":
			m, path := world.fs(a.Op), p1Key(p1Need(a.Op, "path", a.Path))
			value, panicked := p1Guard(func() any {
				handle, err := m.Open(path)
				if err != nil {
					return append([]any{"err"}, p1Err(err)...)
				}
				world.handle, world.handlePath = handle, a.Path
				info, statErr := handle.Stat()
				if statErr != nil {
					return append([]any{"stat_err"}, p1Err(statErr)...)
				}
				_, isDir := handle.(fs.ReadDirFile)
				return []any{"open", info.Name(), info.IsDir(), info.Size(), p1Sys(info.Sys()), isDir}
			})
			row["result"], row["panic"] = value, panicked
			row["path"] = a.Path
		case "open_inner":
			// m.open bypasses MapFS.Open's convertInfo gate entirely, which is
			// how a synthesized parent can be observed instead of panicking.
			m := world.fs(a.Op)
			key := canonicalPath(p1Need(a.Op, "canonical", a.Canonical))
			value, panicked := p1Guard(func() any {
				handle, err := m.open(key)
				if err != nil {
					return append([]any{"err"}, p1Err(err)...)
				}
				info, statErr := handle.Stat()
				if statErr != nil {
					return append([]any{"stat_err"}, p1Err(statErr)...)
				}
				_, converted := convertInfo(info)
				return []any{"open", info.Name(), info.IsDir(), converted}
			})
			row["result"], row["panic"] = value, panicked
			row["canonical"] = a.Canonical
		case "handle_stat":
			handle := world.open(a.Op)
			value, panicked := p1Guard(func() any {
				first, err := handle.Stat()
				if err != nil {
					return append([]any{"err"}, p1Err(err)...)
				}
				second, secondErr := handle.Stat()
				if secondErr != nil {
					return append([]any{"err"}, p1Err(secondErr)...)
				}
				return []any{"stat", first.Name(), first.IsDir(), p1Sys(first.Sys()), first == second}
			})
			row["result"], row["panic"] = value, panicked
			row["path"] = world.handlePath
		case "read_dir":
			handle := world.open(a.Op)
			reader, ok := handle.(fs.ReadDirFile)
			if !ok {
				panic("phase1: read_dir on a handle that is not a directory")
			}
			count := p1NeedInt(a.Op, "n", a.N)
			value, panicked := p1Guard(func() any {
				list, err := reader.ReadDir(count)
				if err != nil {
					return append([]any{"err"}, p1Err(err)...)
				}
				names := []any{}
				for _, entry := range list {
					info, infoErr := entry.Info()
					if infoErr != nil {
						return append([]any{"err"}, p1Err(infoErr)...)
					}
					names = append(names, []any{entry.Name(), entry.IsDir(), p1Sys(info.Sys())})
				}
				return []any{"entries", names}
			})
			row["result"], row["panic"] = value, panicked
			row["n"] = int64(count)
		case "convert_info_raw":
			// Builds a bare fstest.MapFS that vfstest never touched, so the
			// info convertInfo is handed carries no *sys at all.
			raw := p1RawInput(a.Op, a.Files)
			value, panicked := p1Guard(func() any {
				handle, err := raw.Open(p1Key(p1Need(a.Op, "path", a.Path)))
				if err != nil {
					return append([]any{"err"}, p1Err(err)...)
				}
				info, statErr := handle.Stat()
				if statErr != nil {
					return append([]any{"err"}, p1Err(statErr)...)
				}
				_, converted := convertInfo(info)
				return []any{"convert", info.Name(), info.IsDir(), converted}
			})
			row["result"], row["panic"] = value, panicked
			row["path"] = a.Path
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func p1Resolved(file *fstest.MapFile, canonical canonicalPath, err error) []any {
	if err != nil {
		return append([]any{"err", string(canonical)}, p1Err(err)...)
	}
	return []any{"ok", string(canonical), p1Mode(file.Mode), string(file.Data)}
}

// ---- the paired snapshot subject -------------------------------------------
//
// MapFS has no snapshot operation and none is invented here. What it does have
// is a published view: GetFileInfo hands back the live *fstest.MapFile
// (vfstest.go:671), so holding one is how a caller keeps an earlier reading of
// the filesystem. That is the construct the Rust MemorySnapshot is paired with,
// and the two answer the same five questions.
//
// The pin's answer to the last of them is not the port's. WriteFile replaces
// the map entry with a fresh MapFile (vfstest.go:540), so a held view keeps its
// original bytes; Chtimes instead assigns through the stored pointer
// (vfstest.go:610-615), so a held view's modification time moves under it.

type p1View struct {
	paths []string
	held  map[string]*fstest.MapFile
	stamp map[string][]any
}

func p1Stamp(file *fstest.MapFile) []any {
	if file == nil {
		return []any{"absent", "", false, ""}
	}
	return []any{
		"present",
		string(file.Data),
		file.Mode.IsDir(),
		file.ModTime.UTC().Format(time.RFC3339Nano),
	}
}

func p1SnapshotReplay(request p1Request) []any {
	var m *MapFS
	views := map[string]*p1View{}
	need := func(op, name string) *p1View {
		view, ok := views[name]
		if !ok {
			panic("phase1: action " + op + " names an unpublished view: " + name)
		}
		return view
	}
	ordered := []any{}
	for _, a := range p1Decode(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "build":
			m = p1Underlying(FromMapWithClock(
				p1Input(a.Op, a.Files),
				p1NeedBool(a.Op, "case_sensitive", a.CaseSensitive),
				p1NewStep(),
			))
			row["result"] = []any{"built", len(a.Files)}
		case "publish":
			if m == nil {
				panic("phase1: publish ran before build")
			}
			if len(a.Paths) == 0 {
				panic("phase1: publish needs the paths it publishes")
			}
			view := &p1View{paths: slices.Clone(a.Paths), held: map[string]*fstest.MapFile{}, stamp: map[string][]any{}}
			contents := []any{}
			for _, path := range view.paths {
				file := m.GetFileInfo(path)
				view.held[path] = file
				view.stamp[path] = p1Stamp(file)
				if file == nil {
					contents = append(contents, []any{path, "absent", ""})
				} else {
					contents = append(contents, []any{path, "present", string(file.Data)})
				}
			}
			views[p1Need(a.Op, "name", a.Name)] = view
			row["result"] = []any{"published", a.Name, contents}
			row["name"] = a.Name
		case "mutate_write":
			if m == nil {
				panic("phase1: mutate_write ran before build")
			}
			row["result"] = p1Err(m.WriteFile(p1Key(p1Need(a.Op, "path", a.Path)), a.Data, p1Perm(a.Op, a.Perm)))
			row["path"] = a.Path
		case "mutate_chtimes":
			if m == nil {
				panic("phase1: mutate_chtimes ran before build")
			}
			// The request's own spelling is forwarded, as p1Replay's chtimes
			// arm does. Chtimes is the one mutating method that strips no
			// leading separator of its own (vfstest.go:605-617), so p1Key
			// must not be applied here: the pin would not have applied it,
			// and applying it would make this side call a path the paired
			// Rust side never sees. The request therefore carries the key
			// spelling for this action, and a rooted one would miss.
			row["result"] = p1Err(m.Chtimes(
				p1Need(a.Op, "path", a.Path),
				p1Time(a.Op, "a_time", a.ATime),
				p1Time(a.Op, "m_time", a.MTime),
			))
			row["path"] = a.Path
		case "read_view":
			view := need(a.Op, p1Need(a.Op, "name", a.Name))
			file, ok := view.held[p1Need(a.Op, "path", a.Path)]
			if !ok {
				panic("phase1: read_view names a path the view never published: " + a.Path)
			}
			if file == nil {
				row["result"] = []any{"absent", ""}
			} else {
				row["result"] = []any{"bytes", string(file.Data)}
			}
			row["name"], row["path"] = a.Name, a.Path
		case "read_live":
			if m == nil {
				panic("phase1: read_live ran before build")
			}
			file := m.GetFileInfo(p1Need(a.Op, "path", a.Path))
			if file == nil {
				row["result"] = []any{"absent", ""}
			} else {
				row["result"] = []any{"bytes", string(file.Data)}
			}
			row["path"] = a.Path
		case "view_unchanged":
			view := need(a.Op, p1Need(a.Op, "name", a.Name))
			unchanged := true
			for _, path := range view.paths {
				if !slices.Equal(p1Stamp(view.held[path]), view.stamp[path]) {
					unchanged = false
				}
			}
			row["result"] = []any{"unchanged", unchanged}
			row["name"] = a.Name
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

// p1ReplayFor picks the trace player for a request this group owns. A case
// outside the group's prefix is not this probe's: the schedule is shared with
// the other filesystem groups, and every request still gets a row. A case
// inside it with an unknown subject is a defect in this group's own request
// file, so it fails the probe rather than quietly reporting the case
// unavailable.
func p1ReplayFor(request p1Request) func(p1Request) []any {
	if !strings.HasPrefix(request.Case, p1CasePrefix) {
		return nil
	}
	switch request.Subject {
	case "MapFS":
		return p1Replay
	case "MapFSSnapshot":
		return p1SnapshotReplay
	}
	panic("phase1: case " + request.Case + " has unknown subject " + request.Subject)
}

func TestPhase1FilesystemVfstest(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []p1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if replay := p1ReplayFor(request); replay != nil {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the vfstest probe"
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
