package vfsmatch

// Access only: the Phase 1 F2a vfsmatch probe replays an ordered action trace
// against the pinned configuration matcher and records what each action
// observed. It adds no matching logic of its own, reads no expected baseline,
// and reports an unsupported request rather than substituting a result.
//
// It runs under an overlay inside `package vfsmatch`, so the pinned test files
// are its package siblings. Every name it declares therefore carries a
// `phase1` prefix: `vfsmatch_test.go` already declares `contains`, `hasSuffix`
// and `containsAt` at package scope, and a later pin could add more.
//
// Only the exported surface is driven -- ReadDirectory, NewSpecMatcher with
// MatchString/MatchIndex, IsImplicitGlob and Usage.String -- even though being
// in-package would also reach compileGlobPattern, matchFiles and globPattern
// directly. The Rust port exposes only the corresponding public entry points,
// so an observation taken from an unexported Go helper would be one no port
// could ever answer, and the comparison would be with this file rather than
// with the port.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation. A
// result list is emitted as a JSON array and never as an object, because the
// order of ReadDirectory's result is the subject of most of these cases.
//
// Three rules keep the observations portable.
//
// A required action key that is absent is a harness failure, never a default:
// every field below is a pointer, and reading one through the phase1Required*
// helpers panics when the request omitted it. An op this probe does not know
// panics for the same reason. Neither may become a row the two sides could
// agree on without executing anything.
//
// Byte payloads travel as hex in both directions, through the `_hex` ops. A Go
// observation cannot carry invalid UTF-8 through encoding/json, and the
// byte-domain cases -- a lone continuation byte against `?`, an unpaired
// surrogate against IsImplicitGlob -- exist precisely to pin those bytes.
//
// A panic is reduced to a class rather than recorded verbatim, because the Go
// runtime's wording names dynamic types and byte offsets that no Rust port
// could reproduce. Nothing in this package panics with a literal message of
// its own except matchPathParts' "unreachable: literal components never have
// skipPackageFolders", which is documented as unreachable; if it ever fires,
// phase1Classify records it whole, because that text would be the contract.
//
// The host is built outside the guard on purpose. vfstest.FromMap panics on a
// non-rooted or non-normalized path, and that is a defect in the request file,
// not an observation: it must fail the capture loudly.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io/fs"
	"os"
	"runtime"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// phase1UnlimitedDepthSentinel is the request encoding of UnlimitedDepth.
// math.MaxInt does not survive a JSON round trip through every consumer, and a
// missing key must fail rather than default, so the depth key is always
// present and -1 selects the unlimited walk. Every other negative value is a
// malformed request; 0 and above are passed to the pin verbatim, including 0,
// whose post-decrement never reaches the `depth == 0` stop.
const phase1UnlimitedDepthSentinel = -1

type phase1Link struct {
	Path   *string `json:"path"`
	Target *string `json:"target"`
}

