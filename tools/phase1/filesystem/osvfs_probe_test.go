package osvfs

// Access only: replays an ordered action trace against the pinned live-OS
// adapter and records what each action observed.
//
// It is an IN-PACKAGE file, unlike the leaf probes, because half of this
// group's roster is unexported: swapCase, isReparsePoint, osFSRealpath,
// getLimitedWalkDirFunc, putLimitedWalkDirFunc, limitedWalkDirFunc.walker and
// the three write layers (writeFileWithFlag, ensureDirectoryExists,
// writeFileEnsuringDir) have no exported entry point, and the whole point of
// the three-layer write split is that the layers behave differently from each
// other. Every declaration here is prefixed `phase1` so it cannot collide with
// the pinned in-package test files that compile alongside it (helpers_test.go
// declares mklink, realpath_test.go declares setupSymlinks).
//
// Five rules keep a live-filesystem observation portable and safe.
//
//  1. Every mutation happens inside a PER-CASE temporary root created by the
//     replay itself. Nothing is written outside it, and no action is given a
//     relative path that could resolve to a real file: `Remove`, `Chtimes` and
//     `writeFileWithFlag` have no rootedness assert, so a relative path handed
//     to them would act on the process working directory, which under `go
//     test` is the pinned package directory.
//  2. No absolute host path is ever recorded. phase1Place rewrites a result to
//     <root>/... or <realroot>/... and reduces anything else to a class, so a
//     row never carries /var/folders/... or a home directory.
//  3. An OS error is reduced to a portable class: the *PathError Op (which is
//     the pinned contract -- realpath_linux reports "open" where
//     realpath_other reports "lstat") plus an errno class. The message text
//     names the path and the toolchain's wording, and no Rust port could
//     reproduce it.
//  4. A panic the PINNED SOURCE raises is recorded with its literal text,
//     because that text is the contract: internal.RootLength's
//     `vfs: path %q is not absolute`. The probe only ever hands it a relative
//     path it chose itself, so the quoted path in that text is portable.
//     A runtime panic is reduced to a class instead.
//  5. Timestamps are recorded as the exact nanoseconds the probe SET, never as
//     a wall clock reading. A natural mtime is recorded only as a class. That
//     is also how the filesystem's timestamp precision is observed: the probe
//     writes a value with a nanosecond remainder and reads back what survived,
//     instead of sleeping.
//
// An unknown or malformed action is a harness failure on both sides, never an
// observation: phase1Str panics on an absent key rather than defaulting, and
// every replay's default branch panics on an unknown op.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/nativepath"
	"github.com/microsoft/TypeScript/tsc/internal/osutil"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	tsvfs "github.com/microsoft/TypeScript/tsc/internal/vfs"
	"golang.org/x/sys/unix"
)

// ---------------------------------------------------------------- requests --

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's vocabulary.
	Actions json.RawMessage `json:"actions"`
}

type phase1Act map[string]json.RawMessage

func phase1Actions(raw json.RawMessage) []phase1Act {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Act
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1Str reads a required string key. An absent key is a harness failure,
// never a default: a defaulted key would let both sides agree on a row neither
// of them executed.
func phase1Str(a phase1Act, key string) string {
	raw, ok := a[key]
	if !ok {
		panic("phase1: action " + phase1Op(a) + " is missing the key " + key)
	}
	var s string
	if err := json.Unmarshal(raw, &s); err != nil {
		panic("phase1: action " + phase1Op(a) + " key " + key + " is not a string")
	}
	return s
}

func phase1Int(a phase1Act, key string) int64 {
	raw, ok := a[key]
	if !ok {
		panic("phase1: action " + phase1Op(a) + " is missing the key " + key)
	}
	var n int64
	if err := json.Unmarshal(raw, &n); err != nil {
		panic("phase1: action " + phase1Op(a) + " key " + key + " is not an integer")
	}
	return n
}

func phase1Strings(a phase1Act, key string) []string {
	raw, ok := a[key]
	if !ok {
		panic("phase1: action " + phase1Op(a) + " is missing the key " + key)
	}
	var out []string
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: action " + phase1Op(a) + " key " + key + " is not a string array")
	}
	return out
}

func phase1Op(a phase1Act) string {
	raw, ok := a["op"]
	if !ok {
		panic("phase1: action has no op")
	}
	var s string
	if err := json.Unmarshal(raw, &s); err != nil || s == "" {
		panic("phase1: action op is not a nonempty string")
	}
	return s
}

// ------------------------------------------------------------- environment --

// phase1Env is one case's world: a private temporary root and the pinned
// adapter. The root is created per case, so no case can observe another's
// mutations and a failed case cannot leave a later one a different filesystem.
type phase1Env struct {
	root     string // normalized posix spelling of the temp root
	realRoot string // the same root after the pin's own Realpath
	fsys     tsvfs.FS
	impl     *osFS
}

func phase1NewEnv(t *testing.T) *phase1Env {
	t.Helper()
	dir, err := os.MkdirTemp("", "phase1-osvfs-")
	if err != nil {
		panic("phase1: cannot create a per-case temporary root: " + err.Error())
	}
	t.Cleanup(func() { phase1Cleanup(dir) })
	root := tspath.NormalizePath(dir)
	resolvedRoot := root
	if resolved, err := nativepath.Realpath(dir); err == nil {
		resolvedRoot = tspath.NormalizePath(resolved)
	}
	impl, ok := osVFS.(*osFS)
	if !ok {
		panic("phase1: the package osVFS is not an *osFS")
	}
	return &phase1Env{root: root, realRoot: resolvedRoot, fsys: FS(), impl: impl}
}

// phase1Cleanup restores traversal permission before removing the tree: cases
// that observe the denied-directory branch chmod a directory to 0o000, and a
// plain RemoveAll over one of those fails.
func phase1Cleanup(dir string) {
	_ = filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			_ = os.Chmod(path, 0o700)
		}
		return nil
	})
	_ = os.RemoveAll(dir)
}

// phase1Path joins a case-relative name onto the per-case root. "" is the root
// itself. The result is rooted and normalized, which is what the vfs API
// requires of every caller.
func (e *phase1Env) path(name string) string {
	if name == "" {
		return e.root
	}
	return e.root + "/" + name
}

// nameToken renders a basename. It is the probe's own spelling for every
// fixture path, but the per-case root's basename is a random temporary name,
// so that one is recorded as a marker plus the property the case actually
// cares about: that the pin answers a BASENAME rather than the full path.
func (e *phase1Env) nameToken(rel, name string) []any {
	if rel == "" {
		return []any{"<root-basename>", name != "", !strings.Contains(name, "/")}
	}
	tail := rel[strings.LastIndex(rel, "/")+1:]
	if name == tail {
		return []any{name, true, true}
	}
	return []any{e.place(name), false, !strings.Contains(name, "/")}
}

// phase1Place rewrites an observed path so no host path reaches the record.
func (e *phase1Env) place(s string) string {
	switch {
	case s == "":
		return "<empty>"
	case s == e.root:
		return "<root>"
	case strings.HasPrefix(s, e.root+"/"):
		return "<root>/" + s[len(e.root)+1:]
	case s == e.realRoot:
		return "<realroot>"
	case strings.HasPrefix(s, e.realRoot+"/"):
		return "<realroot>/" + s[len(e.realRoot)+1:]
	case s == e.root+"/":
		return "<root>/"
	case filepath.IsAbs(s), strings.HasPrefix(s, "/"):
		return "<other-absolute>"
	default:
		// A spelling the probe itself supplied, such as a DOS or URL root form
		// or a deliberately relative path. Recording it literally is safe
		// because the probe, not the host, chose it.
		return "literal:" + s
	}
}

