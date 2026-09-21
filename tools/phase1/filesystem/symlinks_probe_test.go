package symlinks

// Access only: replays an ordered action trace against the pinned
// internal/symlinks package and records what each action observed. It computes
// nothing of its own beyond rendering, and it never carries an expected value.
//
// It is an IN-PACKAGE test file. Two of the twelve operations are unexported --
// guessDirectorySymlink and isNodeModulesOrScopedPackageDirectory
// (upstream/tsc/internal/symlinks/knownsymlinks.go:114, :132) -- and so are the
// two configuration fields NewKnownSymlink captures, `cwd` and
// `useCaseSensitiveFileNames` (:27-28), which is what the constructor case has
// to read. internal/symlinks needs its own overlay file for a second reason:
// it imports tspath (:10), so the F2a tspath probe, which is itself in-package,
// cannot reach these operations without an import cycle.
//
// Every name this file declares is prefixed `phase1`, because package symlinks
// and its pinned knownsymlinks_test.go share one scope and a later pin must not
// collide with a probe helper.
//
// Byte payloads are hex on the wire. encoding/json replaces invalid UTF-8 with
// U+FFFD, and paths are the whole subject here: an argument travels as UTF-8
// text under its own key or as hex under `<key>_hex`, and exactly one of the
// two must be present. An absent argument is a harness failure and not a
// defaulted observation, which is why actions decode into a raw key map rather
// than into a struct whose missing fields would silently read as the zero
// value -- a defaulted `case_sensitive: false` would make two sides agree on a
// cache neither was asked to build. An unknown action is a harness failure for
// the same reason.
//
// Every action names its cache under `target`, including `new`, which creates
// it. A trace holds several caches at once because the sharpest contrasts in
// this group are between two live caches built with different configuration:
// the pin captures cwd and case sensitivity once, at construction, so the same
// input answered by two caches is what separates that from a port that takes
// them per call.
//
// Two rules keep a recorded observation portable. Everything read back out of
// the cache comes from a Go map -- all four indexes are collections.SyncMap
// over sync.Map (upstream/tsc/internal/collections/syncmap.go:8-12) -- so every
// dump is sorted by key before it is recorded, and each reverse index's member
// set is sorted too; that iteration order is randomized and is not the
// contract. And nothing in this group is rendered as a JSON object: an entry is
// a positional array, because the comparison canonicalises with sorted keys and
// an object's member order would not survive it.
//
// No pinned operation here panics on any input this file supplies, so there is
// no guarded call and no panic class: a panic reaching the test is a harness
// failure and fails the capture, which is what it should do.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/module"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

// phase1Action is one action's raw key map. Decoding into a map rather than a
// struct is what makes a missing key observable: a struct would read
// `case_sensitive` as false for an action that never carried one.
type phase1Action map[string]json.RawMessage

type phase1Request struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared family schedule and
	// another group's actions need not fit this group's shapes.
	Actions json.RawMessage `json:"actions"`
}

func phase1Hex(value []byte) string {
	return hex.EncodeToString(value)
}

func phase1HexAll(values []string) []any {
	out := make([]any, 0, len(values))
	for _, value := range values {
		out = append(out, phase1Hex([]byte(value)))
	}
	return out
}

func phase1Raw(act phase1Action, key string) json.RawMessage {
	raw, ok := act[key]
	if !ok {
		panic("phase1: action is missing the key " + key)
	}
	return raw
}

func phase1JSONString(raw json.RawMessage, key string) string {
	var out string
	if err := json.Unmarshal(raw, &out); err != nil {
		panic("phase1: key " + key + " is not a JSON string: " + err.Error())
	}
	return out
}

// bytes reads a byte argument: UTF-8 text under `key`, or hex under
// `key+"_hex"`. Exactly one of the two must be present.
func (act phase1Action) bytes(key string) string {
	plain, hasPlain := act[key]
	encoded, hasEncoded := act[key+"_hex"]
	if hasPlain == hasEncoded {
		panic("phase1: action must carry exactly one of " + key + " and " + key + "_hex")
	}
	if hasPlain {
		return phase1JSONString(plain, key)
	}
	decoded, err := hex.DecodeString(phase1JSONString(encoded, key+"_hex"))
	if err != nil {
		panic("phase1: key " + key + "_hex is malformed: " + err.Error())
	}
	return string(decoded)
}