type phase1Action struct {
	Op string `json:"op"`

	// vfsmatch.ReadDirectory
	CurrentDirectory          *string       `json:"currentDirectory"`
	Path                      *string       `json:"path"`
	UseCaseSensitiveFileNames *bool         `json:"useCaseSensitiveFileNames"`
	Files                     *[]string     `json:"files"`
	Directories               *[]string     `json:"directories"`
	Symlinks                  *[]phase1Link `json:"symlinks"`
	Extensions                *[]string     `json:"extensions"`
	Excludes                  *[]string     `json:"excludes"`
	Includes                  *[]string     `json:"includes"`
	Depth                     *int          `json:"depth"`

	// vfsmatch.SpecMatcher
	Specs         *[]string `json:"specs"`
	SpecsHex      *[]string `json:"specs_hex"`
	BasePath      *string   `json:"basePath"`
	BasePathHex   *string   `json:"basePath_hex"`
	Paths         *[]string `json:"paths"`
	PathsHex      *[]string `json:"paths_hex"`
	Usage         *int      `json:"usage"`
	CaseSensitive *bool     `json:"caseSensitive"`

	// vfsmatch.IsImplicitGlob
	Component    *string `json:"component"`
	ComponentHex *string `json:"component_hex"`

	// vfsmatch.Usage
	Value *int `json:"value"`
}

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	Dialect   string `json:"dialect"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
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

func phase1RequiredString(value *string, key string, op string) string {
	if value == nil {
		panic("phase1: action " + op + " requires a string " + key)
	}
	return *value
}

func phase1RequiredBool(value *bool, key string, op string) bool {
	if value == nil {
		panic("phase1: action " + op + " requires a boolean " + key)
	}
	return *value
}

func phase1RequiredInt(value *int, key string, op string) int {
	if value == nil {
		panic("phase1: action " + op + " requires an integer " + key)
	}
	return *value
}

func phase1RequiredList(value *[]string, key string, op string) []string {
	if value == nil {
		panic("phase1: action " + op + " requires a list " + key)
	}
	return *value
}

func phase1RequiredBytes(value *string, key string, op string) []byte {
	decoded, err := hex.DecodeString(phase1RequiredString(value, key, op))
	if err != nil {
		panic("phase1: action " + op + " has a malformed hex " + key + ": " + err.Error())
	}
	return decoded
}

func phase1RequiredByteList(value *[]string, key string, op string) []string {
	raw := phase1RequiredList(value, key, op)
	out := make([]string, 0, len(raw))
	for _, item := range raw {
		decoded, err := hex.DecodeString(item)
		if err != nil {
			panic("phase1: action " + op + " has a malformed hex entry in " + key + ": " + err.Error())
		}
		out = append(out, string(decoded))
	}
	return out
}

// phase1Depth reads the depth key, which is always present. See the sentinel's
// comment: -1 is UnlimitedDepth and every other negative value is malformed.
func phase1Depth(a phase1Action) int {
	depth := phase1RequiredInt(a.Depth, "depth", a.Op)
	if depth == phase1UnlimitedDepthSentinel {
		return UnlimitedDepth
	}
	if depth < 0 {
		panic("phase1: action " + a.Op + " has an out-of-domain depth")
	}
	return depth
}

// phase1Usage decodes the pinned Usage discriminants. Only the three named
// values are a legal matcher request; an out-of-range Usage is reachable only
// through the Usage.String trace, which names it `value` rather than `usage`.
func phase1Usage(a phase1Action) Usage {
	usage := phase1RequiredInt(a.Usage, "usage", a.Op)
	switch usage {
	case int(UsageFiles), int(UsageDirectories), int(UsageExclude):
		return Usage(int8(usage))
	}
	panic("phase1: action " + a.Op + " has an out-of-domain usage")
}

// phase1Guarded runs f and converts a panic into a recorded class, so a case
// whose subject is a panic boundary observes it instead of failing the probe.
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
		// nextPathPartParts slices rest[:idx] with an unchecked IndexByte
		// result (vfsmatch.go:345-347). The offset in the runtime's wording is
		// not the contract, so only the shape is recorded.
		return "index_out_of_range"
	case strings.Contains(text, "nil pointer dereference"),
		strings.Contains(text, "invalid memory address"):
		// A nil *SpecMatcher reaches this through MatchString/MatchIndex. No
		// trace here queries one; the class exists so a surprise is recorded
		// rather than lost.
		return "nil_pointer_dereference"
	case strings.Contains(text, "stack overflow"):
		return "stack_overflow"
	default:
		// Reached by a panic the pinned source raises itself, whose literal
		// text is the contract: matchPathParts' "unreachable: literal
		// components never have skipPackageFolders".
		return "other:" + text
	}
}

func phase1ReadDirectory(a phase1Action) map[string]any {
	row := map[string]any{"op": a.Op}
	currentDirectory := phase1RequiredString(a.CurrentDirectory, "currentDirectory", a.Op)
	path := phase1RequiredString(a.Path, "path", a.Op)
	caseSensitive := phase1RequiredBool(a.UseCaseSensitiveFileNames, "useCaseSensitiveFileNames", a.Op)
	extensions := phase1RequiredList(a.Extensions, "extensions", a.Op)
	excludes := phase1RequiredList(a.Excludes, "excludes", a.Op)
	includes := phase1RequiredList(a.Includes, "includes", a.Op)
	depth := phase1Depth(a)

	entries := map[string]any{}
	add := func(name string, value any) {
		if _, seen := entries[name]; seen {
			panic("phase1: action " + a.Op + " declares " + name + " twice")
		}
		entries[name] = value
	}
	for _, file := range phase1RequiredList(a.Files, "files", a.Op) {
		add(file, "")
	}
	for _, directory := range phase1RequiredList(a.Directories, "directories", a.Op) {
		add(directory, &fstest.MapFile{Mode: fs.ModeDir | 0o755})
	}
	if a.Symlinks == nil {
		panic("phase1: action " + a.Op + " requires a list symlinks")
	}
	for _, link := range *a.Symlinks {
		if link.Path == nil || link.Target == nil {
			panic("phase1: action " + a.Op + " has a symlink without a path and a target")
		}
		add(*link.Path, vfstest.Symlink(*link.Target))
	}
	// Outside the guard: a request the test host rejects is a defect in the
	// request file, and must fail the capture rather than become a row.
	host := vfstest.FromMap(entries, caseSensitive)

	result, panicked := phase1Guarded(func() any {
		matched := ReadDirectory(host, currentDirectory, path, extensions, excludes, includes, depth)
		if matched == nil {
			// The pin returns a nil slice for the single-bucket miss and for
			// an all-empty Flatten alike; both marshal to [] and neither is a
			// distinction a port could carry.
			matched = []string{}
		}
		return matched
	})
	row["result"], row["panic"] = result, panicked
	return row
}

func phase1SpecMatcher(a phase1Action, asBytes bool) map[string]any {
	row := map[string]any{"op": a.Op}
	var specs []string
	var basePath string
	var paths []string
	if asBytes {
		specs = phase1RequiredByteList(a.SpecsHex, "specs_hex", a.Op)
		basePath = string(phase1RequiredBytes(a.BasePathHex, "basePath_hex", a.Op))
		paths = phase1RequiredByteList(a.PathsHex, "paths_hex", a.Op)
	} else {
		specs = phase1RequiredList(a.Specs, "specs", a.Op)
		basePath = phase1RequiredString(a.BasePath, "basePath", a.Op)
		paths = phase1RequiredList(a.Paths, "paths", a.Op)
	}
	usage := phase1Usage(a)
	caseSensitive := phase1RequiredBool(a.CaseSensitive, "caseSensitive", a.Op)

	matcher, panicked := phase1Guarded(func() any {
		return NewSpecMatcher(specs, basePath, usage, caseSensitive)
	})
	if panicked != "" {
		row["panic"] = panicked
		return row
	}
	compiled, _ := matcher.(*SpecMatcher)
	row["matcher_present"] = compiled != nil
	// A path triple is a list, not an object: canonicalisation sorts keys and
	// the pairing of MatchString with MatchIndex is what these rows witness.
	queried := []any{}
	if compiled != nil {
		for _, path := range paths {
			rendered := path
			if asBytes {
				rendered = hex.EncodeToString([]byte(path))
			}
			queried = append(queried, []any{rendered, compiled.MatchString(path), compiled.MatchIndex(path)})
		}
	}
	// When NewSpecMatcher answers nil the trace records that and queries
	// nothing: MatchString on a nil receiver dereferences, and a recorded
	// panic class would say less than the presence flag already says.
	row["paths"] = queried
	row["panic"] = ""
	return row
}

func phase1IsImplicitGlob(a phase1Action, asBytes bool) map[string]any {
	row := map[string]any{"op": a.Op}
	var component string
	if asBytes {
		component = string(phase1RequiredBytes(a.ComponentHex, "component_hex", a.Op))
		row["component_hex"] = hex.EncodeToString([]byte(component))
	} else {
		component = phase1RequiredString(a.Component, "component", a.Op)
		row["component"] = component
	}
	result, panicked := phase1Guarded(func() any { return IsImplicitGlob(component) })
	row["result"], row["panic"] = result, panicked
	return row
}

func phase1UsageString(a phase1Action) map[string]any {
	row := map[string]any{"op": a.Op}
	value := phase1RequiredInt(a.Value, "value", a.Op)
	if value < -128 || value > 127 {
		// Usage is an int8. A request outside that domain does not name a
		// value the pinned type can hold, so it is malformed rather than an
		// out-of-range render.
		panic("phase1: action " + a.Op + " has a value outside the Usage domain")
	}
	row["value"] = int64(value)
	result, panicked := phase1Guarded(func() any { return Usage(int8(value)).String() })
	row["result"], row["panic"] = result, panicked
	return row
}

// phase1Replay maps a request's declared subject to the ops it accepts. The
// subject gates ownership and the op names the call; a pair this probe does not
// know is a harness failure, never a row.
func phase1Replay(subject string, actions []phase1Action) []any {
	ordered := []any{}
	for _, a := range actions {
		var row map[string]any
		switch {
		case subject == "vfsmatch.ReadDirectory" && a.Op == "read_directory":
			row = phase1ReadDirectory(a)
		case subject == "vfsmatch.SpecMatcher" && a.Op == "spec_matcher":
			row = phase1SpecMatcher(a, false)
		case subject == "vfsmatch.SpecMatcher" && a.Op == "spec_matcher_hex":
			row = phase1SpecMatcher(a, true)
		case subject == "vfsmatch.IsImplicitGlob" && a.Op == "is_implicit_glob":
			row = phase1IsImplicitGlob(a, false)
		case subject == "vfsmatch.IsImplicitGlob" && a.Op == "is_implicit_glob_hex":
			row = phase1IsImplicitGlob(a, true)
		case subject == "vfsmatch.Usage" && a.Op == "usage_string":
			row = phase1UsageString(a)
		default:
			panic("phase1: unsupported action: " + a.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func phase1Owns(subject string) bool {
	switch subject {
	case "vfsmatch.ReadDirectory", "vfsmatch.SpecMatcher",
		"vfsmatch.IsImplicitGlob", "vfsmatch.Usage":
		return true
	}
	return false
}

func TestPhase1FilesystemVfsmatch(t *testing.T) {
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
		switch {
		case request.Dialect != "vfsmatch":
			// The shared family schedule carries every filesystem group. A
			// request in another dialect belongs to another probe.
			row["result"] = "native_unavailable"
			row["reason"] = "request dialect is not the configuration matcher"
		case !phase1Owns(request.Subject):
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the vfsmatch probe"
		default:
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"ordered": phase1Replay(request.Subject, phase1DecodeActions(request.Actions)),
			}
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