// ---------------------------------------------------------------- reducers --

func phase1Errno(err error) string {
	switch {
	case err == nil:
		return "nil"
	case errors.Is(err, syscall.ENOTDIR):
		return "not_a_directory"
	case errors.Is(err, syscall.EISDIR):
		return "is_a_directory"
	case errors.Is(err, syscall.ELOOP):
		return "too_many_links"
	case errors.Is(err, syscall.ENOTEMPTY):
		return "directory_not_empty"
	case errors.Is(err, syscall.ENAMETOOLONG):
		return "name_too_long"
	case errors.Is(err, syscall.EBADF):
		return "bad_file_descriptor"
	case errors.Is(err, syscall.EINVAL), errors.Is(err, fs.ErrInvalid):
		return "invalid_argument"
	case errors.Is(err, fs.ErrNotExist):
		return "not_exist"
	case errors.Is(err, fs.ErrPermission):
		return "permission_denied"
	case errors.Is(err, fs.ErrExist):
		return "exists"
	default:
		return "other"
	}
}

// phase1Refusal is the one error the probe's own walk callbacks return. It is
// recognised by name so a case whose subject is error PROPAGATION records that
// the callback's error came back, rather than the class "other".
var phase1Refusal = errors.New("phase1: probe callback refusal")

// phase1ContractualOps are the syscall names a *PathError Op may carry into a
// record. The Op is kept for these because it IS the pinned contract on this
// group's most load-bearing branch: realpath_linux.go:48 reports Op "open"
// where realpath_other.go's EvalSymlinks reports "lstat" for the same dangling
// link, and writeFileEnsuringDir is distinguished from a write-only port by
// returning "mkdir" rather than "open". Any other Op is an internal name of
// the Go runtime's own traversal -- os.RemoveAll reports "openfdat" -- which
// is toolchain wording rather than pinned source, so only the shape is kept.
var phase1ContractualOps = []string{
	"chmod", "chtimes", "lstat", "link", "mkdir", "open", "read", "readdir",
	"readfile", "readlink", "remove", "rename", "stat", "symlink", "truncate", "write",
}

// phase1Err reduces an error to a class.
func phase1Err(err error) []any {
	if err == nil {
		return []any{"nil"}
	}
	if errors.Is(err, phase1Refusal) {
		return []any{"plain", "probe_callback_refusal"}
	}
	var pe *os.PathError
	if errors.As(err, &pe) {
		op := pe.Op
		if !slices.Contains(phase1ContractualOps, op) {
			op = "internal"
		}
		return []any{"patherror", op, phase1Errno(pe.Err)}
	}
	var le *os.LinkError
	if errors.As(err, &le) {
		return []any{"linkerror", le.Op, phase1Errno(le.Err)}
	}
	if strings.Contains(err.Error(), "too many links") {
		// filepath.EvalSymlinks reports a symlink cycle as a bare
		// *errors.errorString, not a *PathError. The sentence is stdlib
		// wording rather than pinned source, so only the shape is recorded.
		return []any{"plain", "too_many_links"}
	}
	return []any{"plain", phase1Errno(err)}
}

// phase1Guarded runs f and converts a panic into a recorded class, so a case
// whose subject IS the panic boundary observes it instead of failing.
func phase1Guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = phase1Classify(r)
		}
	}()
	return f(), ""
}