func (act phase1Action) op() string {
	return phase1JSONString(phase1Raw(act, "op"), "op")
}

func (act phase1Action) flag(key string) bool {
	var out bool
	if err := json.Unmarshal(phase1Raw(act, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON boolean: " + err.Error())
	}
	return out
}

// pairs reads a resolution list. Each entry is exactly [originalPath,
// resolvedFileName], the two fields SetSymlinksFromResolutions reads off a
// resolution (knownsymlinks.go:86, :89); a shorter entry is a malformed
// request, not a resolution with an empty half.
func (act phase1Action) pairs(key string) [][]string {
	var out [][]string
	if err := json.Unmarshal(phase1Raw(act, key), &out); err != nil {
		panic("phase1: key " + key + " is not a JSON array of arrays: " + err.Error())
	}
	for _, pair := range out {
		if len(pair) != 2 {
			panic("phase1: key " + key + " must carry [original, resolved] pairs")
		}
	}
	return out
}

func phase1Actions(raw json.RawMessage) []phase1Action {
	if len(raw) == 0 {
		panic("phase1: missing actions")
	}
	var out []phase1Action
	if err := json.Unmarshal(raw, &out); err != nil {
		// Decoding happens before any pinned call. A malformed request must
		// fail the probe, never turn into an empty observation.
		panic("phase1: invalid action payload: " + err.Error())
	}
	if len(out) == 0 {
		panic("phase1: empty action trace")
	}
	return out
}

// phase1Caches holds the live caches of one replay, keyed by the name every
// action carries under `target`.
type phase1Caches map[string]*KnownSymlinks

func (caches phase1Caches) get(act phase1Action) *KnownSymlinks {
	name := act.bytes("target")
	cache, ok := caches[name]
	if !ok {
		panic("phase1: no cache named " + name)
	}
	return cache
}

// phase1Entry is one map entry held with its sort key, so a rendering can be
// ordered before it is recorded.
type phase1Entry struct {
	key string
	row []any
}

func phase1Sorted(entries []phase1Entry) []any {
	slices.SortFunc(entries, func(x, y phase1Entry) int { return strings.Compare(x.key, y.key) })
	out := make([]any, 0, len(entries))
	for _, entry := range entries {
		out = append(out, entry.row)
	}
	return out
}

// phase1Directories renders the forward directory index. A value may be nil --
// SetDirectory stores a nil link without touching the reverse index
// (knownsymlinks.go:55-62) -- so presence travels as its own field rather than
// as an absent one, and an absent link carries two empty strings rather than
// no positions at all.
func phase1Directories(cache *KnownSymlinks) []any {
	entries := []phase1Entry{}
	cache.Directories().Range(func(key tspath.Path, value *KnownDirectoryLink) bool {
		row := []any{phase1Hex([]byte(key)), false, "", ""}
		if value != nil {
			row = []any{
				phase1Hex([]byte(key)),
				true,
				phase1Hex([]byte(value.Real)),
				phase1Hex([]byte(value.RealPath)),
			}
		}
		entries = append(entries, phase1Entry{string(key), row})
		return true
	})
	return phase1Sorted(entries)
}

func phase1Files(cache *KnownSymlinks) []any {
	entries := []phase1Entry{}
	cache.Files().Range(func(key tspath.Path, value string) bool {
		entries = append(entries, phase1Entry{
			string(key),
			[]any{phase1Hex([]byte(key)), phase1Hex([]byte(value))},
		})
		return true
	})
	return phase1Sorted(entries)
}

// phase1ByRealpath renders either reverse index. The members are a SyncSet,
// whose ToSlice walks a sync.Map, so they are sorted; the set holds the
// symlink STRING the caller passed, not a Path.
func phase1ByRealpath(index *collections.SyncMap[tspath.Path, *collections.SyncSet[string]]) []any {
	entries := []phase1Entry{}
	index.Range(func(key tspath.Path, value *collections.SyncSet[string]) bool {
		members := []string{}
		if value != nil {
			members = value.ToSlice()
		}
		slices.Sort(members)
		entries = append(entries, phase1Entry{
			string(key),
			[]any{phase1Hex([]byte(key)), phase1HexAll(members)},
		})
		return true
	})
	return phase1Sorted(entries)
}

// phase1Sizes is the post-state of a mutation: the entry counts of the four
// indexes, in the order Directories, DirectoriesByRealpath, Files,
// FilesByRealpath. It localises which index a single call moved, which is the
// subject of every first-insertion case here.
func phase1Sizes(cache *KnownSymlinks) []any {
	return []any{
		cache.Directories().Size(),
		cache.DirectoriesByRealpath().Size(),
		cache.Files().Size(),
		cache.FilesByRealpath().Size(),
	}
}

// phase1Link builds SetDirectory's third argument. `link_present: false` is the
// nil link, and it must carry empty strings: a request that supplied a real
// path for a link it declared absent would be describing two different calls.
func phase1Link(act phase1Action) *KnownDirectoryLink {
	realDirectory, realPath := act.bytes("real"), act.bytes("real_path")
	if !act.flag("link_present") {
		if realDirectory != "" || realPath != "" {
			panic("phase1: an absent directory link must carry empty real and real_path")
		}
		return nil
	}
	return &KnownDirectoryLink{Real: realDirectory, RealPath: tspath.Path(realPath)}
}

func phase1RowLifecycle(caches phase1Caches, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "new":
		name := act.bytes("target")
		cache := NewKnownSymlink(act.bytes("cwd"), act.flag("case_sensitive"))
		caches[name] = cache
		row["not_nil"] = cache != nil
		// Read back off the cache, not off the request: the contract is that
		// the constructor captured them, and a cache that dropped either would
		// answer the request's own values if the request were echoed instead.
		row["cwd"] = phase1Hex([]byte(cache.cwd))
		row["case_sensitive"] = cache.useCaseSensitiveFileNames
		// No size vector here, deliberately. phase1Sizes reads through the four
		// index accessors, and attributing them to every `new` would make every
		// case in the group claim four operations it is not about; the fresh
		// cache's emptiness is recorded by the dump a case takes when that is
		// what it is testing.
	case "dump":
		cache := caches.get(act)
		row["directories"] = phase1Directories(cache)
		row["directories_by_realpath"] = phase1ByRealpath(cache.DirectoriesByRealpath())
		row["files"] = phase1Files(cache)
		row["files_by_realpath"] = phase1ByRealpath(cache.FilesByRealpath())
	default:
		return false
	}
	return true
}

