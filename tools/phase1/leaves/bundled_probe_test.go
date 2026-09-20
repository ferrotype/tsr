package bundled_test

// Access only: reports what the pinned packaged-library surface answers for the
// Phase 1 F1a `bundled` coverage group. It calls the pinned entry points --
// WrapFS, LibPath, IsBundled, TestingLibPath -- and reimplements none of them.
//
// It is an external test file. Every identifier it needs is exported, so there
// is nothing to gain from compiling into the pinned package, and the pinned
// tests in `bundled_test` cannot be disturbed by anything declared here because
// every name added is prefixed `phase1`.
//
// BUILD REQUIREMENT: this probe must be built WITHOUT -trimpath.
// `bundledSourceDir` locates the package through runtime.Caller(0). Under
// -trimpath that call still reports ok, and returns the module-relative
// "github.com/microsoft/TypeScript/tsc/internal/bundled", so TestingLibPath
// silently yields a path that does not exist rather than panicking. Verified at
// the pin: `go test -trimpath ./internal/bundled/ -run TestTestingLibPath`
// fails with
// "stat github.com/microsoft/TypeScript/tsc/internal/bundled/libs: no such file
// or directory", while the same command without -trimpath passes.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"hash/fnv"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
)

type phase1Action struct {
	Op   string `json:"op"`
	Name string `json:"name"`
	Path string `json:"path"`
}

// decodePhase1Action unmarshals a request's actions once its subject has matched.
func decodePhase1Action(t *testing.T, raw json.RawMessage) []phase1Action {
	t.Helper()
	if len(raw) == 0 {
		return nil
	}
	var out []phase1Action
	// A request this probe has already claimed by subject must decode. Silently
	// returning nil would turn a malformed schedule into a short row rather
	// than a failure.
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("phase1: a claimed request carries undecodable actions: %v", err)
	}
	return out
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

const phase1DelegationPrefix = "phase1: the bundled wrapper delegated "

// phase1StubFS is the inner filesystem handed to WrapFS. Every path this probe
// asks for is a bundled:/// path, so the wrapper must answer all of them from
// the embedded contents and never reach the inner filesystem. A delegation is
// therefore a real finding about the wrapper, not something to record quietly:
// it panics, which fails the probe and invalidates the capture.
type phase1StubFS struct{}

var _ vfs.FS = (*phase1StubFS)(nil)

func phase1Delegated(method string) {
	panic(phase1DelegationPrefix + method +
		", but every path in this probe is a bundled:/// path")
}

func (*phase1StubFS) UseCaseSensitiveFileNames() bool {
	phase1Delegated("UseCaseSensitiveFileNames")
	return false
}

func (*phase1StubFS) FileExists(path string) bool {
	phase1Delegated("FileExists")
	return false
}

func (*phase1StubFS) ReadFile(path string) (string, bool) {
	phase1Delegated("ReadFile")
	return "", false
}

func (*phase1StubFS) WriteFile(path string, data string) error {
	phase1Delegated("WriteFile")
	return nil
}

func (*phase1StubFS) AppendFile(path string, data string) error {
	phase1Delegated("AppendFile")
	return nil
}

func (*phase1StubFS) Remove(path string) error {
	phase1Delegated("Remove")
	return nil
}

func (*phase1StubFS) Chtimes(path string, aTime time.Time, mTime time.Time) error {
	phase1Delegated("Chtimes")
	return nil
}

func (*phase1StubFS) DirectoryExists(path string) bool {
	phase1Delegated("DirectoryExists")
	return false
}

func (*phase1StubFS) GetAccessibleEntries(path string) vfs.Entries {
	phase1Delegated("GetAccessibleEntries")
	return vfs.Entries{}
}

func (*phase1StubFS) Stat(path string) vfs.FileInfo {
	phase1Delegated("Stat")
	return nil
}

func (*phase1StubFS) WalkDir(root string, walkFn vfs.WalkDirFunc) error {
	phase1Delegated("WalkDir")
	return nil
}