func phase1Classify(r any) string {
	text := ""
	switch v := r.(type) {
	case error:
		text = v.Error()
	case string:
		text = v
	default:
		return "non_error_panic"
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

// phase1Mode renders a mode as permission bits plus a type letter. The full
// FileMode string carries bits whose spelling is Go's, not the contract's.
func phase1Mode(mode fs.FileMode) []any {
	kind := "regular"
	switch {
	case mode&fs.ModeDir != 0:
		kind = "dir"
	case mode&fs.ModeSymlink != 0:
		kind = "symlink"
	case mode&fs.ModeNamedPipe != 0:
		kind = "fifo"
	case mode&fs.ModeSocket != 0:
		kind = "socket"
	case mode&fs.ModeDevice != 0:
		kind = "device"
	case mode&fs.ModeIrregular != 0:
		kind = "irregular"
	}
	return []any{fmt.Sprintf("0o%03o", mode.Perm()), kind, mode.IsRegular(), mode.IsDir()}
}

// phase1Times reads both stamps as SECONDS and NANOSECONDS separately.
// os.FileInfo exposes only ModTime, and the whole subject of the Chtimes case
// is that the two arguments are carried independently, so the access time has
// to come from the raw stat. The two components are kept apart rather than
// combined: Timespec.Nano() overflows int64 past the year 2262, and one of the
// fixtures is deliberately further out than that.
func phase1Times(path string) (atime, mtime [2]int64, err error) {
	var st unix.Stat_t
	if err := unix.Stat(path, &st); err != nil {
		return [2]int64{}, [2]int64{}, err
	}
	return [2]int64{st.Atim.Sec, st.Atim.Nsec}, [2]int64{st.Mtim.Sec, st.Mtim.Nsec}, nil
}

func phase1Stamp(t [2]int64) []any { return []any{t[0], t[1]} }

func phase1SortedKeys(m map[string]struct{}) []any {
	names := make([]string, 0, len(m))
	for name := range m {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make([]any, 0, len(names))
	for _, name := range names {
		out = append(out, name)
	}
	return out
}

func phase1List(items []string) []any {
	out := make([]any, 0, len(items))
	for _, item := range items {
		out = append(out, item)
	}
	return out
}

// phase1Size records a size only where the number is the contract. A
// directory's size and a symlink's size are the host's: APFS reports 64 for an
// empty directory where ext4 reports 4096, and a symlink's size is the LENGTH
// OF ITS TARGET PATH, which here is a temporary root. Only a regular file's
// size is portable, and it is the only one this group's cases turn on.
func phase1Size(mode fs.FileMode, size int64) []any {
	if mode.IsRegular() {
		return []any{"exact", size}
	}
	return []any{"host_sized", size > 0}
}

func phase1Row(op string, result any, panicked string) map[string]any {
	return map[string]any{"op": op, "result": result, "panic": panicked}
}

// phase1Flag turns a request's flag name into the open flags the pinned write
// layers take. The flag is a parameter of writeFileWithFlag and
// writeFileEnsuringDir, so it travels in the request: a rule that lived only
// in this file would leave a Rust port reading the request unable to know it.
func phase1Flag(name string) int {
	switch name {
	case "wronly_create_trunc":
		return os.O_WRONLY | os.O_CREATE | os.O_TRUNC
	case "wronly_create_append":
		return os.O_WRONLY | os.O_CREATE | os.O_APPEND
	case "wronly_create_excl":
		return os.O_WRONLY | os.O_CREATE | os.O_EXCL
	case "rdonly":
		return os.O_RDONLY
	case "wronly":
		return os.O_WRONLY
	default:
		panic("phase1: unsupported open flag: " + name)
	}
}

// ---------------------------------------------------------------- fixtures --

// phase1Fixture builds a named on-disk shape inside the per-case root and
// returns what it actually created, in sorted order. It is an observation in
// its own right: if mkfifo or symlink is refused by the host, the row records
// the refusal instead of silently dropping the arm that depends on it.
func phase1Fixture(e *phase1Env, name string) []any {
	made := []any{}
	note := func(rel, kind string, err error) {
		made = append(made, []any{rel, kind, phase1Err(err)})
	}
	mk := func(rel string, perm fs.FileMode) {
		note(rel, "dir", os.MkdirAll(e.path(rel), perm))
	}
	wf := func(rel, content string) {
		note(rel, "file", os.WriteFile(e.path(rel), []byte(content), 0o666))
	}
	ln := func(target, rel string) {
		note(rel, "symlink", os.Symlink(e.path(target), e.path(rel)))
	}
	switch name {
	case "kinds":
		mk("real_dir", 0o777)
		wf("real_file.ts", "one")
		wf("empty.ts", "")
		ln("real_file.ts", "link_to_file")
		ln("real_dir", "link_to_dir")
		note("dangling", "symlink", os.Symlink(e.path("no_such_target"), e.path("dangling")))
		note("hard.ts", "hardlink", os.Link(e.path("real_file.ts"), e.path("hard.ts")))
		note("fifo", "fifo", syscall.Mkfifo(e.path("fifo"), 0o666))
		mk("denied", 0o777)
		wf("denied/hidden.ts", "hidden")
		note("denied", "chmod000", os.Chmod(e.path("denied"), 0o000))
	case "entries":
		// The pinned fixture of realpath_test.go:106-151, extended with the
		// kinds that test reaches for but never creates.
		mk("target", 0o777)
		mk("link", 0o777)
		wf("target/file1", "hello")
		wf("target/file2", "world")
		mk("target/dir1", 0o777)
		mk("target/dir2", 0o777)
		ln("target/file1", "link/file1")
		ln("target/file2", "link/file2")
		ln("target/dir1", "link/dir1")
		ln("target/dir2", "link/dir2")
		note("link/dangling", "symlink", os.Symlink(e.path("target/gone"), e.path("link/dangling")))
		note("link/fifo", "fifo", syscall.Mkfifo(e.path("link/fifo"), 0o666))
		note("link/hard", "hardlink", os.Link(e.path("target/file1"), e.path("link/hard")))
		wf("plain.ts", "plain")
		mk("empty_dir", 0o777)
		mk("blocked", 0o777)
		wf("blocked/inside.ts", "inside")
		note("blocked", "chmod000", os.Chmod(e.path("blocked"), 0o000))
	case "links":
		// A symlink chain, a cycle, and a link that resolves to an ancestor.
		mk("target", 0o777)
		wf("target/file", "hello")
		ln("target", "link")
		ln("link", "link2")
		ln("link2", "link3")
		note("dangling", "symlink", os.Symlink(e.path("nowhere"), e.path("dangling")))
		note("loop_a", "symlink", os.Symlink(e.path("loop_b"), e.path("loop_a")))
		note("loop_b", "symlink", os.Symlink(e.path("loop_a"), e.path("loop_b")))
		ln("", "self")
	case "tree":
		mk("tree/a", 0o777)
		mk("tree/c/c1", 0o777)
		wf("tree/a/a1", "1")
		wf("tree/a/a2", "2")
		wf("tree/b", "b")
		wf("tree/c/c1/c2", "c")
		ln("tree/c", "tree/a/linkc")
		// Named so it sorts BETWEEN a and b: the walk cases turn on what
		// happens AFTER the unreadable directory's two callbacks, and an
		// entry that sorted last would leave that unobserved.
		mk("tree/adenied", 0o777)
		wf("tree/adenied/hidden", "h")
		note("tree/adenied", "chmod000", os.Chmod(e.path("tree/adenied"), 0o000))
		mk("other/x", 0o777)
		wf("other/x/y", "y")
		wf("other/z", "z")
		ln("tree/a", "linkdir")
	case "empty":
		// Nothing: the case builds what it needs action by action.
	default:
		panic("phase1: unsupported fixture: " + name)
	}
	return made
}

// phase1Contents decodes a request's byte payload. Byte fixtures travel as hex
// because the ReadFile contract includes handing back invalid UTF-8 unchanged,
// which cannot survive encoding/json in either direction.
func phase1Bytes(a phase1Act, key string) []byte {
	raw, err := hex.DecodeString(phase1Str(a, key))
	if err != nil {
		panic("phase1: " + key + " is not hex: " + err.Error())
	}
	return raw
}

// -------------------------------------------------------------- swapCase --

func phase1ReplaySwapCase(t *testing.T, request phase1Request) []any {
	_ = t
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "swap_case":
			// The input travels as hex so a deliberately invalid UTF-8
			// fixture survives the request file: strings.Map decodes an
			// invalid byte as U+FFFD and writes the replacement back, which
			// is a byte-level contract a UTF-8-only request could not state.
			in := string(phase1Bytes(a, "input_hex"))
			value, panicked := phase1Guarded(func() any {
				out := swapCase(in)
				return []any{hex.EncodeToString([]byte(out)), len(out), len(in)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "swap_case_round_trip":
			in := string(phase1Bytes(a, "input_hex"))
			value, panicked := phase1Guarded(func() any {
				once := swapCase(in)
				twice := swapCase(once)
				return []any{hex.EncodeToString([]byte(twice)), twice == in}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ------------------------------------------------- case sensitivity probe --

// phase1HostClass records the host as the three PREDICATES the pinned sources
// actually branch on -- os.go:48 (windows), os.go:53 (wasm) and the build
// constraint that splits realpath_linux.go from realpath_other.go -- rather
// than as runtime.GOOS and runtime.GOARCH. Those two are Go's own platform
// vocabulary: this host answers "darwin" and "arm64" where Rust's
// std::env::consts answers "macos" and "aarch64", so a faithful port would be
// scored different for a reason that has nothing to do with the operation, and
// nothing in the request tells it to spell the platform Go's way. The literal
// GOOS stays in the case prose, where it is documentation and not an expected
// value.
func phase1HostClass() []any {
	return []any{
		runtime.GOOS == "windows",
		runtime.GOOS == "linux",
		runtime.GOARCH == "wasm",
	}
}

func phase1ReplayCaseSensitivity(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "use_case_sensitive_file_names":
			value, panicked := phase1Guarded(func() any {
				return []any{e.fsys.UseCaseSensitiveFileNames()}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "host":
			ordered = append(ordered, phase1Row(op, phase1HostClass(), ""))
		case "swap_case_of_executable":
			// The exact derivation os.go:59-75 performs at package init,
			// re-run here so the inputs of that one-time answer are visible.
			// Only classes are recorded: the executable path is a build
			// directory that differs between the two processes by definition.
			value, panicked := phase1Guarded(func() any {
				exe, err := osutil.Executable()
				if err != nil {
					return []any{"executable_error", phase1Err(err)}
				}
				swapped := swapCase(exe)
				_, statErr := os.Stat(swapped)
				return []any{
					"ok",
					swapped != exe,
					phase1Err(statErr),
					os.IsNotExist(statErr),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "cross_check_on_disk":
			// An independent answer from the per-case root, so the derived
			// boolean is checked against the volume the case actually writes
			// to rather than against the volume the executable lives on.
			upper := phase1Str(a, "created")
			lower := phase1Str(a, "probed")
			value, panicked := phase1Guarded(func() any {
				if err := os.WriteFile(e.path(upper), []byte("probe"), 0o666); err != nil {
					return []any{"write_error", phase1Err(err)}
				}
				_, err := os.Stat(e.path(lower))
				return []any{"ok", err == nil, e.fsys.FileExists(e.path(lower))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ---------------------------------------------------- osutil process identity --

func phase1ReplayProcessIdentity(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "args_class":
			// Never the literal argv: under `go test` it carries the test
			// binary's build directory and the -test.run selector, and the
			// Rust driver is a different process with different arguments.
			value, panicked := phase1Guarded(func() any {
				args := osutil.Args()
				argv0 := ""
				if len(args) > 0 {
					argv0 = args[0]
				}
				// NOT `argv0 == exe`: that is a property of how the process
				// was LAUNCHED, not of osutil.Args. Under `go test` argv[0]
				// is the full binary path and it records true; a faithful
				// port invoked through PATH, through a shell wrapper or
				// through `cargo run` records false. os_other.go:7-9 is a
				// verbatim `return os.Args`, so no wrong implementation of
				// the operation under test can make that element false
				// either -- the pass-through row below is the one that
				// actually pins it.
				return []any{
					len(args) >= 1,
					argv0 != "",
					slices.Equal(args, os.Args),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "args_repeat_identical":
			value, panicked := phase1Guarded(func() any {
				first, second := osutil.Args(), osutil.Args()
				return []any{slices.Equal(first, second), len(first) == len(second)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "executable_class":
			value, panicked := phase1Guarded(func() any {
				exe, err := osutil.Executable()
				if err != nil {
					return []any{"error", phase1Err(err)}
				}
				_, statErr := os.Stat(exe)
				resolved, resolveErr := nativepath.Realpath(exe)
				return []any{
					"ok",
					filepath.IsAbs(exe),
					statErr == nil,
					filepath.Base(exe) != "",
					resolveErr == nil && resolved == exe,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "executable_stable_across_chdir":
			// os.Executable resolves through the kernel, so a changed working
			// directory must not change it; argv[0] can.
			value, panicked := phase1Guarded(func() any {
				before, beforeErr := osutil.Executable()
				after, afterErr := "", error(nil)
				phase1InDirectory(e.root, func() {
					after, afterErr = osutil.Executable()
				})
				return []any{
					beforeErr == nil, afterErr == nil, before == after,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// phase1InDirectory runs body with the process working directory moved into
// dir and restores it afterwards. It is used only by the two actions whose
// subject IS the working directory; every other action uses rooted paths.
func phase1InDirectory(dir string, body func()) {
	previous, err := os.Getwd()
	if err != nil {
		panic("phase1: cannot read the working directory: " + err.Error())
	}
	if err := os.Chdir(dir); err != nil {
		panic("phase1: cannot enter the per-case root: " + err.Error())
	}
	defer func() {
		if err := os.Chdir(previous); err != nil {
			panic("phase1: cannot restore the working directory: " + err.Error())
		}
	}()
	body()
}

// --------------------------------------------------------------------- FS --

func phase1ReplayFS(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	var handles []tsvfs.FS
	ordered := []any{}
	at := func(index int64) tsvfs.FS {
		if index < 0 || int(index) >= len(handles) {
			panic(fmt.Sprintf("phase1: handle %d was never acquired", index))
		}
		return handles[index]
	}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "acquire":
			value, panicked := phase1Guarded(func() any {
				handle := FS()
				handles = append(handles, handle)
				same := len(handles) > 1 && handles[0] == handle
				return []any{len(handles) - 1, handle != nil, same}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write":
			handle, rel, content := at(phase1Int(a, "handle")), phase1Str(a, "path"), phase1Str(a, "content")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(handle.WriteFile(e.path(rel), content))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "read":
			handle, rel := at(phase1Int(a, "handle")), phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				contents, ok := handle.ReadFile(e.path(rel))
				return []any{hex.EncodeToString([]byte(contents)), ok}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "entries":
			handle, rel := at(phase1Int(a, "handle")), phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				entries := handle.GetAccessibleEntries(e.path(rel))
				return []any{
					phase1List(entries.Files),
					phase1List(entries.Directories),
					phase1SortedKeys(entries.Symlinks),
					entries.Symlinks != nil,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "symlink":
			target, rel := phase1Str(a, "target"), phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Symlink(e.path(target), e.path(rel)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// --------------------------------------------------------------- ReadFile --

func phase1ReplayReadFile(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "write_bytes":
			rel, raw := phase1Str(a, "path"), phase1Bytes(a, "content_hex")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.WriteFile(e.path(rel), raw, 0o666)), len(raw)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chmod":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Chmod(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "read":
			// (contents_hex, ok). The hex is load-bearing: the pin hands back
			// invalid UTF-8 unchanged, and encoding/json would replace it.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				contents, ok := e.fsys.ReadFile(e.path(rel))
				return []any{hex.EncodeToString([]byte(contents)), ok, len(contents)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "read_literal":
			// A spelling the probe supplies verbatim, for the relative-path
			// assert and the non-posix root forms.
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				contents, ok := e.fsys.ReadFile(literal)
				return []any{hex.EncodeToString([]byte(contents)), ok}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ------------------------------------------------------------------- Stat --

func phase1ReplayStat(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "umask":
			// phase1Mode records mode.Perm(), and the fixture creates with
			// os.MkdirAll(0o777) and os.WriteFile(0o666), so six of the rows
			// below carry 0o777 and 0o666 MASKED by this process's umask.
			// Without this row the bits are not reproducible across hosts.
			value, panicked := phase1Guarded(func() any {
				mask := syscall.Umask(0)
				syscall.Umask(mask)
				return []any{fmt.Sprintf("0o%03o", mask)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "stat":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				info := e.fsys.Stat(e.path(rel))
				if info == nil {
					return []any{"nil"}
				}
				// ModTime is a wall clock reading; only its shape is
				// recorded. The Chtimes case pins the values instead.
				return []any{
					"info", e.nameToken(rel, info.Name()),
					phase1Size(info.Mode(), info.Size()), info.IsDir(),
					phase1Mode(info.Mode()), !info.ModTime().IsZero(),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "stat_literal":
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				info := e.fsys.Stat(literal)
				if info == nil {
					return []any{"nil"}
				}
				return []any{
					"info", e.place(info.Name()),
					phase1Size(info.Mode(), info.Size()), info.IsDir(),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "stat_root_trailing_slash":
			value, panicked := phase1Guarded(func() any {
				info := e.fsys.Stat(e.root + "/")
				if info == nil {
					return []any{"nil"}
				}
				return []any{"info", info.IsDir(), info.Name() != ""}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ------------------------------------------ FileExists / DirectoryExists --

func phase1ReplayExists(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "exists":
			// Both predicates on one path in one row: the pair is the
			// contract, and a port that used lstat flips them together.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				return []any{e.fsys.FileExists(e.path(rel)), e.fsys.DirectoryExists(e.path(rel))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "exists_literal":
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				return []any{e.fsys.FileExists(literal), e.fsys.DirectoryExists(literal)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "exists_root_trailing_slash":
			value, panicked := phase1Guarded(func() any {
				return []any{
					e.fsys.FileExists(e.root + "/"),
					e.fsys.DirectoryExists(e.root + "/"),
					e.fsys.DirectoryExists(e.root),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "remove_and_recheck":
			// Cache detection: the same path answered twice with a real
			// mutation in between.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				before := e.fsys.FileExists(e.path(rel))
				removeErr := os.Remove(e.path(rel))
				after := e.fsys.FileExists(e.path(rel))
				return []any{before, phase1Err(removeErr), after}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ---------------------------------------------- GetAccessibleEntries --

func phase1ReplayEntries(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "entries":
			// Files and Directories are sorted by name: internal.go:123 calls
			// io/fs.ReadDir, os.DirFS implements fs.ReadDirFS, and os.ReadDir
			// sorts (os/dir.go:122-124). Symlinks is a Go map and is sorted
			// before it is recorded because map order is not reproducible.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				entries := e.fsys.GetAccessibleEntries(e.path(rel))
				return []any{
					phase1List(entries.Files),
					phase1List(entries.Directories),
					phase1SortedKeys(entries.Symlinks),
					entries.Symlinks != nil,
					len(entries.Files), len(entries.Directories), len(entries.Symlinks),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "entries_literal":
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				entries := e.fsys.GetAccessibleEntries(literal)
				return []any{
					phase1List(entries.Files), phase1List(entries.Directories),
					phase1SortedKeys(entries.Symlinks), entries.Symlinks != nil,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// --------------------------------------------------------------- Realpath --

func phase1ReplayRealpath(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	var remembered []string
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "realpath":
			// Through the exported method, which is the semaphore wrapper
			// around osFSRealpath.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				got := e.fsys.Realpath(e.path(rel))
				remembered = append(remembered, got)
				return []any{e.place(got), got == e.path(rel), !strings.Contains(got, "\\")}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_direct":
			// The unexported composition, so the rootedness assert and both
			// total-on-failure branches are visible without the semaphore.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				got := osFSRealpath(e.path(rel))
				return []any{e.place(got), got == e.path(rel)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_literal":
			// A root form the probe supplies verbatim: a DOS root, a UNC
			// root, an untitled root or a URL root is rooted to tspath and
			// meaningless to the host, so the total-on-failure branch returns
			// the input untouched. An answer equal to the input is recorded
			// as that input, because the probe chose it; anything else goes
			// through the usual reduction.
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				got := osFSRealpath(literal)
				if got == literal {
					return []any{"literal:" + literal, true}
				}
				return []any{e.place(got), false}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_equal":
			// The pinned realpath_test.go:13-33 assertion, as an observation:
			// the target and the path through the directory symlink resolve
			// to the same string.
			left, right := phase1Str(a, "left"), phase1Str(a, "right")
			value, panicked := phase1Guarded(func() any {
				a1, b1 := e.fsys.Realpath(e.path(left)), e.fsys.Realpath(e.path(right))
				return []any{e.place(a1), e.place(b1), a1 == b1}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_case_swapped":
			// Decisive against a port written as std::fs::canonicalize: on a
			// case-insensitive volume canonicalize corrects the spelling and
			// filepath.EvalSymlinks does not.
			rel, swapped := phase1Str(a, "path"), phase1Str(a, "swapped")
			value, panicked := phase1Guarded(func() any {
				exact := e.fsys.Realpath(e.path(rel))
				varied := e.fsys.Realpath(e.path(swapped))
				return []any{e.place(exact), e.place(varied), exact == varied}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "stable_repeat":
			// Two calls with a mutation in between, against a cached realpath.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				first := e.fsys.Realpath(e.path(rel))
				removeErr := os.Remove(e.path(rel))
				second := e.fsys.Realpath(e.path(rel))
				return []any{e.place(first), phase1Err(removeErr), e.place(second), first == second}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "remembered_count":
			ordered = append(ordered, phase1Row(op, []any{len(remembered)}, ""))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ------------------------------------------------------ nativepath.Realpath --

func phase1ReplayNativeRealpath(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "build":
			// Which implementation is behind the one signature. The two
			// builds differ in error identity, so the row that names the
			// build is what makes the error rows readable. Only the
			// PREDICATE is recorded: runtime.GOOS is Go's own spelling of
			// the platform and a Rust port has no way to produce it, while
			// the build constraint that selects realpath_linux.go over
			// realpath_other.go is exactly `linux`.
			ordered = append(ordered, phase1Row(op, []any{runtime.GOOS == "linux"}, ""))
		case "realpath":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				got, err := nativepath.Realpath(e.path(rel))
				return []any{e.place(got), phase1Err(err)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_trailing_slash":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				got, err := nativepath.Realpath(e.path(rel) + "/")
				return []any{e.place(got), phase1Err(err)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_relative":
			// The one action that needs the working directory, because
			// "a relative input stays relative" is the property being pinned.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				var got string
				var err error
				phase1InDirectory(e.root, func() { got, err = nativepath.Realpath(rel) })
				return []any{e.place(got), phase1Err(err), filepath.IsAbs(got)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_long":
			// Past the 256-byte buffer the linux build starts with, so the
			// doubling loop is entered there. On the other build it is just a
			// long path.
			depth, segment := phase1Int(a, "depth"), phase1Str(a, "segment")
			value, panicked := phase1Guarded(func() any {
				deep := e.root
				for range depth {
					deep += "/" + segment
				}
				if err := os.MkdirAll(deep, 0o777); err != nil {
					return []any{"setup_error", phase1Err(err)}
				}
				got, err := nativepath.Realpath(deep)
				return []any{"ok", phase1Err(err), len(got) > 256, strings.HasSuffix(got, segment)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "realpath_literal":
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				got, err := nativepath.Realpath(literal)
				return []any{e.place(got), phase1Err(err)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ----------------------------------------- IsSymlinkOrReparsePoint --

func phase1ReplaySymlinkPredicate(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "predicate":
			// Both entry points on one path: osvfs.isReparsePoint is
			// nativepath.IsSymlinkOrReparsePoint after filepath.FromSlash,
			// and on a posix host FromSlash is the identity, so the two must
			// agree on every input including one that contains a backslash.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				full := e.path(rel)
				direct := nativepath.IsSymlinkOrReparsePoint(full)
				wrapped := isReparsePoint(full)
				return []any{direct, wrapped, direct == wrapped}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "predicate_literal":
			literal := phase1Str(a, "literal")
			value, panicked := phase1Guarded(func() any {
				return []any{
					nativepath.IsSymlinkOrReparsePoint(literal),
					isReparsePoint(literal),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "make_backslash_name":
			// A file whose name contains a backslash. FromSlash leaves it
			// alone on posix, so both entry points must see the same name; a
			// port that translated separators would not.
			name := phase1Str(a, "name")
			value, panicked := phase1Guarded(func() any {
				target := e.path("real_file.ts")
				link := e.root + "/" + name
				return []any{phase1Err(os.Symlink(target, link)), strings.Contains(name, "\\")}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ------------------------------------------------------- the write layers --

// phase1WriteRow records what a write left behind: the error class, then the
// bytes and the stat of the target, and whether the parent exists.
func phase1WriteObserve(e *phase1Env, rel string, err error) []any {
	full := e.path(rel)
	row := []any{phase1Err(err)}
	if raw, readErr := os.ReadFile(full); readErr == nil {
		row = append(row, "content", hex.EncodeToString(raw), len(raw))
	} else {
		row = append(row, "unreadable", phase1Err(readErr), 0)
	}
	if info, statErr := os.Lstat(full); statErr == nil {
		row = append(row, phase1Mode(info.Mode()), phase1Size(info.Mode(), info.Size()))
	} else {
		row = append(row, []any{"absent"}, []any{"absent"})
	}
	parent := filepath.Dir(full)
	if info, statErr := os.Stat(parent); statErr == nil {
		row = append(row, "parent", phase1Mode(info.Mode()))
	} else {
		row = append(row, "parent", []any{"absent"})
	}
	return row
}

func phase1ReplayWrite(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "umask":
			// The created mode is 0o666 (files) or 0o777 (directories) masked
			// by the process umask, so the row is unreadable without it.
			value, panicked := phase1Guarded(func() any {
				mask := syscall.Umask(0)
				syscall.Umask(mask)
				return []any{fmt.Sprintf("0o%03o", mask)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write":
			rel, content := phase1Str(a, "path"), phase1Str(a, "content")
			value, panicked := phase1Guarded(func() any {
				return phase1WriteObserve(e, rel, e.fsys.WriteFile(e.path(rel), content))
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "append":
			rel, content := phase1Str(a, "path"), phase1Str(a, "content")
			value, panicked := phase1Guarded(func() any {
				return phase1WriteObserve(e, rel, e.fsys.AppendFile(e.path(rel), content))
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_literal":
			literal, content := phase1Str(a, "literal"), phase1Str(a, "content")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(e.fsys.WriteFile(literal, content))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "append_literal":
			literal, content := phase1Str(a, "literal"), phase1Str(a, "content")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(e.fsys.AppendFile(literal, content))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "inspect":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				return phase1WriteObserve(e, rel, nil)
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "truncate_externally":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Truncate(e.path(rel), 0))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chmod":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Chmod(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "parent_absent":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				_, err := os.Stat(e.path(rel))
				return []any{err == nil, phase1Err(err)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "lstat_kind":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				info, err := os.Lstat(e.path(rel))
				if err != nil {
					return []any{"absent", phase1Err(err)}
				}
				return []any{"present", phase1Mode(info.Mode())}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

func phase1ReplayWriteLayers(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "umask":
			value, panicked := phase1Guarded(func() any {
				mask := syscall.Umask(0)
				syscall.Umask(mask)
				return []any{fmt.Sprintf("0o%03o", mask)}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_with_flag":
			// The bottom layer: no rootedness assert, no directory creation.
			rel, content, flag := phase1Str(a, "path"), phase1Str(a, "content"), phase1Flag(phase1Str(a, "flag"))
			value, panicked := phase1Guarded(func() any {
				return phase1WriteObserve(e, rel, e.impl.writeFileWithFlag(e.path(rel), content, flag))
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_with_flag_literal":
			// A relative spelling, to pin that this layer does NOT assert
			// rootedness. The path is chosen so it cannot exist and cannot be
			// created: the write fails without touching the working directory.
			literal, content, flag := phase1Str(a, "literal"), phase1Str(a, "content"), phase1Flag(phase1Str(a, "flag"))
			value, panicked := phase1Guarded(func() any {
				err := e.impl.writeFileWithFlag(literal, content, flag)
				_, statErr := os.Stat(literal)
				return []any{phase1Err(err), statErr == nil}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "ensure_directory":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				err := e.impl.ensureDirectoryExists(e.path(rel))
				info, statErr := os.Lstat(e.path(rel))
				if statErr != nil {
					return []any{phase1Err(err), "absent", phase1Err(statErr)}
				}
				return []any{phase1Err(err), "present", phase1Mode(info.Mode())}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "ensure_directory_relative":
			// The same layer with a relative path, inside the per-case root so
			// the directory it really does create lands nowhere else.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				var err error
				var created bool
				phase1InDirectory(e.root, func() {
					err = e.impl.ensureDirectoryExists(rel)
					_, statErr := os.Stat(rel)
					created = statErr == nil
				})
				return []any{phase1Err(err), created}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_ensuring_dir":
			// The middle layer: asserts rootedness, writes first, and only
			// creates the parent after the first write has already failed.
			rel, content, flag := phase1Str(a, "path"), phase1Str(a, "content"), phase1Flag(phase1Str(a, "flag"))
			value, panicked := phase1Guarded(func() any {
				return phase1WriteObserve(e, rel, e.impl.writeFileEnsuringDir(e.path(rel), content, flag))
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_ensuring_dir_literal":
			literal, content, flag := phase1Str(a, "literal"), phase1Str(a, "content"), phase1Flag(phase1Str(a, "flag"))
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(e.impl.writeFileEnsuringDir(literal, content, flag))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "directory_snapshot":
			// The no-side-effect witness for the layering: the parent's
			// entries and mode before and after a write that succeeded on the
			// first attempt, so a port that ensures unconditionally is caught
			// even where the returned error is the same.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				entries, err := os.ReadDir(e.path(rel))
				if err != nil {
					return []any{"unreadable", phase1Err(err)}
				}
				names := make([]string, 0, len(entries))
				for _, entry := range entries {
					names = append(names, entry.Name())
				}
				sort.Strings(names)
				info, statErr := os.Stat(e.path(rel))
				mode := []any{"absent"}
				if statErr == nil {
					mode = phase1Mode(info.Mode())
				}
				return []any{"entries", phase1List(names), mode}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "mkdir":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.MkdirAll(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "write_file_raw":
			rel, content, mode := phase1Str(a, "path"), phase1Str(a, "content"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.WriteFile(e.path(rel), []byte(content), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chmod":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Chmod(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "symlink":
			target, rel := phase1Str(a, "target"), phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Symlink(e.path(target), e.path(rel)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ----------------------------------------------------------------- Remove --

func phase1ReplayRemove(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "remove":
			// The removal plus what survived it, in one row: the symlink arms
			// turn entirely on whether the TARGET is still there.
			rel := phase1Str(a, "path")
			survivors := phase1Strings(a, "survivors")
			value, panicked := phase1Guarded(func() any {
				err := e.fsys.Remove(e.path(rel))
				_, lstatErr := os.Lstat(e.path(rel))
				alive := []any{}
				for _, name := range survivors {
					_, statErr := os.Lstat(e.path(name))
					alive = append(alive, []any{name, statErr == nil})
				}
				return []any{phase1Err(err), lstatErr == nil, alive}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "remove_empty_path":
			// The pin has no rootedness assert here, so an empty path reaches
			// os.RemoveAll directly. It cannot name anything, which is why it
			// is the only non-rooted spelling this case is willing to pass.
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(e.fsys.Remove(""))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "exists":
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				_, lstatErr := os.Lstat(e.path(rel))
				return []any{
					lstatErr == nil,
					e.fsys.FileExists(e.path(rel)),
					e.fsys.DirectoryExists(e.path(rel)),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chmod":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Chmod(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ---------------------------------------------------------------- Chtimes --

// phase1Time decodes a request's timestamp. "zero" is the time.Time zero
// value, which os.Chtimes specifies as "leave this stamp alone" -- the exact
// property the lost-argument control turns on.
func phase1Time(text string) time.Time {
	if text == "zero" {
		return time.Time{}
	}
	parsed, err := time.Parse(time.RFC3339Nano, text)
	if err != nil {
		panic("phase1: not an RFC3339Nano timestamp: " + text)
	}
	return parsed
}

func phase1ReplayChtimes(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "times_class":
			// A natural stamp is a wall clock reading; only its shape can be
			// recorded, and it is recorded before any Chtimes so a later row
			// that shows a stamp unchanged has something to be unchanged from.
			rel := phase1Str(a, "path")
			value, panicked := phase1Guarded(func() any {
				atime, mtime, err := phase1Times(e.path(rel))
				if err != nil {
					return []any{"error", phase1Err(err)}
				}
				return []any{"ok", atime[0] > 0, mtime[0] > 0}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chtimes":
			// The two arguments are recorded as the exact nanoseconds the
			// probe set and read back, never as a clock reading, so the row
			// is reproducible and doubles as the precision witness.
			rel := phase1Str(a, "path")
			aTime, mTime := phase1Time(phase1Str(a, "atime")), phase1Time(phase1Str(a, "mtime"))
			value, panicked := phase1Guarded(func() any {
				err := e.fsys.Chtimes(e.path(rel), aTime, mTime)
				atime, mtime, statErr := phase1Times(e.path(rel))
				if statErr != nil {
					return []any{phase1Err(err), "unstattable", phase1Err(statErr)}
				}
				return []any{
					phase1Err(err), "times",
					phase1Stamp(atime), phase1Stamp(mtime), atime == mtime,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chtimes_literal":
			literal := phase1Str(a, "literal")
			aTime, mTime := phase1Time(phase1Str(a, "atime")), phase1Time(phase1Str(a, "mtime"))
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(e.fsys.Chtimes(literal, aTime, mTime))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "link_times":
			// Chtimes follows a symlink, so the link's own stamps must be
			// untouched while the target's move.
			link, target := phase1Str(a, "link"), phase1Str(a, "target")
			value, panicked := phase1Guarded(func() any {
				var linkStat unix.Stat_t
				if err := unix.Lstat(e.path(link), &linkStat); err != nil {
					return []any{"lstat_error", phase1Err(err)}
				}
				targetAtime, targetMtime, err := phase1Times(e.path(target))
				if err != nil {
					return []any{"stat_error", phase1Err(err)}
				}
				linkMtime := [2]int64{linkStat.Mtim.Sec, linkStat.Mtim.Nsec}
				return []any{
					"ok", phase1Stamp(targetAtime), phase1Stamp(targetMtime),
					linkMtime == targetMtime,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "precision":
			// What the filesystem kept of a nanosecond remainder. The plan
			// asks for the precision to be recorded rather than slept for.
			rel := phase1Str(a, "path")
			requested := phase1Int(a, "nanoseconds")
			value, panicked := phase1Guarded(func() any {
				stamp := time.Unix(1_000_000_000, requested).UTC()
				err := e.fsys.Chtimes(e.path(rel), stamp, stamp)
				_, mtime, statErr := phase1Times(e.path(rel))
				if statErr != nil {
					return []any{phase1Err(err), "unstattable"}
				}
				return []any{phase1Err(err), requested, mtime[1], mtime[1] == requested}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ---------------------------------------------------------------- WalkDir --

// phase1WalkRow is one callback invocation, as a list. A list, not an object:
// the callback sequence is the subject of every walk case, and canonical
// comparison sorts an object's keys.
func (e *phase1Env) walkRow(path string, d fs.DirEntry, err error, decision string) []any {
	name, isDir, kind, nameMatchesTail := "<nil>", false, "<nil>", false
	if d != nil {
		name, isDir = d.Name(), d.IsDir()
		kind = phase1Mode(d.Type())[1].(string)
		nameMatchesTail = name == path[strings.LastIndex(path, "/")+1:]
	}
	return []any{e.place(path), name, isDir, kind, nameMatchesTail, phase1Err(err), decision}
}

// phase1Decide is the callback rule a walk action carries. The rule travels in
// the request rather than living in this file, so a Rust port reading the
// schedule knows what the callback did.
func phase1Decide(rule string, d fs.DirEntry, err error) (error, string) {
	verb, argument, _ := strings.Cut(rule, ":")
	name := ""
	if d != nil {
		name = d.Name()
	}
	switch verb {
	case "nil":
		return nil, "nil"
	case "skipdir_on":
		if name == argument {
			return fs.SkipDir, "skipdir"
		}
		return nil, "nil"
	case "skipall_on":
		if name == argument {
			return fs.SkipAll, "skipall"
		}
		return nil, "nil"
	case "error_on":
		if name == argument {
			return phase1Refusal, "error"
		}
		return nil, "nil"
	case "skipdir_on_error":
		if err != nil {
			return fs.SkipDir, "skipdir"
		}
		return nil, "nil"
	case "error_on_error":
		if err != nil {
			return phase1Refusal, "error"
		}
		return nil, "nil"
	default:
		panic("phase1: unsupported walk decision: " + rule)
	}
}

func phase1ReplayWalk(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "walk":
			rel, rule := phase1Str(a, "path"), phase1Str(a, "decision")
			value, panicked := phase1Guarded(func() any {
				rows := []any{}
				err := e.fsys.WalkDir(e.path(rel), func(path string, d fs.DirEntry, err error) error {
					decision, label := phase1Decide(rule, d, err)
					rows = append(rows, e.walkRow(path, d, err, label))
					return decision
				})
				return []any{phase1Err(err), len(rows), rows}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "walk_literal":
			literal, rule := phase1Str(a, "literal"), phase1Str(a, "decision")
			value, panicked := phase1Guarded(func() any {
				rows := []any{}
				err := e.fsys.WalkDir(literal, func(path string, d fs.DirEntry, err error) error {
					decision, label := phase1Decide(rule, d, err)
					rows = append(rows, e.walkRow(path, d, err, label))
					return decision
				})
				return []any{phase1Err(err), len(rows), rows}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "chmod":
			rel, mode := phase1Str(a, "path"), phase1Int(a, "mode")
			value, panicked := phase1Guarded(func() any {
				return []any{phase1Err(os.Chmod(e.path(rel), fs.FileMode(mode)))}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// ---------------------------------------------- the walk callback wrapper --

func phase1ReplayWalkBinding(t *testing.T, request phase1Request) []any {
	e := phase1NewEnv(t)
	// Counters per named callback, so "callback A was not invoked during B's
	// walk" is an observation rather than an assertion this file makes.
	counts := map[string]int{}
	var held *limitedWalkDirFunc
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "fixture":
			ordered = append(ordered, phase1Row(op, phase1Fixture(e, phase1Str(a, "fixture")), ""))
		case "walk_counted":
			rel, label := phase1Str(a, "path"), phase1Str(a, "callback")
			others := phase1Strings(a, "others")
			value, panicked := phase1Guarded(func() any {
				before := map[string]int{}
				for _, name := range others {
					before[name] = counts[name]
				}
				rows := []any{}
				err := e.fsys.WalkDir(e.path(rel), func(path string, d fs.DirEntry, _ error) error {
					counts[label]++
					rows = append(rows, []any{e.place(path), d != nil})
					return nil
				})
				untouched := []any{}
				for _, name := range others {
					untouched = append(untouched, []any{name, counts[name] == before[name]})
				}
				allUnderRoot := true
				for _, row := range rows {
					if !strings.HasPrefix(row.([]any)[0].(string), "<root>") {
						allUnderRoot = false
					}
				}
				return []any{phase1Err(err), counts[label], allUnderRoot, untouched, rows}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "walk_nested":
			// A second walk started from inside the first walk's callback.
			// The pin hands out a fresh wrapper per walk, so the outer
			// callback must keep receiving its own rows afterwards.
			outer, inner := phase1Str(a, "path"), phase1Str(a, "nested")
			at := phase1Str(a, "at")
			value, panicked := phase1Guarded(func() any {
				outerRows, innerRows := []any{}, []any{}
				started := 0
				err := e.fsys.WalkDir(e.path(outer), func(path string, d fs.DirEntry, _ error) error {
					outerRows = append(outerRows, e.place(path))
					if d != nil && d.Name() == at {
						started++
						_ = e.fsys.WalkDir(e.path(inner), func(nested string, _ fs.DirEntry, _ error) error {
							innerRows = append(innerRows, e.place(nested))
							return nil
						})
					}
					return nil
				})
				innerUnderNested := true
				for _, row := range innerRows {
					if !strings.HasPrefix(row.(string), "<root>/"+inner) {
						innerUnderNested = false
					}
				}
				return []any{phase1Err(err), started, len(outerRows), len(innerRows), innerUnderNested, outerRows, innerRows}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "walk_reentrant":
			// The same root walked again from inside its own callback, to a
			// bounded depth. Each level's wrapper holds a blocking-op
			// semaphore slot for as long as the inner callback runs, so the
			// depth is deliberately far below the pin's 128-slot bound.
			rel, depth := phase1Str(a, "path"), phase1Int(a, "depth")
			value, panicked := phase1Guarded(func() any {
				perLevel := make([]any, 0, depth)
				var descend func(level int64) error
				descend = func(level int64) error {
					visited := 0
					err := e.fsys.WalkDir(e.path(rel), func(_ string, d fs.DirEntry, _ error) error {
						visited++
						if level < depth && d != nil && d.IsDir() && visited == 1 {
							return descend(level + 1)
						}
						return nil
					})
					perLevel = append(perLevel, []any{level, visited, phase1Err(err)})
					return nil
				}
				_ = descend(1)
				return []any{depth, len(perLevel), perLevel}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "acquire_wrapper":
			// getLimitedWalkDirFunc directly, so the binding itself is
			// visible rather than only its effect.
			label := phase1Str(a, "callback")
			value, panicked := phase1Guarded(func() any {
				held = getLimitedWalkDirFunc(func(path string, d fs.DirEntry, err error) error {
					counts[label]++
					return fmt.Errorf("phase1: %s saw %q d==nil=%v err=%v", label, path, d == nil, err != nil)
				})
				return []any{held != nil, held.inner != nil, held.walk != nil}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "invoke_wrapper":
			// walker forwards all three arguments and returns what the inner
			// callback returned. The path is a literal the probe supplies, so
			// a port that passed the entry name instead of the full path, or
			// that dropped the error argument, differs in this one row.
			literal, label := phase1Str(a, "literal"), phase1Str(a, "callback")
			withError := phase1Str(a, "error") == "yes"
			value, panicked := phase1Guarded(func() any {
				if held == nil {
					return []any{"no_wrapper"}
				}
				var passed error
				if withError {
					passed = errors.New("phase1: probe supplied walk error")
				}
				before := counts[label]
				returned := held.walk(literal, nil, passed)
				text := ""
				if returned != nil {
					text = returned.Error()
				}
				return []any{"ok", counts[label] - before, text}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "release_wrapper":
			// putLimitedWalkDirFunc clears the binding and keeps the walker
			// method value. Both halves are read before the wrapper is let go
			// of, so nothing here depends on what the pool does next.
			value, panicked := phase1Guarded(func() any {
				if held == nil {
					return []any{"no_wrapper"}
				}
				boundBefore := held.inner != nil
				walkBefore := held.walk != nil
				putLimitedWalkDirFunc(held)
				boundAfter := held.inner != nil
				walkAfter := held.walk != nil
				held = nil
				return []any{"ok", boundBefore, walkBefore, boundAfter, walkAfter}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "counts":
			value, panicked := phase1Guarded(func() any {
				names := phase1Strings(a, "callbacks")
				out := []any{}
				for _, name := range names {
					out = append(out, []any{name, counts[name]})
				}
				return out
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// -------------------------------------------- GetGlobalTypingsCacheLocation --

func phase1ReplayTypingsCache(t *testing.T, request phase1Request) []any {
	_ = t
	ordered := []any{}
	for _, a := range phase1Actions(request.Actions) {
		op := phase1Op(a)
		switch op {
		case "location_shape":
			// Never the literal path: its leading component is the host's
			// user cache directory, which names a home directory.
			value, panicked := phase1Guarded(func() any {
				got := GetGlobalTypingsCacheLocation()
				cache, cacheErr := os.UserCacheDir()
				temp := os.TempDir()
				parts := strings.Split(got, "/")
				last, middle := "", ""
				if n := len(parts); n >= 2 {
					last, middle = parts[n-1], parts[n-2]
				}
				return []any{
					got != "",
					cacheErr == nil,
					cacheErr == nil && strings.HasPrefix(got, tspath.NormalizeSlashes(cache)),
					strings.HasPrefix(got, tspath.NormalizeSlashes(strings.TrimSuffix(temp, "/"))),
					!strings.Contains(got, "\\"),
					!strings.Contains(got, "//"),
					middle, last, len(parts),
					strings.HasPrefix(got, "/"),
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "location_repeat_identical":
			value, panicked := phase1Guarded(func() any {
				return []any{GetGlobalTypingsCacheLocation() == GetGlobalTypingsCacheLocation()}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "version_tail":
			// The last component is the major.minor, not the full version:
			// a port that appended core.Version() differs here and nowhere
			// else. The expected text travels in the request so the row says
			// what it is comparing against.
			expected := phase1Str(a, "expected_suffix")
			value, panicked := phase1Guarded(func() any {
				got := GetGlobalTypingsCacheLocation()
				return []any{
					strings.HasSuffix(got, "/"+expected),
					strings.Count(expected, "."),
					expected,
				}
			})
			ordered = append(ordered, phase1Row(op, value, panicked))
		case "host":
			ordered = append(ordered, phase1Row(op, phase1HostClass(), ""))
		default:
			panic("phase1: unsupported action: " + op)
		}
	}
	return ordered
}

// --------------------------------------------------------------- dispatch --

type phase1Replay func(*testing.T, phase1Request) []any

// phase1ReplayFor maps a request's declared subject to the trace player that
// serves it. A subject with no player is not this probe's: the schedule is
// shared with the other filesystem groups, and every request still gets a row.
func phase1ReplayFor(subject string) phase1Replay {
	switch subject {
	case "osvfs.swapCase":
		return phase1ReplaySwapCase
	case "osvfs.UseCaseSensitiveFileNames":
		return phase1ReplayCaseSensitivity
	case "osutil.ProcessIdentity":
		return phase1ReplayProcessIdentity
	case "osvfs.FS":
		return phase1ReplayFS
	case "osvfs.ReadFile":
		return phase1ReplayReadFile
	case "osvfs.Stat":
		return phase1ReplayStat
	case "osvfs.Exists":
		return phase1ReplayExists
	case "osvfs.GetAccessibleEntries":
		return phase1ReplayEntries
	case "osvfs.Realpath":
		return phase1ReplayRealpath
	case "nativepath.Realpath", "nativepath.RealpathLinux":
		return phase1ReplayNativeRealpath
	case "nativepath.IsSymlinkOrReparsePoint":
		return phase1ReplaySymlinkPredicate
	case "osvfs.WriteFile":
		return phase1ReplayWrite
	case "osvfs.WriteLayers":
		return phase1ReplayWriteLayers
	case "osvfs.Remove":
		return phase1ReplayRemove
	case "osvfs.Chtimes":
		return phase1ReplayChtimes
	case "osvfs.WalkDir":
		return phase1ReplayWalk
	case "osvfs.WalkBinding":
		return phase1ReplayWalkBinding
	case "osvfs.GetGlobalTypingsCacheLocation":
		return phase1ReplayTypingsCache
	}
	return nil
}

// phase1Unavailable names a subject this host cannot answer. A build-gated
// operation stays NAMED UNAVAILABLE; it is never replaced by a result computed
// some other way, because an answer from the wrong build would certify nothing.
func phase1Unavailable(subject string) string {
	switch subject {
	case "nativepath.RealpathLinux":
		if runtime.GOOS != "linux" {
			return "tsc/internal/nativepath/realpath_linux.go is the linux build of " +
				"nativepath.Realpath; this capture ran on " + runtime.GOOS +
				", where that file is not compiled and the O_PATH plus /proc/self/fd " +
				"path it pins cannot be reached. One host's results never certify the other."
		}
	case "nativepath.Realpath":
		if runtime.GOOS == "linux" || runtime.GOOS == "windows" {
			return "tsc/internal/nativepath/realpath_other.go is //go:build !windows && !linux; " +
				"this capture ran on " + runtime.GOOS + ", where that file is not compiled."
		}
	}
	return ""
}

func TestPhase1FilesystemOsvfs(t *testing.T) {
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
		replay := phase1ReplayFor(request.Subject)
		switch {
		case replay == nil:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the osvfs probe"
		default:
			if reason := phase1Unavailable(request.Subject); reason != "" {
				row["result"] = "native_unavailable"
				row["reason"] = reason
				break
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replay(t, request)}
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