func phase1RowMutate(caches phase1Caches, act phase1Action, op string, row map[string]any) bool {
	switch op {
	case "has_directory":
		cache := caches.get(act)
		row["result"] = cache.HasDirectory(tspath.Path(act.bytes("path")))
	case "set_directory":
		cache := caches.get(act)
		cache.SetDirectory(
			act.bytes("symlink"),
			tspath.Path(act.bytes("symlink_path")),
			phase1Link(act),
		)
		row["sizes"] = phase1Sizes(cache)
	case "set_file":
		cache := caches.get(act)
		cache.SetFile(
			act.bytes("symlink"),
			tspath.Path(act.bytes("symlink_path")),
			act.bytes("realpath"),
		)
		row["sizes"] = phase1Sizes(cache)
	case "process_resolution":
		cache := caches.get(act)
		cache.ProcessResolution(act.bytes("original"), act.bytes("resolved"))
		row["sizes"] = phase1Sizes(cache)
	default:
		return false
	}
	return true
}

// phase1RowResolutions replays SetSymlinksFromResolutions. The two drivers are
// the caller's, so their invocation order and the `file` argument the pin hands
// them are observations rather than assumptions: each driver records that it
// was entered, whether its `file *ast.SourceFile` was nil, and how many
// resolutions it then delivered.
func phase1RowResolutions(caches phase1Caches, act phase1Action, row map[string]any) {
	cache := caches.get(act)
	modules := act.pairs("modules")
	typeReferences := act.pairs("type_references")
	calls := []any{}
	cache.SetSymlinksFromResolutions(
		func(
			callback func(*module.ResolvedModule, string, core.ResolutionMode, tspath.Path),
			file *ast.SourceFile,
		) {
			calls = append(calls, []any{"drive_modules", file == nil, len(modules)})
			for _, pair := range modules {
				calls = append(calls, []any{
					"module", phase1Hex([]byte(pair[0])), phase1Hex([]byte(pair[1])),
				})
				callback(
					&module.ResolvedModule{OriginalPath: pair[0], ResolvedFileName: pair[1]},
					"", core.ResolutionModeNone, tspath.Path(""),
				)
			}
		},
		func(
			callback func(
				*module.ResolvedTypeReferenceDirective, string, core.ResolutionMode, tspath.Path,
			),
			file *ast.SourceFile,
		) {
			calls = append(calls, []any{"drive_type_references", file == nil, len(typeReferences)})
			for _, pair := range typeReferences {
				calls = append(calls, []any{
					"type_reference", phase1Hex([]byte(pair[0])), phase1Hex([]byte(pair[1])),
				})
				callback(
					&module.ResolvedTypeReferenceDirective{
						OriginalPath: pair[0], ResolvedFileName: pair[1],
					},
					"", core.ResolutionModeNone, tspath.Path(""),
				)
			}
		},
	)
	row["calls"] = calls
	row["sizes"] = phase1Sizes(cache)
}