func (*phase1StubFS) Realpath(path string) string {
	phase1Delegated("Realpath")
	return path
}

// phase1Digest is FNV-1a/64 over the asset bytes, rendered as 16 lowercase hex
// digits. The Rust driver has no hash crate in its dependency set, so the
// digest is the one construction both sides can produce identically; it is a
// rendering of the observation, not behavior under test.
func phase1Digest(contents string) string {
	hash := fnv.New64a()
	// hash.Hash never reports a write error.
	_, _ = hash.Write([]byte(contents))
	return fmt.Sprintf("%016x", hash.Sum64())
}

// phase1Strings renders a []string as a JSON array that is never null, so an
// absent list and an empty list are the same frozen observation.
func phase1Strings(values []string) []any {
	out := []any{}
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1Index enumerates the complete asset index the way the pinned
// TestEmbeddedLibs does: walk LibPath() through the wrapper and take each entry
// as it arrives. Order is the subject, so the rows stay a list.
func phase1Index(t *testing.T) map[string]any {
	t.Helper()
	wrapped := bundled.WrapFS(&phase1StubFS{})
	rows := []any{}
	err := wrapped.WalkDir(bundled.LibPath(), func(path string, entry vfs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		contents, ok := wrapped.ReadFile(path)
		if !ok {
			t.Fatalf("phase1: the walk offered %s but the wrapper could not read it", path)
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		// libsEntries and embeddedContents are generated from the same
		// directory; a disagreement is a harness failure, not an observation.
		if info.Size() != int64(len(contents)) {
			t.Fatalf("phase1: %s is %d bytes in the walk entry and %d bytes read",
				path, info.Size(), len(contents))
		}
		rows = append(rows, []any{entry.Name(), len(contents), phase1Digest(contents)})
		return nil
	})
	if err != nil {
		t.Fatalf("phase1: walking %s failed: %v", bundled.LibPath(), err)
	}
	return map[string]any{"count": len(rows), "ordered": rows}
}

// phase1Lookup asks the same LibPath()+"/"+name wrapper path as Rust.
func phase1Lookup(t *testing.T, request phase1Request) map[string]any {
	t.Helper()
	wrapped := bundled.WrapFS(&phase1StubFS{})
	rows := []any{}
	for _, action := range decodePhase1Action(t, request.Actions) {
		if action.Op != "lookup" {
			rows = append(rows, []any{"unsupported_action", action.Op})
			continue
		}
		path := bundled.LibPath() + "/" + action.Name
		contents, ok := wrapped.ReadFile(path)
		if exists := wrapped.FileExists(path); exists != ok {
			t.Fatalf("phase1: FileExists(%q) is %v but ReadFile reports %v", path, exists, ok)
		}
		info := wrapped.Stat(path)
		if !ok {
			if info != nil {
				t.Fatalf("phase1: Stat(%q) returned an entry for a name that does not read", path)
			}
			rows = append(rows, []any{action.Name, false, -1, ""})
			continue
		}
		if info == nil || info.IsDir() || info.Size() != int64(len(contents)) {
			t.Fatalf("phase1: Stat(%q) disagrees with ReadFile", path)
		}
		rows = append(rows, []any{action.Name, true, len(contents), phase1Digest(contents)})
	}
	return map[string]any{"ordered": rows}
}

// phase1Reads keeps wrapper behavior separate from the asset-content census.
func phase1Reads(t *testing.T, request phase1Request) map[string]any {
	wrapped := bundled.WrapFS(&phase1StubFS{})
	rows := []any{}
	for _, action := range decodePhase1Action(t, request.Actions) {
		if action.Op != "read_path" {
			t.Fatalf("unsupported bundled path action: %q", action.Op)
		}
		path := action.Path
		entries := wrapped.GetAccessibleEntries(path)
		var stat any
		if info := wrapped.Stat(path); info != nil {
			stat = []any{info.IsDir(), info.Size()}
		}
		rows = append(rows, map[string]any{
			"op": action.Op, "path": path,
			"directory_exists": wrapped.DirectoryExists(path),
			"file_exists":      wrapped.FileExists(path),
			"files":            phase1Strings(entries.Files), "directories": phase1Strings(entries.Directories),
			"stat": stat, "realpath": wrapped.Realpath(path),
		})
	}
	return map[string]any{"ordered": rows}
}

func phase1Paths(t *testing.T, request phase1Request) map[string]any {
	t.Helper()
	rows := []any{}
	for _, action := range decodePhase1Action(t, request.Actions) {
		if action.Op != "is_bundled" {
			rows = append(rows, []any{"unsupported_action", action.Op})
			continue
		}
		rows = append(rows, []any{action.Path, bundled.IsBundled(action.Path)})
	}
	return map[string]any{"ordered": rows}
}

func phase1LibPath() map[string]any {
	return map[string]any{
		"lib_path":            bundled.LibPath(),
		"lib_path_is_bundled": bundled.IsBundled(bundled.LibPath()),
	}
}

// phase1WrapperPath records the whole per-path wrapper surface for one path:
// the two existence predicates, the directory listing, the stat entry and the
// realpath. `kind` is "nil" when Stat declines; the name is then "" and the
// size -1, neither of which any real entry can produce.
func phase1WrapperPath(wrapped vfs.FS, path string) []any {
	entries := wrapped.GetAccessibleEntries(path)
	kind, name, size := "nil", "", int64(-1)
	if info := wrapped.Stat(path); info != nil {
		kind, name, size = "file", info.Name(), info.Size()
		if info.IsDir() {
			kind = "dir"
		}
	}
	return []any{
		"path", path,
		wrapped.DirectoryExists(path), wrapped.FileExists(path),
		len(entries.Files), phase1Strings(entries.Directories),
		kind, name, size,
		wrapped.Realpath(path),
	}
}

// phase1WrapperRefusal records what one mutating method does to a bundled path.
// The pinned wrapper panics before it can reach the inner filesystem; the row
// carries whichever of panic or return actually happened, so a wrapper that
// stopped panicking would be a difference rather than a silent pass. A
// delegation is still a harness failure: it means the path never matched.
func phase1WrapperRefusal(t *testing.T, method string, run func() error) (row []any) {
	t.Helper()
	defer func() {
		recovered := recover()
		if recovered == nil {
			return
		}
		text, ok := recovered.(string)
		if !ok {
			t.Fatalf("phase1: %s panicked with a non-string value %#v", method, recovered)
		}
		if strings.HasPrefix(text, phase1DelegationPrefix) {
			t.Fatalf("phase1: %s reached the inner filesystem: %s", method, text)
		}
		row = []any{"mutate", method, "panic", text}
	}()
	err := run()
	text := "nil"
	if err != nil {
		text = err.Error()
	}
	return []any{"mutate", method, "returned", text}
}

// phase1Wrapper records the wrapper dispatch the asset cases never reach: the
// per-path predicates and listings, the walk contract, and the mutation
// refusals. No Rust counterpart is reachable from this harness, so the Rust
// side reports a named gap and this observation is frozen as native authority.
func phase1Wrapper(t *testing.T) map[string]any {
	t.Helper()
	wrapped := bundled.WrapFS(&phase1StubFS{})
	rows := []any{}

	for _, path := range []string{
		"bundled:///",
		"bundled:///libs",
		"bundled:///libs/",
		"bundled:///LIBS",
		"bundled:///libs/../libs",
		"bundled:////libs",
		"bundled:///libs/lib.d.ts",
		"bundled:///libs/nope.d.ts",
	} {
		rows = append(rows, phase1WrapperPath(wrapped, path))
	}

	walk := func(label string, root string, decide func(int) error) {
		count := 0
		first := []any{}
		err := wrapped.WalkDir(root, func(path string, entry vfs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			count++
			if len(first) < 2 {
				first = append(first, []any{path, entry.Name(), entry.IsDir()})
			}
			return decide(count)
		})
		text := "nil"
		if err != nil {
			text = err.Error()
		}
		rows = append(rows, []any{"walk", label, root, text, count, first})
	}
	never := func(int) error { return nil }
	sentinel := errors.New("phase1 walk sentinel")

	walk("root-dispatch", "bundled:///", never)
	walk("root-skip-dir-on-the-libs-entry", "bundled:///", func(int) error { return fs.SkipDir })
	walk("libs-complete", bundled.LibPath(), never)
	walk("libs-skip-all-at-the-first-entry", bundled.LibPath(), func(int) error { return fs.SkipAll })
	walk("libs-skip-dir-at-every-file-entry", bundled.LibPath(), func(int) error { return fs.SkipDir })
	walk("libs-error-at-the-first-entry", bundled.LibPath(), func(int) error { return sentinel })
	walk("unknown-bundled-directory", "bundled:///nope", never)

	for _, mutation := range []struct {
		method string
		run    func() error
	}{
		{"WriteFile", func() error { return wrapped.WriteFile("bundled:///libs/lib.d.ts", "x") }},
		{"AppendFile", func() error { return wrapped.AppendFile("bundled:///libs/lib.d.ts", "x") }},
		{"Remove", func() error { return wrapped.Remove("bundled:///libs/lib.d.ts") }},
		{"Chtimes", func() error {
			return wrapped.Chtimes("bundled:///libs/lib.d.ts", time.Time{}, time.Time{})
		}},
		{"Remove-an-absent-asset", func() error { return wrapped.Remove("bundled:///libs/nope.d.ts") }},
		{"WriteFile-the-scheme-root", func() error { return wrapped.WriteFile("bundled:///", "x") }},
	} {
		rows = append(rows, phase1WrapperRefusal(t, mutation.method, mutation.run))
	}

	return map[string]any{"ordered": rows}
}

// phase1SourceDir records how the pinned package reaches its own source
// directory. The payload carries no absolute path, so the observation is the
// same on every checkout; what it records is whether the accessor resolved to a
// real directory at all, which is exactly what -trimpath destroys.
func phase1SourceDir() map[string]any {
	path := bundled.TestingLibPath()
	observation := map[string]any{
		"embedded":                   bundled.Embedded,
		"lib_path":                   bundled.LibPath(),
		"source_lib_dir_base":        filepath.Base(path),
		"source_lib_dir_parent_base": filepath.Base(filepath.Dir(path)),
		"source_lib_dir_absolute":    filepath.IsAbs(path),
		"source_lib_dir_exists":      false,
		"source_lib_dir_entry_count": -1,
		"source_lib_d_ts_exists":     false,
	}
	if info, err := os.Stat(path); err == nil && info.IsDir() {
		observation["source_lib_dir_exists"] = true
		if entries, err := os.ReadDir(path); err == nil {
			observation["source_lib_dir_entry_count"] = len(entries)
		}
	}
	if _, err := os.Stat(filepath.Join(path, "lib.d.ts")); err == nil {
		observation["source_lib_d_ts_exists"] = true
	}
	return observation
}

func TestPhase1LeavesBundled(t *testing.T) {
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
		case "BundledIndex":
			row["result"] = "observed"
			row["observation"] = phase1Index(t)
		case "BundledLookup":
			row["result"] = "observed"
			row["observation"] = phase1Lookup(t, request)
		case "BundledReads":
			row["result"] = "observed"
			row["observation"] = phase1Reads(t, request)
		case "BundledPath":
			row["result"] = "observed"
			row["observation"] = phase1Paths(t, request)
		case "BundledLibPath":
			row["result"] = "observed"
			row["observation"] = phase1LibPath()
		case "BundledWrapper":
			row["result"] = "observed"
			row["observation"] = phase1Wrapper(t)
		case "BundledSourceDir":
			row["result"] = "observed"
			row["observation"] = phase1SourceDir()
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the bundled probe"
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