// phase1RowHandles writes through the four accessors. The pin returns a pointer
// to the cache's own storage (knownsymlinks.go:37-53), so a write made through
// a returned handle has to be visible to the cache's own methods; a snapshot
// would not be.
func phase1RowHandles(caches phase1Caches, act phase1Action, op string, row map[string]any) bool {
	var cache *KnownSymlinks
	switch op {
	case "store_through_directories", "store_through_files", "store_through_by_realpath":
		cache = caches.get(act)
	default:
		return false
	}
	switch op {
	case "store_through_directories":
		cache.Directories().Store(tspath.Path(act.bytes("symlink_path")), phase1Link(act))
	case "store_through_files":
		cache.Files().Store(tspath.Path(act.bytes("symlink_path")), act.bytes("realpath"))
	case "store_through_by_realpath":
		index := act.bytes("index")
		key := tspath.Path(act.bytes("realpath"))
		var set *collections.SyncSet[string]
		switch index {
		case "directories":
			set, _ = cache.DirectoriesByRealpath().LoadOrStore(key, &collections.SyncSet[string]{})
		case "files":
			set, _ = cache.FilesByRealpath().LoadOrStore(key, &collections.SyncSet[string]{})
		default:
			panic("phase1: unknown reverse index: " + index)
		}
		set.Add(act.bytes("symlink"))
	}
	row["sizes"] = phase1Sizes(cache)
	return true
}

func phase1RowGuess(caches phase1Caches, act phase1Action, op string, row map[string]any) bool {
	var cache *KnownSymlinks
	switch op {
	case "guess_directory_symlink", "is_node_modules_or_scoped_package_directory":
		cache = caches.get(act)
	default:
		return false
	}
	switch op {
	case "guess_directory_symlink":
		// The cwd is the method's own third argument, not the cache's: a trace
		// may pass one that differs from the cache's, and the pin resolves the
		// two relative inputs against the argument.
		resolved, original := cache.guessDirectorySymlink(
			act.bytes("a"), act.bytes("b"), act.bytes("cwd"))
		row["common_resolved"] = phase1Hex([]byte(resolved))
		row["common_original"] = phase1Hex([]byte(original))
	case "is_node_modules_or_scoped_package_directory":
		row["result"] = cache.isNodeModulesOrScopedPackageDirectory(act.bytes("name"))
	}
	return true
}

func phase1Row(caches phase1Caches, act phase1Action) map[string]any {
	op := act.op()
	row := map[string]any{"op": op}
	if op == "set_symlinks_from_resolutions" {
		phase1RowResolutions(caches, act, row)
		return row
	}
	claimed := phase1RowLifecycle(caches, act, op, row) ||
		phase1RowMutate(caches, act, op, row) ||
		phase1RowHandles(caches, act, op, row) ||
		phase1RowGuess(caches, act, op, row)
	if !claimed {
		panic("phase1: unsupported action: " + op)
	}
	return row
}

func phase1Replay(request phase1Request) []any {
	caches := phase1Caches{}
	rows := []any{}
	for _, act := range phase1Actions(request.Actions) {
		rows = append(rows, phase1Row(caches, act))
	}
	return rows
}

func TestPhase1FilesystemSymlinks(t *testing.T) {
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
		if request.Subject == "symlinks.KnownSymlinks" {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the symlinks probe"
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
